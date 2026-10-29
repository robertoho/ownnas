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
const { fillSeries, mountXlsxEditor, openXlsx, parseInput, restructure } = await import('../web/xlsx-editor.js');

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
  assert.deepEqual(names(), ['SUM', 'SUMIF', 'SUMIFS', 'SUBSTITUTE', 'SUMPRODUCT'], 'shortest first');
  type('m');
  assert.deepEqual(names(), ['SUM', 'SUMIF', 'SUMIFS', 'SUMPRODUCT']);
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

// Filling: series go on, other things are copied.
{
  const run = (entries, ks) => { const series = fillSeries(entries); return series && ks.map(k => series(k)?.value); };
  const num = value => ({ kind: 'number', value }), text = value => ({ kind: 'text', value });
  assert.deepEqual(run([num(1), num(2)], [2, 3, -1]), [3, 4, 0], 'numbers count on, and back');
  assert.deepEqual(run([num(0.1), num(0.2)], [2]), [0.3], 'without floating-point dust');
  assert.equal(fillSeries([num(5)]), null, 'one number is copied');
  assert.deepEqual(run([{ kind: 'number', value: 46000, date: true }], [1, 2]), [46001, 46002], 'one date goes on a day at a time');
  assert.deepEqual(run([text('Item 1')], [1, 2]), ['Item 2', 'Item 3']);
  assert.deepEqual(run([text('Q01'), text('Q03')], [2]), ['Q05'], 'zeros in front are kept');
  assert.deepEqual(run([text('Mon')], [1, 6, 7]), ['Tue', 'Sun', 'Mon'], 'weekdays wrap');
  assert.deepEqual(run([text('JANUARY'), text('MARCH')], [2]), ['MAY'], 'months step, in the same case');
  assert.equal(fillSeries([text('a'), num(1)]), null, 'a mix is copied');
  assert.equal(fillSeries([text('x'), text('y')]), null);
}

// Inserting and deleting rows and columns moves cells, formulas everywhere, and merged cells.
{
  const two = { ...files,
    'xl/workbook.xml': `<workbook xmlns="${NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Other" sheetId="2" r:id="rId3"/></sheets><definedNames><definedName name="Total">Data!$A$3</definedName></definedNames></workbook>`,
    'xl/_rels/workbook.xml.rels': `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>`,
    'xl/worksheets/sheet1.xml': sheet.replace('</sheetData>', '<row r="5"><c r="A5"><v>1</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A5:B6"/></mergeCells><conditionalFormatting sqref="A1:A3"><cfRule type="expression" priority="1"><formula>A1&gt;15</formula></cfRule></conditionalFormatting>'),
    'xl/worksheets/sheet2.xml': `<worksheet xmlns="${NS}"><sheetData><row r="1"><c r="A1"><f>Data!A3+Data!B2</f><v>70</v></c></row></sheetData></worksheet>`,
  };
  const book = openXlsx(zipSync(Object.fromEntries(Object.entries(two).map(([name, text]) => [name, strToU8(text)]))));
  restructure(book, 0, 'row', 2, 2);
  const data = book.sheets[0];
  const formula = ref => data.cells.get(ref)?.getElementsByTagNameNS(NS, 'f')[0]?.textContent;
  assert.equal(formula('A5'), 'SUM(A1:A4)', 'a range across the new rows grows');
  assert.equal(formula('B4'), 'A4*2', 'shared formulas become plain and move');
  assert.equal(formula('B1'), 'A1*2');
  assert.ok(!data.cells.has('A3') && data.cells.get('A4'), 'cells after move down');
  assert.equal(book.sheets[1].cells.get('A1').getElementsByTagNameNS(NS, 'f')[0].textContent, 'Data!A5+Data!B4', 'other sheets follow');
  assert.equal(book.workbookXml.getElementsByTagNameNS(NS, 'definedName')[0].textContent, 'Data!$A$5', 'and names');
  assert.deepEqual(data.merges, [{ r1: 7, c1: 1, r2: 8, c2: 2 }], 'merged cells move');
  assert.equal(data.root.getElementsByTagNameNS(NS, 'conditionalFormatting')[0].getAttribute('sqref'), 'A1:A5');
  restructure(book, 0, 'row', 2, -3);
  assert.equal(formula('B1'), 'A1*2');
  assert.equal(formula('A2'), 'SUM(A1:A1)', 'deleting rows in a range shrinks it');
  assert.equal(formula('B1'), 'A1*2');
  assert.equal(book.sheets[1].cells.get('A1').getElementsByTagNameNS(NS, 'f')[0].textContent, 'Data!A2+Data!#REF!', 'a deleted cell is #REF!');
  restructure(book, 0, 'col', 1, -1);
  assert.equal(formula('A1'), '#REF!*2', 'deleting a column');
  assert.deepEqual(data.merges, [{ r1: 4, c1: 1, r2: 5, c2: 1 }]);
}

// The editor: the right-click menu, the fill handle, column widths, undo.
{
  const editor2 = mountXlsxEditor(container, bytes, { onSave: async data => { saved = data; } });
  await new Promise(resolve => requestAnimationFrame(resolve));
  const grid = container.querySelector('.xlsx-scroll');
  const canvas = container.querySelector('.xlsx-canvas');
  const menu = container.querySelector('.xlsx-menu');
  const bar = container.querySelector('.xlsx-formula');
  const key = (target, name, extra = {}) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...extra }));
  const mouse = (target, type, x, y, extra = {}) => target.dispatchEvent(new window.MouseEvent(type, { button: 0, bubbles: true, cancelable: true, clientX: x, clientY: y, ...extra }));
  const cellAt = (col, row) => [46 + 64 * (col - 1) + 10, 24 + 20 * (row - 1) + 8];
  const show = (col, row) => { mouse(canvas, 'pointerdown', ...cellAt(col, row)); mouse(canvas, 'pointerup', ...cellAt(col, row)); return bar.value; };
  const choose = label => {
    const button = [...menu.querySelectorAll('button')].find(item => item.textContent === label);
    assert.ok(button, `menu has "${label}": ${[...menu.querySelectorAll('button')].map(item => item.textContent).join(' | ')}`);
    button.click();
  };
  grid.focus();
  // Row header 2, right-click: insert a row above.
  mouse(canvas, 'contextmenu', 20, 24 + 20 + 8);
  assert.equal(menu.hidden, false);
  choose('Insert row above');
  assert.equal(menu.hidden, true);
  assert.equal(show(1, 4), '=SUM(A1:A3)', 'the total moved down and grew');
  assert.equal(show(1, 2), '', 'the new row is empty');
  key(grid, 'z', { ctrlKey: true });
  assert.equal(show(1, 3), '=SUM(A1:A2)', 'undo puts it back');
  key(grid, 'y', { ctrlKey: true });
  assert.equal(show(1, 4), '=SUM(A1:A3)', 'redo again');
  key(grid, 'z', { ctrlKey: true });
  // Cells: delete a column through the menu.
  mouse(canvas, 'contextmenu', ...cellAt(1, 1));
  choose('Delete column');
  assert.equal(show(1, 1), '=#REF!*2', 'formulas pointing at it break, as in Excel');
  key(grid, 'z', { ctrlKey: true });
  assert.equal(show(2, 1), '=A1*2');

  // Type 1 and 2 in D1:D2 and drag the fill handle to D5.
  show(4, 1); key(grid, '1'); key(container.querySelector('.xlsx-cell-editor'), 'Enter');
  key(grid, '2'); key(container.querySelector('.xlsx-cell-editor'), 'Enter');
  mouse(canvas, 'pointerdown', ...cellAt(4, 1));
  mouse(canvas, 'pointermove', ...cellAt(4, 2));
  mouse(canvas, 'pointerup', ...cellAt(4, 2));
  await new Promise(resolve => requestAnimationFrame(resolve));
  const handle = container.querySelector('.xlsx-fill-handle');
  assert.equal(handle.hidden, false, 'the fill handle shows');
  mouse(handle, 'pointerdown', ...cellAt(4, 2));
  mouse(canvas, 'pointermove', ...cellAt(4, 5));
  assert.equal(container.querySelector('.xlsx-fill').hidden, false, 'the cells to fill are marked');
  mouse(canvas, 'pointerup', ...cellAt(4, 5));
  assert.deepEqual([3, 4, 5].map(row => show(4, row)), ['3', '4', '5'], 'the series goes on');
  // A formula filled down moves its references.
  show(2, 2);
  mouse(handle, 'pointerdown', ...cellAt(2, 2));
  mouse(canvas, 'pointermove', ...cellAt(2, 4));
  mouse(canvas, 'pointerup', ...cellAt(2, 4));
  assert.deepEqual([3, 4].map(row => show(2, row)), ['=A3*2', '=A4*2']);
  key(grid, 'z', { ctrlKey: true });
  assert.equal(show(2, 3), 'Note', 'undo puts back what was there');

  // Drag column A's right edge 36 px wider.
  const edge = 46 + 64;
  mouse(canvas, 'pointermove', edge, 12);
  assert.equal(canvas.style.cursor, 'col-resize', 'the edge shows it can be dragged');
  mouse(canvas, 'pointerdown', edge, 12);
  mouse(canvas, 'pointermove', edge + 36, 12);
  mouse(canvas, 'pointerup', edge + 36, 12);
  await editor2.save();
  const widths = new DOMParser().parseFromString(strFromU8(unzipSync(saved)['xl/worksheets/sheet1.xml']), 'application/xml');
  const col = widths.getElementsByTagNameNS(NS, 'col')[0];
  assert.deepEqual([col.getAttribute('min'), col.getAttribute('max'), col.getAttribute('width'), col.getAttribute('customWidth')], ['1', '1', '14.29', '1']);
  // Hide column B from its header's menu, then show it again.
  mouse(canvas, 'contextmenu', 46 + 100 + 30, 12);
  choose('Hide column');
  mouse(canvas, 'contextmenu', 46 + 100 + 30, 12);
  assert.ok(![...menu.querySelectorAll('button')].some(item => item.textContent === 'Unhide columns'), 'column C has nothing hidden');
  key(menu, 'Escape');
  assert.equal(menu.hidden, true);
  key(grid, 'z', { ctrlKey: true });
  editor2.destroy();
}

// The fx button lists the functions, with a filter; picking one puts it in the formula.
{
  const editor3 = mountXlsxEditor(container, bytes, {});
  await new Promise(resolve => requestAnimationFrame(resolve));
  const grid = container.querySelector('.xlsx-scroll');
  const fx = container.querySelector('.xlsx-fx');
  const panel = container.querySelector('.xlsx-functions');
  const filter = panel.querySelector('.xlsx-functions-filter');
  const input = container.querySelector('.xlsx-cell-editor');
  const key = (target, name, extra = {}) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...extra }));
  const names = () => [...panel.querySelectorAll('.xlsx-function-sig')].map(item => item.textContent.split('(')[0]);
  grid.focus();
  key(grid, 'Home', { ctrlKey: true });
  key(grid, 'ArrowRight'); key(grid, 'ArrowRight'); key(grid, 'ArrowRight');
  fx.click();
  assert.equal(panel.hidden, false, 'fx opens the list');
  assert.equal(document.activeElement, filter, 'with the filter ready to type in');
  assert.equal(names().length, Object.keys((await import('../web/sheet-formula.js')).FUNCTION_HELP).length);
  assert.deepEqual([...panel.querySelectorAll('.xlsx-functions-group')].map(item => item.textContent).slice(0, 2), ['Math', 'Statistics'], 'grouped');
  assert.match(panel.textContent, /Example: =VLOOKUP\(A2, D2:F50, 3, FALSE\)/, 'each has an example');
  filter.value = 'look';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  assert.deepEqual(names().slice(0, 3), ['HLOOKUP', 'VLOOKUP', 'XLOOKUP'], 'the filter matches names first, then categories');
  filter.value = 'average';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  assert.deepEqual(names().slice(0, 3), ['AVERAGE', 'AVERAGEA', 'AVERAGEIF'], 'names first');
  assert.ok(names().includes('MEDIAN') === false && names().length >= 3);
  filter.value = 'remainder';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  assert.deepEqual(names(), ['MOD'], 'and what they do');
  key(filter, 'Enter');
  assert.equal(panel.hidden, true, 'picking one closes the list');
  assert.equal(input.hidden, false);
  assert.equal(input.value, '=MOD(', 'and starts a formula with it');
  input.setRangeText('A1, 2)+', input.selectionStart, input.selectionEnd, 'end');
  input.dispatchEvent(new Event('input', { bubbles: true }));
  fx.dispatchEvent(new window.MouseEvent('pointerdown', { bubbles: true, cancelable: true }));
  fx.click();
  filter.value = 'abs';
  filter.dispatchEvent(new Event('input', { bubbles: true }));
  key(filter, 'Enter');
  assert.equal(input.value, '=MOD(A1, 2)+ABS(', 'while typing, it goes in at the caret');
  fx.click();
  key(filter, 'Escape');
  assert.equal(panel.hidden, true);
  assert.equal(document.activeElement, input, 'Escape goes back to the formula');
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(input.hidden, false, 'still being typed');
  editor3.destroy();
}

// Formulas returning several cells spill, and are saved as dynamic arrays; newer functions get
// the prefix Excel needs.
{
  const editor4 = mountXlsxEditor(container, bytes, { onSave: async data => { saved = data; } });
  await new Promise(resolve => requestAnimationFrame(resolve));
  const grid = container.querySelector('.xlsx-scroll');
  const input = container.querySelector('.xlsx-cell-editor');
  const bar = container.querySelector('.xlsx-formula');
  const key = (target, name, extra = {}) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...extra }));
  const go = ref => { const { row, col } = { row: Number(ref.slice(1)), col: ref.charCodeAt(0) - 64 }; key(grid, 'Home', { ctrlKey: true }); for (let i = 1; i < row; i++) key(grid, 'ArrowDown'); for (let i = 1; i < col; i++) key(grid, 'ArrowRight'); };
  const enter = text => { key(grid, text[0]); input.value = text; input.dispatchEvent(new Event('input')); key(input, 'Enter'); };
  const texts = () => [...container.querySelectorAll('.xlsx-cell')].map(cell => cell.textContent);
  grid.focus();
  go('D1'); enter('=SORT(A1:A2,1,-1)');
  go('E1'); enter('=SUM(D1:D2)');
  go('F1'); enter('=XLOOKUP(20,A1:A2,B1:B2)');
  await new Promise(resolve => requestAnimationFrame(resolve));
  go('D2');
  assert.equal(bar.value, '', 'a spilled cell has nothing typed in it');
  go('E1');
  assert.equal(container.querySelector('.xlsx-live').textContent, 'E1, 30', 'other formulas read spilled cells');
  go('F1');
  assert.equal(bar.value, '=XLOOKUP(20,A1:A2,B1:B2)', 'prefixes stay out of sight');
  go('G2'); enter('x');
  go('G1'); enter('=UNIQUE(A1:A2)');
  go('G1');
  assert.match(container.querySelector('.xlsx-live').textContent, /#SPILL!/, 'nothing spills over other cells');
  await editor4.save();
  const out = unzipSync(saved);
  const xml = strFromU8(out['xl/worksheets/sheet1.xml']);
  assert.match(xml, /<c r="D1"[^>]* cm="1"[^>]*><f t="array" ref="D1:D2">_xlfn\._xlws\.SORT\(A1:A2,1,-1\)<\/f>/, 'a spilling formula is saved as a dynamic array');
  assert.match(xml, /<f>_xlfn\.XLOOKUP\(20,A1:A2,B1:B2\)<\/f>/);
  assert.ok(out['xl/metadata.xml'] && strFromU8(out['xl/metadata.xml']).includes('fDynamic="1"'));
  assert.match(strFromU8(out['[Content_Types].xml']), /\/xl\/metadata\.xml/);
  assert.match(strFromU8(out['xl/_rels/workbook.xml.rels']), /sheetMetadata/);
  editor4.destroy();
  // Opened again, it spills as before.
  const again = mountXlsxEditor(container, saved, {});
  await new Promise(resolve => requestAnimationFrame(resolve));
  assert.ok(texts().includes('20') && texts().filter(text => text === '10').length >= 2, texts().join('|'));
  again.destroy();
}

const readonly = mountXlsxEditor(container, bytes, { readonly: true });
assert.equal(container.querySelector('.office-save'), null);
readonly.destroy();
console.log('XLSX display, editing, recalculation, undo/redo, and package tests passed.');
