// Remotes, in headless Chrome against a throwaway server: rclone's remotes (an alias to a temporary
// folder, in a config file of the test's own) are listed and browsed, files preview and stream in
// byte ranges, paths cannot leave the remote, and other accounts see only the remotes shared with
// them. Skipped where rclone is not installed (CHROME overrides Chrome's path; OWNNAS_TARGET the
// target directory).
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

if (spawnSync('rclone', ['version']).status !== 0) {
  console.log('skipped: rclone is not installed');
  process.exit(0);
}
const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-remotes-'));
const library = path.join(tmp, 'library');
fs.mkdirSync(library, { recursive: true });
const away = path.join(tmp, 'elsewhere');
fs.mkdirSync(path.join(away, 'Docs/Deeper'), { recursive: true });
fs.writeFileSync(path.join(away, 'hello.txt'), 'hello from the remote\nsecond line\n');
fs.writeFileSync(path.join(away, 'Docs/report.md'), '# Report\n');
fs.writeFileSync(path.join(tmp, 'secret.txt'), 'not for you');
const config = path.join(tmp, 'rclone.conf');
fs.writeFileSync(config, `[box]\ntype = alias\nremote = ${away}\n\n[other]\ntype = alias\nremote = ${away}/Docs\n`);
const PASSWORD = randomBytes(12).toString('hex');
const freePort = () => new Promise((done) => {
  const probe = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = probe.address();
    probe.close(() => done(port));
  });
});
const port = await freePort();
const server = spawn(path.join(target, 'ownnas'), ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`, '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env: { ...process.env, HOME: tmp, USERPROFILE: tmp } });
const base = `http://127.0.0.1:${port}/`;
const debugPort = await freePort();
const chromePath = process.env.CHROME || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const chrome = spawn(chromePath, ['--headless=new', '--disable-gpu', '--window-size=1300,900', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${path.join(tmp, 'profile')}`, 'about:blank'], { stdio: 'ignore' });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

let failed = false;
try {
  let targets;
  for (let i = 0; i < 60 && !targets?.length; i++) {
    await sleep(200);
    try { targets = (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json()).filter((item) => item.type === 'page'); } catch { /* starting */ }
  }
  const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
  await new Promise((done) => { ws.onopen = done; });
  let nextId = 0;
  const waiting = new Map();
  const problems = [];
  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && waiting.has(message.id)) {
      waiting.get(message.id)(message);
      waiting.delete(message.id);
    }
    if (message.method === 'Runtime.exceptionThrown') problems.push(message.params.exceptionDetails.exception?.description ?? message.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((done) => {
    const id = ++nextId;
    waiting.set(id, done);
    ws.send(JSON.stringify({ id, method, params }));
  });
  const js = async (expression) => {
    const answer = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (answer.result.exceptionDetails) throw new Error(answer.result.exceptionDetails.exception?.description ?? answer.result.exceptionDetails.text);
    return answer.result.result.value;
  };
  const until = async (expression, what, tries = 200) => {
    for (let i = 0; i < tries; i++) {
      try {
        if (await js(expression)) return;
      } catch (err) {
        if (/SyntaxError/.test(err.message)) throw err;
      }
      await sleep(100);
    }
    throw new Error(`Timed out waiting for ${what}. Page: ${JSON.stringify(await js('document.body.innerText.slice(0, 1200)'))}`);
  };

  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  const signIn = (who, password) => js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: ${JSON.stringify(who)}, password: ${JSON.stringify(password)} }) }).then((r) => r.status)`);
  assert.equal(await signIn('admin', PASSWORD), 200);
  const call = (method, url, body) => js(`fetch(${JSON.stringify(url)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, ${body ? `body: ${JSON.stringify(JSON.stringify(body))}` : ''} }).then(async (r) => ({ status: r.status, json: await r.json().catch(() => null) }))`);

  // rclone's config file, chosen by the administrator.
  assert.equal((await call('POST', '/api/remotes/settings', { program: '', config: path.join(tmp, 'missing.conf') })).status, 400);
  assert.equal((await call('POST', '/api/remotes/settings', { program: '', config })).status, 200);
  const overview = (await call('GET', '/api/remotes')).json;
  assert.equal(overview.available, true);
  assert.equal(overview.config, config);
  assert.deepEqual(overview.remotes.map((remote) => [remote.name, remote.type]), [['box', 'alias'], ['other', 'alias']]);

  // Browsing.
  await send('Page.navigate', { url: `${base}?signed-in#remotes` });
  await until(`document.querySelectorAll('.remote-card').length === 2`, 'the remotes');
  assert.equal(await js(`document.getElementById('remotes-btn').hidden`), false);
  await js(`document.querySelector('.remote-card .remote-open').click(), true`);
  await until(`document.querySelectorAll('.remote-row').length === 2`, 'the folder');
  assert.deepEqual(await js(`[...document.querySelectorAll('.remote-row-name')].map((name) => name.textContent)`), ['Docs', 'hello.txt'], 'folders first');
  await js(`document.querySelector('.remote-row.is-dir').click(), true`);
  await until(`location.hash === '#remotes/box/Docs' && [...document.querySelectorAll('.remote-row-name')].map((name) => name.textContent).join() === 'Deeper,report.md'`, 'the folder inside');
  await until(`document.querySelector('.remotes-crumbs').textContent.replace(/\\s/g, '') === 'Remotes/box:/Docs'`, 'the crumbs');
  await js(`document.querySelector('.remotes-crumbs a[href="#remotes/box"]').click(), true`);
  await until(`document.querySelectorAll('.remote-row').length === 2 && document.querySelector('.remote-row:not(.is-dir)')`, 'back up');
  await js(`document.querySelector('.remote-row:not(.is-dir)').click(), true`);
  await until(`document.querySelector('dialog.remote-preview .remote-preview-text')?.textContent.startsWith('hello from the remote')`, 'the text preview');
  await js(`document.querySelector('dialog.remote-preview').close(), true`);

  // Files stream in byte ranges, and cannot be reached outside the remote.
  const ranged = await js(`fetch('/api/remotes/file?remote=box&path=hello.txt', { headers: { Range: 'bytes=0-4' } }).then(async (r) => [r.status, r.headers.get('content-range'), await r.text()])`);
  assert.deepEqual(ranged, [206, 'bytes 0-4/34', 'hello']);
  assert.equal((await call('GET', '/api/remotes/list?remote=box&path=../')).status, 400);
  assert.equal((await call('GET', '/api/remotes/file?remote=box&path=../secret.txt')).status, 400);
  assert.equal((await call('GET', '/api/remotes/list?remote=a:b')).status, 400);
  assert.equal((await call('GET', '/api/remotes/list?remote=box&path=nothing-here')).status, 502);

  // Another account sees only what is shared with it.
  assert.equal((await call('POST', '/api/users', { username: 'ann', password: 'ann-password-123' })).status, 200);
  const ann = (await call('GET', '/api/users')).json;
  const annId = (ann.users || ann).find((user) => user.username === 'ann').id;
  assert.equal((await call('POST', '/api/remotes/access', { remote: 'other', users: [annId] })).status, 200);
  assert.deepEqual((await call('GET', '/api/remotes')).json.remotes.find((remote) => remote.name === 'other').users, [annId]);
  assert.equal(await signIn('ann', 'ann-password-123'), 200);
  const hers = (await call('GET', '/api/remotes')).json;
  assert.deepEqual(hers.remotes.map((remote) => remote.name), ['other']);
  assert.equal(hers.config, undefined, 'only administrators see where the config is');
  assert.equal((await call('GET', '/api/remotes/list?remote=box')).status, 404);
  assert.equal((await call('GET', '/api/remotes/file?remote=box&path=hello.txt')).status, 404);
  assert.equal((await call('GET', '/api/remotes/list?remote=other')).status, 200);
  assert.equal((await call('POST', '/api/remotes/settings', { program: '', config: '' })).status, 403);
  assert.equal((await call('GET', '/api/me')).json.remotes, true);

  assert.deepEqual(problems, [], 'no errors in the page');
  console.log('ok: remotes');
} catch (err) {
  failed = true;
  console.error(err);
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
