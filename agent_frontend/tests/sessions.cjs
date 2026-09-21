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
  const user = body.messages.filter(m => m.role === 'user').at(-1).content;
  if (user === '模拟错误') {
    res.writeHead(400, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ error: { message: '模拟模型错误', type: 'invalid_request_error' } }));
    return;
  }
  if (user === '工具测试' && body.messages.at(-1).role === 'user') {
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ id: 'fake-tool', object: 'chat.completion', created: 1, model: 'fake',
      choices: [{ index: 0, finish_reason: 'tool_calls', message: { role: 'assistant', content: null,
        tool_calls: [{ id: 'call-1', type: 'function', function: { name: 'get_time_stamp', arguments: '{}' } }] } }] }));
    return;
  }
  const reply = () => {
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ id: 'fake', object: 'chat.completion', created: 1, model: 'fake',
      choices: [{ index: 0, finish_reason: 'stop', message: { role: 'assistant', content: `回答：${user}` } }] }));
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
    MODEL: 'fake', AGENT_HTTP_PORT: String(port), AGENT_TCP_PORT: String(tcp), AGENT_NO_BROWSER: '1',
    AGENT_FRONTEND_DIR: path.join(repo, 'agent_frontend'), AGENT_SKILLS_DIR: path.join(root, 'skills'),
    AGENT_SESSIONS_DIR: path.join(root, 'sessions'), HTTP_PROXY: '', HTTPS_PROXY: '', ALL_PROXY: '',
    http_proxy: '', https_proxy: '', all_proxy: '', NO_PROXY: '*', no_proxy: '*' };
  const binary = process.env.AGENT_TEST_BINARY || path.join(repo, 'agent_backend/target/aarch64-apple-darwin/release/agent');
  async function start() {
    processHandle = spawn(binary, [], { cwd: root, env, stdio: ['ignore', 'pipe', 'pipe'] });
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
  console.log('PASS: real HTTP + WebSocket session creation, background completion, isolation, rapid switching, independent drafts, refresh, reconnect, persisted model context, tool replay, error recovery, mobile layouts.');
  console.log(`Artifacts: ${root}`);
  await stop();
})().catch(error => { console.error(error); console.error(log); process.exitCode = 1; }).finally(async () => {
  for (const reply of held) reply();
  if (browser) await browser.close();
  if (processHandle?.exitCode === null) processHandle.kill('SIGTERM');
  model.closeAllConnections(); model.close();
});
