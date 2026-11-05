// The archive menu's Extract in folder and Extract here, in headless Chrome against a throwaway
// server (CHROME overrides its path; OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { zipSync, strToU8 } from '../web/vendor/three/addons/libs/fflate.module.js';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-extract-'));
const library = path.join(tmp, 'library');
fs.mkdirSync(library);
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
const makeZip = (name) => fs.writeFileSync(path.join(library, name), zipSync({ 'one.txt': strToU8('one'), 'two.txt': strToU8('two'), 'sub/three.txt': strToU8('three') }));

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
  /** The archive's menu, opened again until its card is there and the menu shows. */
  const menuOf = (name) => until(`(() => {
    if (document.querySelector('[data-menu="extract"]')) return true;
    const card = document.querySelector("[data-path='${name}']");
    card?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 300, clientY: 300 }));
    return false;
  })()`, `the menu of ${name}`);
  const open = () => send('Page.navigate', { url: base });
  await send('Page.enable');
  await send('Runtime.enable');
  for (let i = 0; i < 100; i++) {
    try { await fetch(`${base}api/me`); break; } catch { await sleep(100); }
  }
  await open();
  await until(`document.readyState === 'complete'`, 'the page');
  assert.equal(await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`), 200);

  makeZip('folder.zip');
  makeZip('here.zip');
  fs.writeFileSync(path.join(library, 'one.txt'), 'already here');
  await open();

  // The menu has both (it keeps its items in alphabetical order).
  await menuOf('folder.zip');
  const labels = await js(`[...document.querySelectorAll('[data-menu="extract"], [data-menu="extractHere"]')].map((b) => b.textContent.trim())`);
  assert.deepEqual(labels.sort(), ['Extract here', 'Extract in folder']);

  // Extract in folder makes the folder, as before.
  await js(`document.querySelector('[data-menu="extract"]').click()`);
  for (let i = 0; i < 50 && !there('folder/two.txt'); i++) await sleep(100);
  assert.equal(fs.readFileSync(path.join(library, 'folder/two.txt'), 'utf8'), 'two');
  assert.ok(!there('two.txt'), 'nothing loose in the folder');

  // Extract here puts them in this folder, and overwrites nothing.
  await open();
  await menuOf('here.zip');
  await js(`document.querySelector('[data-menu="extractHere"]').click()`);
  for (let i = 0; i < 50 && !there('two.txt'); i++) await sleep(100);
  assert.equal(fs.readFileSync(path.join(library, 'two.txt'), 'utf8'), 'two');
  assert.equal(fs.readFileSync(path.join(library, 'sub/three.txt'), 'utf8'), 'three');
  assert.equal(fs.readFileSync(path.join(library, 'one.txt'), 'utf8'), 'already here');
  assert.equal(fs.readFileSync(path.join(library, 'one 2.txt'), 'utf8'), 'one');
  assert.ok(!there('here'), 'no folder around them');
  await until(`document.body.innerText.includes('Extracted 3 items here')`, 'the message');
  await until(`!!document.querySelector("[data-path='two.txt']")`, 'the folder to show them');

  // The archive's own preview has both as well.
  await until(`(() => {
    if (document.querySelector('.archive-extract')) return true;
    document.querySelector("[data-path='here.zip']")?.click();
    document.querySelector("[data-path='here.zip']")?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    return false;
  })()`, 'the preview');
  assert.deepEqual(await js(`[...document.querySelectorAll('.archive-extract')].map((b) => b.textContent)`), ['Extract in folder', 'Extract here']);
  assert.deepEqual(problems, []);
  console.log('Extract tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
