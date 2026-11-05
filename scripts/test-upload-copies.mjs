// Web uploads: photos and videos the library already has (same contents, anywhere outside the
// Trash) are found before they are sent, and skipped or sent as the person chooses; other files
// upload as usual. In headless Chrome against a throwaway server (CHROME overrides its path;
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
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-upload-copies-'));
const library = path.join(tmp, 'library');
const local = path.join(tmp, 'local');
fs.mkdirSync(local);
for (const dir of ['Photos', '.ownnas-trash']) fs.mkdirSync(path.join(library, dir), { recursive: true });
const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
const beach = Buffer.concat([JPEG, randomBytes(3000)]);
const trashed = Buffer.concat([JPEG, randomBytes(3100)]);
const clip = randomBytes(5000);
fs.writeFileSync(path.join(library, 'Photos/beach.jpg'), beach);
fs.writeFileSync(path.join(library, 'Photos/clip.mp4'), clip);
fs.writeFileSync(path.join(library, '.ownnas-trash/old.jpg'), trashed);
fs.writeFileSync(path.join(library, 'notes.txt'), 'the same notes');
const write = (name, data) => { fs.writeFileSync(path.join(local, name), data); return path.join(local, name); };
// The same photo and video under other names; one only in the Trash; a new one of the same size
// as the photo; and a text file the library has too (not a photo, so not looked at).
const beachCopy = write('beach-copy.jpg', beach);
const clipCopy = write('clip-copy.mp4', clip);
const trashedCopy = write('trashed-copy.jpg', trashed);
const sameSize = write('same-size.jpg', Buffer.concat([JPEG, randomBytes(3000)]));
const notesCopy = write('notes-copy.txt', 'the same notes');
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
      // The page may be between loads, which fails the question rather than answering it.
      try {
        if (await js(expression)) return;
      } catch (err) {
        if (/SyntaxError/.test(err.message)) throw err;
      }
      await sleep(100);
    }
    throw new Error(`Timed out waiting for ${what}. Page: ${await js(`document.body.innerText.slice(0, 500)`)}`);
  };
  const open = () => send('Page.navigate', { url: base });
  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await open();
  await until(`document.readyState === 'complete'`, 'the page');
  assert.equal(await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`), 200);

  const pick = async (...files) => {
    const { result: { root } } = await send('DOM.getDocument');
    const { result: { nodeId } } = await send('DOM.querySelector', { nodeId: root.nodeId, selector: '#upload-input' });
    await send('DOM.setFileInputFiles', { nodeId, files });
  };
  const there = (rel) => fs.existsSync(path.join(library, rel));
  await send('DOM.enable');
  await open();
  await until(`!!document.getElementById('upload-btn') && !document.getElementById('upload-btn').hidden`, 'the app');
  // The library is indexed (by size) once the server knows the photo's size.
  await until(`fetch('/api/upload/same-size', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ sizes: [${beach.length}, ${clip.length}] }) }).then((r) => r.json()).then((a) => a.sizes.length === 2)`, 'the library index');
  const dialogOpen = `document.getElementById('media-copies-dialog').open`;

  // Skip: the photo and video already there stay out; everything else goes.
  await pick(beachCopy, clipCopy, trashedCopy, sameSize, notesCopy);
  await until(dialogOpen, 'the copies dialog');
  const rows = await js(`[...document.querySelectorAll('#media-copies-list li')].map((li) => [...li.children].map((part) => part.textContent).join(' ')).sort()`);
  assert.deepEqual(rows, ['beach-copy.jpg already in Photos/beach.jpg', 'clip-copy.mp4 already in Photos/clip.mp4']);
  assert.match(await js(`document.getElementById('media-copies-copy').textContent`), /^2 photos and videos are already on the server.*The other files upload as usual[.]$/);
  await js(`document.querySelector('#media-copies-dialog button[value="skip"]').click()`);
  for (let i = 0; i < 100 && !(there('trashed-copy.jpg') && there('same-size.jpg') && there('notes-copy.txt')); i++) await sleep(100);
  assert.ok(there('trashed-copy.jpg') && there('same-size.jpg') && there('notes-copy.txt'), 'the others were uploaded');
  assert.ok(!there('beach-copy.jpg') && !there('clip-copy.mp4'), 'the copies were skipped');

  // Upload anyway sends it; Cancel sends nothing.
  await pick(clipCopy);
  await until(dialogOpen, 'the copies dialog again');
  await js(`document.querySelector('#media-copies-dialog button[value="cancel"]').click()`);
  await sleep(1000);
  assert.ok(!there('clip-copy.mp4'), 'nothing sent after Cancel');
  await pick(beachCopy);
  await until(dialogOpen, 'the copies dialog once more');
  assert.equal(await js(`document.getElementById('media-copies-copy').textContent`), '1 photo or video is already on the server, with exactly the same contents.');
  await js(`document.querySelector('#media-copies-dialog button[value="upload"]').click()`);
  for (let i = 0; i < 100 && !there('beach-copy.jpg'); i++) await sleep(100);
  assert.ok(there('beach-copy.jpg'), 'sent anyway');
  assert.deepEqual(problems, []);
  console.log('Upload copies tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
