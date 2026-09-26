use std::{io, time::Duration};
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const INPUT_MIN_HEIGHT: u16 = 3;
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
            status: "正在连接后端…".into(),
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
                self.events = events;
                self.session = Some(session);
                self.ready = true;
                self.sending = false;
                self.status = save_error.unwrap_or_else(|| self.default_status().into());
                self.follow_tail = true;
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
                self.events.push(event);
                self.session = Some(session);
                self.status = self.default_status().into();
            }
            ServerMessage::RequestError { text } | ServerMessage::SessionError { text } => {
                self.sending = false;
                self.status = text;
            }
        }
    }

    fn default_status(&self) -> &'static str {
        if !self.connected {
            "连接已断开"
        } else if !self.ready {
            "正在同步会话…"
        } else if self.is_busy() {
            "正在处理…"
        } else {
            "就绪"
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
            "请输入消息".into()
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
    app.status = "正在发送…".into();
    Ok(())
}

fn draw(frame: &mut Frame, app: &mut App) {
    let input_height = input_height(&app.input, frame.area());
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, areas[0]);
    draw_history(frame, app, areas[1]);
    draw_input(frame, app, areas[2]);
    draw_help(frame, app, areas[3]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let connection = if app.connected {
        "已连接"
    } else {
        "已断开"
    };
    let title = app
        .session
        .as_ref()
        .map(|session| session.title.as_str())
        .filter(|title| !title.is_empty())
        .unwrap_or("新对话");
    let status_color = if app.connected {
        Color::Green
    } else {
        Color::Red
    };
    let line = Line::from(vec![
        Span::styled(" JIsjtu ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!("{title}  ")),
        Span::styled(connection, Style::default().fg(status_color)),
        Span::raw(format!("  {}", app.status)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_history(frame: &mut Frame, app: &mut App, area: Rect) {
    let text = history_text(&app.events);
    let width = area.width.saturating_sub(2).max(1);
    let content_height = wrapped_height(&text, width);
    let viewport_height = area.height.saturating_sub(2);
    let max_scroll = content_height.saturating_sub(viewport_height);
    if app.follow_tail {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
        app.follow_tail = app.scroll >= max_scroll;
    }
    let history = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" 对话 "))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(history, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.is_busy() {
        " 输入（处理中） "
    } else {
        " 输入 "
    };
    let inner_width = area.width.saturating_sub(2).max(1);
    let visible_rows = area.height.saturating_sub(2).max(1);
    let before_cursor: String = app.input.chars().take(app.cursor).collect();
    let (cursor_row, cursor_column) = cursor_position(&before_cursor, inner_width);
    let input_scroll = cursor_row.saturating_sub(visible_rows.saturating_sub(1));
    let input = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((input_scroll, 0));
    frame.render_widget(input, area);

    if app.connected && !app.is_busy() {
        frame.set_cursor_position((
            area.x + 1 + cursor_column.min(inner_width.saturating_sub(1)),
            area.y + 1 + cursor_row.saturating_sub(input_scroll),
        ));
    }
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let hint = if app.is_busy() {
        "模型处理中 · ↑↓/PgUp/PgDn 滚动 · Esc/Ctrl+C 退出"
    } else {
        "Enter 发送 · ↑↓/PgUp/PgDn 滚动 · Esc/Ctrl+C 退出"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn history_text(events: &[ChatEvent]) -> Text<'static> {
    if events.is_empty() {
        return Text::from(vec![Line::styled(
            "输入消息开始对话。",
            Style::default().fg(Color::DarkGray),
        )]);
    }

    let mut lines = Vec::new();
    for event in events {
        match event {
            ChatEvent::User { text } => append_section(
                &mut lines,
                "你",
                text,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            ChatEvent::Answer { text } => append_section(
                &mut lines,
                "助手",
                text,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            ChatEvent::ToolCall { name } => lines.push(Line::styled(
                format!("工具 · {name}"),
                Style::default().fg(Color::Yellow),
            )),
            ChatEvent::ToolResult => continue,
            ChatEvent::Error { text } => append_section(
                &mut lines,
                "错误",
                text,
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            ChatEvent::Done | ChatEvent::Unknown => continue,
        }
        lines.push(Line::default());
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
    let desired = visual_line_count(input, width).max(1).saturating_add(2);
    let max_height = terminal_area.height.saturating_sub(7).max(INPUT_MIN_HEIGHT);
    desired.clamp(INPUT_MIN_HEIGHT, max_height)
}

fn cursor_position(text: &str, width: u16) -> (u16, u16) {
    let width = width.max(1) as usize;
    let mut row = 0usize;
    let mut column = 0usize;
    for character in text.chars() {
        if character == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
        if column > 0 && column + character_width > width {
            row += 1;
            column = 0;
        }
        column += character_width;
        if column >= width {
            row += column / width;
            column %= width;
        }
    }
    (
        row.min(u16::MAX as usize) as u16,
        column.min(u16::MAX as usize) as u16,
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
        assert!(rendered.contains("工具 · read"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("完成"));
    }
}
