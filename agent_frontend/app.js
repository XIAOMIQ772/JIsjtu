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

const toolNames = {
  get_courses: "查看课程", get_exam: "查询作业", course_files: "查找课程资料",
  pdf_analysis: "阅读 PDF", mail_fetch: "读取校园邮件", watch_shuiyuan: "打开水源社区",
  watch_eduinfo: "打开教学信息网", bash: "执行任务", read: "读取文件",
  write: "写入文件", edit: "编辑文件", get_num: "查询信息",
};
let socket = null;
let online = false;
let pending = false;
let reconnectTimer = null;
let toastTimer = null;
let hasConnected = false;
const activeTools = new Map();

document.getElementById("today").textContent = new Intl.DateTimeFormat("zh-CN", {
  month: "long", day: "numeric", weekday: "long",
}).format(new Date());

function updateControls() {
  sendEl.disabled = !online || pending || !inputEl.value.trim();
  sendLabelEl.textContent = pending ? "思考中" : "发送";
  activityEl.hidden = !pending;
}

function setStatus(text, isOnline) {
  online = isOnline;
  statusEl.textContent = text;
  statusEl.classList.toggle("online", isOnline);
  statusEl.classList.toggle("offline", !isOnline);
  updateControls();
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
  const now = new Date();
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
      finishTools("已结束"); setPending(false); appendMessage(event.text || "暂时没有回复内容，请再试一次。", "agent"); break;
    case "error":
      finishTools("已中断"); setPending(false); appendNotice(event.text || "处理时遇到问题，请重试。", true); break;
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
  let connection;
  try {
    connection = new WebSocket(`${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/ws`);
  } catch {
    setStatus("连接失败，重试中", false); scheduleReconnect(); return;
  }
  socket = connection;
  connection.addEventListener("open", () => {
    if (socket !== connection) return;
    setStatus("小集在线", true);
    if (hasConnected) toast("已重新连接，小集将从新的对话上下文开始。 ");
    hasConnected = true;
  });
  connection.addEventListener("close", () => {
    if (socket !== connection) return;
    const interrupted = pending;
    finishTools("连接中断"); setPending(false); setStatus("离线 · 重连中", false);
    if (interrupted) appendNotice("连接中断，本次回答未完成。重新连接后，可以再次发送问题。", true);
    else if (hasConnected && messagesEl.childElementCount) appendNotice("连接已断开。重新连接后会开启新的对话上下文，页面上的内容仍会保留。");
    scheduleReconnect();
  });
  connection.addEventListener("error", () => connection.close());
  connection.addEventListener("message", (event) => {
    if (socket !== connection) return;
    let payload;
    try { payload = JSON.parse(event.data); }
    catch { finishTools("已中断"); setPending(false); appendNotice("暂时无法读取小集的回复，请再试一次。", true); return; }
    handle(payload);
  });
}

formEl.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = inputEl.value.trim();
  if (!text || !online || pending) return;
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    toast("连接暂时断开，请稍后再试。"); return;
  }
  try { socket.send(JSON.stringify({ type: "user", text })); }
  catch { toast("发送失败，消息已保留，请稍后再试。"); return; }
  appendMessage(text, "user");
  inputEl.value = ""; resizeInput(); setPending(true); inputEl.focus();
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

document.getElementById("chat-nav").addEventListener("click", () => inputEl.focus());
clearEl.addEventListener("click", () => {
  if (pending) { toast("等小集完成这次回答后，再清空页面吧。"); return; }
  messagesEl.replaceChildren(); activeTools.clear(); messagesEl.hidden = true; welcomeEl.hidden = false;
  readingEl.scrollTop = 0; toast("页面已清空，当前连接中的对话上下文仍保留。");
});

setStatus("连接中", false);
connect();
