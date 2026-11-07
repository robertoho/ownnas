// Notes in headless Chrome against a throwaway server: Markdown files shown as cards, quick
// capture, checklists ticked on the card, colour, pins, labels, links, the archive and daily notes
// (CHROME overrides Chrome's path; OWNNAS_TARGET the target directory).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const target = process.env.OWNNAS_TARGET || path.join(repo, 'target/debug');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-notes-'));
const library = path.join(tmp, 'library');
const put = (rel, text) => {
  fs.mkdirSync(path.dirname(path.join(library, rel)), { recursive: true });
  fs.writeFileSync(path.join(library, rel), text);
};
put('Notes/Groceries.md', '---\npinned: true\n---\n# Groceries\n\n- [ ] milk\n- [ ] eggs\n');
put('Notes/Recipes.md', 'Pancakes need things from [[Groceries]]. #cooking\n');
put('Notes/Archive/Old.md', '# Old\n\nlong ago\n');
put('Notes/attachments/pic.md', 'not a note\n');
const read = (rel) => fs.readFileSync(path.join(library, rel), 'utf8');
const exists = (rel) => fs.existsSync(path.join(library, rel));
const fileHas = async (rel, pattern) => {
  for (let i = 0; i < 100; i++) {
    if (exists(rel) && pattern.test(read(rel))) return;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.match(exists(rel) ? read(rel) : '(missing)', pattern, rel);
};

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
  const cards = () => js(`[...document.querySelectorAll('#notes-view .note-card')].map((card) => card.querySelector('.note-title').textContent)`);
  const cardOf = (name) => `[...document.querySelectorAll('#notes-view .note-card')].find((card) => card.querySelector('.note-title').textContent === ${JSON.stringify(name)})`;
  const click = (expression) => js(`(${expression}).click(), true`);
  // A new page load, signed in now.
  await send('Page.navigate', { url: `${base}?signed-in#notes` });
  await until(`document.querySelectorAll('#notes-view .note-card').length === 2`, 'the two notes');
  assert.deepEqual(await js(`[...document.querySelectorAll('#notes-view .notes-section')].map((h) => h.textContent)`), ['Pinned', 'Others']);
  assert.equal((await cards())[0], 'Groceries', 'pinned first');
  assert.match(await js(`${cardOf('Groceries')}.textContent`), /Linked from Recipes/);
  assert.equal(await js(`document.querySelector('#notes-view').textContent.includes('not a note')`), false, 'attachments are not notes');

  // Ticking on the card writes the file.
  await click(`${cardOf('Groceries')}.querySelector('input[aria-label="milk"]')`);
  await fileHas('Notes/Groceries.md', /- \[x\] milk\n- \[ \] eggs/);
  assert.equal(await js(`document.getElementById('preview').hidden`), true, 'ticking does not open the note');

  // Quick capture: a note, then a checklist.
  const type = (selector, text) => js(`(() => { const field = document.querySelector(${JSON.stringify(selector)}); field.focus(); field.value = ${JSON.stringify(text)}; field.dispatchEvent(new Event('input')); return true; })()`);
  await type('#notes-view .notes-capture-body', 'first idea #work');
  await type('#notes-view .notes-capture-title', 'Ideas');
  await click(`[...document.querySelectorAll('#notes-view .notes-capture button')].find((b) => b.textContent === 'Add')`);
  await until(`${cardOf('Ideas')}`, 'the new note');
  assert.equal(read('Notes/Ideas.md'), '# Ideas\n\nfirst idea #work\n');
  assert.ok(await js(`[...document.querySelectorAll('#notes-view .notes-label')].some((b) => b.textContent === '#work')`), 'its hashtag is a label');
  await js(`document.querySelector('#notes-view .notes-capture-body').focus(), true`);
  await click(`[...document.querySelectorAll('#notes-view .notes-capture button')].find((b) => b.textContent.includes('List'))`);
  await type('#notes-view .notes-capture-title', 'Shopping');
  await type('#notes-view .notes-capture-body', 'bread\njam');
  await click(`[...document.querySelectorAll('#notes-view .notes-capture button')].find((b) => b.textContent === 'Add')`);
  await until(`${cardOf('Shopping')}`, 'the checklist');
  assert.equal(read('Notes/Shopping.md'), '# Shopping\n\n- [ ] bread\n- [ ] jam\n');

  // Colour and pin, in the front matter.
  await click(`${cardOf('Ideas')}.querySelector('button[title="Colour"]')`);
  await click(`${cardOf('Ideas')}.querySelector('.notes-color-menu button[title="Blue"]')`);
  await until(`${cardOf('Ideas')}?.classList.contains('note-color-blue')`, 'a blue card');
  await click(`${cardOf('Ideas')}.querySelector('.note-pin')`);
  await until(`${cardOf('Ideas')}?.classList.contains('is-pinned')`, 'a pinned card');
  assert.equal(read('Notes/Ideas.md'), '---\ncolor: blue\npinned: true\n---\n# Ideas\n\nfirst idea #work\n');

  if (process.env.NOTES_GRID_SHOT) {
    await js(`document.activeElement?.blur(), true`);
    const { result } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(process.env.NOTES_GRID_SHOT, Buffer.from(result.data, 'base64'));
  }

  // Searching and labels.
  await type('#notes-view .notes-search', 'jam');
  await until(`document.querySelectorAll('#notes-view .note-card').length === 1`, 'one match');
  assert.deepEqual(await cards(), ['Shopping']);
  await type('#notes-view .notes-search', '');
  await click(`[...document.querySelectorAll('#notes-view .notes-label')].find((b) => b.textContent === '#cooking')`);
  await until(`document.querySelectorAll('#notes-view .note-card').length === 1`, 'one labelled note');
  assert.deepEqual(await cards(), ['Recipes']);
  await click(`[...document.querySelectorAll('#notes-view .notes-label')].find((b) => b.textContent === 'All')`);

  // The archive.
  await click(`${cardOf('Ideas')}.querySelector('button[title="Archive"]')`);
  await until(`!${cardOf('Ideas')}`, 'Ideas archived');
  assert.ok(exists('Notes/Archive/Ideas.md'));
  await click(`[...document.querySelectorAll('#notes-view .notes-tools button')].find((b) => b.textContent === 'Archive')`);
  await until(`location.hash === '#notes/archive' && document.querySelectorAll('#notes-view .note-card').length === 2`, 'the archive');
  assert.deepEqual((await cards()).sort(), ['Ideas', 'Old']);
  await click(`${cardOf('Ideas')}.querySelector('button[title="Unarchive"]')`);
  await until(`!${cardOf('Ideas')}`, 'Ideas back');
  assert.ok(exists('Notes/Ideas.md'));

  // Opening a note, and today's daily note.
  await send('Page.navigate', { url: `${base}#notes` });
  await until(`${cardOf('Recipes')}`, 'the notes again');
  await click(cardOf('Recipes'));
  await until(`!document.getElementById('preview').hidden && document.getElementById('preview-title').textContent === 'Recipes.md'`, 'the note open');
  await js(`document.getElementById('preview-close')?.click(), true`);
  await click(`[...document.querySelectorAll('#notes-view .notes-tools button')].find((b) => b.textContent === 'Today')`);
  const day = new Date();
  const stamp = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, '0')}-${String(day.getDate()).padStart(2, '0')}`;
  await until(`document.getElementById('preview-title').textContent === '${stamp}.md'`, 'the daily note');
  assert.equal(read(`Notes/Daily/${stamp}.md`), `# ${stamp}\n\n`);
  const { result: shot } = await send('Page.captureScreenshot', { format: 'png' });
  if (process.env.NOTES_SHOT) fs.writeFileSync(process.env.NOTES_SHOT, Buffer.from(shot.data, 'base64'));
  await js(`document.getElementById('preview-close').click(), true`);

  // A [[link]] in the preview opens the note it names.
  await click(cardOf('Recipes'));
  await until(`document.querySelector('#preview-body .md-wikilink[data-wiki="Groceries"]')`, 'the link in the preview');
  await click(`document.querySelector('#preview-body .md-wikilink')`);
  await until(`document.getElementById('preview-title').textContent === 'Groceries.md'`, 'the linked note');
  assert.equal(await js(`document.querySelector('#preview-body .md-preview').textContent.includes('pinned')`), false, 'front matter is not shown');
  assert.equal(await js(`document.querySelectorAll('#preview-body .md-task input[type=checkbox]').length`), 2, 'checklists show as boxes');

  // Typing [[ suggests notes; a pasted picture lands in attachments, linked.
  await click(`document.querySelector('#preview [data-editor="mode"][data-mode="edit"]')`);
  await until(`document.querySelector('#preview-body [data-editor-input]')`, 'the editor');
  await js(`(() => { const area = document.querySelector('#preview-body [data-editor-input]'); area.focus(); area.value += 'See [[Rec'; area.setSelectionRange(area.value.length, area.value.length); area.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
  await until(`[...document.querySelectorAll('.link-menu li')].some((li) => li.textContent === 'Recipes')`, 'the suggestions');
  await js(`document.querySelector('#preview-body [data-editor-input]').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })), true`);
  await until(`document.querySelector('#preview-body [data-editor-input]').value.endsWith('See [[Recipes]]')`, 'the link completed');
  await js(`(() => {
    const area = document.querySelector('#preview-body [data-editor-input]');
    area.setRangeText('\\n', area.value.length, area.value.length, 'end');
    const png = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='), (c) => c.charCodeAt(0));
    const transfer = new DataTransfer();
    transfer.items.add(new File([png], 'image.png', { type: 'image/png' }));
    area.dispatchEvent(new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }));
    return true;
  })()`);
  await until(`document.querySelector('#preview-body [data-editor-input]').value.includes('![](attachments/Pasted')`, 'the picture linked');
  assert.equal(fs.readdirSync(path.join(library, 'Notes/attachments')).filter((name) => name.startsWith('Pasted')).length, 1);
  await click(`document.querySelector('#preview [data-editor="save"]')`);
  await fileHas('Notes/Groceries.md', /See \[\[Recipes\]\]\n!\[\]\(attachments\/Pasted/);
  await click(`document.querySelector('#preview [data-editor="mode"][data-mode="view"]')`);
  await until(`document.querySelector('#preview-body img.md-image')?.naturalWidth === 1`, 'the picture in the preview');

  // Renaming a note takes the links to it along.
  await js(`document.getElementById('preview-close').click(), true`);
  assert.equal((await call('POST', '/api/rename', { path: 'Notes/Groceries.md', name: 'Shopping list.md' })).status, 200);
  await fileHas('Notes/Recipes.md', /\[\[Shopping list\]\]/);

  // Notebooks: the sub-folders, and new notes go into the one shown.
  put('Notes/Work/Plan.md', '# Plan\n\nship it\n');
  await send('Page.navigate', { url: `${base}?notebooks#notes` });
  await until(`[...document.querySelectorAll('#notes-view .notes-notebooks button')].some((b) => b.textContent.includes('Work'))`, 'the Work notebook');
  await click(`[...document.querySelectorAll('#notes-view .notes-notebooks button')].find((b) => b.textContent.includes('Work'))`);
  await until(`document.querySelectorAll('#notes-view .note-card').length === 1`, 'only the Work notes');
  assert.deepEqual(await cards(), ['Plan']);
  await type('#notes-view .notes-capture-body', 'work idea');
  await click(`[...document.querySelectorAll('#notes-view .notes-capture button')].find((b) => b.textContent === 'Add')`);
  await fileHas('Notes/Work/work idea.md', /^work idea\n$/);

  assert.deepEqual(problems, [], 'no errors in the page');
  console.log('ok: notes');
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
