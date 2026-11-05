// The web app's uploads: the Keep newer choice, the report that stays open when a file fails, and
// Recent uploads, in headless Chrome against a throwaway server (CHROME overrides its path;
// OWNNAS_TARGET the target directory; SCREENSHOT saves a picture of the report).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-upload-'));
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

  // One file's connection drops: the others still go, and the report stays.
  const good = write(local, 'good.txt', 'good');
  const bad = write(local, 'bad.txt', 'bad bad bad');
  const last = write(local, 'last.txt', 'last');
  await js(`(() => {
    const send = XMLHttpRequest.prototype.send;
    window.dropBad = true;
    XMLHttpRequest.prototype.send = function (body) {
      if (window.dropBad && body instanceof FormData && body.get('file').name === 'bad.txt') {
        setTimeout(() => this.onerror());
        return;
      }
      send.call(this, body);
    };
  })()`);
  await pick(good, bad, last);
  await until(`!!document.querySelector('.jobs-item.problem')`, 'the report');
  await sleep(1500);
  const report = () => js(`(() => { const item = document.querySelector('.jobs-item.problem'); return item && !document.getElementById('jobs-panel').hidden ? item.innerText : null; })()`);
  const shown = await report();
  assert.ok(shown, 'the report is still showing');
  assert.match(shown, /1 of 3 files failed/);
  assert.match(shown, /bad\.txt: The connection to the server was lost/);
  assert.match(shown, /42% done, 58% left \(11 B of 19 B\)/, shown);
  assert.equal(onServer('good.txt'), 'good');
  assert.equal(onServer('last.txt'), 'last', 'the files after the failed one still went');
  assert.ok(!there('bad.txt'));
  if (process.env.SCREENSHOT) {
    const { result } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(process.env.SCREENSHOT, Buffer.from(result.data, 'base64'));
  }

  // Retrying sends only what failed, and the report goes away.
  await js(`window.dropBad = false; [...document.querySelectorAll('.jobs-item.problem button')].find((b) => b.textContent === 'Retry failed').click()`);
  await until(`!document.querySelector('.jobs-item.problem')`, 'the report to close');
  for (let i = 0; i < 50 && !there('bad.txt'); i++) await sleep(100);
  assert.equal(onServer('bad.txt'), 'bad bad bad');
  await until(`document.body.innerText.includes('confirmed on the server')`, 'the confirmation');

  // Keep newer, with the same choice for the rest: replaces the older, skips the newer.
  write(library, 'older-here.txt', 'server, old', 5000);
  write(library, 'newer-here.txt', 'server, new', 100);
  const incomingNewer = write(local, 'older-here.txt', 'uploaded, new', 100);
  const incomingOlder = write(local, 'newer-here.txt', 'uploaded, old', 5000);
  await pick(incomingNewer, incomingOlder);
  await until(`document.getElementById('conflict-dialog').open`, 'the conflict question');
  assert.equal(await js(`document.querySelector('#conflict-dialog button[value="keep-newer"]').hidden`), false);
  await js(`document.getElementById('conflict-rest').checked = true; document.querySelector('#conflict-dialog button[value="keep-newer"]').click()`);
  for (let i = 0; i < 50 && onServer('older-here.txt') !== 'uploaded, new'; i++) await sleep(100);
  assert.equal(onServer('older-here.txt'), 'uploaded, new', 'the older file was replaced');
  assert.equal(onServer('newer-here.txt'), 'server, new', 'the newer file was kept');

  // Recent uploads checks against the server, also after a reload and after a file goes away.
  await open();
  await openRecentUploads();
  await until(`document.getElementById('uploads-dialog').open && document.querySelector('.upload-log-batch')`, 'Recent uploads');
  await until(`document.querySelector('.upload-log-tally').textContent.includes('1 confirmed')`, 'the check');
  let text = await js(`document.getElementById('uploads-list').innerText`);
  assert.match(text, /1 confirmed, 1 skipped/, text);
  assert.match(text, /Skipped: the file already there was kept/);
  fs.rmSync(path.join(library, 'older-here.txt'));
  await js(`document.querySelector('[data-upload-check]').click()`);
  await until(`document.getElementById('uploads-list').innerText.includes('Not on the server now')`, 'the missing file');
  assert.match(await js(`document.querySelector('.upload-log-batch').className`), /has-problems/);
  // A page closed part way shows what did not finish.
  await js(`(() => {
    const list = JSON.parse(localStorage.getItem('ownnas-upload-log'));
    list.unshift({ id: 'closed', at: 1700000000, user: 'admin', destination: '', files: [{ name: 'good.txt', size: 4, modified: 1, status: 'saved' }, { name: 'never.txt', size: 9, modified: 1, status: 'waiting' }] });
    localStorage.setItem('ownnas-upload-log', JSON.stringify(list));
  })()`);
  await js(`document.querySelector('#uploads-dialog button[value="close"]').click()`);
  await open();
  await openRecentUploads();
  await until(`document.querySelector('.upload-log-tally')?.textContent.includes('1 confirmed, 1 with problems')`, 'the closed page');
  text = await js(`document.querySelector('.upload-log-batch').innerText`);
  assert.match(text, /never\.txt[\s\S]*Not on the server/, text);
  await js(`document.getElementById('uploads-clear').click()`);
  assert.equal(await js(`document.querySelectorAll('.upload-log-batch').length`), 0);
  assert.deepEqual(problems, []);
  console.log('Upload report tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
