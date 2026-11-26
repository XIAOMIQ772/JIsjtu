/* Run after cargo build --release --target aarch64-apple-darwin.
 * PLAYWRIGHT_MODULE may point to an existing Playwright installation.
 * Uses a local fake model; no real credentials or external APIs are used.
 */
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const http = require('node:http');
const net = require('node:net');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const assert = require('node:assert/strict');
const { setTimeout: delay } = require('node:timers/promises');

const repo = path.resolve(__dirname, '../..');
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'jisjtu-session-e2e-'));
const contextBudget = 32768;
const calls = [];
const held = [];
let processHandle, browser;
let log = '';
const errors = [];
const model = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const body = JSON.parse(Buffer.concat(chunks));
  calls.push(body);
  const user = body.messages.filter(m => m.role === 'user').at(-1).content.trim();
  assert.equal(body.stream, true, 'Every model request must use streaming');
  if (user === '模拟错误') {
    res.writeHead(400, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { message: '模拟模型错误', type: 'invalid_request_error' } }));
    return;
  }
  res.setHeader('content-type', 'text/event-stream');
  const chunk = (delta, finish_reason = null) => {
    const bytes = Buffer.from(`data: ${JSON.stringify({ id: 'fake', object: 'chat.completion.chunk', created: 1, model: 'fake',
      choices: [{ index: 0, finish_reason, delta }] })}\n\n`);
    // Exercise UTF-8 decoding when a Chinese character crosses HTTP chunks.
    const split = bytes.findIndex(byte => byte >= 0x80) + 1;
    if (split > 0) { res.write(bytes.subarray(0, split)); res.write(bytes.subarray(split)); }
    else res.write(bytes);
  };
  const finish = (reason = 'stop') => { chunk({}, reason); res.end('data: [DONE]\n\n'); };
  chunk({ role: 'assistant', content: '' });
  if (user === '工具测试' && body.messages.at(-1).role === 'user') {
    chunk({ tool_calls: [{ index: 0, id: 'call-1', type: 'function', function: { name: 'get_time_stamp', arguments: '{' } }] });
    chunk({ tool_calls: [{ index: 0, function: { arguments: '}' } }] });
    finish('tool_calls');
    return;
  }
  const readFixture = /^读取测试资料 ([1-4])$/.exec(user);
  if (readFixture && body.messages.at(-1).role === 'user') {
    const line = Number(readFixture[1]);
    chunk({ tool_calls: [{ index: 0, id: `read-fixture-${line}`, type: 'function', function: {
      name: 'read', arguments: JSON.stringify({ path: path.join(root, 'context-fixture.txt'), offset: line, limit: 1 }),
    } }] });
    finish('tool_calls');
    return;
  }
  if (user === '流式输出测试' || user === 'TCP 流式') {
    chunk({ content: '第一段内容' });
    held.push(() => { chunk({ content: '，后续内容。' }); finish(); });
    return;
  }
  if (user === '模拟流式中断') {
    chunk({ content: '中断前已经生成的内容' });
    res.end('data: [DONE]\n\n');
    return;
  }
  chunk({ content: '回答：' });
  const reply = () => {
    chunk({ content: user });
    finish();
  };
  if (user === 'A 的慢任务') held.push(reply);
  else reply();
});

async function freePort() {
  const s = net.createServer();
  await new Promise(resolve => s.listen(0, '127.0.0.1', resolve));
  const port = s.address().port;
  await new Promise(resolve => s.close(resolve));
  return port;
}

async function until(check, message) {
  for (let i = 0; i < 100; i++) { if (await check()) return; await delay(100); }
  throw new Error(message);
}

(async () => {
  fs.writeFileSync(path.join(root, '.env'), '');
  await new Promise(resolve => model.listen(0, '127.0.0.1', resolve));
  const port = await freePort();
  const tcp = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const env = { ...process.env, OPENAI_API_KEY: 'fake-test-key', OPENAI_BASE_URL: `http://127.0.0.1:${model.address().port}/v1`,
    MODEL: 'fake', AGENT_HTTP_PORT: String(port), AGENT_TCP_PORT: String(tcp), AGENT_NO_BROWSER: '1', AGENT_SSO_AUTO_LOGIN: '0',
    AGENT_BIND_ADDR: '127.0.0.1', AGENT_REMINDERS: '0', AGENT_CONTEXT_BYTES: String(contextBudget),
    AGENT_FRONTEND_DIR: path.join(repo, 'agent_frontend'), AGENT_SKILLS_DIR: path.join(root, 'skills'),
    AGENT_SESSIONS_DIR: path.join(root, 'sessions'), HTTP_PROXY: '', HTTPS_PROXY: '', ALL_PROXY: '',
    http_proxy: '', https_proxy: '', all_proxy: '', NO_PROXY: '*', no_proxy: '*' };
  delete env.AGENT_DREAMING;
  const binary = process.env.AGENT_TEST_BINARY || path.join(repo, 'agent_backend/target/aarch64-apple-darwin/release/agent');
  async function start() {
    processHandle = spawn(binary, ['--web'], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    processHandle.stdout.on('data', data => { log += data; });
    processHandle.stderr.on('data', data => { log += data; });
    await until(async () => {
      if (processHandle.exitCode !== null) throw new Error(log);
      return fetch(base + '/api/sessions').then(r => r.ok).catch(() => false);
    }, 'Backend did not start');
  }
  async function stop() {
    if (processHandle.exitCode === null) {
      const exited = new Promise(resolve => processHandle.once('exit', resolve));
      processHandle.kill('SIGTERM'); await exited;
    }
  }
  await start();
  const assets = await fetch(base + '/style.css?v=sessions-20260921-2');
  assert.equal(assets.headers.get('cache-control'), 'no-store');
  browser = await chromium.launch({ headless: true, channel: process.env.BROWSER_CHANNEL || 'msedge' });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(base);
  await page.getByText('小集在线', { exact: true }).waitFor();
  const sessionA = await page.locator('.session-item.active').getAttribute('data-session-id');
  assert.equal(await page.locator('.session-name').first().evaluate(el => getComputedStyle(el).display), 'block', 'Session CSS must load');
  await page.locator('#input').fill('A 的慢任务');
  await page.locator('#send').click();
  await until(() => held.length === 1, 'Model did not receive A');
  await page.getByText('回答：', { exact: true }).waitFor();
  await page.locator('#input').fill('A 的独立草稿');
  await page.locator('#chat-nav').click();
  await until(() => page.locator('.session-item').count().then(count => count === 2), 'New session was not created');
  await page.getByText('小集在线', { exact: true }).waitFor();
  const sessionB = await page.locator('.session-item.active').getAttribute('data-session-id');
  assert.notEqual(sessionA, sessionB);
  assert.equal(await page.locator('#input').inputValue(), '');
  assert.equal(await page.locator('.message').count(), 0);
  await page.locator('#input').fill('B 的问题');
  await page.locator('#send').click();
  await page.getByText('回答：B 的问题', { exact: true }).waitFor();
  assert(!calls.at(-1).messages.some(m => m.content === 'A 的慢任务'), 'B must not receive A history');
  held.shift()();
  await until(async () => !(await (await fetch(base + '/api/sessions')).json()).find(s => s.id === sessionA).busy, 'A did not finish');
  assert.equal(await page.getByText('回答：A 的慢任务', { exact: true }).count(), 0, 'A reply must not leak into B');
  await page.locator('#input').fill('B 的独立草稿');
  await page.locator(`[data-session-id="${sessionA}"]`).click();
  await page.getByText('回答：A 的慢任务', { exact: true }).waitFor();
  assert.equal(await page.locator('#input').inputValue(), 'A 的独立草稿');
  await page.locator(`[data-session-id="${sessionB}"]`).click();
  await page.getByText('回答：B 的问题', { exact: true }).waitFor();
  assert.equal(await page.locator('#input').inputValue(), 'B 的独立草稿');
  // Fire rapid switches before the server has acknowledged the previous one.
  await page.evaluate(([a, b]) => {
    document.querySelector(`[data-session-id="${a}"]`).click();
    document.querySelector(`[data-session-id="${b}"]`).click();
    document.querySelector(`[data-session-id="${a}"]`).click();
  }, [sessionA, sessionB]);
  await page.getByText('回答：A 的慢任务', { exact: true }).waitFor();
  await page.getByText('小集在线', { exact: true }).waitFor();
  assert.equal(await page.getByText('回答：B 的问题', { exact: true }).count(), 0);
  await page.reload();
  await page.getByText('回答：A 的慢任务', { exact: true }).waitFor();
  await page.locator('#input').fill('继续 A');
  await page.locator('#send').click();
  await page.getByText('回答：继续 A', { exact: true }).waitFor();
  assert(calls.at(-1).messages.some(m => m.role === 'assistant' && m.content === '回答：A 的慢任务'));
  assert(!calls.at(-1).messages.some(m => m.content === 'B 的问题'));
  await stop(); await start();
  await page.getByText('小集在线', { exact: true }).waitFor();
  await page.locator('#input').fill('重启后继续 A');
  await page.locator('#send').click();
  await page.getByText('回答：重启后继续 A', { exact: true }).waitFor();
  assert(calls.at(-1).messages.some(m => m.content === '继续 A'), 'Context must survive backend restart');
  await page.locator('#input').fill('工具测试');
  await page.locator('#send').click();
  await page.getByText('回答：工具测试', { exact: true }).waitFor();
  assert.equal(await page.locator('.tool-state').textContent(), '已完成');
  assert(calls.at(-1).messages.some(m => m.role === 'tool' && m.tool_call_id === 'call-1'));
  await page.locator(`[data-session-id="${sessionB}"]`).click();
  await page.getByText('回答：B 的问题', { exact: true }).waitFor();
  assert.equal(await page.locator('.tool').count(), 0);
  await page.locator(`[data-session-id="${sessionA}"]`).click();
  await page.getByText('回答：工具测试', { exact: true }).waitFor();
  assert.equal(await page.locator('.tool-state').textContent(), '已完成');
  await page.locator('#input').fill('模拟错误');
  await page.locator('#send').click();
  await page.locator('.notice.error').waitFor();
  await page.locator('#input').fill('错误后继续');
  await page.locator('#send').click();
  await page.getByText('回答：错误后继续', { exact: true }).waitFor();

  await page.locator('#input').fill('流式输出测试');
  await page.locator('#send').click();
  await page.getByText('第一段内容', { exact: true }).waitFor();
  assert.equal(held.length, 1, 'The model must still be waiting to finish');
  assert.equal(await page.locator('#send').isDisabled(), true);
  let streamingSnapshot = await (await fetch(`${base}/api/sessions/${sessionA}`)).json();
  assert.equal(streamingSnapshot.events.at(-1).type, 'answer_delta');
  assert.equal(streamingSnapshot.events.at(-1).text, '第一段内容');
  await page.reload();
  await page.getByText('第一段内容', { exact: true }).waitFor();
  assert.equal(await page.getByText('第一段内容', { exact: true }).count(), 1);
  await page.locator(`[data-session-id="${sessionB}"]`).click();
  await page.getByText('回答：B 的问题', { exact: true }).waitFor();
  assert.equal(await page.getByText('第一段内容', { exact: true }).count(), 0);
  await page.locator(`[data-session-id="${sessionA}"]`).click();
  await page.getByText('第一段内容', { exact: true }).waitFor();
  held.shift()();
  await page.getByText('第一段内容，后续内容。', { exact: true }).waitFor();
  assert.equal(await page.getByText('第一段内容，后续内容。', { exact: true }).count(), 1);
  await until(async () => !(await (await fetch(`${base}/api/sessions/${sessionA}`)).json()).session.busy, 'Stream did not finish');
  const saved = JSON.parse(fs.readFileSync(path.join(root, 'sessions', `${sessionA}.json`)));
  assert.equal(saved.events.filter(e => e.type === 'answer' && e.text === '第一段内容，后续内容。').length, 1);
  assert(!saved.events.some(e => e.type === 'answer_delta'), 'Completed streams must not retain duplicate fragments');
  await page.reload();
  await page.getByText('第一段内容，后续内容。', { exact: true }).waitFor();
  assert.equal(await page.getByText('第一段内容，后续内容。', { exact: true }).count(), 1);

  await page.locator('#input').fill('模拟流式中断');
  await page.locator('#send').click();
  await page.getByText('中断前已经生成的内容', { exact: true }).waitFor();
  await page.locator('.notice.error').filter({ hasText: '提前结束' }).waitFor();
  await page.locator('#input').fill('中断后继续');
  await page.locator('#send').click();
  await page.getByText('回答：中断后继续', { exact: true }).waitFor();
  assert(calls.at(-1).messages.some(m => m.role === 'assistant' && m.content === '中断前已经生成的内容'));

  const tcpClient = net.createConnection({ host: '127.0.0.1', port: tcp });
  tcpClient.setEncoding('utf8');
  let tcpOutput = '';
  tcpClient.on('data', data => { tcpOutput += data; });
  try {
    await until(() => tcpOutput === 'you:', 'TCP prompt did not arrive');
    tcpClient.write('TCP 流式\n');
    await until(() => tcpOutput.includes('model:第一段内容'), 'TCP did not stream the first fragment');
    assert(!tcpOutput.includes('后续内容'));
    held.shift()();
    await until(() => tcpOutput.endsWith('第一段内容，后续内容。\nyou:'), 'TCP did not finish streaming');
    assert.equal(tcpOutput.match(/model:/g).length, 1);
  } finally { tcpClient.destroy(); }

  for (const [width, height] of [[1280, 800], [768, 1024], [390, 844], [320, 740]]) {
    await page.setViewportSize({ width, height });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `Horizontal overflow at ${width}`);
    const composer = await page.locator('#composer').boundingBox();
    assert(composer.y >= 0 && composer.y + composer.height <= height, `Composer does not fit ${width}`);
    assert(await page.locator('#session-list').isVisible());
    await page.screenshot({ path: path.join(root, `sessions-${width}.png`), fullPage: true });
  }
  assert.deepEqual(errors, []);
  const stalled = await browser.newPage();
  await stalled.routeWebSocket('**/ws?*', () => {});
  await stalled.goto(base);
  await stalled.locator('#connection-notice-text').filter({ hasText: '连接超时' }).waitFor({ timeout: 15000 });
  assert(await stalled.locator('#retry-connection').isVisible(), 'Stalled connection must offer retry');
  await stalled.close();
  await page.setViewportSize({ width: 1280, height: 800 });
  await testDreaming(page, base, async () => {
    await stop();
    env.AGENT_DREAMING = '1';
    await start();
    await page.reload();
    await page.getByText('小集在线', { exact: true }).waitFor();
  });
  console.log('PASS: real SSE model streaming, HTTP + WebSocket sessions, partial-answer refresh and switching, persisted answers without duplicates, fragmented tools, interruption recovery, incremental TCP output, mobile layouts.');
  console.log('PASS: opt-in preference consolidation, source deletion, durable forgetting and ephemeral Web memory commands.');
  console.log(`Artifacts: ${root}`);
  await stop();
})().catch(error => { console.error(error); console.error(log); process.exitCode = 1; }).finally(async () => {
  for (const reply of held) reply();
  if (browser) await browser.close();
  if (processHandle?.exitCode === null) processHandle.kill('SIGTERM');
  model.closeAllConnections(); model.close();
});

async function testDreaming(page, base, restart) {
  const active = page.locator('.session-item.active');
  const memory = page.locator('#memory-status');
  const probe = '说明一下什么是函数';
  const privateText = '来源会话独有的文字 memory-e2e-1247';
  const chinese = '默认用中文回答';
  const bullets = '默认分点回答';
  async function newChat() {
    const previous = await active.getAttribute('data-session-id');
    await page.locator('#chat-nav').click();
    await until(async () => {
      const id = await active.getAttribute('data-session-id');
      return id && id !== previous;
    }, 'New memory test session did not open');
    await page.getByText('小集在线', { exact: true }).waitFor();
    assert.equal(await memory.count(), 0);
    return active.getAttribute('data-session-id');
  }
  async function send(text) {
    await page.locator('#input').fill(text);
    await page.locator('#send').click();
  }
  async function ask(text, expectedCalls = 1) {
    const index = calls.length;
    const id = await active.getAttribute('data-session-id');
    await send(text);
    await until(async () => calls.length > index &&
      !(await (await fetch(`${base}/api/sessions/${id}`)).json()).session.busy, 'Model turn did not finish');
    assert.equal(calls.length, index + expectedCalls, 'Unexpected number of model requests');
    return JSON.stringify(calls.at(-1).messages);
  }
  async function status(command, match) {
    const index = calls.length;
    await send(command);
    await memory.filter({ hasText: match }).waitFor();
    assert.equal(index, calls.length, 'Memory commands are handled locally');
    return memory.innerText();
  }
  async function probeFresh() {
    await newChat();
    return ask(probe);
  }
  const baseline = await probeFresh();
  const disabledSource = await active.getAttribute('data-session-id');
  await ask('以后请用中文回答');
  assert.equal(await probeFresh(), baseline, 'Cross-session memory must be opt-in');
  assert.equal((await fetch(`${base}/api/sessions/${disabledSource}`, { method: 'DELETE' })).status, 204);

  await restart();
  const source = await newChat();
  await ask(privateText);
  await ask('请分点回答');
  await ask('请分点回答');
  assert.equal(await probeFresh(), baseline, 'Repeated requests in a single conversation count as one source');
  await page.locator(`[data-session-id="${source}"]`).click();
  await ask('以后请用中文回答');
  const secondSource = await newChat();
  await ask('请分点回答');

  await restart();
  const recalled = await probeFresh();
  assert(recalled.includes(chinese) && recalled.includes(bullets), 'The next model request should use both preferences after a restart');
  assert(!recalled.includes(privateText), 'Only the recognized preferences may cross sessions');
  const report = await status('/memory', chinese);
  assert(report.includes(bullets));
  const reader = await active.getAttribute('data-session-id');
  const record = fs.readFileSync(path.join(root, 'sessions', `${reader}.json`), 'utf8');
  assert(!record.includes(chinese) && !record.includes('/memory'), 'Recall and command output must remain ephemeral');

  assert.equal((await fetch(`${base}/api/sessions/${source}`, { method: 'DELETE' })).status, 204);
  assert.equal(await probeFresh(), baseline, 'Deleting the source must revoke the explicit and corroborated preferences');
  await page.locator(`[data-session-id="${secondSource}"]`).click();
  await ask('以后请用中文回答');
  assert((await probeFresh()).includes(chinese));
  await status('/forget', /清空|清除|忘记/);
  assert.equal(await probeFresh(), baseline);
  await restart();
  assert.equal(await probeFresh(), baseline, 'Cleared preferences must stay cleared across restarts');

  const longChat = await newChat();
  const firstCall = calls.length;
  const lines = [1, 2, 3, 4].map(n => '课'.repeat(2400) + `fixture-middle-${n}` + '表'.repeat(2400));
  fs.writeFileSync(path.join(root, 'context-fixture.txt'), lines.join('\n'));
  for (let n = 1; n <= lines.length; n++) await ask(`读取测试资料 ${n}`, 2);
  await restart();
  assert.equal(await active.getAttribute('data-session-id'), longChat);
  const resumed = await ask('汇总刚才的资料');
  assert(resumed.includes('汇总刚才的资料') && resumed.includes('回答：读取测试资料 4'));
  assert(!resumed.includes('fixture-middle-1'), 'An old tool payload must be compacted on the wire');
  const history = JSON.parse(fs.readFileSync(path.join(root, 'sessions', `${longChat}.json`)));
  for (const [index, line] of lines.entries()) {
    const result = history.messages.find(m => m.role === 'tool' && m.tool_call_id === `read-fixture-${index + 1}`);
    assert(result?.content.includes(line), 'Persist the original tool result even when requests are compacted');
  }
  const beforeOversized = calls.length;
  await send('输入'.repeat(6000)); // 36,000 UTF-8 bytes before the message envelope.
  await page.locator('.notice.error').filter({ hasText: /上下文预算/ }).waitFor();
  assert.equal(calls.length, beforeOversized, 'Unsendable input must fail locally');
  await ask('恢复后继续提问');
  await page.getByText('回答：恢复后继续提问', { exact: true }).waitFor();
  assert.equal(await active.getAttribute('data-session-id'), longChat);
  for (const { messages } of calls.slice(firstCall)) {
    assert(Buffer.byteLength(JSON.stringify(messages)) <= contextBudget, 'Projected messages exceed the UTF-8 byte budget');
    const pending = new Set();
    for (const message of messages) {
      if (message.role === 'tool') {
        assert(pending.delete(message.tool_call_id), 'Orphan tool result on the wire');
      } else {
        assert.equal(pending.size, 0, 'Missing tool response on the wire');
        for (const call of message.tool_calls || []) pending.add(call.id);
      }
    }
    assert.equal(pending.size, 0, 'Incomplete final tool group on the wire');
  }
  console.log('PASS: compacted model requests stay in budget, preserve paired tools and resume with full on-disk history.');
}
