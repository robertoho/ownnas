// Uploading to a Gallery workspace, in headless Chrome against a throwaway server: duplicates are
// found before anything is sent, wherever they are (CHROME overrides Chrome's path; OWNNAS_TARGET
// the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-photos-'));
const library = path.join(tmp, 'library');
const local = path.join(tmp, 'local');
for (const dir of ['Family', 'Work']) fs.mkdirSync(path.join(library, dir), { recursive: true });
fs.mkdirSync(local);
// A minimal JPEG, with bytes after it that make each one different.
const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
const photo = (seed, padding = 0) => Buffer.concat([JPEG, Buffer.from(`${seed}`), Buffer.alloc(padding, seed.length)]);
const onDisk = (root, rel, data) => { fs.writeFileSync(path.join(root, rel), data); return path.join(root, rel); };

const inFamily = photo('in family');
const inWork = photo('in work');
onDisk(library, 'Family/existing.jpg', inFamily);
onDisk(library, 'Work/elsewhere.jpg', inWork);
const sameName = photo('another');
onDisk(library, 'Family/taken.jpg', sameName);

const dupHere = onDisk(local, 'copy-of-existing.jpg', inFamily);
const dupElsewhere = onDisk(local, 'copy-of-elsewhere.jpg', inWork);
const fresh = onDisk(local, 'fresh.jpg', photo('fresh'));
fs.mkdirSync(path.join(local, 'b'));
const freshTwin = onDisk(local, 'b/fresh-twin.jpg', photo('fresh'));
const nameTaken = onDisk(local, 'taken.jpg', photo('differs from what is there'));
const notPhoto = onDisk(local, 'notes.txt', 'hello');
// Larger than a hashing chunk, with the same size as nothing else.
const big = onDisk(local, 'big.jpg', Buffer.concat([JPEG, randomBytes(9 * 1024 * 1024)]));

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
const list = (dir) => fs.readdirSync(path.join(library, dir)).filter((name) => !name.startsWith('.')).sort();

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
  const pick = async (...files) => {
    const { result: { root } } = await send('DOM.getDocument');
    const { result: { nodeId } } = await send('DOM.querySelector', { nodeId: root.nodeId, selector: '.photo-upload-input' });
    await send('DOM.setFileInputFiles', { nodeId, files });
  };
  const rowOf = (name) => js(`[...document.querySelectorAll('.photo-upload-row')].find((row) => row.querySelector('.photo-upload-name').textContent.startsWith(${JSON.stringify(name)}))?.innerText`);
  const summaryIs = (text) => until(`document.querySelector('.photo-upload-summary').textContent === ${JSON.stringify(text)}`, `the summary “${text}”`);

  await send('Page.enable');
  await send('Runtime.enable');
  await send('DOM.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await send('Page.navigate', { url: base });
  await until(`document.readyState === 'complete'`, 'the page');
  const signIn = `fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`;
  assert.equal(await js(signIn), 200);
  const call = (method, url, body) => js(`fetch(${JSON.stringify(url)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, ${body ? `body: ${JSON.stringify(JSON.stringify(body))}` : ''} }).then((r) => r.json())`);
  const made = await call('POST', '/api/workspaces', { name: 'Family', folders: ['Family'] });
  const id = made.id ?? made.workspace?.id;
  assert.ok(id, JSON.stringify(made));
  // What is in the library has to be indexed for it to be found.
  await call('POST', '/api/gallery/rescan');
  await until(`fetch('/api/gallery/timeline?workspace=${id}&limit=50').then((r) => r.json()).then((page) => (page.items || []).length === 2)`, 'the index');
  await send('Page.reload');
  await until(`!!document.querySelector('#files [data-path]')`, 'the app');
  await js(`location.hash = '#gallery/w${id}'`);
  await until(`document.querySelectorAll('.gallery-tile').length === 2`, 'the workspace to show its photos');

  // Plain HTTP on a real address has no browser hashing: ours matches the right answer anyway.
  const hashes = await js(`import('/assets/photo-upload.js').then(async ({ Sha256 }) => {
    const hex = (bytes) => { const hash = new Sha256(); hash.update(bytes); return hash.hex(); };
    const pieces = new Sha256();
    const long = new Uint8Array(1000).fill(97);
    for (let at = 0; at < 1000; at += 37) pieces.update(long.subarray(at, at + 37));
    return [hex(new Uint8Array()), hex(new TextEncoder().encode('abc')), hex(new Uint8Array(55).fill(97)), hex(new Uint8Array(64).fill(97)), pieces.hex()];
  })`);
  const right = ['', 'abc', 'a'.repeat(55), 'a'.repeat(64), 'a'.repeat(1000)].map((text) => createHash('sha256').update(text).digest('hex'));
  assert.deepEqual(hashes, right);

  await until(`!!document.querySelector('.gallery-upload-button')`, 'the Upload button');
  await js(`document.querySelector('.gallery-upload-button').click()`);
  await until(`document.querySelector('dialog.photo-upload')?.open`, 'the upload window');
  assert.match(await js(`document.querySelector('dialog.photo-upload').innerText`), /Upload to Family[\s\S]*Into Family/);

  await pick(dupHere, dupElsewhere, fresh, freshTwin, nameTaken, notPhoto, big);
  await summaryIs('3 uploaded, 3 already there, 1 skipped');

  assert.match(await rowOf('copy-of-existing.jpg'), /Already here: Family\/existing\.jpg/);
  assert.match(await rowOf('copy-of-elsewhere.jpg'), /Already in the library, outside this workspace: Work\/elsewhere\.jpg/);
  assert.match(await rowOf('notes.txt'), /not a photo or video/);
  const one = await rowOf('fresh.jpg');
  const two = await rowOf('fresh-twin.jpg');
  assert.ok(/in this upload/.test(one) !== /in this upload/.test(two), `one of the twins is reported: ${one} / ${two}`);
  assert.match(await rowOf('taken.jpg'), /Uploaded to Family\/taken \(2\)\.jpg/);
  assert.match(await rowOf('big.jpg'), /Uploaded to Family\/big\.jpg/);

  // Nothing already there was sent; what was not is exact, and nothing was overwritten.
  const names = list('Family');
  assert.ok(!names.includes('copy-of-existing.jpg') && !names.includes('copy-of-elsewhere.jpg') && !names.includes('notes.txt'), names.join());
  assert.equal(names.filter((name) => name.startsWith('fresh')).length, 1, names.join());
  assert.deepEqual(fs.readFileSync(path.join(library, 'Family/taken.jpg')), sameName);
  assert.deepEqual(fs.readFileSync(path.join(library, 'Family/big.jpg')), fs.readFileSync(big));

  // Again straight away, before any scan could have indexed them: found where they went.
  await pick(big);
  await summaryIs('3 uploaded, 4 already there, 1 skipped');

  // Upload anyway does, under a new name.
  await js(`[...document.querySelectorAll('.photo-upload-row')].find((row) => row.innerText.includes('copy-of-existing.jpg')).querySelector('button').click()`);
  await summaryIs('4 uploaded, 3 already there, 1 skipped');
  assert.deepEqual(fs.readFileSync(path.join(library, 'Family/copy-of-existing.jpg')), inFamily);

  // A lost connection says so, and Try again finishes it.
  const retry = onDisk(local, 'retry.jpg', photo('retry'));
  await js(`(() => {
    const original = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (body) {
      XMLHttpRequest.prototype.send = original;
      setTimeout(() => this.onerror());
    };
  })()`);
  await pick(retry);
  await summaryIs('4 uploaded, 3 already there, 1 skipped, 1 failed');
  assert.match(await rowOf('retry.jpg'), /connection to the server was lost/);
  await js(`[...document.querySelectorAll('.photo-upload-row')].find((row) => row.innerText.includes('retry.jpg')).querySelector('button').click()`);
  await summaryIs('5 uploaded, 3 already there, 1 skipped');
  assert.ok(list('Family').includes('retry.jpg'));

  // They show in the workspace once the window closes.
  await js(`document.querySelector('.photo-upload-close').click()`);
  await until(`document.querySelector(".gallery-tile[data-path='Family/retry.jpg']")`, 'the new photos in the Gallery');
  assert.deepEqual(problems, []);
  console.log('Gallery upload tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
