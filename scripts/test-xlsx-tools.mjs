// The spreadsheet editor's AutoSum, copy formatting, duplicates, text to columns, gridlines and
// tab colors, driven in headless Chrome (CHROME overrides its path).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { zipSync, unzipSync, strToU8, strFromU8 } from '../web/vendor/three/addons/libs/fflate.module.js';

const webDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'web');
const MAIN = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const text = (ref, value, s) => `<c r="${ref}" t="inlineStr"${s ? ` s="${s}"` : ''}><is><t>${value}</t></is></c>`;
const num = (ref, value) => `<c r="${ref}"><v>${value}</v></c>`;
const sheet = `<worksheet xmlns="${MAIN}"><sheetData>
<row r="1">${num('A1', 10)}${text('C1', 'Name', 1)}${text('D1', 'City')}</row>
<row r="2">${num('A2', 20)}${text('C2', 'Ann')}${text('D2', 'Rome')}</row>
<row r="3">${num('A3', 30)}${text('C3', 'Bob')}${text('D3', 'Oslo')}</row>
<row r="4">${text('C4', 'Ann')}${text('D4', 'Rome')}</row>
<row r="5">${text('C5', 'Cid')}${text('D5', 'Rome')}</row>
<row r="7">${text('A7', 'x,y,z')}</row>
<row r="8">${text('A8', 'p,q')}</row>
</sheetData></worksheet>`;
const parts = {
  '[Content_Types].xml': strToU8(`<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>`),
  '_rels/.rels': strToU8(`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${R}/officeDocument" Target="xl/workbook.xml"/></Relationships>`),
  'xl/workbook.xml': strToU8(`<workbook xmlns="${MAIN}" xmlns:r="${R}"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>`),
  'xl/_rels/workbook.xml.rels': strToU8(`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${R}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="${R}/styles" Target="styles.xml"/></Relationships>`),
  'xl/styles.xml': strToU8(`<styleSheet xmlns="${MAIN}"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="2"><xf/><xf fontId="1" applyFont="1"/></cellXfs></styleSheet>`),
  'xl/worksheets/sheet1.xml': strToU8(sheet),
};
const types = { '.js': 'text/javascript', '.css': 'text/css' };
const server = http.createServer((req, res) => {
  const url = req.url.split('?')[0];
  if (url === '/') { res.writeHead(200, { 'content-type': 'text/html' }); res.end('<!doctype html><link rel=stylesheet href=/app.css><link rel=stylesheet href=/docx-editor.css><body><div id=host style="height:700px;display:flex"></div>'); return; }
  const file = path.join(webDir, url);
  if (!file.startsWith(webDir) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' });
  res.end(fs.readFileSync(file));
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const base = `http://127.0.0.1:${server.address().port}/`;
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-xlsx-'));
const chromePath = process.env.CHROME || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const port = 9800 + Math.floor(Math.random() * 150);
const chrome = spawn(chromePath, ['--headless=new', '--disable-gpu', '--window-size=1300,900', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
const sleep = ms => new Promise(done => setTimeout(done, ms));
let targets;
for (let i = 0; i < 60 && !targets?.length; i++) { await sleep(200); try { targets = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).filter(t => t.type === 'page'); } catch {} }
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
await new Promise(done => { ws.onopen = done; });
let nextId = 0; const waiting = new Map(), problems = [];
ws.onmessage = event => {
  const m = JSON.parse(event.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  if (m.method === 'Runtime.exceptionThrown') problems.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
};
const send = (method, params = {}) => new Promise(done => { const id = ++nextId; waiting.set(id, done); ws.send(JSON.stringify({ id, method, params })); });
const js = async expression => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails.text);
  return r.result.result.value;
};
const press = async (name, code, keyCode, modifiers = 0) => {
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
};
const RIGHT = ['ArrowRight', 'ArrowRight', 39], DOWN = ['ArrowDown', 'ArrowDown', 40], LEFT = ['ArrowLeft', 'ArrowLeft', 37], UP = ['ArrowUp', 'ArrowUp', 38];
/** From A1 to a cell, by keys; with `to`, Shift+arrows to the end of a range. */
const go = async (row, col, to) => {
  await js(`document.querySelector('[aria-label="Spreadsheet cells"]').focus()`);
  await press('Home', 'Home', 36, 2);
  for (let i = 1; i < col; i++) await press(...RIGHT);
  for (let i = 1; i < row; i++) await press(...DOWN);
  if (to) { for (let i = col; i < to[1]; i++) await press(...RIGHT, 8); for (let i = row; i < to[0]; i++) await press(...DOWN, 8); }
  await sleep(30);
};
const clickTool = label => js(`document.querySelector('[aria-label=${JSON.stringify(label)}]').click()`);
const menu = label => js(`(()=>{const b=[...document.querySelectorAll('.xlsx-menu button, [role=menu] button, .menu button')].find(b=>b.textContent.trim()===${JSON.stringify(label)});if(!b)throw new Error('no menu item '+${JSON.stringify(label)}+': '+[...document.querySelectorAll('button')].map(n=>n.textContent).join('|').slice(0,300));b.click()})()`);
const mount = () => js(`(async()=>{window.ed?.destroy?.();const m=await import('/xlsx-editor.js');const b=Uint8Array.from(atob(window.fixture),c=>c.charCodeAt(0));window.ed=m.mountXlsxEditor(document.getElementById('host'),b,{onSave:async d=>{window.saved=btoa(Array.from(d,c=>String.fromCharCode(c)).join(''))}});await new Promise(r=>setTimeout(r,120));return true})()`);
const out = async () => {
  // Saving hands over the file; with nothing changed since the last save there is nothing new.
  await js(`(async()=>{window.saved=null;await window.ed.save()})()`);
  const saved = await js('window.saved');
  if (saved) lastXml = strFromU8(unzipSync(Buffer.from(saved, 'base64'))['xl/worksheets/sheet1.xml']);
  return lastXml;
};
const cellOf = (xml, ref) => new RegExp(`<c r="${ref}"[^>]*?(?:/>|>.*?</c>)`, 's').exec(xml)?.[0] ?? '';
const texts = xml => [...xml.matchAll(/<t(?: [^>]*)?>([^<]*)<\/t>/g)].map(m => m[1]);
let failed = null, lastXml = sheet;
try {
  await send('Runtime.enable'); await send('Page.enable');
  await send('Page.navigate', { url: base }); await sleep(400);
  await js(`window.fixture=${JSON.stringify(Buffer.from(zipSync(parts)).toString('base64'))}`);
  await mount();

  // AutoSum under a column of numbers, with its value worked out, and undo.
  await go(4, 1); await clickTool('AutoSum (Alt+=)'); await sleep(60);
  let xml = await out();
  assert.match(cellOf(xml, 'A4'), /<f>SUM\(A1:A3\)<\/f>/); assert.match(cellOf(xml, 'A4'), /<v>60<\/v>/);
  await press('z', 'KeyZ', 90, process.platform === 'darwin' ? 4 : 2); await sleep(40);
  xml = await out(); assert.equal(cellOf(xml, 'A4'), '', 'undo takes the sum away');
  await go(4, 1); await press('=', 'Equal', 187, 1); await sleep(60);
  assert.match(cellOf(await out(), 'A4'), /SUM\(A1:A3\)/, 'Alt+= does the same');

  // Copy formatting from a bold header onto other cells.
  await mount();
  await go(1, 3); await clickTool('Copy formatting: then select the cells to apply it to');
  assert.equal(await js(`document.querySelector('[aria-label^="Copy formatting"]').getAttribute('aria-pressed')`), 'true');
  await go(2, 3, [3, 3]); await js(`window.dispatchEvent(new PointerEvent('pointerup',{bubbles:true}))`); await sleep(60);
  xml = await out(); assert.match(cellOf(xml, 'C2'), /s="1"/); assert.match(cellOf(xml, 'C3'), /s="1"/); assert.doesNotMatch(cellOf(xml, 'D2'), /s="1"/);
  assert.equal(await js(`document.querySelector('[aria-label^="Copy formatting"]').getAttribute('aria-pressed')`), 'false');

  // Clear formatting takes the style off and leaves the value.
  await clickTool('More tools'); await menu('Clear formatting'); await sleep(40);
  xml = await out(); assert.doesNotMatch(cellOf(xml, 'C2'), /s="1"/); assert.match(cellOf(xml, 'C2'), /Ann/);

  // Remove duplicates: a repeat goes, others close up, the header and other columns stay; undo.
  await mount();
  await go(1, 3, [5, 4]); await clickTool('More tools'); await menu('Remove duplicates'); await sleep(60);
  xml = await out();
  assert.deepEqual(texts(xml).filter(t => /Ann|Bob|Cid/.test(t)), ['Ann', 'Bob', 'Cid']);
  assert.match(cellOf(xml, 'C4'), /Cid/); assert.equal(cellOf(xml, 'C5'), ''); assert.match(cellOf(xml, 'C1'), /Name/); assert.match(cellOf(xml, 'A2'), /<v>20<\/v>/);
  await press('z', 'KeyZ', 90, process.platform === 'darwin' ? 4 : 2); await sleep(40);
  xml = await out(); assert.deepEqual(texts(xml).filter(t => /Ann/.test(t)).length, 2);

  // Text to columns.
  await mount();
  await go(7, 1, [8, 1]); await clickTool('More tools'); await menu('Text to columns…'); await sleep(30);
  await js(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Split').click()`); await sleep(50);
  xml = await out(); for (const [ref, want] of [['A7', 'x'], ['B7', 'y'], ['C7', 'z'], ['A8', 'p'], ['B8', 'q']]) assert.match(cellOf(xml, ref), new RegExp(`>${want}<`), ref);

  // It warns before replacing what is to the right.
  await mount();
  await go(1, 3, [2, 3]);
  await clickTool('More tools'); await menu('Text to columns…'); await sleep(30);
  await js(`(()=>{[...document.querySelectorAll('button')].find(b=>b.textContent==='Split').click()})()`); await sleep(30);
  assert.match(await js(`document.querySelector('.xlsx-panel-problem').textContent`), /None of these cells/);
  await js(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Cancel').click()`);

  // Gridlines, saved in the sheet and undoable.
  await mount();
  assert.ok(await js(`document.querySelectorAll('.xlsx-hline').length`) > 0);
  await clickTool('More tools'); await menu('Hide gridlines'); await sleep(60);
  assert.match(await out(), /showGridLines="0"/); assert.equal(await js(`document.querySelectorAll('.xlsx-hline').length`), 0);
  await clickTool('More tools'); await menu('Show gridlines'); await sleep(60);
  assert.doesNotMatch(await out(), /showGridLines/);

  // Tab color: in sheetPr, which comes first in the sheet, and shown on the tab.
  await mount();
  await clickTool('More tools'); await menu('Tab color…'); await sleep(30);
  await js(`(()=>{document.querySelector('.xlsx-format-panel input[type=color]').value='#ff8800';[...document.querySelectorAll('button')].find(b=>b.textContent==='Apply').click()})()`); await sleep(50);
  xml = await out(); assert.match(xml, /<sheetPr[^>]*><tabColor rgb="FFFF8800"\/><\/sheetPr>/); assert.ok(xml.indexOf('<sheetPr') < xml.indexOf('<sheetData'));
  assert.equal(await js(`document.querySelector('.xlsx-tab.has-color').style.getPropertyValue('--tab-color').toLowerCase()`), '#ff8800');

  assert.deepEqual(problems, [], 'the page reports no errors');
  console.log('Spreadsheet tools tests passed in Chrome.');
} catch (err) { failed = err; }
ws.close(); chrome.kill(); server.close();
await new Promise(done => { chrome.once('exit', done); setTimeout(done, 3000); });
try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); } catch {}
if (failed) { console.error(failed); if (problems.length) console.error('Page errors:', problems); process.exit(1); }
