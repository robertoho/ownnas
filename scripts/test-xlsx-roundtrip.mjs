// Round trip for the XLSX editor (web/xlsx-editor.js): opens sample workbooks (made with openpyxl,
// and the same saved again by LibreOffice), edits them as people do, saves, and checks that
// LibreOffice and openpyxl read the result with everything done and nothing lost.
//
// Needs jsdom (XLSX_TEST_DOM, as for test-xlsx-editor.mjs), python3 with openpyxl and Pillow, and
// LibreOffice (SOFFICE, by default where the macOS app puts it).
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const SOFFICE = process.env.SOFFICE || '/Applications/LibreOffice.app/Contents/MacOS/soffice';
const { JSDOM } = await import(pathToFileURL(process.env.XLSX_TEST_DOM || '/tmp/ownnas-docx-test/node_modules/jsdom/lib/api.js'));
const dom = new JSDOM('<div id="editor"></div>', { pretendToBeVisual: true });
for (const key of ['window', 'document', 'DOMParser', 'XMLSerializer', 'Event', 'KeyboardEvent', 'Node', 'requestAnimationFrame', 'cancelAnimationFrame']) {
  globalThis[key] = key === 'window' ? dom.window : dom.window[key];
}
Object.defineProperty(dom.window.HTMLElement.prototype, 'clientHeight', { get() { return 600; } });
Object.defineProperty(dom.window.HTMLElement.prototype, 'clientWidth', { get() { return 800; } });
dom.window.HTMLCanvasElement.prototype.getContext = () => null;
URL.createObjectURL = () => 'blob:picture';
URL.revokeObjectURL = () => {};
const { mountXlsxEditor } = await import('../web/xlsx-editor.js');

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'ownnas-xlsx-'));
execFileSync('python3', [path.join(here, 'xlsx-fixtures.py'), work]);
// The same workbook as LibreOffice writes it.
const libre = path.join(work, 'libre');
execFileSync(SOFFICE, ['--headless', '--convert-to', 'xlsx:Calc MS Excel 2007 XML', '--outdir', libre, path.join(work, 'rich.xlsx')], { stdio: 'ignore' });

const frame = () => new Promise(resolve => requestAnimationFrame(resolve));

/** Edits a workbook through the editor, as a person would, and returns what it saves. */
async function edit(bytes) {
  const container = document.getElementById('editor');
  let saved = null;
  const editor = mountXlsxEditor(container, bytes, { onSave: async data => { saved = data; } });
  await frame();
  const grid = container.querySelector('.xlsx-scroll');
  const canvas = container.querySelector('.xlsx-canvas');
  const input = container.querySelector('.xlsx-cell-editor');
  const menu = container.querySelector('.xlsx-menu');
  const key = (target, name, extra = {}) => target.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true, ...extra }));
  const go = (col, row) => { key(grid, 'Home', { ctrlKey: true }); for (let i = 1; i < row; i++) key(grid, 'ArrowDown'); for (let i = 1; i < col; i++) key(grid, 'ArrowRight'); };
  const type = (col, row, text) => { go(col, row); key(grid, text[0]); input.value = text; input.dispatchEvent(new Event('input')); key(input, 'Enter'); };
  const tab = name => [...container.querySelectorAll('.xlsx-tab')].find(item => item.textContent === name);
  const choose = label => {
    const button = [...menu.querySelectorAll('button')].find(item => item.textContent === label);
    assert.ok(button, `menu has "${label}"`);
    button.click();
  };
  grid.focus();
  // Sales: a new row inside the table, with a formula like the others.
  canvas.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 20, clientY: 24 + 20 * 2 + 8 }));
  choose('Insert row above');
  assert.equal(container.querySelector('.xlsx-note').textContent, '', 'the row went in');
  type(1, 3, 'Mug');
  type(2, 3, '4');
  type(3, 3, '3');
  type(4, 3, '=B3*C3');
  type(5, 2, 'Yes');
  // Sort the table by quantity, largest first: the comment on Ink goes with it.
  go(2, 2);
  container.querySelector('.xlsx-toolbar button[title^="Sort Z to A"]').click();
  // Summary: one more formula, then renamed.
  tab('Summary').click();
  type(1, 3, 'Check');
  type(2, 3, '=SUMPRODUCT(Sales!B2:B6,Sales!C2:C6)');
  tab('Summary').dispatchEvent(new window.MouseEvent('dblclick', { bubbles: true }));
  const rename = container.querySelector('.xlsx-tab-input');
  rename.value = 'Overview';
  key(rename, 'Enter');
  // A new sheet with its top row frozen.
  tab('+').click();
  type(1, 1, 'Hello');
  // A spilling formula, and a filter hiding a row.
  type(1, 2, 'c');
  type(1, 3, 'a');
  type(1, 4, 'b');
  type(2, 2, '=SORT(A2:A4)');
  go(1, 1);
  container.querySelector('.xlsx-toolbar button[title^="Filter"]').click();
  await frame();
  container.querySelector('.xlsx-filter-button').dispatchEvent(new window.MouseEvent('pointerdown', { button: 0, bubbles: true, cancelable: true }));
  const panel = container.querySelector('.xlsx-filter-panel');
  [...panel.querySelectorAll('.xlsx-filter-item')].find(item => item.textContent.trim() === 'a').querySelector('input').click();
  [...panel.querySelectorAll('button')].find(item => item.textContent === 'OK').click();
  container.querySelector('.xlsx-toolbar button[title="Freeze panes"]').click();
  choose('Freeze top row');
  await editor.save();
  editor.destroy();
  return saved;
}

/** What openpyxl reads in a workbook. */
function openpyxlView(file) {
  const script = `
import json, sys
from openpyxl import load_workbook
book = load_workbook(sys.argv[1])
sales = book["Sales"]
print(json.dumps({
  "sheets": book.sheetnames,
  "table": [table.ref for table in sales.tables.values()],
  "freeze": sales.freeze_panes,
  "merged": sorted(str(item) for item in sales.merged_cells.ranges),
  "conditional": sorted(str(item.sqref) for item in sales.conditional_formatting),
  "validation": sorted(str(item.sqref) for item in sales.data_validations.dataValidation),
  "comments": sorted(cell.coordinate for row in sales.iter_rows() for cell in row if cell.comment),
  "links": sorted(cell.coordinate for row in sales.iter_rows() for cell in row if cell.hyperlink),
  "images": len(sales._images),
  "names": {name: item.attr_text for name, item in book.defined_names.items()},
  "notesFreeze": book[book.sheetnames[-1]].freeze_panes,
  "notesFilter": book[book.sheetnames[-1]].auto_filter.ref,
  "notesHidden": [row for row, dim in book[book.sheetnames[-1]].row_dimensions.items() if dim.hidden],
}))`;
  return JSON.parse(execFileSync('python3', ['-c', script, file], { encoding: 'utf8' }));
}

/** CSV rows; numbers written with a decimal comma (LibreOffice follows the system's language)
 * come back with a point. */
function parseCsv(text) {
  const rows = [];
  let row = [], field = '', quoted = false, wasQuoted = false;
  const end = () => { row.push(wasQuoted && /^-?\d+,\d+$/.test(field) ? field.replace(',', '.') : field); field = ''; wasQuoted = false; };
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (quoted) {
      if (char === '"' && text[i + 1] === '"') { field += '"'; i++; } else if (char === '"') quoted = false; else field += char;
    } else if (char === '"') { quoted = true; wasQuoted = true; }
    else if (char === ',') end();
    else if (char === '\n' || char === '\r') {
      if (char === '\r' && text[i + 1] === '\n') i++;
      end();
      rows.push(row);
      row = [];
    } else field += char;
  }
  if (field || row.length) { end(); rows.push(row); }
  return rows;
}

/** The values LibreOffice calculates, sheet by sheet (CSV rows). */
function libreValues(file) {
  const out = fs.mkdtempSync(path.join(work, 'csv-'));
  execFileSync(SOFFICE, ['--headless', '--convert-to', 'csv:Text - txt - csv (StarCalc):44,34,76,1,,0,false,true,false,false,false,-1', '--outdir', out, file], { stdio: 'ignore' });
  const sheets = {};
  for (const name of fs.readdirSync(out)) {
    const sheet = name.slice(path.basename(file, '.xlsx').length + 1, -'.csv'.length);
    sheets[sheet] = parseCsv(fs.readFileSync(path.join(out, name), 'utf8'));
  }
  return sheets;
}

for (const [label, source] of [['openpyxl', path.join(work, 'rich.xlsx')], ['LibreOffice', path.join(libre, 'rich.xlsx')]]) {
  const saved = await edit(new Uint8Array(fs.readFileSync(source)));
  const file = path.join(work, `edited-${label}.xlsx`);
  fs.writeFileSync(file, saved);

  const view = openpyxlView(file);
  assert.deepEqual(view.sheets, ['Sales', 'Overview', 'Sheet3'], `${label}: sheets`);
  assert.deepEqual(view.table, ['A1:D6'], `${label}: the table grew by the row`);
  assert.equal(view.freeze, 'A2', `${label}: frozen panes kept`);
  assert.deepEqual(view.merged, ['F6:G7'], `${label}: merged cells moved down`);
  assert.deepEqual(view.conditional, ['A2:A6', 'B2:B6', 'C2:C6'], `${label}: conditional formatting grew`);
  assert.deepEqual(view.validation, ['E2:E6'], `${label}: validation grew`);
  assert.deepEqual(view.comments, ['A3'], `${label}: the comment moved with its cell, and its row when sorted`);
  assert.deepEqual(view.links, ['F4'], `${label}: the link moved`);
  assert.equal(view.images, 1, `${label}: the picture kept`);
  assert.equal(view.names.Grand.replace(/'/g, ''), 'Sales!$G$1', `${label}: names kept`);
  assert.equal(view.notesFreeze, 'A2', `${label}: the new sheet's frozen row`);
  assert.equal(view.notesFilter, 'A1:B4', `${label}: its filter`);
  assert.deepEqual(view.notesHidden, [3], `${label}: and the row it hides`);

  const values = libreValues(file);
  const cell = (sheet, ref) => {
    const col = ref.charCodeAt(0) - 65, row = Number(ref.slice(1)) - 1;
    return values[sheet]?.[row]?.[col];
  };
  assert.deepEqual(['A2', 'A3', 'A4', 'A5', 'A6'].map(ref => cell('Sales', ref)), ['Cap', 'Ink', 'Mug', 'Pen', 'Pad'], `${label}: LibreOffice reads the new row, sorted`);
  assert.deepEqual(['D2', 'D3', 'D4'].map(ref => Number(cell('Sales', ref))), [3.5, 20, 12], `${label}: formulas moved with their rows`);
  assert.equal(Number(cell('Sales', 'G1')), 44.5, `${label}: and recalculates the total over it`);
  assert.equal(Number(cell('Overview', 'B1')), 44.5, `${label}: a renamed sheet's formulas`);
  assert.equal(Number(cell('Overview', 'B2')), 5);
  assert.equal(Number(cell('Overview', 'B3')), 44.5);
  assert.equal(cell('Sheet3', 'A1'), 'Hello', `${label}: the added sheet`);
  assert.deepEqual(['B2', 'B3', 'B4'].map(ref => cell('Sheet3', ref)), ['a', 'b', 'c'], `${label}: LibreOffice spills the SORT as a dynamic array`);
}
fs.rmSync(work, { recursive: true, force: true });
console.log('XLSX round trip through openpyxl and LibreOffice passed.');
