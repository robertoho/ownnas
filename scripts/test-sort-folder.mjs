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
  await js(`[...document.querySelectorAll('.jobs-item button')].find((b) => b.textContent === 'Dismiss')?.click(), true`);

  // Rename by pattern: in the menu of files, with a preview, and undone.
  await send('Page.navigate', { url: `${base}?again#/Inbox` });
  await until(`document.querySelector('.card[data-path="Inbox/a.txt"]')`, 'the Inbox again');
  await js(`(() => { const card = document.querySelector('.card[data-path="Inbox/a.txt"]'); const box = card.getBoundingClientRect(); card.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 10, clientY: box.top + 10 })); return true; })()`);
  await until(`document.querySelector('#menu:not([hidden]) [data-menu="renameMany"]')`, 'Rename by pattern… in the menu');
  await js(`document.body.click(); openRenameDialog(['Inbox/c.txt', 'Inbox/a.txt', 'Inbox/b.pdf']); true`);
  const renameText = () => js(`document.querySelector('dialog.sort-dialog [data-rename="preview"]').textContent`);
  await until(`document.querySelector('dialog.sort-dialog [data-rename="preview"]')?.textContent.includes('Every name stays')`, 'nothing to rename yet');
  await js(`(() => { const select = document.querySelector('dialog.sort-dialog [data-rename="preset"]'); select.value = '{name} {n:3}'; select.dispatchEvent(new Event('change')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-rename="preview"]').textContent.includes('3 files')`, 'the preview');
  assert.match(await renameText(), /c\.txt→ c 001\.txt/);
  assert.match(await renameText(), /b\.pdf→ b 003\.pdf/);
  await js(`(() => { const input = document.querySelector('dialog.sort-dialog [data-rename="find"]'); input.value = 'C '; input.dispatchEvent(new Event('input')); const other = document.querySelector('dialog.sort-dialog [data-rename="replace"]'); other.value = 'Copy '; other.dispatchEvent(new Event('input')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-rename="preview"]').textContent.includes('Copy 001.txt')`, 'find and replace');
  await until(`document.querySelector('dialog.sort-dialog [data-rename="apply"]').textContent === 'Rename 3' && !document.querySelector('dialog.sort-dialog [data-rename="apply"]').disabled`, 'the Rename button');
  await js(`document.querySelector('dialog.sort-dialog [data-rename="apply"]').click(), true`);
  for (let i = 0; i < 50 && !exists('Inbox/Copy 001.txt'); i++) await new Promise((done) => setTimeout(done, 100));
  for (const rel of ['Inbox/Copy 001.txt', 'Inbox/a 002.txt', 'Inbox/b 003.pdf']) assert.ok(exists(rel), rel);
  await until(`[...document.querySelectorAll('.jobs-item')].some((item) => item.textContent.includes('Renamed 3 files'))`, 'the result');
  await js(`[...document.querySelectorAll('.jobs-item button')].find((b) => b.textContent === 'Undo').click(), true`);
  for (let i = 0; i < 50 && !exists('Inbox/c.txt'); i++) await new Promise((done) => setTimeout(done, 100));
  for (const rel of ['Inbox/a.txt', 'Inbox/b.pdf', 'Inbox/c.txt']) assert.ok(exists(rel), `${rel} back`);

  // The date in the file's name (WhatsApp, phones), rather than when it was copied.
  put('Inbox/IMG-20190704-WA0001.jpg', 'x', '2024-11-01T12:00:00');
  await js(`openSortDialog('Inbox'); true`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="preview"]')?.textContent.includes('into')`, 'the preview');
  assert.match(await js(`document.querySelector('dialog.sort-dialog [data-sort="preview"]').textContent`), /2024\/11/);
  await js(`(() => { const select = document.querySelector('dialog.sort-dialog [data-sort="date"]'); select.value = 'name'; select.dispatchEvent(new Event('change')); return true; })()`);
  await until(`document.querySelector('dialog.sort-dialog [data-sort="preview"]').textContent.includes('2019/07')`, 'the date from the name');
  await js(`document.querySelector('dialog.sort-dialog').close(); true`);

  // Tidy up: the largest files, empty folders, dates from names, and comparing two folders.
  fs.mkdirSync(path.join(library, 'Inbox/empty-one/inner'), { recursive: true });
  put('Inbox/big.bin', Buffer.alloc(300 * 1024, 7), '2024-01-01T12:00:00');
  put('Copy/a.txt', 'a', '2023-05-10T12:00:00');
  put('Copy/extra.txt', 'e', '2023-05-10T12:00:00');
  const tidyText = () => js(`document.querySelector('dialog.tidy-dialog [data-tidy="body"]').textContent`);
  const tab = (name) => js(`document.querySelector('dialog.tidy-dialog [data-tab="${name}"]').click(), true`);
  for (let i = 0; i < 60; i++) {
    await js(`document.querySelector('dialog.tidy-dialog')?.close(); openTidyDialog('Inbox'); true`);
    await until(`!document.querySelector('dialog.tidy-dialog [data-tidy="body"]').textContent.includes('Looking')`, 'the largest files');
    if ((await tidyText()).includes('big.bin')) break;
    await new Promise((done) => setTimeout(done, 300));
  }
  assert.match(await tidyText(), /big\.bin.*300 KB/);
  await tab('empty');
  await until(`document.querySelector('dialog.tidy-dialog [data-tidy="body"]').textContent.includes('empty-one')`, 'the empty folder');
  assert.equal((await tidyText()).includes('inner'), false, 'only the outermost empty folder');
  await js(`document.querySelector('dialog.tidy-dialog [data-tidy="act"]').click(), true`);
  for (let i = 0; i < 50 && exists('Inbox/empty-one'); i++) await new Promise((done) => setTimeout(done, 100));
  assert.equal(exists('Inbox/empty-one'), false, 'moved to the Trash');
  for (let i = 0; i < 60; i++) {
    await tab('dates');
    await until(`!document.querySelector('dialog.tidy-dialog [data-tidy="body"]').textContent.includes('Looking')`, 'the dates');
    if ((await tidyText()).includes('IMG-20190704-WA0001.jpg')) break;
    await new Promise((done) => setTimeout(done, 300));
  }
  assert.match(await tidyText(), /IMG-20190704-WA0001\.jpg2024-11-01 12:00 → 2019-07-04 00:00/);
  await js(`document.querySelector('dialog.tidy-dialog [data-tidy="act"]').click(), true`);
  for (let i = 0; i < 50 && fs.statSync(path.join(library, 'Inbox/IMG-20190704-WA0001.jpg')).mtime.getFullYear() !== 2019; i++) await new Promise((done) => setTimeout(done, 100));
  assert.equal(fs.statSync(path.join(library, 'Inbox/IMG-20190704-WA0001.jpg')).mtime.getFullYear(), 2019, 'the date from its name');
  const compared = await js(`fetch('/api/cleanup/compare?first=Inbox&second=Copy&contents=true').then((r) => r.json())`);
  assert.deepEqual(compared.onlyInSecond, ['extra.txt']);
  assert.ok(compared.onlyInFirst.includes('b.pdf') && compared.same === 1, JSON.stringify(compared));
  await js(`document.querySelector('dialog.tidy-dialog').close(); true`);

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
