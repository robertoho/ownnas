// Uploading more files at once than the server takes names in one request (5000): the page asks
// about them a part at a time, and every file arrives. Headless Chrome against a throwaway server
// (CHROME overrides its path; OWNNAS_TARGET the target directory). Takes a minute or two.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-upload-many-'));
const library = path.join(tmp, 'library');
const local = path.join(tmp, 'local');
fs.mkdirSync(library);
fs.mkdirSync(local);
const PASSWORD = randomBytes(12).toString('hex');
const freePort = () => new Promise((done) => {
  const probe = net.createServer().listen(0, '127.0.0.1', () => {
    const { port } = probe.address();
    probe.close(() => done(port));
  });
});
const port = await freePort();
const env = { ...process.env, HOME: tmp, USERPROFILE: tmp };
const server = spawn(path.join(target, 'ownnas'), ['serve', '--root', library, '--data', path.join(tmp, 'data'), '--addr', `127.0.0.1:${port}`, '--username', 'admin', '--password', PASSWORD], { stdio: 'ignore', env });
const base = `http://127.0.0.1:${port}/`;
const chromePath = process.env.CHROME || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const debugPort = await freePort();
const chrome = spawn(chromePath, ['--headless=new', '--disable-gpu', '--window-size=1300,900', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${path.join(tmp, 'profile')}`, 'about:blank'], { stdio: 'ignore' });
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const write = (root, name, data, secondsAgo = 0) => {
  const file = path.join(root, name);
  fs.writeFileSync(file, data);
  const when = new Date(Date.now() - secondsAgo * 1000);
  fs.utimesSync(file, when, when);
  return file;
};
const onServer = (name) => fs.readFileSync(path.join(library, name), 'utf8');
const there = (name) => fs.existsSync(path.join(library, name));

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
  const until = async (expression, what) => {
    for (let i = 0; i < 100; i++) {
      if (await js(expression)) return;
      await sleep(100);
    }
    throw new Error(`Timed out waiting for ${what}. Page: ${await js('document.body.innerText.slice(0, 600)')}`);
  };
  const pick = async (...files) => {
    const { result: { root } } = await send('DOM.getDocument');
    const { result: { nodeId } } = await send('DOM.querySelector', { nodeId: root.nodeId, selector: '#upload-input' });
    await send('DOM.setFileInputFiles', { nodeId, files });
  };
  const open = async () => {
    await send('Page.navigate', { url: base });
    await until(`!!document.getElementById('upload-btn') && !document.getElementById('upload-btn').hidden`, 'the app');
  };
  // Asked again until it opens: the page may still be loading its folder.
  const openRecentUploads = () => until(`(() => {
    const item = document.querySelector('[data-menu="uploadsLog"]');
    if (item) {
      item.click();
      return true;
    }
    document.getElementById('files')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 700, clientY: 700 }));
    return false;
  })()`, 'the menu');
  await send('Page.enable');
  await send('Runtime.enable');
  await send('DOM.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  const signedIn = await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`);
  assert.equal(signedIn, 200);
  await open();

  const COUNT = 5050;
  const picked = [];
  for (let i = 0; i < COUNT; i++) picked.push(write(local, `file-${String(i).padStart(5, '0')}.txt`, `file ${i}`));
  await pick(...picked);
  const started = Date.now();
  let arrived = 0;
  while (Date.now() - started < 600_000) {
    arrived = fs.readdirSync(library).filter((name) => name.startsWith('file-')).length;
    if (arrived === COUNT) break;
    if (await js(`!!document.querySelector('.jobs-item.problem')`)) break;
    await sleep(500);
  }
  const report = await js(`document.querySelector('.jobs-item.problem')?.innerText || ''`);
  assert.equal(report, '', 'no file failed');
  assert.equal(arrived, COUNT, `${arrived} of ${COUNT} arrived`);
  assert.deepEqual(problems, []);
  console.log(`ok: ${COUNT} files uploaded in one go (${Math.round((Date.now() - started) / 1000)} s)`);
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
