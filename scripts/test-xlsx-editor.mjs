// Checks for the XLSX editor (web/xlsx-editor.js) in jsdom. Install jsdom as for the document editors,
// or set XLSX_TEST_DOM to another jsdom/lib/api.js.
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const { JSDOM } = await import(pathToFileURL(process.env.XLSX_TEST_DOM || '/tmp/ownnas-docx-test/node_modules/jsdom/lib/api.js'));
const dom = new JSDOM('<div id="editor"></div>', { pretendToBeVisual: true });
for (const key of ['window', 'document', 'DOMParser', 'XMLSerializer', 'Event', 'KeyboardEvent', 'Node', 'requestAnimationFrame', 'cancelAnimationFrame']) {
  globalThis[key] = key === 'window' ? dom.window : dom.window[key];
}
// jsdom has no layout or canvas: give the grid a viewport and let text measuring fall back.
Object.defineProperty(dom.window.HTMLElement.prototype, 'clientHeight', { get() { return 600; } });
Object.defineProperty(dom.window.HTMLElement.prototype, 'clientWidth', { get() { return 800; } });
dom.window.HTMLCanvasElement.prototype.getContext = () => null;
const { zipSync, unzipSync, strToU8, strFromU8 } = await import('../web/vendor/three/addons/libs/fflate.module.js');
const { mountXlsxEditor, parseInput } = await import('../web/xlsx-editor.js');

const NS = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const sheet = `<worksheet xmlns="${NS}"><dimension ref="A1:B3"/><sheetData>
<row r="1"><c r="A1"><v>10</v></c><c r="B1"><f t="shared" ref="B1:B2" si="0">A1*2</f><v>20</v></c></row>
<row r="2"><c r="A2"><v>20</v></c><c r="B2"><f t="shared" si="0"/><v>40</v></c></row>
<row r="3"><c r="A3" s="1"><f>SUM(A1:A2)</f><v>30</v></c><c r="B3" t="s"><v>0</v></c></row>
</sheetData><sheetProtection sheet="false"/></worksheet>`;
const files = {
  '[Content_Types].xml': `<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/calcChain.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml"/></Types>`,
  'xl/workbook.xml': `<workbook xmlns="${NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>`,
  'xl/_rels/workbook.xml.rels': `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/calcChain" Target="calcChain.xml"/></Relationships>`,
  'xl/worksheets/sheet1.xml': sheet,
  'xl/sharedStrings.xml': `<sst xmlns="${NS}"><si><t>Note</t></si></sst>`,
  'xl/styles.xml': `<styleSheet xmlns="${NS}"><numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0.00 &quot;€&quot;"/></numFmts><fonts count="1"><font><sz val="11"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" applyNumberFormat="1"/></cellXfs></styleSheet>`,
  'xl/calcChain.xml': `<calcChain xmlns="${NS}"><c r="A3" i="1"/></calcChain>`,
  'docProps/custom.xml': '<Properties/>',
};
const bytes = zipSync(Object.fromEntries(Object.entries(files).map(([name, text]) => [name, strToU8(text)])));

assert.deepEqual(parseInput('2026-03-15'), { kind: 'number', value: 46096, format: 'yyyy-mm-dd' });
assert.deepEqual(parseInput('12.5%'), { kind: 'number', value: 0.125, format: '0.00%' });
assert.deepEqual(parseInput('1.234,5', { decimal: ',' }), { kind: 'number', value: 1234.5 });
assert.deepEqual(parseInput("'007"), { kind: 'text', value: '007' });
assert.deepEqual(parseInput('=A1+1'), { kind: 'formula', value: 'A1+1' });

const container = document.getElementById('editor');
let saved = null;
const editor = mountXlsxEditor(container, bytes, { onSave: async data => { saved = data; } });
await new Promise(resolve => requestAnimationFrame(resolve));
const texts = () => [...container.querySelectorAll('.xlsx-cell')].map(cell => cell.textContent);
assert.ok(texts().includes('30.00 €'), 'number formats are applied');
assert.ok(texts().includes('Note'), 'shared strings are shown');
assert.match(container.querySelector('.xlsx-note').textContent, /^$/, 'sheet="false" protection is not protection');

// Type 15 into A1: Ctrl+Home, then a key starts editing and Enter commits.
const grid = container.querySelector('.xlsx-scroll');
grid.focus();
grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', ctrlKey: true, bubbles: true }));
grid.dispatchEvent(new KeyboardEvent('keydown', { key: '1', bubbles: true }));
const input = container.querySelector('.xlsx-cell-editor');
assert.equal(input.hidden, false);
input.value = '15';
input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
assert.equal(container.querySelector('.xlsx-address').textContent, 'A2', 'Enter moves down');
await new Promise(resolve => requestAnimationFrame(resolve));
assert.ok(texts().includes('35.00 €'), 'the total recalculates');
assert.ok(texts().includes('30'), 'the shared formula recalculates');
assert.equal(editor.dirty, true);

// Undo and redo.
grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, bubbles: true }));
await new Promise(resolve => requestAnimationFrame(resolve));
assert.ok(texts().includes('30.00 €'));
grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'y', ctrlKey: true, bubbles: true }));
await editor.save();
assert.equal(editor.dirty, false);

const out = unzipSync(saved);
assert.ok(!out['xl/calcChain.xml'], 'the stale calculation chain is dropped');
assert.ok(!strFromU8(out['xl/_rels/workbook.xml.rels']).includes('calcChain'));
assert.ok(!strFromU8(out['[Content_Types].xml']).includes('calcChain'));
assert.match(strFromU8(out['xl/workbook.xml']), /fullCalcOnLoad="1"/);
assert.equal(strFromU8(out['docProps/custom.xml']), '<Properties/>', 'other parts are kept');
const xml = new DOMParser().parseFromString(strFromU8(out['xl/worksheets/sheet1.xml']), 'application/xml');
const cell = ref => [...xml.getElementsByTagNameNS(NS, 'c')].find(c => c.getAttribute('r') === ref);
const value = ref => cell(ref).getElementsByTagNameNS(NS, 'v')[0]?.textContent;
assert.equal(value('A1'), '15');
assert.equal(value('A3'), '35', 'formula results are saved for apps that read cached values');
assert.equal(value('B1'), '30');
assert.equal(cell('B2').getElementsByTagNameNS(NS, 'f')[0].getAttribute('t'), 'shared', 'untouched shared formulas stay shared');
editor.destroy();

// Typing a formula: function suggestions, argument hints, and cells clicked or reached with arrows.
{
  const typing = mountXlsxEditor(container, bytes, {});
  await new Promise(resolve => requestAnimationFrame(resolve));
  const grid = container.querySelector('.xlsx-scroll');
  const canvas = container.querySelector('.xlsx-canvas');
  const input = container.querySelector('.xlsx-cell-editor');
  const assist = container.querySelector('.xlsx-assist');
  const key = (target, name, extra = {}) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...extra }));
  const type = text => { input.setRangeText(text, input.selectionStart, input.selectionEnd, 'end'); input.dispatchEvent(new Event('input', { bubbles: true })); };
  // Cell centres: row headers are 46 px wide, column headers 24 px high.
  const at = (type, col, row) => canvas.dispatchEvent(new window.MouseEvent(type, { button: 0, bubbles: true, cancelable: true, clientX: 46 + 64 * (col - 1) + 10, clientY: 24 + 20 * (row - 1) + 8 }));
  grid.focus();
  key(grid, 'Home', { ctrlKey: true });
  key(grid, 'ArrowRight'); key(grid, 'ArrowRight');
  key(grid, '=');
  type('su');
  const names = () => [...assist.querySelectorAll('.xlsx-assist-name')].map(item => item.textContent);
  assert.equal(assist.hidden, false, 'functions are suggested');
  assert.deepEqual(names(), ['SUM', 'SUMIF', 'SUMIFS', 'SUBSTITUTE'], 'shortest first');
  type('m');
  assert.deepEqual(names(), ['SUM', 'SUMIF', 'SUMIFS']);
  key(input, 'ArrowDown');
  assert.equal(assist.querySelector('[aria-selected="true"]').textContent.startsWith('SUMIF'), true, 'arrows choose');
  key(input, 'ArrowUp');
  key(input, 'Tab');
  assert.equal(input.value, '=SUM(', 'Tab puts the function in');
  assert.equal(container.querySelector('.xlsx-address').textContent, 'C1', 'and does not leave the cell');
  assert.match(assist.textContent, /^SUM\(number1, \[number2\], …\)/, 'its arguments are shown');
  assert.equal(assist.querySelector('b').textContent, 'number1');
  at('pointerdown', 1, 1);
  assert.equal(input.value, '=SUM(A1', 'a click puts the cell in');
  assert.equal(input.hidden, false, 'and keeps editing');
  at('pointermove', 1, 2);
  at('pointerup', 1, 2);
  assert.equal(input.value, '=SUM(A1:A2', 'dragging makes it a range');
  assert.equal(container.querySelector('.xlsx-point').hidden, false, 'the range pointed at is marked');
  type(',');
  assert.equal(assist.querySelector('b').textContent, '[number2]', 'the argument typed is the one shown');
  at('pointerdown', 2, 1);
  at('pointerup', 2, 1);
  at('pointerdown', 2, 2);
  at('pointerup', 2, 2);
  assert.equal(input.value, '=SUM(A1:A2,B2', 'clicking again replaces the cell just put in');
  type(')');
  at('pointerdown', 1, 3);
  assert.equal(input.hidden, true, 'a click where no reference can go ends the edit');
  at('pointerup', 1, 3);
  key(grid, 'ArrowUp'); key(grid, 'ArrowUp'); key(grid, 'ArrowRight'); key(grid, 'ArrowRight');
  assert.equal(container.querySelector('.xlsx-address').textContent, 'C1');
  assert.equal(container.querySelector('.xlsx-formula').value, '=SUM(A1:A2,B2)');

  key(grid, 'ArrowDown');
  key(grid, '=');
  key(input, 'ArrowLeft');
  assert.equal(input.value, '=B2', 'an arrow puts the cell beside in');
  key(input, 'ArrowUp');
  assert.equal(input.value, '=B1', 'and moves it');
  key(input, 'ArrowLeft', { shiftKey: true });
  assert.equal(input.value, '=A1:B1', 'Shift+arrow makes it a range');
  type('+1');
  key(input, 'ArrowUp');
  assert.equal(input.value, '=A1:B1+1', 'where no reference can go, arrows move in the text as before');
  typing.destroy();
}

const readonly = mountXlsxEditor(container, bytes, { readonly: true });
assert.equal(container.querySelector('.office-save'), null);
readonly.destroy();
console.log('XLSX display, editing, recalculation, undo/redo, and package tests passed.');
