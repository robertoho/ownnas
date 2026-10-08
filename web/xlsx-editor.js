/** Small preservation-first XLSX cell editor. Formula results are not calculated in-browser. */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';

const MAIN = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const DOC_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PKG_REL = 'http://schemas.openxmlformats.org/package/2006/relationships';
const XML = 'http://www.w3.org/XML/1998/namespace';
const MAX_COMPRESSED = 25 * 1024 * 1024;
const MAX_EXPANDED = 100 * 1024 * 1024;
const MAX_PARTS = 4096;
const MAX_CELLS = 100000;
const MAX_ROWS = 100000;
const MAX_COLS = 2000;
const ROW_H = 28;
const COL_W = 120;
const HEAD_H = 30;
const HEAD_W = 48;

function parseXml(source, label) {
  if (/<!DOCTYPE|<!ENTITY/i.test(source)) throw new Error(`${label} contains unsupported XML entities.`);
  const xml = new DOMParser().parseFromString(source, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error(`${label} is invalid.`);
  return xml;
}

function direct(node, localName) {
  return [...node.children].find(child => child.namespaceURI === MAIN && child.localName === localName) || null;
}

function columnNumber(letters) {
  let result = 0;
  for (const char of letters.toUpperCase()) result = result * 26 + char.charCodeAt(0) - 64;
  return result;
}

function columnName(column) {
  let value = column, result = '';
  while (value > 0) {
    const remainder = (value - 1) % 26;
    result = String.fromCharCode(65 + remainder) + result;
    value = Math.floor((value - 1) / 26);
  }
  return result || 'A';
}

function parseRef(ref) {
  const match = /^\$?([A-Z]{1,4})\$?([1-9]\d*)$/i.exec(ref || '');
  if (!match) return null;
  const row = Number(match[2]), col = columnNumber(match[1]);
  if (row > 1048576 || col > 16384) return null;
  return { row, col };
}

function normalizePart(target) {
  const segments = target.startsWith('/') ? [] : ['xl'];
  for (const segment of target.replace(/^\//, '').split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      if (segments.length) segments.pop();
    } else segments.push(segment);
  }
  const path = segments.join('/');
  if (!path.startsWith('xl/')) throw new Error('Workbook references a worksheet outside its package.');
  return path;
}

function textOf(node) {
  return [...node.getElementsByTagNameNS(MAIN, 't')].map(item => item.textContent || '').join('');
}

function getCellText(cell, sharedStrings) {
  if (!cell) return '';
  const type = cell.getAttribute('t');
  if (type === 'inlineStr') {
    const inline = direct(cell, 'is');
    return inline ? textOf(inline) : '';
  }
  const value = direct(cell, 'v')?.textContent || '';
  if (type === 's') return sharedStrings[Number(value)] ?? '';
  if (type === 'b') return value === '1' ? 'TRUE' : 'FALSE';
  return value;
}

function formulaOf(cell) {
  const formula = direct(cell, 'f')?.textContent;
  return formula == null ? null : `=${formula}`;
}

function listChildren(node, localName) {
  return [...node.children].filter(child => child.namespaceURI === MAIN && child.localName === localName);
}

export function openXlsx(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length === 0 || bytes.length > MAX_COMPRESSED) {
    throw new Error('Spreadsheets must be between 1 byte and 25 MB.');
  }
  let expanded = 0, count = 0;
  const parts = unzipSync(bytes, { filter(part) {
    expanded += part.originalSize;
    if (++count > MAX_PARTS || expanded > MAX_EXPANDED) throw new Error('Spreadsheet package is too large.');
    if (part.name.startsWith('_xmlsignatures/')) throw new Error('Digitally signed spreadsheets cannot be edited.');
    return true;
  }});
  if (!parts['[Content_Types].xml'] || !parts['xl/workbook.xml'] || !parts['xl/_rels/workbook.xml.rels']) {
    throw new Error('This is not a supported XLSX workbook.');
  }
  const workbookSource = strFromU8(parts['xl/workbook.xml']);
  const relsSource = strFromU8(parts['xl/_rels/workbook.xml.rels']);
  const workbookXml = parseXml(workbookSource, 'Workbook XML');
  const relsXml = parseXml(relsSource, 'Workbook relationships');
  const relationships = new Map([...relsXml.getElementsByTagNameNS(PKG_REL, 'Relationship')]
    .map(rel => [rel.getAttribute('Id'), { target: rel.getAttribute('Target'), type: rel.getAttribute('Type') }]));
  const sharedStrings = [];
  if (parts['xl/sharedStrings.xml']) {
    const stringsXml = parseXml(strFromU8(parts['xl/sharedStrings.xml']), 'Shared strings');
    for (const item of stringsXml.getElementsByTagNameNS(MAIN, 'si')) sharedStrings.push(textOf(item));
  }
  const sheetNodes = workbookXml.getElementsByTagNameNS(MAIN, 'sheet');
  if (!sheetNodes.length) throw new Error('This workbook has no worksheets.');
  const sheets = [];
  let cellCount = 0;
  for (const sheetNode of sheetNodes) {
    const relId = sheetNode.getAttributeNS(DOC_REL, 'id') || sheetNode.getAttribute('r:id');
    const relationship = relationships.get(relId);
    if (!relationship?.type.endsWith('/worksheet')) continue;
    const path = normalizePart(relationship.target);
    if (!parts[path]) continue;
    const xml = parseXml(strFromU8(parts[path]), `Worksheet ${sheetNode.getAttribute('name') || ''}`);
    const sheetData = direct(xml.documentElement, 'sheetData');
    if (!sheetData) throw new Error('Worksheet data is missing.');
    const cells = new Map();
    let maxRow = 0, maxCol = 0;
    for (const row of listChildren(sheetData, 'row')) {
      for (const cell of listChildren(row, 'c')) {
        if (++cellCount > MAX_CELLS) throw new Error('This workbook contains too many cells to edit in the browser.');
        const ref = cell.getAttribute('r');
        const position = parseRef(ref);
        if (!position) continue;
        cells.set(ref.toUpperCase(), cell);
        maxRow = Math.max(maxRow, position.row);
        maxCol = Math.max(maxCol, position.col);
      }
    }
    const protectedSheet = !!direct(xml.documentElement, 'sheetProtection');
    sheets.push({
      name: sheetNode.getAttribute('name') || `Sheet ${sheets.length + 1}`, path,
      xml, sheetData, cells, maxRow: Math.max(100, maxRow), maxCol: Math.max(26, maxCol),
      protected: protectedSheet,
    });
  }
  if (!sheets.length) throw new Error('No readable worksheets were found.');
  const workbookProtection = !!direct(workbookXml.documentElement, 'workbookProtection');
  const initiallyProtected = workbookProtection || sheets.some(sheet => sheet.protected);
  const model = {
    parts, workbookXml, sheets, sharedStrings, protected: initiallyProtected,
    export() {
      for (const sheet of sheets) parts[sheet.path] = strToU8(new XMLSerializer().serializeToString(sheet.xml));
      parts['xl/workbook.xml'] = strToU8(new XMLSerializer().serializeToString(workbookXml));
      return zipSync(parts, { level: 6 });
    },
  };
  return model;
}

function setCellValue(sheet, position, input) {
  const ref = `${columnName(position.col)}${position.row}`;
  let cell = sheet.cells.get(ref);
  if (!cell) {
    let row = listChildren(sheet.sheetData, 'row').find(item => Number(item.getAttribute('r')) === position.row);
    if (!row) {
      row = sheet.xml.createElementNS(MAIN, 'row');
      row.setAttribute('r', String(position.row));
      const before = listChildren(sheet.sheetData, 'row').find(item => Number(item.getAttribute('r')) > position.row);
      sheet.sheetData.insertBefore(row, before || null);
    }
    cell = sheet.xml.createElementNS(MAIN, 'c');
    cell.setAttribute('r', ref);
    const before = listChildren(row, 'c').find(item => (parseRef(item.getAttribute('r'))?.col || 0) > position.col);
    row.insertBefore(cell, before || null);
    sheet.cells.set(ref, cell);
  }
  for (const child of [...cell.children]) cell.removeChild(child);
  const value = String(input);
  if (value.startsWith('=')) {
    cell.removeAttribute('t');
    const formula = sheet.xml.createElementNS(MAIN, 'f');
    formula.textContent = value.slice(1);
    const result = sheet.xml.createElementNS(MAIN, 'v');
    cell.append(formula, result);
  } else if (/^(?:TRUE|FALSE)$/i.test(value)) {
    cell.setAttribute('t', 'b');
    const data = sheet.xml.createElementNS(MAIN, 'v');
    data.textContent = value.toUpperCase() === 'TRUE' ? '1' : '0';
    cell.append(data);
  } else if (value.trim() !== '' && Number.isFinite(Number(value)) && /^[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[-+]?\d+)?$/i.test(value.trim())) {
    cell.removeAttribute('t');
    const data = sheet.xml.createElementNS(MAIN, 'v');
    data.textContent = value.trim();
    cell.append(data);
  } else {
    cell.setAttribute('t', 'inlineStr');
    const inline = sheet.xml.createElementNS(MAIN, 'is');
    const text = sheet.xml.createElementNS(MAIN, 't');
    if (/^\s|\s$/.test(value)) text.setAttributeNS(XML, 'xml:space', 'preserve');
    text.textContent = value;
    inline.append(text);
    cell.append(inline);
  }
  sheet.maxRow = Math.max(sheet.maxRow, position.row);
  sheet.maxCol = Math.max(sheet.maxCol, position.col);
  const dimension = direct(sheet.xml.documentElement, 'dimension');
  const endRef = `${columnName(sheet.maxCol)}${sheet.maxRow}`;
  if (dimension) {
    dimension.setAttribute('ref', endRef === 'A1' ? 'A1' : `A1:${endRef}`);
  }
}

function displayValue(sheet, model, position) {
  const cell = sheet.cells.get(`${columnName(position.col)}${position.row}`);
  return getCellText(cell, model.sharedStrings);
}

/** Mount the editor; onSave receives the updated OOXML package as Uint8Array. */
export function mountXlsxEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openXlsx(bytes);
  const canWrite = !readonly && !model.protected;
  let activeSheet = 0, selected = { row: 1, col: 1 }, dirty = false, saving = false, destroyed = false;
  const root = document.createElement('section'); root.className = 'xlsx-editor';
  const toolbar = document.createElement('div'); toolbar.className = 'xlsx-toolbar';
  const tabs = document.createElement('div'); tabs.className = 'xlsx-tabs'; tabs.setAttribute('role', 'tablist'); tabs.setAttribute('aria-label', 'Workbook sheets');
  const save = document.createElement('button'); save.type = 'button'; save.textContent = 'Save'; save.disabled = true; save.hidden = !canWrite;
  const status = document.createElement('span'); status.className = 'xlsx-status'; status.setAttribute('role', 'status'); status.textContent = 'Saved';
  toolbar.append(tabs, save, status);
  const note = document.createElement('p'); note.className = 'muted xlsx-note';
  note.textContent = model.protected
    ? 'This workbook or one of its sheets is protected and is open read-only.'
    : readonly
      ? 'Read-only workbook. Formulas are shown with their saved values; calculations run when opened in a spreadsheet app.'
      : 'Edit a cell in the formula bar. Formulas are saved for recalculation when opened in a spreadsheet app.';
  if (model.sheets.some(sheet => sheet.maxRow > MAX_ROWS || sheet.maxCol > MAX_COLS)) {
    note.textContent += ' The grid view is capped at row 100,000 and column ' + columnName(MAX_COLS) + '; other workbook data stays preserved.';
  }
  const formula = document.createElement('div'); formula.className = 'xlsx-formula-row';
  const address = document.createElement('span'); address.className = 'xlsx-address';
  const formulaInput = document.createElement('input'); formulaInput.className = 'xlsx-formula'; formulaInput.setAttribute('aria-label', 'Selected cell value or formula');
  formulaInput.readOnly = !canWrite; formulaInput.autocomplete = 'off'; formulaInput.spellcheck = false;
  formula.append(address, formulaInput);
  const scroll = document.createElement('div'); scroll.className = 'xlsx-scroll'; scroll.tabIndex = 0; scroll.setAttribute('role', 'grid'); scroll.setAttribute('aria-label', 'Spreadsheet cells');
  const canvas = document.createElement('div'); canvas.className = 'xlsx-canvas'; scroll.append(canvas);
  root.append(toolbar, note, formula, scroll); container.replaceChildren(root);

  function setDirty(value) {
    dirty = value; save.disabled = !canWrite || !dirty || saving;
    status.textContent = saving ? 'Saving…' : dirty ? 'Unsaved changes' : 'Saved';
    onChange(dirty);
  }
  function updateSelectedInput() {
    const sheet = model.sheets[activeSheet];
    const ref = `${columnName(selected.col)}${selected.row}`;
    address.textContent = ref;
    const cell = sheet.cells.get(ref);
    formulaInput.value = formulaOf(cell) || displayValue(sheet, model, selected);
  }
  function drawTabs() {
    tabs.replaceChildren();
    model.sheets.forEach((sheet, index) => {
      const button = document.createElement('button'); button.type = 'button'; button.className = 'xlsx-tab';
      button.textContent = sheet.name; button.setAttribute('role', 'tab');
      button.setAttribute('aria-selected', String(index === activeSheet)); button.disabled = saving;
      button.addEventListener('click', () => { activeSheet = index; selected = { row: 1, col: 1 }; drawTabs(); updateSelectedInput(); renderGrid(); });
      tabs.append(button);
    });
  }
  function renderGrid() {
    if (destroyed) return;
    const sheet = model.sheets[activeSheet];
    const rows = Math.min(MAX_ROWS, Math.max(100, sheet.maxRow));
    const cols = Math.min(MAX_COLS, Math.max(26, sheet.maxCol));
    canvas.style.width = `${HEAD_W + cols * COL_W}px`;
    canvas.style.height = `${HEAD_H + rows * ROW_H}px`;
    const top = scroll.scrollTop, left = scroll.scrollLeft;
    const firstRow = Math.max(1, Math.floor(Math.max(0, top - HEAD_H) / ROW_H) + 1);
    const lastRow = Math.min(rows, firstRow + Math.ceil((scroll.clientHeight || 600) / ROW_H) + 3);
    const firstCol = Math.max(1, Math.floor(Math.max(0, left - HEAD_W) / COL_W) + 1);
    const lastCol = Math.min(cols, firstCol + Math.ceil((scroll.clientWidth || 800) / COL_W) + 3);
    const fragment = document.createDocumentFragment();
    const corner = document.createElement('div'); corner.className = 'xlsx-corner';
    corner.style.cssText = `left:${left}px;top:${top}px;width:${HEAD_W}px;height:${HEAD_H}px`; fragment.append(corner);
    for (let col = firstCol; col <= lastCol; col++) {
      const head = document.createElement('div'); head.className = 'xlsx-col-head'; head.textContent = columnName(col);
      head.style.cssText = `left:${HEAD_W + (col - 1) * COL_W}px;top:${top}px;width:${COL_W}px;height:${HEAD_H}px`;
      fragment.append(head);
    }
    for (let row = firstRow; row <= lastRow; row++) {
      const head = document.createElement('div'); head.className = 'xlsx-row-head'; head.textContent = String(row);
      head.style.cssText = `left:${left}px;top:${HEAD_H + (row - 1) * ROW_H}px;width:${HEAD_W}px;height:${ROW_H}px`;
      fragment.append(head);
      for (let col = firstCol; col <= lastCol; col++) {
        const position = { row, col }, cell = document.createElement('button');
        cell.type = 'button'; cell.className = 'xlsx-cell'; cell.setAttribute('role', 'gridcell');
        if (row === selected.row && col === selected.col) cell.classList.add('is-selected');
        cell.textContent = displayValue(sheet, model, position);
        cell.setAttribute('aria-label', `${columnName(col)}${row}: ${cell.textContent}`);
        cell.style.cssText = `left:${HEAD_W + (col - 1) * COL_W}px;top:${HEAD_H + (row - 1) * ROW_H}px;width:${COL_W}px;height:${ROW_H}px`;
        cell.addEventListener('click', () => {
          selected = position; updateSelectedInput();
          canvas.querySelectorAll('.xlsx-cell.is-selected').forEach(item => item.classList.remove('is-selected'));
          cell.classList.add('is-selected'); formulaInput.focus(); formulaInput.select();
        });
        fragment.append(cell);
      }
    }
    canvas.replaceChildren(fragment);
  }
  function commitInput() {
    if (!canWrite || saving) return;
    const sheet = model.sheets[activeSheet];
    setCellValue(sheet, selected, formulaInput.value);
    const formula = direct(sheet.cells.get(`${columnName(selected.col)}${selected.row}`), 'f');
    if (formula) {
      const calcPr = direct(model.workbookXml.documentElement, 'calcPr') || model.workbookXml.createElementNS(MAIN, 'calcPr');
      if (!calcPr.parentNode) model.workbookXml.documentElement.append(calcPr);
      calcPr.setAttribute('calcMode', 'auto'); calcPr.setAttribute('fullCalcOnLoad', '1');
    }
    setDirty(true); renderGrid();
  }
  formulaInput.addEventListener('input', commitInput);
  formulaInput.addEventListener('keydown', event => {
    if (!['Enter', 'ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Tab'].includes(event.key)) return;
    if (event.key === 'ArrowLeft' && formulaInput.selectionStart > 0 || event.key === 'ArrowRight' && formulaInput.selectionEnd < formulaInput.value.length) return;
    event.preventDefault();
    const delta = event.key === 'ArrowUp' ? [-1, 0] : event.key === 'ArrowLeft' ? [0, -1] : event.key === 'Tab' ? [0, 1] : [1, 0];
    selected = { row: Math.max(1, Math.min(MAX_ROWS, selected.row + delta[0])), col: Math.max(1, Math.min(MAX_COLS, selected.col + delta[1])) };
    const x = HEAD_W + (selected.col - 1) * COL_W, y = HEAD_H + (selected.row - 1) * ROW_H;
    if (x < scroll.scrollLeft || x + COL_W > scroll.scrollLeft + scroll.clientWidth) scroll.scrollLeft = Math.max(0, x - HEAD_W);
    if (y < scroll.scrollTop || y + ROW_H > scroll.scrollTop + scroll.clientHeight) scroll.scrollTop = Math.max(0, y - HEAD_H);
    updateSelectedInput(); renderGrid(); formulaInput.focus(); formulaInput.select();
  });
  scroll.addEventListener('scroll', renderGrid, { passive: true });
  scroll.addEventListener('keydown', event => {
    if (['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Enter'].includes(event.key)) {
      formulaInput.focus(); formulaInput.dispatchEvent(new KeyboardEvent('keydown', { key: event.key, bubbles: true }));
    }
  });
  async function persist() {
    if (!canWrite || !dirty || saving || !onSave) return;
    saving = true; save.disabled = true; status.textContent = 'Saving…'; formulaInput.readOnly = true;
    try {
      await onSave(model.export());
      setDirty(false);
    } catch (error) {
      status.textContent = error?.message || 'Could not save';
      throw error;
    } finally {
      saving = false; formulaInput.readOnly = !canWrite; save.disabled = !canWrite || !dirty;
      if (dirty) status.textContent = 'Unsaved changes';
    }
  }
  save.addEventListener('click', () => persist().catch(error => { status.textContent = error?.message || 'Could not save'; }));
  drawTabs(); updateSelectedInput(); renderGrid();
  return { destroy() { destroyed = true; root.remove(); }, save: persist };
}
