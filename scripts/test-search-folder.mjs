// File search: each match has a button that opens the folder it is in, with it selected, in
// headless Chrome against a throwaway server (CHROME overrides its path; OWNNAS_TARGET the target
// directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-search-folder-'));
const library = path.join(tmp, 'library');
fs.mkdirSync(path.join(library, 'Projects/2026'), { recursive: true });
fs.writeFileSync(path.join(library, 'Projects/2026/report-alpha.txt'), 'the report');
for (let i = 0; i < 40; i++) fs.writeFileSync(path.join(library, `Projects/2026/aaa-${String(i).padStart(2, '0')}.txt`), 'x');
fs.writeFileSync(path.join(library, 'report-beta.txt'), 'at the top');
fs.writeFileSync(path.join(library, 'report-gamma.txt'), 'at the top too');
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

  await open();
  await until(`document.querySelector('#search')`, 'the file list');
  const search = async (q) => {
    await js(`(() => { const box = document.querySelector('#search'); box.value = ${JSON.stringify(q)}; box.dispatchEvent(new Event('input', { bubbles: true })); box.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); })()`);
    await until(`document.querySelectorAll('#search-results [data-hit-folder]').length > 0`, `matches for ${q}`);
  };

  // Every match has the button, named for its folder.
  await search('report');
  const buttons = await js(`Object.fromEntries([...document.querySelectorAll('#search-results [data-hit-folder]')].map((b) => [b.dataset.hitFolder, b.title]))`);
  assert.deepEqual(Object.keys(buttons).sort(), ['Projects/2026/report-alpha.txt', 'report-beta.txt', 'report-gamma.txt']);
  assert.equal(buttons['Projects/2026/report-alpha.txt'], 'Open the containing folder (Projects/2026)');

  // It opens the folder with the match selected (scrolled to: it is after 40 others), not the file.
  await js(`document.querySelector('#search-results [data-hit-folder="Projects/2026/report-alpha.txt"]').click()`);
  await until(`document.querySelector('#files .card.selected')?.dataset.path === 'Projects/2026/report-alpha.txt'`, 'the match selected in its folder');
  assert.equal(await js(`document.querySelector('#search-panel').hidden`), true);
  assert.equal(await js(`document.querySelectorAll('#files .card.selected').length`), 1);
  assert.equal(await js(`!!document.querySelector('dialog[open]')`), false, 'no preview opened');
  const shown = await js(`(() => { const r = document.querySelector('#files .card.selected').getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight; })()`);
  assert.ok(shown, 'the match is scrolled into view');

  // The match itself still opens the file.
  await open();
  await until(`document.querySelector("#files .card[data-path='report-gamma.txt']")`, 'the top folder');
  await search('alpha');
  await js(`document.querySelector('#search-results .search-hit').click()`);
  await until(`!!document.querySelector('dialog[open]') || !document.querySelector('#preview')?.hidden`, 'the preview');
  assert.deepEqual(problems, []);
  console.log('Search folder button tests passed in Chrome.');
} finally {
  chrome.kill();
  server.kill();
  await sleep(300);
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 5 });
}
