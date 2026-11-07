// Showing whole folders in Gallery workspaces from Files, in headless Chrome against a throwaway
// server: right-click a folder (or the folder you're in), tick workspaces, and everything in it
// shows there, now and later (CHROME overrides Chrome's path; OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-ws-folders-'));
const library = path.join(tmp, 'library');
const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
for (const [rel, seed] of [['Phone/a.jpg', 'a'], ['Phone/2024/b.jpg', 'b'], ['Other/c.jpg', 'c']]) {
  fs.mkdirSync(path.dirname(path.join(library, rel)), { recursive: true });
  fs.writeFileSync(path.join(library, rel), Buffer.concat([JPEG, Buffer.from(seed)]));
}

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
  const work = (await call('POST', '/api/workspaces', { name: 'Work', folders: ['Phone/2024'] })).json.workspace;
  const personal = (await call('POST', '/api/workspaces', { name: 'Private', folders: [] })).json.workspace;
  assert.ok(work && personal, 'workspaces made');
  const foldersOf = async () => Object.fromEntries((await call('GET', '/api/workspaces')).json.workspaces.map((space) => [space.name, space.folders]));

  // Reload signed in, so Files knows there are workspaces.
  await send('Page.navigate', { url: base });
  await until(`document.querySelector('.card[data-path="Phone"]')`, 'the Phone folder');
  const rightClick = (selector) => js(`(() => {
    const node = document.querySelector(${JSON.stringify(selector)});
    const box = node.getBoundingClientRect();
    node.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 5, clientY: box.top + 5 }));
    return true;
  })()`);
  const menuItem = (name) => `document.querySelector('#menu:not([hidden]) [data-menu="${name}"]')`;
  const rows = () => js(`[...document.querySelectorAll('dialog[open] .workspace-pick')].map((row) => {
    const box = row.querySelector('input');
    return { name: row.querySelector('.workspace-pick-name').textContent, checked: box.checked, partly: box.indeterminate, disabled: box.disabled, note: row.querySelector('.workspace-pick-note')?.textContent || '' };
  })`);
  const tick = (name, on) => js(`(() => {
    const row = [...document.querySelectorAll('dialog[open] .workspace-pick')].find((item) => item.querySelector('.workspace-pick-name').textContent === ${JSON.stringify(name)});
    const box = row.querySelector('input');
    box.indeterminate = false;
    box.checked = ${on};
    return true;
  })()`);
  const saveDialog = async () => {
    await js(`[...document.querySelectorAll('dialog[open] button')].find((button) => button.textContent === 'Save').click(), true`);
    await until(`!document.querySelector('dialog[open]')`, 'the dialog to close');
  };

  // A folder holding one of Work's folders: partly in Work, not in Private.
  await rightClick('.card[data-path="Phone"]');
  await until(menuItem('showFolderInWorkspace'), 'Show in workspace… for a folder');
  assert.equal(await js(`document.querySelector('#menu [data-menu="showInWorkspace"]')`), null, 'not the photos version for a folder');
  await js(`${menuItem('showFolderInWorkspace')}.click(), true`);
  await until(`document.querySelector('dialog[open] .workspace-pick')`, 'the folder dialog');
  assert.match(await js(`document.querySelector('dialog[open] h3').textContent`), /Show everything in Phone in/);
  const before = Object.fromEntries((await rows()).map((row) => [row.name, row]));
  assert.deepEqual([before.Work.checked, before.Work.partly, before.Work.note], [false, true, 'Some folders inside it']);
  assert.deepEqual([before.Private.checked, before.Private.partly, before.Private.disabled], [false, false, false]);
  await tick('Work', true);
  await tick('Private', true);
  await saveDialog();
  await until(`document.querySelector('.toast')?.textContent.includes('shows in')`, 'the confirmation');
  assert.deepEqual(await foldersOf(), { Work: ['Phone'], Private: ['Phone'] }, 'Phone/2024 is taken in by Phone');

  // Inside it, through the folder above: ticked, and only changed from that folder.
  await send('Page.navigate', { url: `${base}#/Phone/2024` });
  await until(`document.querySelector('.card[data-path="Phone/2024/b.jpg"]')`, 'the 2024 folder');
  await js(`(() => {
    const files = document.getElementById('files');
    const box = files.getBoundingClientRect();
    files.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.right - 5, clientY: box.bottom - 5 }));
    return true;
  })()`);
  await until(menuItem('showFolderInWorkspace'), 'Show this folder in workspace… in the empty space');
  await js(`${menuItem('showFolderInWorkspace')}.click(), true`);
  await until(`document.querySelector('dialog[open] .workspace-pick')`, 'the folder dialog');
  for (const row of await rows()) assert.deepEqual([row.checked, row.disabled, row.note], [true, true, 'Through “Phone”'], row.name);
  await js(`[...document.querySelectorAll('dialog[open] button')].find((button) => button.textContent === 'Cancel').click(), true`);
  await until(`!document.querySelector('dialog[open]')`, 'the dialog to close');
  const refused = await call('POST', '/api/workspaces/folders', { workspaceIds: [work.id], folders: ['Phone/2024'], action: 'remove' });
  assert.equal(refused.status, 400);
  assert.match(refused.json.error, /Take “Phone” out of Work/);

  // Taking it out of one workspace leaves the other.
  await send('Page.navigate', { url: base });
  await until(`document.querySelector('.card[data-path="Phone"]')`, 'the Phone folder');
  await rightClick('.card[data-path="Phone"]');
  await until(menuItem('showFolderInWorkspace'), 'Show in workspace… for a folder');
  await js(`${menuItem('showFolderInWorkspace')}.click(), true`);
  await until(`document.querySelector('dialog[open] .workspace-pick')`, 'the folder dialog');
  assert.ok((await rows()).every((row) => row.checked && !row.disabled));
  await tick('Private', false);
  await saveDialog();
  assert.deepEqual(await foldersOf(), { Work: ['Phone'], Private: [] });

  // Two folders at once: Phone is in Work, Other is not, so Work is partly ticked.
  const both = (await call('POST', '/api/workspaces/folder-membership', { folders: ['Phone', 'Other'] })).json.workspaces;
  assert.deepEqual(both.find((entry) => entry.id === work.id), { id: work.id, direct: 1, through: 0, via: null, inner: 0 });
  // Only administrators.
  assert.equal((await call('POST', '/api/users', { username: 'plain', password: 'another-long-passphrase', admin: false })).status, 200);
  await js(`fetch('/api/logout', { method: 'POST', headers: { 'x-ownnas': '1' } }).then((r) => r.status)`);
  await js(`fetch('/api/login', { method: 'POST', headers: { 'content-type': 'application/json', 'x-ownnas': '1' }, body: JSON.stringify({ username: 'plain', password: 'another-long-passphrase' }) }).then((r) => r.status)`);
  assert.equal((await call('POST', '/api/workspaces/folders', { workspaceIds: [personal.id], folders: ['Other'] })).status, 403);

  assert.deepEqual(problems, [], 'no errors in the page');
  console.log('ok: folders shown in workspaces from Files');
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
