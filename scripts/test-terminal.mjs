// End-to-end checks for browser terminals. Run after `cargo build --bin ownnas`
// (set OWNNAS_TARGET to use a target directory other than ./target/debug). Needs Node 22+.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const binary = path.join(process.env.OWNNAS_TARGET || path.join(repo, 'target/debug'), 'ownnas');
const PASSWORD = 'integration-test-passphrase';
const TERMINAL_PASSWORD = 'terminal-passphrase';

const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'ownnas-terminal-'));
const library = path.join(tmp, 'library');
await fs.mkdir(path.join(library, 'proj'), { recursive: true });
const port = await new Promise((resolve) => {
  const server = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = server.address();
    server.close(() => resolve(port));
  });
});
const base = `http://127.0.0.1:${port}`;
const server = spawn(binary, ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`,
  '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env: { ...process.env, OWNNAS_PASSWORD: PASSWORD } });

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let cookie = '';

async function call(method, url, body, headers = {}) {
  const response = await fetch(base + url, {
    method,
    headers: { cookie, 'x-ownnas': '1', ...(body ? { 'content-type': 'application/json' } : {}), ...headers },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  let data = {};
  try { data = JSON.parse(text); } catch { data = { text }; }
  return { status: response.status, data, headers: response.headers };
}

async function login() {
  const response = await fetch(`${base}/api/login`, {
    method: 'POST',
    headers: { 'x-ownnas': '1', 'content-type': 'application/json' },
    body: JSON.stringify({ username: 'admin', password: PASSWORD }),
  });
  assert.equal(response.status, 200);
  cookie = response.headers.get('set-cookie').split(';')[0];
}

/// Opens a terminal socket and collects its output as text.
function attach(id, origin = base) {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/api/terminals/ws?id=${id}`, { headers: { cookie, origin } });
  socket.binaryType = 'arraybuffer';
  const state = { socket, text: '', messages: [], opened: false, closed: false };
  socket.addEventListener('open', () => { state.opened = true; });
  socket.addEventListener('close', () => { state.closed = true; });
  socket.addEventListener('error', () => {});
  socket.addEventListener('message', (event) => {
    if (typeof event.data === 'string') state.messages.push(JSON.parse(event.data));
    else state.text += Buffer.from(event.data).toString('utf8');
  });
  return state;
}

async function until(check, what, ms = 10000) {
  const deadline = Date.now() + ms;
  while (!check()) {
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`);
    await sleep(50);
  }
}

try {
  for (let i = 0; ; i++) {
    try {
      if ((await fetch(`${base}/api/health`)).ok) break;
    } catch {}
    if (i > 100) throw new Error('server did not start');
    await sleep(100);
  }
  await login();

  // Off by default.
  assert.equal((await call('GET', '/api/me')).data.terminal, false);
  let response = await call('POST', '/api/terminals', { path: 'proj' });
  assert.equal(response.status, 403);
  assert.match(response.data.error, /turned off/);

  // Changing settings needs the account password.
  response = await call('POST', '/api/terminal/config', { accountPassword: 'wrong', enabled: true });
  assert.equal(response.status, 403);
  response = await call('POST', '/api/terminal/config', { accountPassword: PASSWORD, enabled: true, password: 'short' });
  assert.equal(response.status, 400, 'terminal passwords follow the account password rules');
  response = await call('POST', '/api/terminal/config', { accountPassword: PASSWORD, enabled: true, password: TERMINAL_PASSWORD });
  assert.equal(response.status, 200, JSON.stringify(response.data));
  assert.equal((await call('GET', '/api/me')).data.terminal, true);
  const config = (await call('GET', '/api/terminal/config')).data;
  assert.equal(config.enabled, true);
  assert.equal(config.passwordSet, true);
  assert.equal(config.unlocked, false);
  assert.equal(config.secureTransport, true, 'loopback counts as a local network');

  // The terminal password is required before anything opens.
  response = await call('POST', '/api/terminals', { path: 'proj' });
  assert.equal(response.status, 403);
  assert.equal(response.data.locked, true);
  assert.equal((await call('POST', '/api/terminal/unlock', { password: 'nope' })).status, 403);
  assert.equal((await call('POST', '/api/terminal/unlock', { password: TERMINAL_PASSWORD })).status, 200);

  // Plain HTTP from the internet (through a local proxy) is refused.
  response = await call('POST', '/api/terminals', { path: 'proj' }, { 'x-forwarded-for': '8.8.8.8' });
  assert.equal(response.status, 403);
  assert.match(response.data.error, /HTTPS/);
  response = await call('GET', '/api/terminals', null, { 'x-forwarded-for': '8.8.8.8', 'x-forwarded-proto': 'https' });
  assert.equal(response.status, 200, 'HTTPS at the proxy is accepted');

  // Paths outside the library are refused.
  assert.equal((await call('POST', '/api/terminals', { path: '../' })).status, 403);

  response = await call('POST', '/api/terminals', { path: 'proj', cols: 80, rows: 24 });
  assert.equal(response.status, 200, JSON.stringify(response.data));
  const id = response.data.terminal.id;
  assert.equal(response.data.terminal.path, 'proj');
  assert.equal(response.data.terminal.running, true);

  // Cross-site WebSocket connections are refused.
  const foreign = attach(id, 'http://evil.example');
  await until(() => foreign.closed, 'cross-site socket to close');
  assert.equal(foreign.opened, false);

  const first = attach(id);
  await until(() => first.opened, 'socket to open');
  first.socket.send(JSON.stringify({ type: 'resize', cols: 120, rows: 40 }));
  first.socket.send(new TextEncoder().encode('echo hello-$((40+2)); pwd; stty size\r'));
  await until(() => first.text.includes('hello-42') && first.text.includes('40 120'), 'command output');
  assert.match(first.text, /proj/);
  first.socket.send(JSON.stringify({ type: 'input', data: 'echo second-$((1+1))\r' }));
  await until(() => first.text.includes('second-2'), 'text input');
  first.socket.close();
  await until(() => first.closed, 'socket to close');

  // The shell keeps running without a browser, and a new connection replays its output.
  first.socket = null;
  const again = attach(id);
  await until(() => again.text.includes('hello-42'), 'scrollback replay');
  const listed = (await call('GET', '/api/terminals')).data.terminals;
  assert.equal(listed.length, 1);
  assert.equal(listed[0].running, true);
  again.socket.send(new TextEncoder().encode('exit 7\r'));
  await until(() => again.messages.some((m) => m.type === 'exit'), 'exit message');
  assert.equal(again.messages.find((m) => m.type === 'exit').code, 7);
  assert.equal((await call('GET', '/api/terminals')).data.terminals[0].running, false);
  assert.equal((await call('DELETE', `/api/terminals?id=${id}`)).status, 200);
  assert.deepEqual((await call('GET', '/api/terminals')).data.terminals, []);

  // The terminal page may use inline styles (for xterm.js); the main page may not.
  const page = await fetch(`${base}/terminal?id=x`);
  assert.match(page.headers.get('content-security-policy'), /style-src 'self' 'unsafe-inline'/);
  assert.match(page.headers.get('content-security-policy'), /frame-ancestors 'self'/);
  const index = await fetch(`${base}/`);
  assert.doesNotMatch(index.headers.get('content-security-policy'), /unsafe-inline/);
  for (const asset of ['/assets/terminal.js', '/assets/terminal-panel.js', '/assets/vendor/xterm/xterm.mjs', '/assets/vendor/xterm/addon-fit.mjs', '/assets/vendor/xterm/xterm.css']) {
    assert.equal((await fetch(base + asset)).status, 200, asset);
  }

  const activity = (await call('GET', '/api/activity')).data.events.map((event) => event.action);
  for (const action of ['terminal-settings', 'terminal-open', 'terminal-close']) assert.ok(activity.includes(action), action);

  // Signing out locks terminals again.
  const running = (await call('POST', '/api/terminals', { path: '' })).data.terminal.id;
  await call('POST', '/api/logout');
  await login();
  assert.equal((await call('GET', '/api/terminals')).data.locked, true);
  await call('POST', '/api/terminal/unlock', { password: TERMINAL_PASSWORD });

  // Turning terminals off stops running shells.
  await call('POST', '/api/terminal/config', { accountPassword: PASSWORD, enabled: false });
  assert.equal((await call('GET', '/api/terminals')).status, 403);
  await call('POST', '/api/terminal/config', { accountPassword: PASSWORD, enabled: true, password: '' });
  const after = (await call('GET', '/api/terminals')).data;
  assert.ok(!(after.terminals || []).some((item) => item.id === running), 'disabling ends running terminals');
  assert.equal((await call('GET', '/api/terminal/config')).data.passwordSet, false);
  console.log('terminal checks passed');
} finally {
  server.kill();
  await fs.rm(tmp, { recursive: true, force: true });
}
