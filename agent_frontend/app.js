"use strict";

const statusEl = document.getElementById("status");
const messagesEl = document.getElementById("messages");
const readingEl = document.getElementById("reading-area");
const welcomeEl = document.getElementById("welcome");
const formEl = document.getElementById("composer");
const inputEl = document.getElementById("input");
const sendEl = document.getElementById("send");
const sendLabelEl = document.getElementById("send-label");
const activityEl = document.getElementById("activity");
const activityTextEl = document.getElementById("activity-text");
const clearEl = document.getElementById("clear");
const toastEl = document.getElementById("toast");
const connectionNoticeEl = document.getElementById("connection-notice");
const connectionNoticeTextEl = document.getElementById("connection-notice-text");

const toolNames = {
  get_courses: "查看课程", get_exam: "查询作业", course_files: "查找课程资料",
  pdf_analysis: "阅读 PDF", mail_fetch: "读取校园邮件", watch_shuiyuan: "打开水源社区",
  watch_eduinfo: "打开教学信息网", bash: "执行任务", read: "读取文件",
  write: "写入文件", edit: "编辑文件", get_num: "查询信息",
  get_time_stamp: "获取当前时间", open_usual_website: "打开网页",
};
let socket = null;
let online = false;
let pending = false;
let reconnectTimer = null;
let toastTimer = null;
let handshakeTimer = null;
const activeTools = new Map();
const sessions = new Map();
const sessionListEl = document.getElementById("session-list");
const sessionEmptyEl = document.getElementById("session-empty");
const sessionTitleEl = document.getElementById("session-title");
const newSessionEl = document.getElementById("chat-nav");
let selectedId = null;
let readyId = null;
let creating = false;
let refreshing = false;
let replaying = false;
let listSignature = "";
let displayedTime = null;

document.getElementById("today").textContent = new Intl.DateTimeFormat("zh-CN", {
  month: "long", day: "numeric", weekday: "long",
}).format(new Date());

function updateControls() {
  sendEl.disabled = !online || readyId !== selectedId || !selectedId || pending || !inputEl.value.trim();
  sendLabelEl.textContent = pending ? "思考中" : "发送";
  activityEl.hidden = !pending;
}

function setStatus(text, isOnline) {
  online = isOnline;
  statusEl.textContent = text;
  statusEl.classList.toggle("online", isOnline);
  statusEl.classList.toggle("offline", !isOnline);
  if (isOnline) connectionNoticeEl.hidden = true;
  updateControls();
}

function connectionProblem(text) {
  connectionNoticeTextEl.textContent = text;
  connectionNoticeEl.hidden = false;
}

function waitForSession(connection) {
  clearTimeout(handshakeTimer);
  handshakeTimer = setTimeout(() => {
    if (socket !== connection || readyId === selectedId) return;
    connectionProblem("连接超时。请确认启动服务的终端仍在运行；可点击重新连接。");
    setStatus("连接超时", false);
    connection.close();
  }, 10000);
}

function setPending(value, text = "小集正在整理思路…") {
  pending = value;
  activityTextEl.textContent = text;
  updateControls();
}

function resizeInput() {
  inputEl.style.height = "auto";
  inputEl.style.height = `${Math.min(inputEl.scrollHeight, 160)}px`;
  updateControls();
}

function showConversation() {
  welcomeEl.hidden = true;
  messagesEl.hidden = false;
}

function atBottom() {
  return readingEl.scrollHeight - readingEl.scrollTop - readingEl.clientHeight < 100;
}

function scrollToBottom(force = false, wasAtBottom = true) {
  if (replaying) return;
  if (force || wasAtBottom) readingEl.scrollTop = readingEl.scrollHeight;
}

function toast(text) {
  clearTimeout(toastTimer);
  toastEl.textContent = text;
  toastEl.hidden = false;
  toastTimer = setTimeout(() => { toastEl.hidden = true; }, 3600);
}

// Render a small, safe Markdown subset. Model output never becomes raw HTML.
function inline(parent, text) {
  const tokens = /(\*\*([^*]+)\*\*|`([^`]+)`|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\))/g;
  let start = 0;
  for (const match of text.matchAll(tokens)) {
    parent.append(document.createTextNode(text.slice(start, match.index)));
    const el = document.createElement(match[2] ? "strong" : match[3] ? "code" : "a");
    el.textContent = match[2] || match[3] || match[4];
    if (match[5]) {
      el.href = match[5];
      el.target = "_blank";
      el.rel = "noopener noreferrer";
    }
    parent.appendChild(el);
    start = match.index + match[0].length;
  }
  parent.append(document.createTextNode(text.slice(start)));
}

function renderAnswer(parent, text) {
  const lines = String(text).replace(/\r\n/g, "\n").split("\n");
  let paragraph = [];
  let list = null;
  const flush = () => {
    if (!paragraph.length) return;
    const p = document.createElement("p");
    inline(p, paragraph.join("\n"));
    parent.appendChild(p);
    paragraph = [];
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      flush(); list = null;
      const codeLines = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i])) codeLines.push(lines[i]);
      const pre = document.createElement("pre");
      const code = document.createElement("code");
      code.textContent = codeLines.join("\n");
      pre.appendChild(code); parent.appendChild(pre);
    } else if (/^#{1,6}\s+/.test(line)) {
      flush(); list = null;
      const heading = line.match(/^(#{1,6})\s+(.*)$/);
      const el = document.createElement(`h${Math.min(heading[1].length + 1, 4)}`);
      inline(el, heading[2]); parent.appendChild(el);
    } else if (/^\s*(?:[-*+] |\d+\. )/.test(line)) {
      flush();
      const item = line.match(/^\s*([-*+]|\d+\.)\s+(.*)$/);
      const tag = /\d/.test(item[1]) ? "OL" : "UL";
      if (!list || list.tagName !== tag) {
        list = document.createElement(tag.toLowerCase());
        if (tag === "OL") list.start = parseInt(item[1], 10);
        parent.appendChild(list);
      }
      const li = document.createElement("li"); inline(li, item[2]); list.appendChild(li);
    } else if (/^>\s?/.test(line)) {
      flush(); list = null;
      const quote = document.createElement("blockquote"); inline(quote, line.replace(/^>\s?/, "")); parent.appendChild(quote);
    } else if (!line.trim()) {
      flush(); list = null;
    } else {
      list = null; paragraph.push(line);
    }
  }
  flush();
}

function appendMessage(text, role) {
  const wasAtBottom = atBottom();
  showConversation();
  const article = document.createElement("article");
  article.className = `message ${role}`;
  const avatar = document.createElement("span"); avatar.className = "avatar";
  avatar.textContent = role === "user" ? "我" : "集"; avatar.setAttribute("aria-hidden", "true");
  const body = document.createElement("div"); body.className = "message-body";
  const label = document.createElement("div"); label.className = "message-label";
  label.textContent = role === "user" ? "你" : "小集";
  const now = new Date(displayedTime || Date.now());
  const time = document.createElement("time"); time.dateTime = now.toISOString();
  time.textContent = now.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
  label.appendChild(time);
  const content = document.createElement("div"); content.className = "message-text";
  if (role === "agent") renderAnswer(content, text);
  else content.textContent = text;
  body.append(label, content); article.append(avatar, body); messagesEl.appendChild(article);
  scrollToBottom(role === "user", wasAtBottom);
}

function appendNotice(text, isError = false) {
  const wasAtBottom = atBottom();
  showConversation();
  const el = document.createElement("div");
  el.className = isError ? "notice error" : "notice";
  el.textContent = text;
  messagesEl.appendChild(el);
  scrollToBottom(false, wasAtBottom);
}

function createTool(name, args) {
  showConversation();
  const details = document.createElement("details"); details.className = "tool";
  const summary = document.createElement("summary");
  summary.append(document.createTextNode(toolNames[name] || name || "处理任务"));
  const state = document.createElement("span"); state.className = "tool-state"; state.textContent = "进行中";
  summary.appendChild(state);
  const pre = document.createElement("pre"); pre.textContent = args ? `请求：${args}` : "";
  details.append(summary, pre); messagesEl.appendChild(details);
  return { details, state, pre };
}

function finishTools(label) {
  for (const queue of activeTools.values()) {
    for (const tool of queue) tool.state.textContent = label;
  }
  activeTools.clear();
}

function handle(event) {
  if (!event || typeof event !== "object") {
    appendNotice("收到的消息格式不正确，请重试。", true); setPending(false); return;
  }
  const wasAtBottom = atBottom();
  switch (event.type) {
    case "user":
      appendMessage(event.text || "", "user"); break;
    case "tool_call": {
      const tool = createTool(event.name, event.arguments);
      const queue = activeTools.get(event.name) || [];
      queue.push(tool); activeTools.set(event.name, queue);
      setPending(true, `小集正在${toolNames[event.name] || "处理任务"}…`);
      scrollToBottom(false, wasAtBottom);
      break;
    }
    case "tool_result": {
      const queue = activeTools.get(event.name);
      const tool = queue?.shift() || createTool(event.name, "");
      if (!queue?.length) activeTools.delete(event.name);
      tool.state.textContent = event.ok ? "已完成" : "未完成";
      tool.details.classList.toggle("fail", !event.ok);
      tool.pre.textContent += `${tool.pre.textContent ? "\n\n" : ""}结果：${event.preview || "无返回内容"}`;
      if (pending) activityTextEl.textContent = "小集正在整理结果…";
      scrollToBottom(false, wasAtBottom);
      break;
    }
    case "answer":
      finishTools("已结束"); appendMessage(event.text || "暂时没有回复内容，请再试一次。", "agent"); break;
    case "error":
      finishTools("已中断"); setPending(false); appendNotice(event.text || "处理时遇到问题，请重试。", true); break;
    case "done":
      finishTools("已结束"); setPending(false); break;
    default:
      appendNotice("收到暂不支持的消息，请刷新页面后重试。", true);
      finishTools("已中断"); setPending(false);
  }
}

function scheduleReconnect() {
  clearTimeout(reconnectTimer);
  reconnectTimer = setTimeout(connect, 2000);
}

function connect() {
  if (!selectedId) return;
  clearTimeout(reconnectTimer);
  const old = socket;
  socket = null;
  if (old) old.close();
  readyId = null;
  setStatus("连接中", false);
  let connection;
  try {
    connection = new WebSocket(`${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws?session_id=${encodeURIComponent(selectedId)}`);
  } catch {
    setStatus("连接失败，重试中", false); scheduleReconnect(); return;
  }
  socket = connection;
  waitForSession(connection);
  connection.addEventListener("open", () => {
    if (socket !== connection) return;
    setStatus("恢复对话中", false);
    // Selection may have changed while the socket was connecting.
    connection.send(JSON.stringify({ type: "switch_session", session_id: selectedId }));
  });
  connection.addEventListener("close", () => {
    if (socket !== connection) return;
    clearTimeout(handshakeTimer);
    readyId = null;
    setStatus("离线 · 重连中", false);
    if (connectionNoticeEl.hidden) connectionProblem("暂时无法连接小集。请保持启动服务的终端运行，正在自动重连…");
    scheduleReconnect();
  });
  connection.addEventListener("error", () => connection.close());
  connection.addEventListener("message", (event) => {
    if (socket !== connection) return;
    let payload;
    try { payload = JSON.parse(event.data); }
    catch { toast("回复格式异常，正在重新同步会话。"); connection.close(); return; }
    try { receive(payload); }
    catch (error) {
      console.error("会话同步失败", error);
      connectionProblem("会话同步失败，请重新连接。若刚升级，请刷新页面。");
      connection.close();
    }
  });
}

function rememberSelection() {
  try { localStorage.setItem("jisjtu.active-session", selectedId); } catch { /* Storage may be disabled. */ }
}

function upsert(summary) {
  let session = sessions.get(summary.id);
  if (!session) {
    session = { events: [], draft: "", scroll: null, loaded: false, unread: false, eventRevision: -1, sending: false };
    sessions.set(summary.id, session);
  }
  if ((summary.revision ?? 0) >= (session.revision ?? -1)) Object.assign(session, summary);
  return session;
}

function renderSessionList() {
  const ordered = [...sessions.values()].sort((a, b) => b.updated_at - a.updated_at);
  const signature = JSON.stringify(ordered.map(s => [s.id, s.title, s.busy, s.unread, s.updated_at, s.id === selectedId]));
  if (signature === listSignature) return;
  listSignature = signature;
  const focusedId = document.activeElement?.dataset.sessionId;
  sessionListEl.replaceChildren();
  for (const session of ordered) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `session-item${session.id === selectedId ? " active" : ""}`;
    button.dataset.sessionId = session.id;
    button.setAttribute("aria-current", session.id === selectedId ? "true" : "false");
    button.title = session.title;
    const title = document.createElement("span"); title.className = "session-name"; title.textContent = session.title;
    const meta = document.createElement("span"); meta.className = "session-meta";
    const date = new Date(session.updated_at).toLocaleString("zh-CN", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
    meta.textContent = session.busy ? "小集处理中…" : session.unread ? "有新回复" : date;
    button.classList.toggle("running", session.busy);
    button.append(title, meta);
    button.addEventListener("click", () => selectSession(session.id));
    sessionListEl.appendChild(button);
    if (focusedId === session.id) button.focus({ preventScroll: true });
  }
  sessionEmptyEl.hidden = ordered.length > 0;
  sessionEmptyEl.textContent = "还没有对话，开启新的一页吧。";
}

function renderSession(session, preserveScroll = false) {
  const scroll = preserveScroll ? readingEl.scrollTop : session.scroll;
  const bottom = preserveScroll && atBottom();
  replaying = true;
  messagesEl.replaceChildren(); activeTools.clear();
  welcomeEl.hidden = session.events.length > 0;
  messagesEl.hidden = !session.events.length;
  for (const event of session.events) {
    displayedTime = event.at || session.create_time;
    handle(event);
  }
  displayedTime = null;
  replaying = false;
  if (!session.busy) finishTools("已中断");
  setPending(session.busy || session.sending);
  sessionTitleEl.textContent = session.title;
  readingEl.scrollTop = bottom || scroll === null ? readingEl.scrollHeight : scroll;
}

function selectSession(id) {
  const session = sessions.get(id);
  if (!session || (selectedId === id && readyId === id)) return;
  const previous = sessions.get(selectedId);
  if (previous) { previous.draft = inputEl.value; previous.scroll = readingEl.scrollTop; }
  selectedId = id; readyId = null; session.unread = false;
  rememberSelection();
  inputEl.value = session.draft;
  renderSession(session); resizeInput(); renderSessionList();
  setStatus("切换对话中", false);
  if (socket?.readyState === WebSocket.OPEN) {
    waitForSession(socket);
    socket.send(JSON.stringify({ type: "switch_session", session_id: id }));
  } else if (!socket || socket.readyState !== WebSocket.CONNECTING) connect();
}

function receive(payload) {
  if (payload.type === "session_error") {
    if (!payload.session_id || payload.session_id === selectedId) {
      readyId = null; setStatus("会话加载失败", false);
      toast(payload.text || "无法打开这个会话，请刷新列表或新建对话。");
    }
    return;
  }
  if (payload.type === "request_error") {
    const session = sessions.get(payload.session_id);
    if (session) session.sending = false;
    if (payload.session_id === selectedId) { setPending(session?.busy || false); toast(payload.text); }
    return;
  }
  if (!payload.session || !["session_snapshot", "session_event"].includes(payload.type)) return;
  const session = upsert(payload.session);
  const selected = session.id === selectedId;
  if (payload.type === "session_snapshot") {
    if (payload.session.revision < session.eventRevision) return;
    const changed = payload.session.revision !== session.eventRevision || !session.loaded;
    session.events = payload.events || []; session.loaded = true;
    session.eventRevision = payload.session.revision; session.sending = false;
    if (selected) {
      const wasReady = readyId === selectedId;
      readyId = selectedId;
      clearTimeout(handshakeTimer);
      if (changed || !wasReady) renderSession(session, wasReady);
      setStatus("小集在线", true);
      if (payload.save_error) toast(payload.save_error);
    }
  } else {
    if (payload.session.revision <= session.eventRevision) return;
    session.events.push(payload.event); session.eventRevision = payload.session.revision;
    session.loaded = true;
    if (payload.event.type === "user") {
      if (session.sending) {
        session.draft = "";
        if (selected) { inputEl.value = ""; resizeInput(); }
      }
      session.sending = false;
    }
    if (selected) {
      displayedTime = payload.event.at;
      handle(payload.event); displayedTime = null;
      setPending(session.busy || session.sending, activityTextEl.textContent);
      sessionTitleEl.textContent = session.title;
    } else session.unread = true;
  }
  renderSessionList();
}

async function api(path, options) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10000);
  try {
  const response = await fetch(path, { ...options, cache: "no-store", signal: controller.signal });
  if (!response.ok) throw new Error(await response.text() || `请求失败（${response.status}）`);
  if (!response.headers.get("content-type")?.includes("application/json")) throw new Error("请启动支持 sessions 的新版后端。");
  return await response.json();
  } finally { clearTimeout(timeout); }
}

async function refreshSessions(showError = false) {
  if (refreshing) return;
  refreshing = true;
  try {
    const list = await api("/api/sessions");
    for (const summary of list) {
      const previous = sessions.get(summary.id);
      const changed = previous && summary.revision > previous.revision;
      const session = upsert(summary);
      if (changed && summary.id !== selectedId) session.unread = true;
    }
    renderSessionList();
    if (!selectedId) {
      let saved;
      try { saved = localStorage.getItem("jisjtu.active-session"); } catch { /* optional */ }
      if (list.length) selectSession(sessions.has(saved) ? saved : list[0].id);
      else await createSession();
    }
  } catch (error) {
    sessionEmptyEl.textContent = "对话读取失败，点击刷新重试。";
    if (!selectedId) setStatus("会话服务不可用", false);
    connectionProblem(`无法读取会话：${error.message}。请确认新版后端仍在运行。`);
    if (showError) toast(error.message);
  } finally { refreshing = false; }
}

async function createSession() {
  if (creating) return;
  creating = true; newSessionEl.disabled = true; clearEl.disabled = true;
  try {
    const summary = await api("/api/sessions", { method: "POST" });
    upsert(summary); selectSession(summary.id); inputEl.focus();
  } catch (error) { toast(`无法新建对话：${error.message}`); }
  finally { creating = false; newSessionEl.disabled = false; clearEl.disabled = false; }
}

formEl.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = inputEl.value.trim();
  if (!text || !online || pending || readyId !== selectedId) return;
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    toast("连接暂时断开，请稍后再试。"); return;
  }
  try { socket.send(JSON.stringify({ type: "user", text, session_id: selectedId })); }
  catch { toast("发送失败，消息已保留，请稍后再试。"); return; }
  sessions.get(selectedId).sending = true;
  sessions.get(selectedId).draft = inputEl.value;
  setPending(true); inputEl.focus();
});

inputEl.addEventListener("input", resizeInput);
inputEl.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing && event.keyCode !== 229) {
    event.preventDefault(); formEl.requestSubmit();
  }
});

document.querySelectorAll("[data-prompt]").forEach((button) => {
  button.addEventListener("click", () => {
    if (inputEl.value.trim()) {
      inputEl.value += `\n${button.dataset.prompt}`;
      toast("已将问题补充到输入框，可以编辑后发送。");
    } else inputEl.value = button.dataset.prompt;
    resizeInput(); inputEl.focus();
    inputEl.setSelectionRange(inputEl.value.length, inputEl.value.length);
  });
});

newSessionEl.addEventListener("click", createSession);
clearEl.addEventListener("click", createSession);
document.getElementById("refresh-sessions").addEventListener("click", () => refreshSessions(true));
document.getElementById("retry-connection").addEventListener("click", () => {
  if (selectedId) connect();
  else refreshSessions(true);
});

setStatus("连接中", false);
refreshSessions(true);
setInterval(() => { if (!document.hidden) refreshSessions(); }, 5000);
document.addEventListener("visibilitychange", () => { if (!document.hidden) refreshSessions(); });
