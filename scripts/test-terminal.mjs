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
// A throwaway home folder, so installing agent integrations never touches the real ones.
const home = path.join(tmp, 'home');
await fs.mkdir(path.join(home, '.claude'), { recursive: true });
const serverEnv = { ...process.env, OWNNAS_PASSWORD: PASSWORD, HOME: home, USERPROFILE: home };
delete serverEnv.CLAUDE_CONFIG_DIR;
delete serverEnv.XDG_CONFIG_HOME;
const server = spawn(binary, ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`,
  '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env: serverEnv });

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
  response = await call('POST', '/api/terminals/rename', { id, name: '  API   server  ' });
  assert.equal(response.data.terminal.name, 'API server', 'names are trimmed');
  assert.equal((await call('GET', '/api/terminals')).data.terminals[0].name, 'API server');
  response = await call('POST', '/api/terminals/rename', { id, name: '' });
  assert.equal(response.data.terminal.name, undefined, 'an empty name restores the default');
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

  // --- Coding agents ---
  const agentTerminal = (await call('POST', '/api/terminals', { path: 'proj' })).data.terminal.id;
  const shell = attach(agentTerminal);
  await until(() => shell.opened, 'agent terminal');
  const type = (text) => shell.socket.send(new TextEncoder().encode(text));
  const agentOf = async () => (await call('GET', '/api/terminals')).data.terminals.find((t) => t.id === agentTerminal).agent;

  // Live updates arrive over server-sent events.
  const streamed = [];
  const streamAbort = new AbortController();
  fetch(`${base}/api/agents/stream`, { headers: { cookie }, signal: streamAbort.signal }).then(async (response) => {
    const decoder = new TextDecoder();
    for await (const chunk of response.body) streamed.push(decoder.decode(chunk));
  }).catch(() => {});

  // A bell in a plain shell asks for attention; typing clears it.
  type("printf '\\a'\r");
  let agent;
  for (let i = 0; i < 100 && !(agent = await agentOf()).attention; i++) await sleep(50);
  assert.equal(agent.attention, 'The terminal rang its bell');
  type(' ');
  for (let i = 0; i < 100 && (agent = await agentOf()).attention; i++) await sleep(50);
  assert.equal(agent.attention, null, 'typing marks the terminal as seen');
  type('\x15'); // clear the line

  // Claude Code hooks run `ownnas agent-hook claude` with the event on stdin.
  await fs.writeFile(path.join(library, 'proj', 'a.txt'), 'x');
  const hook = (event) => `printf '%s' '${JSON.stringify(event)}' | '${binary}' agent-hook claude\r`;
  type(hook({ hook_event_name: 'SessionStart', session_id: 'sess-1', model: 'claude-opus-5-5' }));
  type(hook({ hook_event_name: 'UserPromptSubmit', session_id: 'sess-1', prompt: 'Fix the tests' }));
  type(hook({ hook_event_name: 'PreToolUse', session_id: 'sess-1', tool_name: 'Edit', tool_input: { file_path: path.join(library, 'proj', 'a.txt'), old_string: 'x', new_string: 'y' } }));
  type(hook({ hook_event_name: 'PostToolUse', session_id: 'sess-1', tool_name: 'Edit', tool_input: { file_path: path.join(library, 'proj', 'a.txt') }, tool_response: { big: 'output' } }));
  for (let i = 0; i < 200 && !((agent = await agentOf()).files || []).length; i++) await sleep(50);
  assert.equal(agent.name, 'claude');
  assert.equal(agent.label, 'Claude Code');
  assert.equal(agent.hooked, true);
  assert.equal(agent.sessionId, 'sess-1');
  assert.equal(agent.detail, 'Editing proj/a.txt');
  assert.deepEqual(agent.files, ['proj/a.txt']);
  type(hook({ hook_event_name: 'Notification', session_id: 'sess-1', notification_type: 'permission_prompt', message: 'Claude needs your permission to use Bash' }));
  for (let i = 0; i < 200 && (agent = await agentOf()).status !== 'waiting'; i++) await sleep(50);
  assert.equal(agent.attention, 'Claude needs your permission to use Bash');
  assert.equal((await call('POST', '/api/terminals/seen', { id: agentTerminal })).status, 200);
  assert.equal((await agentOf()).attention, null);
  type(hook({ hook_event_name: 'Stop', session_id: 'sess-1', last_assistant_message: 'Fixed 2 tests.' }));
  for (let i = 0; i < 200 && (agent = await agentOf()).status !== 'done'; i++) await sleep(50);
  assert.equal(agent.lastMessage, 'Fixed 2 tests.');
  await until(() => streamed.join('').includes('Fixed 2 tests.'), 'stream update');
  assert.match(streamed.join(''), /event: terminals/);
  streamAbort.abort();

  // The hook endpoint listens on loopback only and needs the terminal's token.
  type('echo "url=$OWNNAS_AGENT_URL"\r');
  await until(() => /url=http:\/\/127\.0\.0\.1:\d+\/agent-events/.test(shell.text), 'agent url');
  const agentUrl = shell.text.match(/url=(http:\/\/127\.0\.0\.1:\d+\/agent-events)/)[1];
  const forged = await fetch(agentUrl, { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas-agent-token': 'guess' }, body: '{"agent":"claude","event":{}}' });
  assert.equal(forged.status, 401);
  // Outside OwnNAS terminals the hook does nothing and still succeeds.
  const { spawnSync } = await import('node:child_process');
  const outside = spawnSync(binary, ['agent-hook', 'claude'], { input: '{"hook_event_name":"Stop"}', env: { PATH: process.env.PATH } });
  assert.equal(outside.status, 0);

  // Agents without hooks are recognized from the foreground process (macOS and Linux).
  if (process.platform !== 'win32') {
    const fakeDir = path.join(tmp, 'bin');
    await fs.mkdir(fakeDir, { recursive: true });
    await fs.writeFile(path.join(fakeDir, 'codex'), '#!/bin/sh\nsleep 30\n', { mode: 0o755 });
    type(`${path.join(fakeDir, 'codex')}\r`);
    for (let i = 0; i < 200 && (agent = await agentOf()).name !== 'codex'; i++) await sleep(50);
    assert.equal(agent.name, 'codex');
    assert.equal(agent.label, 'Codex');
    assert.equal(agent.hooked, false);
    type('\x03');
    for (let i = 0; i < 200 && (agent = await agentOf()).name; i++) await sleep(50);
    assert.equal(agent.name, null, 'the agent is forgotten when it exits');
  }

  // Files pasted into a terminal go to its own temporary folder, removed with the terminal.
  const upload = (name, body, headers = { 'x-ownnas': '1' }) => fetch(`${base}/api/terminals/upload?id=${agentTerminal}&name=${encodeURIComponent(name)}`, { method: 'POST', headers: { cookie, ...headers }, body });
  let pasted = await upload('my screen shot.png', 'png-bytes');
  assert.equal(pasted.status, 200);
  const firstPath = (await pasted.json()).path;
  assert.ok(firstPath.startsWith(path.join(tmp, 'data', 'terminal-uploads', agentTerminal)), firstPath);
  assert.ok(firstPath.endsWith(`${path.sep}my_screen_shot.png`), 'names are made safe to type');
  assert.equal(await fs.readFile(firstPath, 'utf8'), 'png-bytes');
  const secondPath = (await (await upload('my screen shot.png', 'again')).json()).path;
  assert.ok(secondPath.endsWith('my_screen_shot-2.png'), 'same names do not overwrite');
  assert.ok((await (await upload('../../escape', 'x')).json()).path.endsWith(`${path.sep}escape`));
  assert.equal((await upload('x.txt', 'x', {})).status, 403, 'needs the request header');

  // The current folder follows `cd` inside the library.
  type('mkdir -p sub && cd sub\r');
  let cwd;
  for (let i = 0; i < 100 && (cwd = (await call('GET', `/api/terminals/cwd?id=${agentTerminal}`)).data).path !== 'proj/sub'; i++) await sleep(50);
  assert.deepEqual(cwd, { path: 'proj/sub', outside: false });
  type(`cd ${os.tmpdir()}\r`);
  for (let i = 0; i < 100 && !(cwd = (await call('GET', `/api/terminals/cwd?id=${agentTerminal}`)).data).outside; i++) await sleep(50);
  assert.equal(cwd.path, 'proj', 'outside the library, the start folder is shown');
  type(`cd ${path.join(library, 'proj')}\r`);

  // Git changes for the terminal's folder.
  type('git init -q . && git -c user.email=t@t -c user.name=t commit -q --allow-empty -m init && echo hi > b.txt && echo y > a.txt && echo git-ready\r');
  await until(() => shell.text.includes('git-ready\r\n') || shell.text.split('git-ready').length > 2, 'git setup');
  const changes = (await call('GET', `/api/terminals/changes?id=${agentTerminal}`)).data;
  assert.equal(changes.repo, true);
  assert.deepEqual(changes.files.map((f) => [f.status, f.path]).sort(), [['new', 'proj/a.txt'], ['new', 'proj/b.txt']]);

  // Installing integrations writes to the agent's config and can be undone.
  let integrations = (await call('GET', '/api/agents/integrations')).data.integrations;
  assert.deepEqual(integrations.map((i) => [i.agent, i.installed, i.found]), [['claude', false, true], ['opencode', false, false]]);
  await fs.writeFile(path.join(home, '.claude', 'settings.json'), '{\n  "theme": "dark"\n}\n');
  assert.equal((await call('POST', '/api/agents/integrations', { agent: 'claude', installed: true })).status, 200);
  assert.equal((await call('POST', '/api/agents/integrations', { agent: 'opencode', installed: true })).status, 200);
  const settings = JSON.parse(await fs.readFile(path.join(home, '.claude', 'settings.json'), 'utf8'));
  assert.equal(settings.theme, 'dark');
  assert.match(settings.hooks.PreToolUse[0].hooks[0].command, /agent-hook claude$/);
  assert.equal(settings.hooks.PreToolUse[0].hooks[0].async, true);
  const plugin = await fs.readFile(path.join(home, '.config', 'opencode', 'plugins', 'ownnas.js'), 'utf8');
  assert.match(plugin, /export const OwnNASAgentPlugin/);
  integrations = (await call('GET', '/api/agents/integrations')).data.integrations;
  assert.deepEqual(integrations.map((i) => i.installed), [true, true]);
  await call('POST', '/api/agents/integrations', { agent: 'claude', installed: false });
  await call('POST', '/api/agents/integrations', { agent: 'opencode', installed: false });
  assert.deepEqual(JSON.parse(await fs.readFile(path.join(home, '.claude', 'settings.json'), 'utf8')), { theme: 'dark' });
  await assert.rejects(fs.access(path.join(home, '.config', 'opencode', 'plugins', 'ownnas.js')));
  shell.socket.close();
  await call('DELETE', `/api/terminals?id=${agentTerminal}`);
  await assert.rejects(fs.access(path.dirname(firstPath)), 'pasted files are deleted with the terminal');

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
