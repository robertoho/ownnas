// Sort into folders, in headless Chrome against a throwaway server: the preview for a pattern,
// files in folders inside, sorting, and putting everything back (CHROME overrides Chrome's path;
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
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-sort-'));
const library = path.join(tmp, 'library');
const put = (rel, text, when) => {
  const full = path.join(library, rel);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, text);
  const date = new Date(when);
  fs.utimesSync(full, date, date);
};
put('Inbox/a.txt', 'a', '2023-05-10T12:00:00');
put('Inbox/b.pdf', '%PDF-1.4 b', '2024-03-09T12:00:00');
put('Inbox/c.txt', 'c', '2024-03-20T12:00:00');
put('Inbox/sub/d.txt', 'd', '2022-01-01T12:00:00');
put('Inbox/.hidden.txt', 'h', '2021-01-01T12:00:00');
const exists = (rel) => fs.existsSync(path.join(library, rel));

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
  const signIn = `fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'admin', password: ${JSON.stringify(PASSWORD)} }) }).then((r) => r.status)`;
  assert.equal(await js(signIn), 200);
  const call = (method, url, body) => js(`fetch(${JSON.stringify(url)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, ${body ? `body: ${JSON.stringify(JSON.stringify(body))}` : ''} }).then(async (r) => ({ status: r.status, json: await r.json().catch(() => null) }))`);
  await send('Page.navigate', { url: `${base}?signed-in#/Inbox` });
  await until(`document.querySelector('.card[data-path="Inbox/a.txt"]')`, 'the Inbox');
  await js(`(() => { const files = document.getElementById('files'); const box = files.getBoundingClientRect(); files.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.right - 5, clientY: box.bottom - 5 })); return true; })()`);
  await until(`document.querySelector('#menu:not([hidden]) [data-menu="sortFolder"]')`, 'Sort into folders… in the menu');
  await js(`document.querySelector('#menu [data-menu="sortFolder"]').click(), true`);
  const previewText = () => js(`document.querySelector('dialog.sort-dialog [data-sort="preview"]').textContent`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="preview"]')?.textContent.includes('into')`, 'the preview');
  assert.match(await previewText(), /3 files into 2 folders/);
  assert.match(await previewText(), /2023\/05/);
  assert.match(await previewText(), /2024\/03/);
  assert.equal((await previewText()).includes('hidden'), false);

  await js(`(() => { const box = document.querySelector('dialog.sort-dialog [data-sort="recursive"]'); box.checked = true; box.dispatchEvent(new Event('change')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="preview"]').textContent.includes('4 files into 3 folders')`, 'the files inside too');
  await js(`(() => { const box = document.querySelector('dialog.sort-dialog [data-sort="recursive"]'); box.checked = false; box.dispatchEvent(new Event('change')); return true; })()`);
  await js(`(() => { const select = document.querySelector('dialog.sort-dialog [data-sort="preset"]'); select.value = '{type}'; select.dispatchEvent(new Event('change')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="preview"]').textContent.includes('Documents')`, 'by kind');
  assert.equal(await js(`document.querySelector('dialog.sort-dialog [data-sort="date-row"]').hidden`), true, 'no date for kinds');
  await js(`(() => { const input = document.querySelector('dialog.sort-dialog [data-sort="pattern"]'); input.value = '../{YYYY}'; input.dispatchEvent(new Event('input')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog .sort-problem')`, 'a refused pattern');
  await js(`(() => { const select = document.querySelector('dialog.sort-dialog [data-sort="preset"]'); select.value = '{YYYY}-{MM}'; select.dispatchEvent(new Event('change')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="apply"]').textContent === 'Sort 3 files' && !document.querySelector('dialog.sort-dialog [data-sort="apply"]').disabled`, 'the button');
  if (process.env.SORT_SHOT) {
    const { result } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(process.env.SORT_SHOT, Buffer.from(result.data, 'base64'));
  }
  await js(`document.querySelector('dialog.sort-dialog [data-sort="apply"]').click(), true`);
  await until(`!document.querySelector('dialog.sort-dialog')`, 'the dialog to close');
  for (let i = 0; i < 50 && !exists('Inbox/2024-03/b.pdf'); i++) await new Promise((done) => setTimeout(done, 100));
  for (const rel of ['Inbox/2023-05/a.txt', 'Inbox/2024-03/b.pdf', 'Inbox/2024-03/c.txt', 'Inbox/sub/d.txt', 'Inbox/.hidden.txt']) assert.ok(exists(rel), rel);
  assert.equal(exists('Inbox/a.txt'), false);
  await until(`[...document.querySelectorAll('.jobs-item')].some((item) => item.textContent.includes('Sorted 3 files into 2 folders'))`, 'the result');

  await js(`[...document.querySelectorAll('.jobs-item button')].find((b) => b.textContent === 'Undo').click(), true`);
  for (let i = 0; i < 50 && !exists('Inbox/a.txt'); i++) await new Promise((done) => setTimeout(done, 100));
  for (const rel of ['Inbox/a.txt', 'Inbox/b.pdf', 'Inbox/c.txt', 'Inbox/sub/d.txt']) assert.ok(exists(rel), rel);
  assert.equal(exists('Inbox/2023-05'), false, 'the folders it made are gone');
  assert.equal(exists('Inbox/2024-03'), false);

  assert.deepEqual(problems, [], 'no errors in the page');
  console.log('ok: sort into folders');
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
