// Dropping several files and folders at once, as a real drag from outside the browser: the
// browser empties what was dropped once the drop event has passed, which a made-up event does not
// show. In headless Chrome against a throwaway server (CHROME overrides Chrome's path;
// OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-drop-'));
const library = path.join(tmp, 'library');
const local = path.join(tmp, 'local');
fs.mkdirSync(library);
fs.mkdirSync(path.join(local, 'first folder/inner'), { recursive: true });
fs.mkdirSync(path.join(local, 'second folder'), { recursive: true });
fs.mkdirSync(path.join(local, 'empty folder'), { recursive: true });
const write = (rel, data) => { fs.writeFileSync(path.join(local, rel), data); return path.join(local, rel); };
write('first folder/a.txt', 'a');
write('first folder/inner/b.txt', 'b');
write('second folder/c.txt', 'c');
const one = write('one.txt', 'one');
const two = write('two.txt', 'two');
const three = write('three.txt', 'three');

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
const there = (rel) => fs.existsSync(path.join(library, rel));
const read = (rel) => fs.readFileSync(path.join(library, rel), 'utf8');

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
      try {
        if (await js(expression)) return;
      } catch (err) {
        if (/SyntaxError/.test(err.message)) throw err;
      }
      await sleep(100);
    }
    throw new Error(`Timed out waiting for ${what}. Page: ${JSON.stringify(await js('document.body.innerText.slice(0, 500)'))}`);
  };
  const drop = async (...files) => {
    const data = { items: [], files, dragOperationsMask: 1 };
    const at = { x: 600, y: 500 };
    for (const type of ['dragEnter', 'dragOver', 'drop']) {
      const answer = await send('Input.dispatchDragEvent', { type, ...at, data });
      assert.ok(!answer.error, JSON.stringify(answer.error));
    }
  };
  const arrived = async (...rels) => {
    for (let i = 0; i < 100 && !rels.every(there); i++) await sleep(100);
    assert.deepEqual(rels.filter((rel) => !there(rel)), [], 'these were dropped and did not arrive');
  };
  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  assert.equal(await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`), 200);
  await send('Page.reload');
  await until(`!!document.querySelector('#files [data-path]')`, 'the app');

  // Several files.
  await drop(one, two, three);
  await arrived('one.txt', 'two.txt', 'three.txt');
  assert.equal(read('three.txt'), 'three');
  await until(`!!document.querySelector("#files [data-path='three.txt']")`, 'the folder to show them');

  // Several folders with files beside them, inside and all.
  await drop(path.join(local, 'first folder'), path.join(local, 'second folder'), path.join(local, 'empty folder'), write('four.txt', 'four'));
  await arrived('first folder/a.txt', 'first folder/inner/b.txt', 'second folder/c.txt', 'empty folder', 'four.txt');
  assert.equal(read('first folder/inner/b.txt'), 'b');
  assert.ok(fs.statSync(path.join(library, 'empty folder')).isDirectory());

  // On the Gallery, into the workspace shown: straight in, with what is already there found.
  fs.mkdirSync(path.join(library, 'Family'));
  const made = await js(`fetch('/api/workspaces', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ name: 'Family', folders: ['Family'] }) }).then((r) => r.json())`);
  const id = made.id ?? made.workspace?.id;
  assert.ok(id, JSON.stringify(made));
  fs.mkdirSync(path.join(local, 'pictures'));
  const JPEG = (seed) => Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xd9]), Buffer.from(seed)]);
  fs.writeFileSync(path.join(local, 'pictures/in-folder.jpg'), JPEG('in a folder'));
  const loose = write('loose.jpg', JPEG('loose'));
  await js(`location.hash = '#gallery/w${id}'`);
  await until(`!!document.querySelector('.gallery-upload-button')`, 'the Gallery');
  await drop(loose, path.join(local, 'pictures'));
  await until(`document.querySelector('.photo-upload-summary')?.textContent === '2 uploaded'`, 'the Gallery upload');
  assert.ok(there('Family/loose.jpg') && there('Family/in-folder.jpg'), 'straight into the workspace folder');
  assert.ok(!there('pictures'), 'not into the folder the file list last showed');
  await js(`document.querySelector('.photo-upload-close').click()`);
  await until(`!document.querySelector('dialog.photo-upload')`, 'the window to close');
  await drop(loose);
  await until(`document.querySelector('.photo-upload-summary')?.textContent === '1 already there'`, 'the duplicate');
  assert.deepEqual(problems, []);
  console.log('Drop tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
