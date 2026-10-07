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
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, io, time::Duration};
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
    #[serde(default)]
    updated_at: i64,
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
    RequestError {
        session_id: Option<String>,
        text: String,
    },
    #[serde(rename = "session_error")]
    SessionError {
        session_id: Option<String>,
        text: String,
    },
}

#[derive(Debug)]
enum NetworkEvent {
    Message(ServerMessage),
    SessionList {
        request_id: u64,
        result: Result<Vec<SessionSummary>, String>,
    },
    InvalidMessage(String),
    Disconnected(String),
}

#[derive(Default)]
struct SessionPicker {
    sessions: Vec<SessionSummary>,
    selection: ListState,
    request_id: u64,
    loading: bool,
    error: Option<String>,
}

struct SessionView {
    input: String,
    cursor: usize,
    scroll: u16,
    follow_tail: bool,
}

impl Default for SessionView {
    fn default() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
            scroll: 0,
            follow_tail: true,
        }
    }
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
    picker: Option<SessionPicker>,
    pending_switch: Option<String>,
    saved_views: HashMap<String, SessionView>,
    list_request_id: u64,
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
                let same_session = self.is_current_session(&session.id);
                let switch_confirmed = self.pending_switch.as_deref() == Some(&session.id);
                if self.session.is_some() && !same_session && !switch_confirmed {
                    return;
                }
                if same_session
                    && self.ready
                    && session.revision < self.session.as_ref().unwrap().revision
                {
                    return;
                }
                if !same_session && self.session.is_some() {
                    self.restore_view(&session.id);
                }
                self.events.clear();
                for event in events {
                    self.append_event(event);
                }
                self.update_picker_session(&session);
                self.session = Some(session);
                if switch_confirmed {
                    self.pending_switch = None;
                    self.picker = None;
                }
                self.ready = true;
                self.sending = false;
                self.status = save_error.unwrap_or_else(|| self.default_status().into());
            }
            ServerMessage::SessionEvent { session, event } => {
                // Revisions are local to a session. Late events must not enter another chat.
                if !self.is_current_session(&session.id) {
                    return;
                }
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
                self.update_picker_session(&session);
                self.session = Some(session);
                self.status = self.default_status().into();
            }
            ServerMessage::RequestError { session_id, text } => {
                if session_id
                    .as_deref()
                    .is_none_or(|id| self.is_current_session(id))
                {
                    self.sending = false;
                    self.status = text;
                }
            }
            ServerMessage::SessionError { session_id, text } => {
                if self.pending_switch.is_some()
                    && (session_id.is_none() || session_id == self.pending_switch)
                {
                    self.pending_switch = None;
                    if let Some(picker) = &mut self.picker {
                        picker.error = Some(text.clone());
                    }
                    self.status = text;
                } else if session_id
                    .as_deref()
                    .is_none_or(|id| self.is_current_session(id))
                {
                    self.sending = false;
                    self.status = text;
                }
            }
        }
    }

    fn is_current_session(&self, id: &str) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.id == id)
    }

    fn restore_view(&mut self, id: &str) {
        if let Some(current) = &self.session {
            self.saved_views.insert(
                current.id.clone(),
                SessionView {
                    input: std::mem::take(&mut self.input),
                    cursor: self.cursor,
                    scroll: self.scroll,
                    follow_tail: self.follow_tail,
                },
            );
        }
        let view = self.saved_views.remove(id).unwrap_or_default();
        self.input = view.input;
        self.cursor = view.cursor;
        self.scroll = view.scroll;
        self.follow_tail = view.follow_tail;
    }

    fn update_picker_session(&mut self, summary: &SessionSummary) {
        if let Some(picker) = &mut self.picker {
            if let Some(item) = picker
                .sessions
                .iter_mut()
                .find(|item| item.id == summary.id)
            {
                *item = summary.clone();
            }
        }
    }

    fn request_sessions(&mut self, requests: &mpsc::UnboundedSender<u64>) {
        self.list_request_id += 1;
        let picker = self.picker.get_or_insert_with(SessionPicker::default);
        picker.request_id = self.list_request_id;
        picker.error = None;
        picker.loading = true;
        if requests.send(picker.request_id).is_err() {
            picker.loading = false;
            picker.error = Some("会话列表连接已关闭".into());
        }
    }

    fn receive_sessions(&mut self, request_id: u64, result: Result<Vec<SessionSummary>, String>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        if picker.request_id != request_id {
            return;
        }
        picker.loading = false;
        match result {
            Ok(mut sessions) => {
                let selected_id = picker
                    .selection
                    .selected()
                    .and_then(|index| picker.sessions.get(index))
                    .map(|session| session.id.as_str())
                    .or_else(|| self.session.as_ref().map(|session| session.id.as_str()));
                if let Some(current) = &self.session {
                    if let Some(item) = sessions.iter_mut().find(|item| item.id == current.id) {
                        if current.revision >= item.revision {
                            *item = current.clone();
                        }
                    }
                }
                sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
                let selected = sessions
                    .iter()
                    .position(|session| Some(session.id.as_str()) == selected_id)
                    .or_else(|| (!sessions.is_empty()).then_some(0));
                picker.selection.select(selected);
                picker.sessions = sessions;
                picker.error = None;
            }
            Err(error) => picker.error = Some(error),
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
        } else if !self.ready || self.pending_switch.is_some() {
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
            && self.picker.is_none()
            && self.pending_switch.is_none()
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
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()?;
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
    let (list_tx, mut list_rx) = mpsc::unbounded_channel::<u64>();
    let list_network_tx = network_tx.clone();

    let list_task = tokio::spawn(async move {
        while let Some(request_id) = list_rx.recv().await {
            let result = fetch_sessions(&http, http_port)
                .await
                .map_err(|error| format!("{error:#}"));
            if list_network_tx
                .send(NetworkEvent::SessionList { request_id, result })
                .is_err()
            {
                break;
            }
        }
    });

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
    let result = event_loop(
        &mut terminal,
        &mut app,
        &outgoing_tx,
        &list_tx,
        &mut network_rx,
    )
    .await;

    drop(outgoing_tx);
    drop(list_tx);
    writer_task.abort();
    reader_task.abort();
    list_task.abort();
    let _ = writer_task.await;
    let _ = reader_task.await;
    let _ = list_task.await;
    drop(terminal);
    drop(guard);
    result
}

async fn fetch_sessions(http: &reqwest::Client, port: u16) -> anyhow::Result<Vec<SessionSummary>> {
    http.get(format!("http://127.0.0.1:{port}/api/sessions"))
        .send()
        .await
        .context("无法读取会话列表")?
        .error_for_status()
        .context("读取会话列表失败")?
        .json()
        .await
        .context("会话列表格式错误")
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    outgoing: &mpsc::UnboundedSender<Message>,
    list_requests: &mpsc::UnboundedSender<u64>,
    network: &mut mpsc::UnboundedReceiver<NetworkEvent>,
) -> anyhow::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| draw(frame, app))?;

        while let Ok(event) = network.try_recv() {
            match event {
                NetworkEvent::Message(message) => app.apply(message),
                NetworkEvent::SessionList { request_id, result } => {
                    app.receive_sessions(request_id, result);
                }
                NetworkEvent::InvalidMessage(error) => {
                    app.status = format!("收到无法解析的消息：{error}");
                }
                NetworkEvent::Disconnected(reason) => {
                    app.connected = false;
                    app.ready = false;
                    app.sending = false;
                    app.pending_switch = None;
                    if let Some(picker) = &mut app.picker {
                        picker.loading = false;
                        picker.error = Some(reason.clone());
                    }
                    app.status = reason;
                }
            }
        }

        if event::poll(EVENT_POLL_INTERVAL)? {
            match event::read()? {
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key, outgoing, list_requests)?;
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
    list_requests: &mpsc::UnboundedSender<u64>,
) -> anyhow::Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(());
    }
    if app.picker.is_some() {
        handle_picker_key(app, key, outgoing, list_requests);
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => app.should_quit = true,
        KeyCode::Enter => send_input(app, outgoing, list_requests)?,
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

fn handle_picker_key(
    app: &mut App,
    key: KeyEvent,
    outgoing: &mpsc::UnboundedSender<Message>,
    list_requests: &mpsc::UnboundedSender<u64>,
) {
    if app.pending_switch.is_some() {
        return;
    }
    match key.code {
        KeyCode::Esc => app.picker = None,
        KeyCode::Enter => switch_selected_session(app, outgoing),
        KeyCode::Char('r') if app.connected && !app.picker.as_ref().unwrap().loading => {
            app.request_sessions(list_requests);
        }
        _ => {
            let picker = app.picker.as_mut().unwrap();
            if picker.loading || picker.sessions.is_empty() {
                return;
            }
            let selected = picker.selection.selected().unwrap_or(0);
            let last = picker.sessions.len() - 1;
            let next = match key.code {
                KeyCode::Up => selected.saturating_sub(1),
                KeyCode::Down => selected.saturating_add(1).min(last),
                KeyCode::PageUp => selected.saturating_sub(5),
                KeyCode::PageDown => selected.saturating_add(5).min(last),
                KeyCode::Home => 0,
                KeyCode::End => last,
                _ => return,
            };
            picker.selection.select(Some(next));
        }
    }
}

fn switch_selected_session(app: &mut App, outgoing: &mpsc::UnboundedSender<Message>) {
    let Some(id) = app
        .picker
        .as_ref()
        .filter(|picker| !picker.loading)
        .and_then(|picker| {
            picker
                .selection
                .selected()
                .and_then(|index| picker.sessions.get(index))
        })
        .map(|session| session.id.clone())
    else {
        return;
    };
    if app.is_current_session(&id) {
        app.picker = None;
        return;
    }
    if !app.connected || !app.ready || app.sending {
        app.picker.as_mut().unwrap().error = Some("连接尚未就绪，请稍后再试".into());
        return;
    }
    let payload = json!({ "type": "switch_session", "session_id": id });
    if outgoing
        .send(Message::Text(payload.to_string().into()))
        .is_err()
    {
        app.picker.as_mut().unwrap().error = Some("WebSocket 写入通道已关闭".into());
        return;
    }
    app.pending_switch = Some(id);
    app.picker.as_mut().unwrap().error = None;
    app.status = app.default_status().into();
}

fn send_input(
    app: &mut App,
    outgoing: &mpsc::UnboundedSender<Message>,
    list_requests: &mpsc::UnboundedSender<u64>,
) -> anyhow::Result<()> {
    // Local commands remain available while the model is streaming.
    if app.input.trim() == "/sessions" {
        if app.connected && app.ready && !app.sending && app.pending_switch.is_none() {
            app.input.clear();
            app.cursor = 0;
            app.request_sessions(list_requests);
        } else {
            app.status = app.default_status().into();
        }
        return Ok(());
    }
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
    if app.picker.is_some() {
        draw_session_picker(frame, app, area);
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

fn draw_session_picker(frame: &mut Frame, app: &mut App, area: Rect) {
    let current_id = app.session.as_ref().map(|session| session.id.as_str());
    let switching = app.pending_switch.is_some();
    let picker = app.picker.as_mut().unwrap();
    let notice = if switching {
        "正在打开会话…"
    } else if picker.loading {
        "正在读取会话…"
    } else if let Some(error) = &picker.error {
        error.as_str()
    } else {
        "选择一个会话，继续之前的对话。"
    };
    let notice_height = visual_line_count(notice, area.width).clamp(1, 3) + 1;
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(notice_height),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    let heading = Layout::horizontal([Constraint::Min(0), Constraint::Length(10)]).split(Rect {
        height: 1,
        ..areas[0]
    });
    let title = if heading[0].width >= 20 {
        Line::from(vec![
            Span::styled("JIsjtu", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("  /  会话", Style::default().fg(MUTED)),
        ])
    } else {
        Line::styled("会话", Style::default().add_modifier(Modifier::BOLD))
    };
    frame.render_widget(Paragraph::new(title), heading[0]);
    frame.render_widget(
        Paragraph::new(format!("{} 个会话", picker.sessions.len()))
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED)),
        heading[1],
    );
    frame.render_widget(
        Paragraph::new(notice)
            .style(Style::default().fg(if picker.error.is_some() { ERROR } else { MUTED }))
            .wrap(Wrap { trim: false }),
        areas[1],
    );

    let items: Vec<_> = picker
        .sessions
        .iter()
        .map(|session| {
            let title = if session.title.trim().is_empty() {
                "新的对话"
            } else {
                &session.title
            };
            let mut detail = Vec::new();
            if Some(session.id.as_str()) == current_id {
                detail.push("当前".to_string());
            }
            if session.busy {
                detail.push("处理中".to_string());
            }
            if let Some(time) = chrono::DateTime::from_timestamp_millis(session.updated_at) {
                detail.push(
                    time.with_timezone(&chrono::Local)
                        .format(if area.width < 40 {
                            "%m-%d"
                        } else {
                            "%m-%d %H:%M"
                        })
                        .to_string(),
                );
            }
            ListItem::new(vec![
                Line::raw(title.replace(['\n', '\r', '\t'], " ")),
                Line::styled(detail.join(" · "), Style::default().fg(MUTED)),
                Line::default(),
            ])
        })
        .collect();
    if items.is_empty() && !picker.loading && picker.error.is_none() {
        frame.render_widget(
            Paragraph::new("暂无已保存的会话").style(Style::default().fg(MUTED)),
            areas[2],
        );
    } else {
        frame.render_stateful_widget(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            areas[2],
            &mut picker.selection,
        );
    }
    let hint = if switching {
        "正在同步会话…"
    } else if area.width < 30 && picker.error.is_some() {
        "r 刷新  Esc 返回"
    } else if area.width < 30 {
        "↑↓  ↵ 打开  Esc 返回"
    } else if area.width < 40 {
        "↑↓ 选择  ↵ 打开  Esc 返回"
    } else {
        "↑↓ 选择   Enter 打开   Esc 返回   r 刷新"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(MUTED)),
        areas[3],
    );
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
    let history_width = if !app.follow_tail && area.width >= 48 {
        10
    } else {
        0
    };
    let hint_area = Rect {
        width: area.width.saturating_sub(history_width),
        ..area
    };
    let hint = if hint_area.width < 32 {
        "↵ 发送  /sessions"
    } else if hint_area.width < 52 {
        "↵ 发送  /sessions 会话  Esc 退出"
    } else {
        "Enter 发送   ↑↓ 滚动   /sessions 会话   Esc 退出"
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(MUTED)),
        hint_area,
    );
    if history_width > 0 {
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
            ..SessionSummary::default()
        }
    }

    fn ready_app(revision: u64, busy: bool) -> App {
        let mut app = App::new();
        app.connected = true;
        app.apply(ServerMessage::SessionSnapshot {
            session: session(revision, busy),
            events: vec![],
            save_error: None,
        });
        app
    }

    fn load_picker(app: &mut App, sessions: Vec<SessionSummary>) {
        let (requests, mut received) = mpsc::unbounded_channel();
        app.request_sessions(&requests);
        app.receive_sessions(received.try_recv().unwrap(), Ok(sessions));
    }

    #[test]
    fn sessions_command_opens_while_streaming_without_sending_to_model() {
        let mut app = ready_app(10, true);
        app.input = " /sessions ".into();
        app.cursor = app.input.chars().count();
        app.scroll = 12;
        app.follow_tail = false;
        let (outgoing, mut messages) = mpsc::unbounded_channel();
        let (requests, mut list_requests) = mpsc::unbounded_channel();
        send_input(&mut app, &outgoing, &requests).unwrap();
        assert!(messages.try_recv().is_err());
        assert!(app.picker.as_ref().unwrap().loading);
        assert!(app.input.is_empty());
        assert!(app.events.is_empty());
        app.receive_sessions(
            list_requests.try_recv().unwrap(),
            Ok(vec![session(10, true)]),
        );
        assert_eq!(app.picker.as_ref().unwrap().selection.selected(), Some(0));
        app.apply(ServerMessage::SessionEvent {
            session: session(11, true),
            event: ChatEvent::AnswerDelta {
                text: "持续生成".into(),
            },
        });
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        assert!(app.picker.is_none());
        assert!(!app.should_quit);
        assert_eq!(app.scroll, 12);
        assert!(!app.follow_tail);
        assert!(history_text(&app.events).to_string().contains("持续生成"));
    }

    #[test]
    fn switching_waits_for_target_snapshot_and_restores_each_sessions_view() {
        let mut app = ready_app(30, false);
        app.input = "尚未发送的草稿".into();
        app.cursor = 3;
        app.scroll = 7;
        app.follow_tail = false;
        let other = SessionSummary {
            id: "session-2".into(),
            title: "另一个会话".into(),
            ..session(1, false)
        };
        load_picker(&mut app, vec![session(30, false), other.clone()]);
        let (outgoing, mut messages) = mpsc::unbounded_channel();
        let (requests, _) = mpsc::unbounded_channel();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        let request: serde_json::Value =
            serde_json::from_str(messages.try_recv().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(
            request,
            json!({"type": "switch_session", "session_id": "session-2"})
        );
        assert!(!app.can_send());
        assert!(app.is_current_session("session-1"));

        // An old session's final snapshot may arrive before the switch acknowledgment.
        app.apply(ServerMessage::SessionSnapshot {
            session: session(40, false),
            events: vec![ChatEvent::Answer {
                text: "原会话".into(),
            }],
            save_error: None,
        });
        assert!(app.picker.is_some());
        assert_eq!(app.pending_switch.as_deref(), Some("session-2"));
        app.apply(ServerMessage::SessionSnapshot {
            session: other.clone(),
            events: vec![ChatEvent::Answer {
                text: "另一个会话的内容".into(),
            }],
            save_error: None,
        });
        assert!(app.is_current_session("session-2"));
        assert!(app.picker.is_none());
        assert!(app.pending_switch.is_none());
        assert!(app.input.is_empty());
        assert!(app.follow_tail);

        app.apply(ServerMessage::SessionEvent {
            session: session(41, true),
            event: ChatEvent::AnswerDelta {
                text: "过期片段".into(),
            },
        });
        app.apply(ServerMessage::SessionSnapshot {
            session: session(42, false),
            events: vec![],
            save_error: None,
        });
        assert!(app.is_current_session("session-2"));
        assert_eq!(app.events.len(), 1);
        assert!(
            history_text(&app.events)
                .to_string()
                .contains("另一个会话的内容")
        );

        app.input = "在新会话提问".into();
        assert!(app.can_send());
        send_input(&mut app, &outgoing, &requests).unwrap();
        let request: serde_json::Value =
            serde_json::from_str(messages.try_recv().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(request["session_id"], "session-2");
        assert_eq!(request["type"], "user");
        app.apply(ServerMessage::SessionEvent {
            session: SessionSummary {
                revision: 2,
                ..other.clone()
            },
            event: ChatEvent::User {
                text: "在新会话提问".into(),
            },
        });
        app.input = "另一个草稿".into();
        app.cursor = 2;
        load_picker(&mut app, vec![session(40, false), other]);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        switch_selected_session(&mut app, &outgoing);
        app.apply(ServerMessage::SessionSnapshot {
            session: session(40, false),
            events: vec![],
            save_error: None,
        });
        assert!(app.is_current_session("session-1"));
        assert_eq!(app.input, "尚未发送的草稿");
        assert_eq!(app.cursor, 3);
        assert_eq!(app.scroll, 7);
        assert!(!app.follow_tail);
        assert_eq!(app.saved_views["session-2"].input, "另一个草稿");
        assert_eq!(app.saved_views["session-2"].cursor, 2);
    }

    #[test]
    fn failed_switch_keeps_original_chat_and_allows_cancel() {
        let mut app = ready_app(4, false);
        app.input = "保留内容".into();
        let other = SessionSummary {
            id: "missing-session".into(),
            ..session(1, false)
        };
        load_picker(&mut app, vec![other]);
        let (outgoing, _messages) = mpsc::unbounded_channel();
        let (requests, _) = mpsc::unbounded_channel();
        switch_selected_session(&mut app, &outgoing);
        app.apply(ServerMessage::SessionError {
            session_id: Some("missing-session".into()),
            text: "会话不存在".into(),
        });
        assert!(app.pending_switch.is_none());
        assert_eq!(
            app.picker.as_ref().unwrap().error.as_deref(),
            Some("会话不存在")
        );
        assert!(app.is_current_session("session-1"));
        assert_eq!(app.input, "保留内容");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        assert!(!app.should_quit);
        assert!(app.can_send());
    }

    #[test]
    fn picker_ignores_cancelled_requests_and_can_retry_loading_errors() {
        let mut app = ready_app(1, false);
        let (outgoing, mut messages) = mpsc::unbounded_channel();
        let (requests, mut list_requests) = mpsc::unbounded_channel();
        app.request_sessions(&requests);
        let cancelled_id = list_requests.try_recv().unwrap();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        app.request_sessions(&requests);
        let active_id = list_requests.try_recv().unwrap();
        app.receive_sessions(cancelled_id, Ok(vec![session(1, false)]));
        assert!(app.picker.as_ref().unwrap().loading);
        app.receive_sessions(active_id, Err("暂时无法读取".into()));
        assert!(!app.picker.as_ref().unwrap().loading);
        assert!(app.picker.as_ref().unwrap().error.is_some());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        app.receive_sessions(list_requests.try_recv().unwrap(), Ok(vec![]));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &outgoing,
            &requests,
        )
        .unwrap();
        assert!(app.picker.as_ref().unwrap().sessions.is_empty());
        assert!(app.pending_switch.is_none());
        assert!(messages.try_recv().is_err());
    }

    #[test]
    fn picker_is_a_separate_scrollable_screen_at_different_terminal_sizes() {
        use ratatui::backend::TestBackend;
        let mut app = ready_app(1, false);
        app.events.push(ChatEvent::Answer {
            text: "聊天内容不应出现在选择界面".into(),
        });
        let sessions = (0..30)
            .map(|index| SessionSummary {
                id: format!("saved-{index}"),
                title: format!("已保存会话{index:02}"),
                updated_at: index,
                ..session(1, false)
            })
            .collect();
        load_picker(&mut app, sessions);
        app.picker.as_mut().unwrap().selection.select(Some(29));
        for (width, height) in [(80, 24), (42, 12), (24, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            let rendered: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            // Wide characters leave a padding cell in the test buffer.
            let compact: String = rendered.split_whitespace().collect();
            assert!(
                compact.contains("已保存会话00"),
                "{width}x{height}: {rendered}"
            );
            assert!(compact.contains("Esc返回"), "{width}x{height}: {rendered}");
            assert!(!compact.contains("聊天内容不应出现在选择界面"));
            assert!(app.picker.as_ref().unwrap().selection.offset() > 0);
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
