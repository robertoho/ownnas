// Drives the DOCX editor in headless Chrome (CHROME overrides its path): real typing, selections
// and saving, then checks the XML that comes out. Run: node scripts/test-docx-editor.mjs
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { zipSync, unzipSync, strToU8, strFromU8 } from '../web/vendor/three/addons/libs/fflate.module.js';

const webDir = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'web');
const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAEBgIApD5fRAAAAABJRU5ErkJggg==', 'base64');
const documentXml = `<?xml version="1.0"?><w:document xmlns:w="${W}" xmlns:r="${R}"><w:body>
<w:p><w:bookmarkStart w:id="0" w:name="keep"/><w:r><w:rPr><w:i/></w:rPr><w:t>Hello brave world</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Chapter</w:t></w:r></w:p>
<w:p><w:r><w:t>Second </w:t></w:r><w:fldSimple w:instr="PAGE"><w:r><w:t>7</w:t></w:r></w:fldSimple><w:r><w:t> tail</w:t></w:r></w:p>
<w:p><w:ins w:author="A"><w:r><w:t>added</w:t></w:r></w:ins><w:del w:author="A"><w:r><w:delText>gone</w:delText></w:r></w:del></w:p>
<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>Last paragraph</w:t></w:r></w:p>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:bottom="1440" w:left="1440" w:right="1440"/></w:sectPr></w:body></w:document>`;
const styles = `<w:styles xmlns:w="${W}"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="40"/></w:rPr></w:style></w:styles>`;
const packageParts = {
  '[Content_Types].xml': strToU8('<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>'),
  '_rels/.rels': strToU8(`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${R}/officeDocument" Target="word/document.xml"/></Relationships>`),
  'word/document.xml': strToU8(documentXml),
  'word/styles.xml': strToU8(styles),
  'word/_rels/document.xml.rels': strToU8(`<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="${R}/styles" Target="styles.xml"/></Relationships>`),
  'word/header9.xml': strToU8('<kept/>'),
};
const types = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html' };
const server = http.createServer((req, res) => {
  const url = req.url.split('?')[0];
  if (url === '/') {
    res.writeHead(200, { 'content-type': 'text/html', 'content-security-policy': "default-src 'self'; img-src 'self' blob:; style-src 'self'; script-src 'self' 'unsafe-eval'" });
    res.end('<!doctype html><link rel=stylesheet href=/docx-editor.css><body><div id=host></div>');
    return;
  }
  const file = path.join(webDir, url);
  if (!file.startsWith(webDir) || !fs.existsSync(file)) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' });
  res.end(fs.readFileSync(file));
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const base = `http://127.0.0.1:${server.address().port}/`;
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-docx-'));
const chromePath = process.env.CHROME || (process.platform === 'darwin' ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' : 'google-chrome');
const debugPort = 9300 + Math.floor(Math.random() * 500);
const chrome = spawn(chromePath, ['--headless=new', '--disable-gpu', '--no-first-run', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
const sleep = ms => new Promise(done => setTimeout(done, ms));
let targets;
for (let i = 0; i < 60 && !targets?.length; i++) { await sleep(200); try { targets = (await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json()).filter(t => t.type === 'page'); } catch {} }
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
await new Promise(done => { ws.onopen = done; });
let nextId = 0; const waiting = new Map(), problems = [];
ws.onmessage = event => {
  const m = JSON.parse(event.data);
  if (m.id && waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); }
  if (m.method === 'Runtime.exceptionThrown') problems.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
  if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error' && !/favicon/.test(m.params.entry.url ?? '') && !/inline event handler/.test(m.params.entry.text)) problems.push(m.params.entry.text);
};
const send = (method, params = {}) => new Promise(done => { const id = ++nextId; waiting.set(id, done); ws.send(JSON.stringify({ id, method, params })); });
const js = async expression => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.result.exceptionDetails) throw new Error(r.result.exceptionDetails.exception?.description ?? r.result.exceptionDetails.text);
  return r.result.result.value;
};
const type = text => send('Input.insertText', { text });
const key = async (name, code, modifiers = 0, keyCode = 0, text) => {
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: name, code, modifiers, windowsVirtualKeyCode: keyCode, ...(text ? { text, unmodifiedText: text } : {}) });
  if (text) await send('Input.dispatchKeyEvent', { type: 'char', key: name, code, modifiers, text, unmodifiedText: text });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: name, code, modifiers, windowsVirtualKeyCode: keyCode });
};
const ENTER = () => key('Enter', 'Enter', 0, 13, '\r'), BACKSPACE = () => key('Backspace', 'Backspace', 0, 8);
const tool = label => js(`document.querySelector('[aria-label=${JSON.stringify(label)}]').click()`);
const pick = (label, value) => js(`(()=>{const s=document.querySelector('[aria-label=${JSON.stringify(label)}]');s.value=${JSON.stringify(value)};s.dispatchEvent(new Event('change'))})()`);
const paragraphs = () => js(`[...document.querySelectorAll('.docx-page p[data-p]')].map(p=>p.textContent)`);
/** Selects by text inside the nth paragraph, across runs. */
const select = (index, from, to) => js(`(()=>{const b=document.querySelectorAll('.docx-page p[data-p]')[${index}];const w=document.createTreeWalker(b,NodeFilter.SHOW_TEXT);let n,a=${from},z=${to},r=document.createRange(),s=0,e=0;while(n=w.nextNode()){if(n.parentElement.closest('[data-x]'))continue;const l=n.nodeValue.length;if(!s&&a<=l){r.setStart(n,a);s=1}else if(!s)a-=l;if(!e&&z<=l){r.setEnd(n,z);e=1}else if(!e)z-=l}document.querySelector('.docx-page').focus();const g=getSelection();g.removeAllRanges();g.addRange(r)})()`);
const caret = (index, at) => select(index, at, at);
const mount = (extra = '') => js(`(async()=>{window.ed?.destroy();const m=await import('/docx-editor.js');window.saved=null;const bytes=Uint8Array.from(atob(window.fixture),c=>c.charCodeAt(0));window.ed=m.mountDocxEditor(document.getElementById('host'),bytes,{onSave:async d=>{window.saved=btoa(String.fromCharCode(...d))}${extra}});return true})()`);
const out = async () => {
  await js(`window.lastExport=btoa(Array.from(window.ed.export(),c=>String.fromCharCode(c)).join(''))`);
  const parts = unzipSync(Buffer.from(await js('window.lastExport'), 'base64'));
  return { parts, xml: strFromU8(parts['word/document.xml']), text: name => strFromU8(parts[name]) };
};
const pageText = () => js(`document.querySelector('.docx-page').textContent`);
let failed = null;
try {
  await send('Runtime.enable'); await send('Log.enable'); await send('Page.enable');
  await send('Page.navigate', { url: base }); await sleep(500);
  await js(`window.fixture=${JSON.stringify(Buffer.from(zipSync(packageParts)).toString('base64'))}`);
  await mount();

  // Untouched: every paragraph's XML is kept as it was, even though the page was rendered.
  let { xml, parts } = await out();
  assert.match(xml, /<w:bookmarkStart w:id="0" w:name="keep"\/>/); const bare = text => text.slice(text.indexOf('<w:body>')); // The serializer may order the root's namespaces differently.
  { const a = bare(xml), b = bare(documentXml); let at = 0; while (at < a.length && a[at] === b[at]) at++; assert.equal(a === b, true, `an unedited document comes out unchanged; differs at ${at}: ${JSON.stringify(a.slice(at - 30, at + 60))} vs ${JSON.stringify(b.slice(at - 30, at + 60))}`); }
  assert.deepEqual(parts['word/header9.xml'], packageParts['word/header9.xml']);
  assert.equal(await js('ed.dirty'), false);

  // Formatting part of a run, the thing the old editor could not do.
  await select(0, 6, 11); await tool('Bold');
  ({ xml } = await out());
  assert.match(xml, /<w:rPr><w:i\/><\/w:rPr><w:t[^>]*>Hello <\/w:t>/);
  assert.match(xml, /<w:rPr><w:b\/><w:i\/><\/w:rPr><w:t[^>]*>brave<\/w:t>/, 'b goes before i, as the schema orders them');
  assert.match(xml, /<w:bookmarkStart w:id="0" w:name="keep"\/>/, 'a bookmark survives its paragraph being edited');
  assert.equal(await js('ed.dirty'), true);
  await tool('Undo'); ({ xml } = await out()); assert.doesNotMatch(xml, /<w:b\/>/); assert.equal(await js('ed.dirty'), false);
  await tool('Redo'); ({ xml } = await out()); assert.match(xml, /<w:b\/>/);

  // Across two paragraphs, including one with a style.
  await js(`(()=>{const ps=document.querySelectorAll('.docx-page p[data-p]');const r=document.createRange();r.setStart(ps[0].querySelector('[data-r]').firstChild,0);r.setEnd(ps[1].querySelector('[data-r]').firstChild,3);document.querySelector('.docx-page').focus();getSelection().removeAllRanges();getSelection().addRange(r)})()`);
  await tool('Underline'); ({ xml } = await out());
  assert.match(xml, /<w:u w:val="single"\/><\/w:rPr><w:t[^>]*>Cha<\/w:t>/);
  await tool('Undo');

  // Typing, in the middle of a run; Enter splits; Backspace joins again.
  await caret(6, 4); await type('XY');
  assert.equal((await paragraphs())[6], 'LastXY paragraph');
  await ENTER();
  let list = await paragraphs(); assert.deepEqual(list.slice(-2), ['LastXY', ' paragraph']);
  await type('Z'); list = await paragraphs(); assert.equal(list[list.length - 1], 'Z paragraph');
  await caret(list.length - 1, 0); await BACKSPACE();
  list = await paragraphs(); assert.equal(list[list.length - 1], 'LastXYZ paragraph');
  ({ xml } = await out()); assert.match(xml, /LastXY/); assert.match(xml, /<\/w:tbl>\s*<w:p>/);
  await js('ed.undo()');await js('ed.redo()');

  // A deletion across paragraphs joins them; a field in the way goes with it only if selected.
  await mount();
  await select(0, 5, 5); await js(`(()=>{const ps=document.querySelectorAll('.docx-page p[data-p]');const g=getSelection();const r=g.getRangeAt(0);r.setEnd(ps[1].querySelector('[data-r]').firstChild,4)})()`);
  await BACKSPACE(); list = await paragraphs(); assert.equal(list[0], 'Helloter');
  await tool('Undo'); list = await paragraphs(); assert.equal(list[0], 'Hello brave world');

  // Typed format with no selection applies to what is typed next.
  await caret(0, 5); await tool('Italic'); await type('Q');
  ({ xml } = await out()); assert.match(xml, /<w:t[^>]*>Q<\/w:t>/); assert.doesNotMatch(xml.match(/<w:r>(?:(?!<\/w:r>).)*>Q<\/w:t>/)[0], /<w:i\/>/, 'italic was on, so this turned it off');

  // Fields and tracked changes are locked but kept.
  await mount();
  assert.equal(await js(`document.querySelector('.docx-field').closest('[contenteditable=false]')!==null`), true);
  await select(2, 0, 2); await tool('Bold'); ({ xml } = await out());
  assert.match(xml, /<w:fldSimple w:instr="PAGE">/); assert.match(xml, /<w:delText>gone<\/w:delText>/);
  assert.equal(await js(`!!document.querySelector('.docx-removed')&&!!document.querySelector('.docx-added')`), true);
  await js(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Accept all changes').click()`);
  ({ xml } = await out()); assert.doesNotMatch(xml, /<w:ins|<w:del |delText/); assert.match(xml, /<w:t>added<\/w:t>/); assert.doesNotMatch(xml, /gone/);

  // Styles: a built-in one is written into styles.xml when first used.
  await mount();
  await caret(0, 2); await pick('Paragraph style', 'Heading2');
  let exported = await out(); assert.match(exported.xml, /<w:pStyle w:val="Heading2"\/>/); assert.match(exported.text('word/styles.xml'), /w:styleId="Heading2"/);
  assert.equal(await js(`document.querySelector('.docx-page p').getAttribute('aria-level')`), '2');
  await pick('Paragraph style', ''); ({ xml } = await out()); assert.doesNotMatch(xml.split('</w:p>')[0], /pStyle/);
  await pick('Font', 'Georgia'); await pick('Font size', '18');

  // Lists: numbering.xml made, relationship and type declared, markers counted, Enter continues,
  // an empty item ends the list, Tab nests.
  await mount();
  await caret(6, 0); await tool('Numbered list'); await key('End', 'End', 0, 35);
  await ENTER(); await type('Two'); await ENTER(); await type('Sub'); await key('Tab', 'Tab', 0, 9); await ENTER(); await key('Tab', 'Tab', 2, 9);
  let markers = await js(`[...document.querySelectorAll('.docx-page p[data-marker]')].map(p=>p.dataset.marker)`);
  assert.deepEqual(markers.slice(0, 3), ['1.', '2.', 'a.']); 
  exported = await out();
  assert.match(exported.text('word/numbering.xml'), /<w:abstractNum /);
  assert.match(exported.text('[Content_Types].xml'), /numbering\+xml/); assert.match(exported.text('word/_rels/document.xml.rels'), /numbering.xml/);
  assert.ok(exported.text('word/numbering.xml').indexOf('<w:abstractNum') < exported.text('word/numbering.xml').indexOf('<w:num '), 'abstractNum comes before num');
  assert.match(exported.xml, /<w:ilvl w:val="1"\/>/);
  await ENTER(); await ENTER();

  // Links, through the dialog.
  await mount();
  await select(0, 0, 5); await js(`document.querySelector('[aria-label=Link]').click()`); await sleep(50);
  await js(`(()=>{const i=document.querySelector('dialog input');i.value='https://example.com/a?b=1&c=2';document.querySelector('dialog form').requestSubmit()})()`); await sleep(50);
  exported = await out();
  assert.match(exported.xml, /<w:hyperlink [^>]*r:id="rId\d+"[^>]*>.*Hello/s); assert.match(exported.text('word/_rels/document.xml.rels'), /example\.com\/a\?b=1&amp;c=2/); assert.match(exported.text('word/_rels/document.xml.rels'), /TargetMode="External"/);
  await caret(0, 2); await js(`document.querySelector('[aria-label=Link]').click()`); await sleep(50);
  await js(`document.querySelector('dialog input').value='javascript:alert(1)';document.querySelector('dialog form').requestSubmit()`); await sleep(50);
  assert.doesNotMatch((await out()).text('word/_rels/document.xml.rels'), /javascript/, 'only safe link kinds are written');

  // Tables: inserted, and rows and columns changed, keeping the text of other cells.
  await mount();
  await caret(6, 0); await js(`document.querySelector('[aria-label="Insert table"]').click()`); await sleep(50);
  await js(`(()=>{const f=document.querySelectorAll('dialog input');f[0].value='2';f[1].value='3';document.querySelector('dialog form').requestSubmit()})()`); await sleep(50);
  assert.equal(await js(`document.querySelectorAll('.docx-page table')[1].querySelectorAll('td').length`), 6);
  await js(`(()=>{const p=document.querySelector('.docx-page td p');const r=document.createRange();r.setStart(p.querySelector('[data-r]').firstChild,1);r.collapse(true);document.querySelector('.docx-page').focus();getSelection().removeAllRanges();getSelection().addRange(r)})()`); await sleep(30);
  await type('!'); await sleep(250);
  assert.equal(await js(`document.querySelector('.docx-table-bar').hidden`), false);
  await tool('Insert row below'); await tool('Insert column right');
  assert.equal(await js(`document.querySelector('.docx-page table').querySelectorAll('tr').length`), 2);
  assert.equal(await js(`document.querySelector('.docx-page table').querySelectorAll('td').length`), 6);
  ({ xml } = await out()); assert.match(xml, /A!1/); assert.match(xml, /B1/); assert.equal((xml.split('</w:tbl>')[0].match(/<w:gridCol /g) ?? []).length, 3);
  await tool('Delete column'); await tool('Delete row');
  assert.equal(await js(`document.querySelector('.docx-page table').querySelectorAll('td').length`), 2, 'column and row deleted');
  await js(`(()=>{const c=document.querySelector('[aria-label="Cell color"]');c.value='#112233';c.dispatchEvent(new Event('change'))})()`); ({ xml } = await out()); assert.match(xml, /w:fill="112233"/);
  await tool('Delete table'); assert.equal(await js(`document.querySelectorAll('.docx-page table').length`), 1);

  // Find and replace, across runs and leaving formats alone.
  await mount();
  await select(0, 6, 11); await tool('Bold');
  await js(`document.querySelector('[aria-label="Find and replace"]').click()`);
  await js(`(()=>{const i=document.querySelector('.docx-find-bar [aria-label=Find]');i.value='brave wor';i.dispatchEvent(new Event('input'))})()`);
  assert.equal(await js(`getSelection().toString()`), 'brave wor');
  await js(`document.querySelector('.docx-find-bar [aria-label="Replace with"]').value='kind pla'`);
  await js(`[...document.querySelectorAll('.docx-find-bar button')].find(b=>b.textContent==='Replace').click()`);
  assert.equal((await paragraphs())[0], 'Hello kind plald');
  await js(`(()=>{const i=document.querySelector('.docx-find-bar [aria-label=Find]');i.value='a';i.dispatchEvent(new Event('input'));document.querySelector('.docx-find-bar [aria-label="Replace with"]').value='4';[...document.querySelectorAll('.docx-find-bar button')].find(b=>b.textContent==='Replace all').click()})()`);
  list = await paragraphs(); assert.equal(list[6], 'L4st p4r4gr4ph', 'replace all reaches every paragraph'); assert.match(list[3], /added/, 'but not locked tracked changes');

  // Pasting: rich text as data only; scripts do nothing and markup never reaches the file.
  await mount();
  await caret(6, 0);
  await js(`(()=>{const d=new DataTransfer();d.setData('text/html','<p>One <b>bold</b><script>window.bad=1<\\/script></p><h2>Head</h2><ul><li>Item</li></ul><p onclick="x()"><a href="javascript:bad()">bad link</a> <a href="https://ok.example/">ok</a></p>');d.setData('text/plain','x');document.querySelector('.docx-page').dispatchEvent(new ClipboardEvent('paste',{clipboardData:d,bubbles:true,cancelable:true}))})()`);
  exported = await out();
  assert.equal(await js('window.bad'), undefined); assert.equal(await js(`document.querySelector('.docx-page [onclick], .docx-page script')`), null);
  assert.match(exported.xml, /<w:b\/><\/w:rPr><w:t[^>]*>bold</); assert.match(exported.xml, /<w:pStyle w:val="Heading2"\/>/); assert.match(exported.xml, /<w:numPr>/);
  assert.doesNotMatch(exported.text('word/_rels/document.xml.rels'), /javascript/); assert.match(exported.text('word/_rels/document.xml.rels'), /ok\.example/); assert.doesNotMatch(exported.xml, /onclick|<script/);
  await js(`(()=>{const d=new DataTransfer();d.setData('text/plain','l1\\nl2');document.querySelector('.docx-page').dispatchEvent(new ClipboardEvent('paste',{clipboardData:d,bubbles:true,cancelable:true}))})()`);

  // Pictures: added as a part, a type and a relationship, shown from a blob, and resized in step.
  await mount();
  await caret(0, 5);
  await js(`(()=>{const f=new File([Uint8Array.from(atob(${JSON.stringify(PNG.toString('base64'))}),c=>c.charCodeAt(0))],'dot.png',{type:'image/png'});const dt=new DataTransfer();dt.items.add(f);const i=document.querySelector('.docx-toolbar input[type=file]');i.files=dt.files;i.dispatchEvent(new Event('change'))})()`); await sleep(200);
  assert.match(await js(`document.querySelector('.docx-page img.docx-image').src`), /^blob:/);
  exported = await out();
  assert.ok(exported.parts['word/media/image1.png']); assert.match(exported.text('[Content_Types].xml'), /Extension="png"/); assert.match(exported.text('word/_rels/document.xml.rels'), /media\/image1\.png/);
  assert.match(exported.xml, /<wp:inline.*r:embed="rId\d+"/s); assert.equal((await paragraphs())[0].length, 17, 'the text around it is whole');

  // Page breaks, line breaks, page setup.
  await mount();
  await caret(0, 5); await key('Enter', 'Enter', 2, 13, '\r'); await sleep(30);
  ({ xml } = await out()); assert.match(xml, /<w:br w:type="page"\/>/);
  await caret(6, 4); await key('Enter', 'Enter', 8, 13, '\r'); await type('after');
  ({ xml } = await out()); assert.match(xml, /Last<\/w:t><w:br\/>/); assert.match(xml, /after/);
  await js(`document.querySelector('[aria-label="Page setup"]').click()`); await sleep(50);
  await js(`(()=>{const s=document.querySelectorAll('dialog select');s[0].value='letter';s[1].value='landscape';document.querySelector('dialog form').requestSubmit()})()`); await sleep(50);
  ({ xml } = await out()); assert.match(xml, /w:w="15840"[^>]*w:h="12240"/); assert.match(xml, /w:orient="landscape"/);

  // A real office program opens what it writes, with every feature in one file.
  const office = [process.env.SOFFICE, '/Applications/LibreOffice.app/Contents/MacOS/soffice', '/usr/bin/soffice', '/usr/bin/libreoffice'].find(file => file && fs.existsSync(file));
  if (office) {
    await mount();
    await select(0, 6, 11); await tool('Bold'); await pick('Paragraph style', 'Heading2');
    await caret(6, 0); await tool('Numbered list'); await key('End', 'End', 0, 35); await ENTER(); await type('Second item'); await ENTER(); await type('Nested'); await key('Tab', 'Tab', 0, 9);
    await ENTER(); await ENTER(); await type('After the list');
    await js(`document.querySelector('[aria-label=Link]').click()`); await sleep(50);
    await js(`(()=>{const f=document.querySelectorAll('dialog input');f[0].value='https://example.com/x';f[1].value='A link';document.querySelector('dialog form').requestSubmit()})()`); await sleep(50);
    await js(`document.querySelector('[aria-label="Insert table"]').click()`); await sleep(50);
    await js(`document.querySelector('dialog form').requestSubmit()`); await sleep(50);
    await type('InCell'); await sleep(250);
    await js(`(()=>{const f=new File([Uint8Array.from(atob(${JSON.stringify(PNG.toString('base64'))}),c=>c.charCodeAt(0))],'dot.png',{type:'image/png'});const dt=new DataTransfer();dt.items.add(f);const i=document.querySelector('.docx-toolbar input[type=file]');i.files=dt.files;i.dispatchEvent(new Event('change'))})()`); await sleep(200);
    await key('Enter', 'Enter', 2, 13, '\r'); await sleep(30);
    await js(`[...document.querySelectorAll('button')].find(b=>b.textContent==='Reject all changes').click()`);
    const made = await out();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-docx-office-'));
    fs.writeFileSync(path.join(dir, 'made.docx'), Buffer.from(await js('window.lastExport'), 'base64'));
    await new Promise((done, fail) => { const run = spawn(office, ['--headless', `-env:UserInstallation=file://${dir}/profile`, '--convert-to', 'txt:Text', '--outdir', dir, path.join(dir, 'made.docx')], { stdio: 'ignore' }); run.once('exit', done); run.once('error', fail); });
    const converted = fs.readFileSync(path.join(dir, 'made.txt'), 'utf8');
    for (const word of ['brave', 'Second item', 'Nested', 'After the list', 'A link', 'InCell', 'gone']) assert.ok(converted.includes(word), `${word} in: ${converted}`);
    assert.match(converted, /\b1\. /, 'numbering comes through');
    assert.match(made.xml, /w:pStyle w:val="Heading2"/);
    fs.rmSync(dir, { recursive: true, force: true });
    console.log('LibreOffice opened the edited document.');
  } else console.log('LibreOffice not found: skipped opening the document in it.');

  // Saving: a failed save keeps the edits; a good one clears dirty; undo past it is dirty again.
  await mount(`,onChange:d=>{window.dirty=d}`);
  await caret(0, 0); await type('S'); await sleep(250);
  assert.equal(await js('window.dirty'), true);
  await js('ed.save()'); assert.ok(await js('window.saved')); assert.equal(await js('ed.dirty'), false);
  await js('ed.undo()'); assert.equal(await js('ed.dirty'), true); await js('ed.redo()'); assert.equal(await js('ed.dirty'), false);
  await js(`(async()=>{ed.destroy();const m=await import('/docx-editor.js');window.ed=m.mountDocxEditor(document.getElementById('host'),Uint8Array.from(atob(window.fixture),c=>c.charCodeAt(0)),{onSave:async()=>{throw new Error('Conflict')}})})()`);
  await caret(0, 0); await type('U'); await sleep(250); await js('ed.save()');
  assert.match(await js(`document.querySelector('.docx-status').textContent`), /Save failed: Conflict/); assert.equal(await js('ed.dirty'), true); assert.match(await pageText(), /UHello/);

  // Read only: no editing surface, but finding still works.
  await js(`(async()=>{ed.destroy();const m=await import('/docx-editor.js');window.ed=m.mountDocxEditor(document.getElementById('host'),Uint8Array.from(atob(window.fixture),c=>c.charCodeAt(0)),{readonly:true})})()`);
  assert.equal(await js(`document.querySelector('.docx-page').getAttribute('contenteditable')`), null);
  assert.equal(await js(`!!document.querySelector('[aria-label=Find]')&&!document.querySelector('[aria-label=Bold]')`), true);
  await js('ed.destroy()'); assert.equal(await js(`document.getElementById('host').children.length`), 0);

  assert.deepEqual(problems, [], 'the page reports no errors');
  console.log('DOCX editor tests passed in Chrome.');
} catch (err) { failed = err; }
ws.close(); chrome.kill(); server.close();
await new Promise(done => { chrome.once('exit', done); setTimeout(done, 3000); });
try { fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); } catch {}
if (failed) { console.error(failed); if (problems.length) console.error('Page errors:', problems); process.exit(1); }
