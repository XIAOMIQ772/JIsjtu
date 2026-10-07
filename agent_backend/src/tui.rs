use anyhow::Context;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::{SinkExt, StreamExt};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::{io, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const INPUT_MIN_HEIGHT: u16 = 4;
const INPUT_MAX_HEIGHT: u16 = 8;
const CONTENT_MAX_WIDTH: u16 = 96;
const ACCENT: Color = Color::Rgb(132, 161, 174);
const MUTED: Color = Color::Indexed(245);
const SEPARATOR: Color = Color::Indexed(239);
const ERROR: Color = Color::Rgb(196, 120, 120);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default, Deserialize)]
struct SessionSummary {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    busy: bool,
    #[serde(default)]
    revision: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type")]
enum ChatEvent {
    #[serde(rename = "user")]
    User { text: String },
    #[serde(rename = "answer")]
    Answer { text: String },
    #[serde(rename = "answer_delta")]
    AnswerDelta { text: String },
    #[serde(rename = "tool_call")]
    ToolCall { name: String },
    #[serde(rename = "tool_result")]
    ToolResult,
    #[serde(rename = "error")]
    Error { text: String },
    #[serde(rename = "done")]
    Done,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "session_snapshot")]
    SessionSnapshot {
        session: SessionSummary,
        #[serde(default)]
        events: Vec<ChatEvent>,
        save_error: Option<String>,
    },
    #[serde(rename = "session_event")]
    SessionEvent {
        session: SessionSummary,
        event: ChatEvent,
    },
    #[serde(rename = "request_error")]
    RequestError { text: String },
    #[serde(rename = "session_error")]
    SessionError { text: String },
}

#[derive(Debug)]
enum NetworkEvent {
    Message(ServerMessage),
    InvalidMessage(String),
    Disconnected(String),
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

#[derive(Default)]
struct App {
    session: Option<SessionSummary>,
    events: Vec<ChatEvent>,
    input: String,
    cursor: usize,
    scroll: u16,
    follow_tail: bool,
    connected: bool,
    ready: bool,
    sending: bool,
    status: String,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            follow_tail: true,
            status: "同步中".into(),
            ..Self::default()
        }
    }

    fn apply(&mut self, message: ServerMessage) {
        match message {
            ServerMessage::SessionSnapshot {
                session,
                events,
                save_error,
            } => {
                let current_revision = self
                    .session
                    .as_ref()
                    .map(|current| current.revision)
                    .unwrap_or_default();
                if self.ready && session.revision < current_revision {
                    return;
                }
                self.events.clear();
                for event in events {
                    self.append_event(event);
                }
                let new_session =
                    self.session.as_ref().map(|current| &current.id) != Some(&session.id);
                self.session = Some(session);
                self.ready = true;
                self.sending = false;
                self.status = save_error.unwrap_or_else(|| self.default_status().into());
                if new_session {
                    self.follow_tail = true;
                }
            }
            ServerMessage::SessionEvent { session, event } => {
                let current_revision = self
                    .session
                    .as_ref()
                    .map(|current| current.revision)
                    .unwrap_or_default();
                if self.ready && session.revision <= current_revision {
                    return;
                }
                if matches!(event, ChatEvent::User { .. }) && self.sending {
                    self.input.clear();
                    self.cursor = 0;
                    self.sending = false;
                }
                self.append_event(event);
                self.session = Some(session);
                self.status = self.default_status().into();
            }
            ServerMessage::RequestError { text } | ServerMessage::SessionError { text } => {
                self.sending = false;
                self.status = text;
            }
        }
    }

    fn append_event(&mut self, event: ChatEvent) {
        match (self.events.last_mut(), &event) {
            (Some(ChatEvent::AnswerDelta { text }), ChatEvent::AnswerDelta { text: delta }) => {
                text.push_str(delta);
            }
            (Some(previous @ ChatEvent::AnswerDelta { .. }), ChatEvent::Answer { .. }) => {
                *previous = event;
            }
            _ => self.events.push(event),
        }
    }

    fn default_status(&self) -> &'static str {
        if !self.connected {
            "unconnect"
        } else if !self.ready {
            "syncing"
        } else if self.sending {
            "sending"
        } else if self.is_busy() {
            "addressing"
        } else {
            "ready"
        }
    }

    fn is_busy(&self) -> bool {
        self.session
            .as_ref()
            .map(|session| session.busy)
            .unwrap_or(false)
    }

    fn can_send(&self) -> bool {
        self.connected
            && self.ready
            && !self.is_busy()
            && !self.sending
            && !self.input.trim().is_empty()
    }

    fn insert(&mut self, character: char) {
        let byte_index = char_to_byte_index(&self.input, self.cursor);
        self.input.insert(byte_index, character);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte_index(&self.input, self.cursor - 1);
        let end = char_to_byte_index(&self.input, self.cursor);
        self.input.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.chars().count() {
            return;
        }
        let start = char_to_byte_index(&self.input, self.cursor);
        let end = char_to_byte_index(&self.input, self.cursor + 1);
        self.input.replace_range(start..end, "");
    }

    fn scroll_up(&mut self, amount: u16) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_add(amount);
    }
}

pub async fn run(http_port: u16) -> anyhow::Result<()> {
    let url = format!("ws://127.0.0.1:{http_port}/ws");
    let socket = {
        let mut attempts = 0;
        loop {
            match connect_async(&url).await {
                Ok((socket, _)) => break socket,
                Err(_) if attempts < 20 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("无法连接 WebSocket：{url}"));
                }
            }
        }
    };
    let (mut writer, mut reader) = socket.split();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<Message>();
    let (network_tx, mut network_rx) = mpsc::unbounded_channel::<NetworkEvent>();

    let writer_task = tokio::spawn(async move {
        while let Some(message) = outgoing_rx.recv().await {
            if writer.send(message).await.is_err() {
                break;
            }
        }
        let _ = writer.close().await;
    });
    let reader_task = tokio::spawn(async move {
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Text(text)) => match serde_json::from_str::<ServerMessage>(&text) {
                    Ok(message) => {
                        if network_tx.send(NetworkEvent::Message(message)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = network_tx.send(NetworkEvent::InvalidMessage(error.to_string()));
                    }
                },
                Ok(Message::Close(frame)) => {
                    let reason = frame
                        .map(|frame| frame.reason.to_string())
                        .filter(|reason| !reason.is_empty())
                        .unwrap_or_else(|| "后端关闭了连接".into());
                    let _ = network_tx.send(NetworkEvent::Disconnected(reason));
                    return;
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = network_tx.send(NetworkEvent::Disconnected(error.to_string()));
                    return;
                }
            }
        }
        let _ = network_tx.send(NetworkEvent::Disconnected("连接已结束".into()));
    });

    let guard = TerminalGuard::enter().context("无法初始化终端界面")?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("无法创建终端")?;
    terminal.clear()?;
    let mut app = App::new();
    app.connected = true;
    let result = event_loop(&mut terminal, &mut app, &outgoing_tx, &mut network_rx).await;

    drop(outgoing_tx);
    writer_task.abort();
    reader_task.abort();
    let _ = writer_task.await;
    let _ = reader_task.await;
    drop(terminal);
    drop(guard);
    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    outgoing: &mpsc::UnboundedSender<Message>,
    network: &mut mpsc::UnboundedReceiver<NetworkEvent>,
) -> anyhow::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| draw(frame, app))?;

        while let Ok(event) = network.try_recv() {
            match event {
                NetworkEvent::Message(message) => app.apply(message),
                NetworkEvent::InvalidMessage(error) => {
                    app.status = format!("收到无法解析的消息：{error}");
                }
                NetworkEvent::Disconnected(reason) => {
                    app.connected = false;
                    app.ready = false;
                    app.sending = false;
                    app.status = reason;
                }
            }
        }

        if event::poll(EVENT_POLL_INTERVAL)? {
            match event::read()? {
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key, outgoing)?;
                }
                TerminalEvent::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

fn handle_key(
    app: &mut App,
    key: KeyEvent,
    outgoing: &mpsc::UnboundedSender<Message>,
) -> anyhow::Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Enter => send_input(app, outgoing)?,
        KeyCode::Char(character) => app.insert(character),
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),
        KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Right => {
            app.cursor = (app.cursor + 1).min(app.input.chars().count());
        }
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = app.input.chars().count(),
        KeyCode::Up | KeyCode::PageUp => app.scroll_up(5),
        KeyCode::Down | KeyCode::PageDown => app.scroll_down(5),
        _ => {}
    }
    Ok(())
}

fn send_input(app: &mut App, outgoing: &mpsc::UnboundedSender<Message>) -> anyhow::Result<()> {
    if !app.can_send() {
        app.status = if app.input.trim().is_empty() {
            "send your message".into()
        } else {
            app.default_status().into()
        };
        return Ok(());
    }
    let session_id = app
        .session
        .as_ref()
        .map(|session| session.id.clone())
        .context("会话尚未准备好")?;
    let payload = json!({
        "type": "user",
        "text": app.input.trim(),
        "session_id": session_id,
    });
    outgoing
        .send(Message::Text(payload.to_string().into()))
        .context("WebSocket 写入通道已关闭")?;
    app.sending = true;
    app.status = app.default_status().into();
    Ok(())
}

fn draw(frame: &mut Frame, app: &mut App) {
    let area = content_area(frame.area());
    if area.width < 20 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("请放大终端窗口")
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let notice_height = if !app.status.is_empty() && app.status != app.default_status() {
        visual_line_count(&app.status, area.width)
            .min(3)
            .min(area.height.saturating_sub(8))
    } else {
        0
    };
    let input_height = input_height(
        &app.input,
        Rect {
            height: area.height.saturating_sub(notice_height),
            ..area
        },
    );
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(notice_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, areas[0]);
    draw_history(frame, app, areas[1]);
    if notice_height > 0 {
        frame.render_widget(
            Paragraph::new(app.status.as_str())
                .style(Style::default().fg(ERROR))
                .wrap(Wrap { trim: false }),
            areas[2],
        );
    }
    draw_input(frame, app, areas[3]);
    draw_help(frame, app, areas[4]);
}

fn content_area(area: Rect) -> Rect {
    let horizontal_margin = if area.width >= 60 { 3 } else { 1 };
    let vertical_margin = if area.height >= 12 { 1 } else { 0 };
    let width = area
        .width
        .saturating_sub(horizontal_margin * 2)
        .min(CONTENT_MAX_WIDTH);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + vertical_margin,
        width,
        area.height.saturating_sub(vertical_margin * 2),
    )
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = app
        .session
        .as_ref()
        .map(|session| session.title.as_str())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("新的对话")
        .replace(['\n', '\r'], " ");
    let status_color = if !app.connected {
        ERROR
    } else if app.is_busy() || app.sending {
        ACCENT
    } else {
        MUTED
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(8)])
        .split(Rect { height: 1, ..area });
    let mut heading = vec![Span::styled(
        "JIsjtu",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if columns[0].width >= 22 {
        heading.push(Span::styled(
            format!("  /  {title}"),
            Style::default().fg(MUTED),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(heading)), columns[0]);
    frame.render_widget(
        Paragraph::new(app.default_status())
            .style(Style::default().fg(status_color))
            .alignment(Alignment::Right),
        columns[1],
    );
}

fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    let text = history_text(&app.events);
    if text.lines.is_empty() {
        let top = area.height.saturating_sub(4) / 3;
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from(""),
                Line::default(),
                Line::styled("", Style::default().fg(MUTED)),
            ]))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
            Rect {
                y: area.y + top,
                height: area.height.saturating_sub(top),
                ..area
            },
        );
        return;
    }

    let content_height = wrapped_height(&text, area.width.max(1));
    let max_scroll = content_height.saturating_sub(area.height);
    if app.follow_tail {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
        app.follow_tail = app.scroll >= max_scroll;
    }
    let history = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(history, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let inner_width = area.width.saturating_sub(2).max(1);
    let visible_rows = area.height.saturating_sub(3).max(1);
    let inner = Rect::new(area.x + 2, area.y + 2, inner_width, visible_rows);
    let before_cursor: String = app.input.chars().take(app.cursor).collect();
    let (cursor_row, cursor_column) = cursor_position(&before_cursor, inner_width);
    let input_scroll = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    let display_text = if app.input.is_empty() {
        Text::styled(
            if app.is_busy() {
                "addressing，could send your next message in advance…"
            } else {
                "input message…"
            },
            Style::default().fg(MUTED),
        )
    } else {
        Text::from(
            wrap_input(&app.input, inner_width)
                .into_iter()
                .map(Line::raw)
                .collect::<Vec<_>>(),
        )
    };

    frame.render_widget(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(SEPARATOR)),
        area,
    );
    frame.render_widget(
        Paragraph::new("›").style(Style::default().fg(if app.connected { ACCENT } else { MUTED })),
        Rect::new(area.x, inner.y, 1, 1),
    );
    frame.render_widget(
        Paragraph::new(display_text).scroll((input_scroll, 0)),
        inner,
    );

    if app.connected && app.ready && !app.sending {
        frame.set_cursor_position((
            inner.x + cursor_column.min(inner_width.saturating_sub(1)),
            inner.y + cursor_row.saturating_sub(input_scroll),
        ));
    }
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let hint = if area.width < 32 {
        "↵ 发送  Esc 退出"
    } else {
        "Enter 发送   ↑↓ 滚动   Esc 退出"
    };
    frame.render_widget(Paragraph::new(hint).style(Style::default().fg(MUTED)), area);
    if !app.follow_tail && area.width >= 48 {
        frame.render_widget(
            Paragraph::new("查看历史")
                .alignment(Alignment::Right)
                .style(Style::default().fg(MUTED)),
            Rect::new(area.right().saturating_sub(8), area.y, 8, area.height),
        );
    }
}

fn history_text(events: &[ChatEvent]) -> Text<'static> {
    let mut lines = Vec::new();
    let mut previous_was_tool = false;
    for event in events {
        if matches!(
            event,
            ChatEvent::ToolResult | ChatEvent::Done | ChatEvent::Unknown
        ) {
            continue;
        }
        let is_tool = matches!(event, ChatEvent::ToolCall { .. });
        if !lines.is_empty() && !(is_tool && previous_was_tool) {
            lines.push(Line::default());
        }
        match event {
            ChatEvent::User { text } => {
                append_section(&mut lines, "你", text, Style::default().fg(MUTED))
            }
            ChatEvent::Answer { text } | ChatEvent::AnswerDelta { text } => append_section(
                &mut lines,
                "小集",
                text,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            ChatEvent::ToolCall { name } => {
                lines.push(Line::styled(format!("{name}"), Style::default().fg(MUTED)))
            }
            ChatEvent::ToolResult => continue,
            ChatEvent::Error { text } => append_section(
                &mut lines,
                "错误",
                text,
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            ),
            ChatEvent::Done | ChatEvent::Unknown => continue,
        }
        previous_was_tool = is_tool;
    }
    Text::from(lines)
}

fn append_section(lines: &mut Vec<Line<'static>>, label: &str, content: &str, style: Style) {
    lines.push(Line::styled(label.to_string(), style));
    if content.is_empty() {
        lines.push(Line::raw("（无内容）"));
    } else {
        lines.extend(content.lines().map(|line| Line::raw(line.to_string())));
    }
}

fn input_height(input: &str, terminal_area: Rect) -> u16 {
    let width = terminal_area.width.saturating_sub(2).max(1);
    let desired = (wrap_input(input, width).len().min(u16::MAX as usize) as u16).saturating_add(3);
    let max_height = terminal_area
        .height
        .saturating_sub(4)
        .clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT);
    desired.clamp(INPUT_MIN_HEIGHT, max_height)
}

// Use the same cell wrapping for the input and its cursor, including wide Chinese characters.
fn wrap_input(text: &str, width: u16) -> Vec<String> {
    let width = width.max(1) as usize;
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\n' {
            lines.push(std::mem::take(&mut line));
            column = 0;
            continue;
        }
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if column > 0 && column + character_width > width {
            lines.push(std::mem::take(&mut line));
            column = 0;
        }
        line.push(character);
        column += character_width;
    }
    lines.push(line);
    if column >= width {
        lines.push(String::new());
    }
    lines
}

fn cursor_position(text: &str, width: u16) -> (u16, u16) {
    let lines = wrap_input(text, width);
    (
        lines.len().saturating_sub(1).min(u16::MAX as usize) as u16,
        lines
            .last()
            .map(|line| UnicodeWidthStr::width(line.as_str()))
            .unwrap_or_default()
            .min(u16::MAX as usize) as u16,
    )
}

fn wrapped_height(text: &Text<'_>, width: u16) -> u16 {
    text.lines
        .iter()
        .map(|line| visual_line_count(&line.to_string(), width))
        .fold(0u16, u16::saturating_add)
}

fn visual_line_count(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    if text.is_empty() {
        return 1;
    }
    text.split('\n')
        .map(|line| UnicodeWidthStr::width(line).max(1).div_ceil(width))
        .sum::<usize>()
        .min(u16::MAX as usize) as u16
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(revision: u64, busy: bool) -> SessionSummary {
        SessionSummary {
            id: "session-1".into(),
            title: "测试".into(),
            busy,
            revision,
        }
    }

    #[test]
    fn snapshot_replaces_events_and_ignores_older_revision() {
        let mut app = App::new();
        app.connected = true;
        app.apply(ServerMessage::SessionSnapshot {
            session: session(2, false),
            events: vec![ChatEvent::Answer { text: "新".into() }],
            save_error: None,
        });
        app.apply(ServerMessage::SessionSnapshot {
            session: session(1, false),
            events: vec![ChatEvent::Answer { text: "旧".into() }],
            save_error: None,
        });

        assert_eq!(app.events.len(), 1);
        assert!(matches!(&app.events[0], ChatEvent::Answer { text } if text == "新"));
    }

    #[test]
    fn incremental_events_are_deduplicated_by_revision() {
        let mut app = App::new();
        app.connected = true;
        app.apply(ServerMessage::SessionSnapshot {
            session: session(1, true),
            events: vec![],
            save_error: None,
        });
        app.apply(ServerMessage::SessionEvent {
            session: session(2, true),
            event: ChatEvent::ToolCall {
                name: "read".into(),
            },
        });
        app.apply(ServerMessage::SessionEvent {
            session: session(2, true),
            event: ChatEvent::ToolCall {
                name: "read".into(),
            },
        });

        assert_eq!(app.events.len(), 1);
    }

    #[test]
    fn send_requires_ready_idle_session_and_text() {
        let mut app = App::new();
        app.connected = true;
        app.ready = true;
        app.session = Some(session(0, false));
        app.input = "你好".into();
        assert!(app.can_send());

        app.session.as_mut().unwrap().busy = true;
        assert!(!app.can_send());
    }

    #[test]
    fn input_editing_handles_multibyte_characters() {
        let mut app = App::new();
        app.insert('你');
        app.insert('a');
        app.insert('好');
        app.cursor = 2;
        app.backspace();
        assert_eq!(app.input, "你好");
        app.delete();
        assert_eq!(app.input, "你");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn input_layout_uses_terminal_width_for_chinese() {
        assert_eq!(visual_line_count("你好ab", 4), 2);
        assert_eq!(cursor_position("你好ab", 4), (1, 2));
        assert_eq!(
            input_height("你".repeat(30).as_str(), Rect::new(0, 0, 12, 20)),
            8
        );
    }

    #[test]
    fn tool_history_only_shows_the_tool_name() {
        let call: ChatEvent = serde_json::from_value(json!({
            "type": "tool_call",
            "name": "read",
            "arguments": "secret"
        }))
        .unwrap();
        let result: ChatEvent = serde_json::from_value(json!({
            "type": "tool_result",
            "name": "read",
            "ok": true,
            "preview": "secret result"
        }))
        .unwrap();
        let rendered = history_text(&[call, result]).to_string();
        assert!(rendered.contains("read"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("完成"));
    }

    #[test]
    fn streaming_answer_updates_one_message_and_deduplicates_revisions() {
        let mut app = App::new();
        app.connected = true;
        app.apply(ServerMessage::SessionSnapshot {
            session: session(1, true),
            events: vec![],
            save_error: None,
        });
        for (revision, text) in [(2, "第一段"), (2, "不应重复"), (3, "，后续内容")] {
            app.apply(ServerMessage::SessionEvent {
                session: session(revision, true),
                event: ChatEvent::AnswerDelta { text: text.into() },
            });
        }
        assert_eq!(app.events.len(), 1);
        assert!(
            history_text(&app.events)
                .to_string()
                .contains("第一段，后续内容")
        );
        app.apply(ServerMessage::SessionEvent {
            session: session(4, true),
            event: ChatEvent::Answer {
                text: "第一段，后续内容".into(),
            },
        });
        assert_eq!(app.events.len(), 1);
        assert!(matches!(&app.events[0], ChatEvent::Answer { text } if text == "第一段，后续内容"));
        app.follow_tail = false;
        app.apply(ServerMessage::SessionSnapshot {
            session: session(4, false),
            events: app.events.clone(),
            save_error: None,
        });
        assert!(
            !app.follow_tail,
            "a completed snapshot must not move the reader's scroll position"
        );
    }

    #[test]
    fn interrupted_stream_does_not_merge_with_the_next_answer() {
        let mut app = App::new();
        app.append_event(ChatEvent::AnswerDelta {
            text: "部分回复".into(),
        });
        app.append_event(ChatEvent::Error {
            text: "中断".into(),
        });
        app.append_event(ChatEvent::AnswerDelta {
            text: "新的回复".into(),
        });
        app.append_event(ChatEvent::Answer {
            text: "新的回复".into(),
        });
        assert_eq!(app.events.len(), 3);
        assert!(matches!(&app.events[0], ChatEvent::AnswerDelta { text } if text == "部分回复"));
        assert!(matches!(&app.events[2], ChatEvent::Answer { text } if text == "新的回复"));
    }
}
