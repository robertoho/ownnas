/**
 * Preservation-first XLSX workbook editor. It edits cell XML in place and keeps every other part of
 * the package, shows cells with their number formats, fonts, fills, column widths and merges, and
 * recalculates the formulas it understands (see sheet-formula.js). Formulas it can't calculate keep
 * their saved values and are recalculated by the spreadsheet app on open.
 */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';
import {
  BUILTIN_FORMATS, CellError, FUNCTION_HELP, Unsupported, callAt, canPointAt, columnName, dateSerial, evaluate,
  formatGeneral, formatValue, formulaWordAt, localeSeparators, parseFormula, parseRef, refName, serialParts, shiftFormula,
} from './sheet-formula.js';

const MAIN = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const DOC_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PKG_REL = 'http://schemas.openxmlformats.org/package/2006/relationships';
const CONTENT_TYPES = 'http://schemas.openxmlformats.org/package/2006/content-types';
const DRAWING = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const XML = 'http://www.w3.org/XML/1998/namespace';
const MAX_COMPRESSED = 25 * 1024 * 1024;
const MAX_EXPANDED = 100 * 1024 * 1024;
const MAX_PARTS = 4096;
const MAX_CELLS = 200000;
const LAST_ROW = 1048576;
const LAST_COL = 16384;
const HEAD_H = 24;
const HEAD_W = 46;
const MAX_FORMAT_CELLS = 50000;
// Excel sizes columns for Calibri; other fonts run wider, so text is drawn a little smaller.
const FONT_SCALE = 0.92;
let measurer = null;
function textWidth(text, font) {
  measurer ||= document.createElement('canvas').getContext('2d');
  if (!measurer) return text.length * 7;
  measurer.font = font;
  return measurer.measureText(text).width;
}

// Excel's legacy indexed colours.
const INDEXED = ['000000', 'FFFFFF', 'FF0000', '00FF00', '0000FF', 'FFFF00', 'FF00FF', '00FFFF', '000000', 'FFFFFF', 'FF0000', '00FF00', '0000FF', 'FFFF00', 'FF00FF', '00FFFF', '800000', '008000', '000080', '808000', '800080', '008080', 'C0C0C0', '808080', '9999FF', '993366', 'FFFFCC', 'CCFFFF', '660066', 'FF8080', '0066CC', 'CCCCFF', '000080', 'FF00FF', 'FFFF00', '00FFFF', '800080', '800000', '008080', '0000FF', '00CCFF', 'CCFFFF', 'CCFFCC', 'FFFF99', '99CCFF', 'FF99CC', 'CC99FF', 'FFCC99', '3366FF', '33CCCC', '99CC00', 'FFCC00', 'FF9900', 'FF6600', '666699', '969696', '003366', '339966', '003300', '333300', '993300', '993366', '333399', '333333'];

function parseXml(source, label) {
  if (/<!DOCTYPE|<!ENTITY/i.test(source)) throw new Error(`${label} contains unsupported XML entities.`);
  const xml = new DOMParser().parseFromString(source, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error(`${label} is invalid.`);
  return xml;
}

const serialize = node => new XMLSerializer().serializeToString(node);
const kids = (node, local, ns = MAIN) => (node ? [...node.children].filter(child => child.namespaceURI === ns && child.localName === local) : []);
const direct = (node, local) => kids(node, local)[0] || null;
const flag = value => value === '1' || value === 'true';

function textOf(node) {
  // Phonetic runs (<rPh>) are reading hints, not part of the text.
  return [...node.getElementsByTagNameNS(MAIN, 't')].filter(t => t.parentNode.localName !== 'rPh').map(item => item.textContent || '').join('');
}

function normalizePart(target, base = 'xl') {
  const segments = target.startsWith('/') ? [] : base.split('/');
  for (const segment of target.replace(/^\//, '').split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') { if (segments.length) segments.pop(); } else segments.push(segment);
  }
  const path = segments.join('/');
  if (!path.startsWith('xl/')) throw new Error('Workbook references a worksheet outside its package.');
  return path;
}

// --- Colours ------------------------------------------------------------------------------------

function readTheme(parts) {
  const colors = [];
  const source = parts['xl/theme/theme1.xml'];
  if (!source) return colors;
  try {
    const xml = parseXml(strFromU8(source), 'Theme');
    const scheme = xml.getElementsByTagNameNS(DRAWING, 'clrScheme')[0];
    const pick = name => {
      const node = scheme && [...scheme.children].find(child => child.localName === name);
      const color = node?.firstElementChild;
      return color?.getAttribute('lastClr') || color?.getAttribute('val') || null;
    };
    // Theme indexes 0-3 swap light and dark: lt1, dk1, lt2, dk2, then the accents.
    for (const name of ['lt1', 'dk1', 'lt2', 'dk2', 'accent1', 'accent2', 'accent3', 'accent4', 'accent5', 'accent6', 'hlink', 'folHlink']) colors.push(pick(name));
  } catch { /* A theme we can't read leaves theme colours unset. */ }
  return colors;
}

function tint(hex, amount) {
  if (!amount) return hex;
  let [r, g, b] = [0, 2, 4].map(i => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0, l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    h /= 6;
  }
  l = amount < 0 ? l * (1 + amount) : l * (1 - amount) + amount;
  const hue = (p, q, t) => {
    if (t < 0) t += 1;
    if (t > 1) t -= 1;
    if (t < 1 / 6) return p + (q - p) * 6 * t;
    if (t < 1 / 2) return q;
    if (t < 2 / 3) return p + (q - p) * (2 / 3 - t) * 6;
    return p;
  };
  if (s === 0) r = g = b = l;
  else {
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s, p = 2 * l - q;
    r = hue(p, q, h + 1 / 3); g = hue(p, q, h); b = hue(p, q, h - 1 / 3);
  }
  return [r, g, b].map(v => Math.round(v * 255).toString(16).padStart(2, '0')).join('').toUpperCase();
}

function colorOf(node, theme) {
  if (!node) return null;
  let hex = null;
  const rgb = node.getAttribute('rgb');
  if (rgb && /^[0-9a-f]{6,8}$/i.test(rgb)) hex = rgb.slice(-6);
  else if (node.hasAttribute('theme')) hex = theme[Number(node.getAttribute('theme'))] || null;
  else if (node.hasAttribute('indexed')) hex = INDEXED[Number(node.getAttribute('indexed'))] || null;
  if (!hex) return null;
  return `#${tint(hex.toUpperCase(), Number(node.getAttribute('tint') || 0))}`;
}

// --- Styles -------------------------------------------------------------------------------------

function readStyles(parts, theme) {
  if (!parts['xl/styles.xml']) return null;
  const xml = parseXml(strFromU8(parts['xl/styles.xml']), 'Styles');
  const root = xml.documentElement;
  const numFmts = new Map();
  for (const fmt of kids(direct(root, 'numFmts'), 'numFmt')) numFmts.set(Number(fmt.getAttribute('numFmtId')), fmt.getAttribute('formatCode') || 'General');
  return { xml, root, theme, numFmts, cache: new Map() };
}

function section(styles, name) {
  let node = direct(styles.root, name);
  if (!node) {
    node = styles.xml.createElementNS(MAIN, name);
    node.setAttribute('count', '0');
    // styleSheet children have a fixed order.
    const order = ['numFmts', 'fonts', 'fills', 'borders', 'cellStyleXfs', 'cellXfs', 'cellStyles', 'dxfs', 'tableStyles', 'colors', 'extLst'];
    const after = order.slice(order.indexOf(name) + 1);
    const before = [...styles.root.children].find(child => after.includes(child.localName));
    styles.root.insertBefore(node, before || null);
  }
  return node;
}

function cellStyle(model, index) {
  const styles = model.styles;
  const key = Number(index) || 0;
  if (!styles) return { numFmt: 'General' };
  if (styles.cache.has(key)) return styles.cache.get(key);
  const xf = kids(direct(styles.root, 'cellXfs'), 'xf')[key] || kids(direct(styles.root, 'cellXfs'), 'xf')[0];
  const style = { numFmt: 'General' };
  if (xf) {
    const fmtId = Number(xf.getAttribute('numFmtId') || 0);
    style.numFmtId = fmtId;
    style.numFmt = styles.numFmts.get(fmtId) || BUILTIN_FORMATS[fmtId] || 'General';
    const font = kids(direct(styles.root, 'fonts'), 'font')[Number(xf.getAttribute('fontId') || 0)];
    if (font) {
      const on = name => { const node = direct(font, name); return !!node && node.getAttribute('val') !== '0' && node.getAttribute('val') !== 'false'; };
      style.bold = on('b'); style.italic = on('i'); style.strike = on('strike');
      const u = direct(font, 'u'); style.underline = !!u && u.getAttribute('val') !== 'none';
      style.color = colorOf(direct(font, 'color'), styles.theme);
      const size = Number(direct(font, 'sz')?.getAttribute('val'));
      if (size > 0 && size < 200) style.size = size;
      style.font = direct(font, 'name')?.getAttribute('val') || null;
    }
    const fill = kids(direct(styles.root, 'fills'), 'fill')[Number(xf.getAttribute('fillId') || 0)];
    const pattern = direct(fill, 'patternFill');
    if (pattern && pattern.getAttribute('patternType') && pattern.getAttribute('patternType') !== 'none') {
      style.fill = colorOf(direct(pattern, 'fgColor'), styles.theme) || colorOf(direct(pattern, 'bgColor'), styles.theme);
    }
    const align = direct(xf, 'alignment');
    if (align) {
      style.align = align.getAttribute('horizontal') || null;
      style.valign = align.getAttribute('vertical') || null;
      style.wrap = flag(align.getAttribute('wrapText'));
      style.indent = Number(align.getAttribute('indent') || 0);
    }
  }
  styles.cache.set(key, style);
  return style;
}

/** Adds `node` to a styles list unless an identical entry exists; returns its index. */
function addUnique(listNode, local, node) {
  const items = kids(listNode, local);
  const text = serialize(node);
  const found = items.findIndex(item => serialize(item) === text);
  if (found >= 0) return found;
  listNode.append(node);
  listNode.setAttribute('count', String(items.length + 1));
  return items.length;
}

function numFmtId(model, code) {
  const styles = model.styles;
  for (const [id, builtin] of Object.entries(BUILTIN_FORMATS)) if (builtin === code && ![14, 22].includes(Number(id))) return Number(id);
  for (const [id, existing] of styles.numFmts) if (existing === code) return id;
  const id = Math.max(163, ...styles.numFmts.keys()) + 1;
  const list = section(styles, 'numFmts');
  const fmt = styles.xml.createElementNS(MAIN, 'numFmt');
  fmt.setAttribute('numFmtId', String(id));
  fmt.setAttribute('formatCode', code);
  list.append(fmt);
  list.setAttribute('count', String(kids(list, 'numFmt').length));
  styles.numFmts.set(id, code);
  return id;
}

// <font> children must keep the schema's order.
const FONT_ORDER = ['b', 'i', 'strike', 'condense', 'extend', 'outline', 'shadow', 'u', 'vertAlign', 'sz', 'color', 'name', 'family', 'charset', 'scheme'];
function setFontChild(font, name, make) {
  for (const old of kids(font, name)) old.remove();
  if (!make) return;
  const node = font.ownerDocument.createElementNS(MAIN, name);
  make(node);
  const after = FONT_ORDER.slice(FONT_ORDER.indexOf(name) + 1);
  const before = [...font.children].find(child => after.includes(child.localName));
  font.insertBefore(node, before || null);
}

/** The style index for `base` with `change` applied: bold, italic, color, fill, align or numFmt. */
function restyle(model, base, change) {
  const styles = model.styles;
  if (!styles) return base;
  const doc = styles.xml;
  const xfs = section(styles, 'cellXfs');
  const list = kids(xfs, 'xf');
  const xf = (list[Number(base) || 0] || list[0])?.cloneNode(true) || doc.createElementNS(MAIN, 'xf');
  if ('bold' in change || 'italic' in change || 'color' in change) {
    const fonts = section(styles, 'fonts');
    const font = (kids(fonts, 'font')[Number(xf.getAttribute('fontId') || 0)] || kids(fonts, 'font')[0])?.cloneNode(true) || doc.createElementNS(MAIN, 'font');
    if ('bold' in change) setFontChild(font, 'b', change.bold && (() => {}));
    if ('italic' in change) setFontChild(font, 'i', change.italic && (() => {}));
    if ('color' in change) setFontChild(font, 'color', change.color && (node => node.setAttribute('rgb', `FF${change.color.slice(1).toUpperCase()}`)));
    xf.setAttribute('fontId', String(addUnique(fonts, 'font', font)));
    xf.setAttribute('applyFont', '1');
  }
  if ('fill' in change) {
    if (!change.fill) xf.setAttribute('fillId', '0');
    else {
      const fills = section(styles, 'fills');
      const fill = doc.createElementNS(MAIN, 'fill');
      const pattern = doc.createElementNS(MAIN, 'patternFill');
      pattern.setAttribute('patternType', 'solid');
      const fg = doc.createElementNS(MAIN, 'fgColor');
      fg.setAttribute('rgb', `FF${change.fill.slice(1).toUpperCase()}`);
      const bg = doc.createElementNS(MAIN, 'bgColor');
      bg.setAttribute('indexed', '64');
      pattern.append(fg, bg);
      fill.append(pattern);
      xf.setAttribute('fillId', String(addUnique(fills, 'fill', fill)));
    }
    xf.setAttribute('applyFill', '1');
  }
  if ('align' in change) {
    let align = direct(xf, 'alignment');
    if (!align && change.align) {
      align = doc.createElementNS(MAIN, 'alignment');
      xf.insertBefore(align, xf.firstChild);
    }
    if (align) {
      if (change.align) align.setAttribute('horizontal', change.align);
      else align.removeAttribute('horizontal');
      if (!align.attributes.length) align.remove();
    }
    xf.setAttribute('applyAlignment', '1');
  }
  if ('numFmt' in change) {
    xf.setAttribute('numFmtId', String(numFmtId(model, change.numFmt)));
    xf.setAttribute('applyNumberFormat', '1');
  }
  if (!xf.hasAttribute('xfId')) xf.setAttribute('xfId', '0');
  const index = addUnique(xfs, 'xf', xf);
  model.stylesTouched = true;
  return index;
}

// --- Workbook model -----------------------------------------------------------------------------

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
  const workbookXml = parseXml(strFromU8(parts['xl/workbook.xml']), 'Workbook XML');
  const relsXml = parseXml(strFromU8(parts['xl/_rels/workbook.xml.rels']), 'Workbook relationships');
  const relationships = new Map([...relsXml.getElementsByTagNameNS(PKG_REL, 'Relationship')]
    .map(rel => [rel.getAttribute('Id'), { target: rel.getAttribute('Target'), type: rel.getAttribute('Type') || '' }]));
  const sharedStrings = [];
  if (parts['xl/sharedStrings.xml']) {
    const stringsXml = parseXml(strFromU8(parts['xl/sharedStrings.xml']), 'Shared strings');
    for (const item of stringsXml.getElementsByTagNameNS(MAIN, 'si')) sharedStrings.push(textOf(item));
  }
  const theme = readTheme(parts);
  const styles = readStyles(parts, theme);
  const date1904 = flag(direct(workbookXml.documentElement, 'workbookPr')?.getAttribute('date1904'));
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
    const name = sheetNode.getAttribute('name') || `Sheet ${sheets.length + 1}`;
    const xml = parseXml(strFromU8(parts[path]), `Worksheet ${name}`);
    const root = xml.documentElement;
    const sheetData = direct(root, 'sheetData');
    if (!sheetData) throw new Error('Worksheet data is missing.');
    const sheet = {
      name, path, xml, root, sheetData, cells: new Map(), rows: new Map(), shared: new Map(),
      maxRow: 0, maxCol: 0, rowHeights: new Map(), hiddenRows: new Set(), cols: [], merges: [], covered: new Map(),
      hidden: ['hidden', 'veryHidden'].includes(sheetNode.getAttribute('state')),
      protected: flag(direct(root, 'sheetProtection')?.getAttribute('sheet')),
    };
    let lastRow = 0;
    for (const row of kids(sheetData, 'row')) {
      const r = Number(row.getAttribute('r')) || lastRow + 1;
      if (!row.hasAttribute('r')) row.setAttribute('r', String(r));
      lastRow = r;
      sheet.rows.set(r, row);
      if (row.hasAttribute('ht')) sheet.rowHeights.set(r, Number(row.getAttribute('ht')));
      if (flag(row.getAttribute('hidden'))) sheet.hiddenRows.add(r);
      let lastCol = 0;
      for (const cell of kids(row, 'c')) {
        if (++cellCount > MAX_CELLS) throw new Error('This workbook contains too many cells to edit in the browser.');
        let position = parseRef(cell.getAttribute('r'));
        if (!position) {
          position = { row: r, col: lastCol + 1 };
          cell.setAttribute('r', refName(position.row, position.col));
        }
        lastCol = position.col;
        const ref = refName(position.row, position.col);
        sheet.cells.set(ref, cell);
        sheet.maxRow = Math.max(sheet.maxRow, position.row);
        sheet.maxCol = Math.max(sheet.maxCol, position.col);
        const f = direct(cell, 'f');
        if (f?.getAttribute('t') === 'shared' && f.getAttribute('ref') && f.textContent) {
          sheet.shared.set(f.getAttribute('si'), { formula: f.textContent, row: position.row, col: position.col });
        }
      }
    }
    const format = direct(root, 'sheetFormatPr');
    sheet.defaultColPx = Math.round((Number(format?.getAttribute('defaultColWidth')) || (Number(format?.getAttribute('baseColWidth')) || 8) + 1.14) * 7);
    sheet.defaultRowPx = Math.round((Number(format?.getAttribute('defaultRowHeight')) || 15) * 4 / 3);
    for (const col of kids(direct(root, 'cols'), 'col')) {
      sheet.cols.push({
        min: Number(col.getAttribute('min')), max: Number(col.getAttribute('max')),
        px: col.hasAttribute('width') ? Math.round(Number(col.getAttribute('width')) * 7) : null,
        hidden: flag(col.getAttribute('hidden')),
      });
    }
    for (const merge of kids(direct(root, 'mergeCells'), 'mergeCell')) {
      const [a, b] = (merge.getAttribute('ref') || '').split(':').map(parseRef);
      if (!a || !b) continue;
      const range = { r1: Math.min(a.row, b.row), c1: Math.min(a.col, b.col), r2: Math.max(a.row, b.row), c2: Math.max(a.col, b.col) };
      if ((range.r2 - range.r1 + 1) * (range.c2 - range.c1 + 1) > 100000) continue;
      sheet.merges.push(range);
      for (let r = range.r1; r <= range.r2; r++) for (let c = range.c1; c <= range.c2; c++) sheet.covered.set(refName(r, c), range);
    }
    sheets.push(sheet);
  }
  if (!sheets.length) throw new Error('No readable worksheets were found.');
  const view = direct(direct(workbookXml.documentElement, 'bookViews'), 'workbookView');
  const model = {
    parts, workbookXml, relsXml, sheets, sharedStrings, styles, date1904,
    activeSheet: Math.min(sheets.length - 1, Number(view?.getAttribute('activeTab') || 0)),
    computed: new Map(), failed: new Set(), trees: new Map(), touched: new Set(), stylesTouched: false, edited: false,
    export() { return exportWorkbook(model); },
  };
  model.protected = sheets.every(sheet => sheet.protected);
  // Formulas saved without a value (some apps write none) are calculated now.
  recalc(model, { onlyMissing: true });
  return model;
}

function exportWorkbook(model) {
  const { parts } = model;
  if (model.edited) {
    for (const [key, value] of model.computed) {
      if (model.failed.has(key)) continue;
      const [index, ref] = key.split('!');
      const sheet = model.sheets[Number(index)];
      const cell = sheet.cells.get(ref);
      if (!cell || !direct(cell, 'f')) continue;
      writeResult(sheet, cell, value);
      model.touched.add(sheet);
    }
    // Excel rebuilds the calculation chain; a stale one makes it "repair" the file.
    if (parts['xl/calcChain.xml']) {
      delete parts['xl/calcChain.xml'];
      for (const rel of [...model.relsXml.getElementsByTagNameNS(PKG_REL, 'Relationship')]) {
        if ((rel.getAttribute('Type') || '').endsWith('/calcChain')) rel.remove();
      }
      parts['xl/_rels/workbook.xml.rels'] = strToU8(serialize(model.relsXml));
      const types = parseXml(strFromU8(parts['[Content_Types].xml']), 'Content types');
      for (const override of [...types.getElementsByTagNameNS(CONTENT_TYPES, 'Override')]) {
        if (override.getAttribute('PartName') === '/xl/calcChain.xml') override.remove();
      }
      parts['[Content_Types].xml'] = strToU8(serialize(types));
    }
    if (model.sheets.some(sheet => [...sheet.cells.values()].some(cell => direct(cell, 'f')))) {
      const root = model.workbookXml.documentElement;
      let calcPr = direct(root, 'calcPr');
      if (!calcPr) {
        calcPr = model.workbookXml.createElementNS(MAIN, 'calcPr');
        // calcPr follows sheets/functionGroups/externalReferences/definedNames.
        const after = ['oleSize', 'customWorkbookViews', 'pivotCaches', 'smartTagPr', 'smartTagTypes', 'webPublishing', 'fileRecoveryPr', 'webPublishObjects', 'extLst'];
        root.insertBefore(calcPr, [...root.children].find(child => after.includes(child.localName)) || null);
      }
      calcPr.setAttribute('fullCalcOnLoad', '1');
      parts['xl/workbook.xml'] = strToU8(serialize(model.workbookXml));
    }
  }
  for (const sheet of model.touched) parts[sheet.path] = strToU8(serialize(sheet.xml));
  if (model.stylesTouched && model.styles) parts['xl/styles.xml'] = strToU8(serialize(model.styles.xml));
  return zipSync(parts, { level: 6 });
}

function writeResult(sheet, cell, value) {
  for (const old of [...cell.children]) if (old.localName === 'v' || old.localName === 'is') old.remove();
  cell.removeAttribute('t');
  if (value == null) return;
  const v = sheet.xml.createElementNS(MAIN, 'v');
  if (value instanceof CellError) { cell.setAttribute('t', 'e'); v.textContent = value.code; }
  else if (typeof value === 'boolean') { cell.setAttribute('t', 'b'); v.textContent = value ? '1' : '0'; }
  else if (typeof value === 'string') { cell.setAttribute('t', 'str'); v.textContent = value; }
  else if (Number.isFinite(value)) v.textContent = String(value);
  else { cell.setAttribute('t', 'e'); v.textContent = '#NUM!'; }
  const f = direct(cell, 'f');
  if (f) f.after(v); else cell.append(v);
}

/** The value the file saved for a cell. */
function cachedValue(model, cell) {
  if (!cell) return null;
  const type = cell.getAttribute('t');
  if (type === 'inlineStr') { const inline = direct(cell, 'is'); return inline ? textOf(inline) : ''; }
  const v = direct(cell, 'v');
  if (!v) return null;
  const raw = v.textContent || '';
  if (type === 's') return model.sharedStrings[Number(raw)] ?? '';
  if (type === 'str') return raw;
  if (type === 'b') return raw === '1' || raw.toLowerCase() === 'true';
  if (type === 'e') return new CellError(raw);
  if (type === 'd') {
    const date = new Date(raw);
    if (Number.isNaN(date.getTime())) return raw;
    const day = dateSerial(date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), model.date1904);
    return day + (date.getUTCHours() * 3600 + date.getUTCMinutes() * 60 + date.getUTCSeconds()) / 86400;
  }
  if (raw === '') return null;
  const number = Number(raw);
  return Number.isFinite(number) ? number : raw;
}

/** A cell's formula without the "=", expanding shared formulas. */
function formulaOf(sheet, cell) {
  const f = cell && direct(cell, 'f');
  if (!f) return null;
  if (f.getAttribute('t') === 'shared' && !f.textContent) {
    const master = sheet.shared.get(f.getAttribute('si'));
    const position = parseRef(cell.getAttribute('r'));
    if (!master || !position) return null;
    return shiftFormula(master.formula, position.row - master.row, position.col - master.col);
  }
  return f.textContent;
}

function valueAt(model, index, row, col) {
  const sheet = model.sheets[index];
  const ref = refName(row, col);
  const cell = sheet.cells.get(ref);
  if (!cell) return null;
  const key = `${index}!${ref}`;
  return model.computed.has(key) ? model.computed.get(key) : cachedValue(model, cell);
}

/** Recalculates every formula the engine understands; the others keep their saved values. */
function recalc(model, { onlyMissing = false } = {}) {
  const results = new Map(), busy = new Set(), failed = new Set();
  const byName = new Map(model.sheets.map((sheet, index) => [sheet.name.toLowerCase(), index]));
  function cellValue(index, row, col) {
    const sheet = model.sheets[index];
    const ref = refName(row, col);
    const cell = sheet.cells.get(ref);
    if (!cell) return null;
    const f = direct(cell, 'f');
    const formula = f && !['array', 'dataTable'].includes(f.getAttribute('t')) ? formulaOf(sheet, cell) : null;
    if (formula == null) return cachedValue(model, cell);
    const key = `${index}!${ref}`;
    if (results.has(key)) return results.get(key);
    if (busy.has(key)) throw new Unsupported('Circular reference');
    busy.add(key);
    let value;
    try {
      let tree = model.trees.get(formula);
      if (!tree) { tree = parseFormula(formula); model.trees.set(formula, tree); }
      value = evaluate(tree, {
        row, col, date1904: model.date1904,
        size: name => {
          const target = name == null ? index : byName.get(name.toLowerCase());
          if (target === undefined) return null;
          return { rows: model.sheets[target].maxRow, cols: model.sheets[target].maxCol };
        },
        cell: (name, r, c) => {
          const target = name == null ? index : byName.get(name.toLowerCase());
          if (target === undefined) throw new Unsupported('Unknown sheet');
          return cellValue(target, r, c);
        },
      });
    } catch (error) {
      if (!(error instanceof Unsupported)) throw error;
      failed.add(key);
      value = cachedValue(model, cell);
    } finally {
      busy.delete(key);
    }
    results.set(key, value);
    return value;
  }
  model.sheets.forEach((sheet, index) => {
    for (const [ref, cell] of sheet.cells) {
      if (!direct(cell, 'f')) continue;
      if (onlyMissing && direct(cell, 'v')) continue;
      const { row, col } = parseRef(ref);
      const value = cellValue(index, row, col);
      if (onlyMissing) model.computed.set(`${index}!${ref}`, value);
    }
  });
  if (!onlyMissing) {
    model.computed = results;
    model.failed = failed;
  } else for (const key of failed) model.failed.add(key);
}

// --- Cell edits ---------------------------------------------------------------------------------

function rowElement(sheet, r) {
  let row = sheet.rows.get(r);
  if (!row) {
    row = sheet.xml.createElementNS(MAIN, 'row');
    row.setAttribute('r', String(r));
    const before = kids(sheet.sheetData, 'row').find(item => Number(item.getAttribute('r')) > r);
    sheet.sheetData.insertBefore(row, before || null);
    sheet.rows.set(r, row);
  }
  return row;
}

function cellElement(sheet, row, col) {
  const ref = refName(row, col);
  let cell = sheet.cells.get(ref);
  if (cell) return cell;
  const parent = rowElement(sheet, row);
  cell = sheet.xml.createElementNS(MAIN, 'c');
  cell.setAttribute('r', ref);
  // A new cell takes its row's or column's style, as Excel does.
  const style = parent.getAttribute('customFormat') && parent.getAttribute('s') || columnStyle(sheet, col);
  if (style && style !== '0') cell.setAttribute('s', style);
  const before = kids(parent, 'c').find(item => (parseRef(item.getAttribute('r'))?.col || 0) > col);
  parent.insertBefore(cell, before || null);
  sheet.cells.set(ref, cell);
  sheet.maxRow = Math.max(sheet.maxRow, row);
  sheet.maxCol = Math.max(sheet.maxCol, col);
  updateDimension(sheet);
  return cell;
}

function columnStyle(sheet, col) {
  const node = kids(direct(sheet.root, 'cols'), 'col').find(item => Number(item.getAttribute('min')) <= col && col <= Number(item.getAttribute('max')));
  return node?.getAttribute('style') || null;
}

/** Puts a saved copy of a cell back (or removes it), for undo and redo. */
function placeCell(sheet, ref, copy) {
  const existing = sheet.cells.get(ref);
  if (!copy) {
    if (existing) { existing.remove(); sheet.cells.delete(ref); }
    return;
  }
  const node = sheet.xml.importNode(copy, true);
  if (existing) existing.replaceWith(node);
  else {
    const { row, col } = parseRef(ref);
    const parent = rowElement(sheet, row);
    const before = kids(parent, 'c').find(item => (parseRef(item.getAttribute('r'))?.col || 0) > col);
    parent.insertBefore(node, before || null);
  }
  sheet.cells.set(ref, node);
  const f = direct(node, 'f');
  if (f?.getAttribute('t') === 'shared' && f.getAttribute('ref') && f.textContent) {
    const { row, col } = parseRef(ref);
    sheet.shared.set(f.getAttribute('si'), { formula: f.textContent, row, col });
  }
}

function updateDimension(sheet) {
  const dimension = direct(sheet.root, 'dimension');
  if (!dimension) return;
  const end = refName(Math.max(1, sheet.maxRow), Math.max(1, sheet.maxCol));
  dimension.setAttribute('ref', end === 'A1' ? 'A1' : `A1:${end}`);
}

function isDateFormat(code) {
  const body = String(code || '').replace(/"[^"]*"|\\.|\[[^\]]*\]/g, '');
  return /[dy]|h|s/i.test(body) && !/^general$/i.test(body.trim()) && !/[0#?]/.test(body.replace(/s+\.0+/gi, ''));
}

/** What a typed entry means: a formula, number (maybe a date or percentage), boolean, text, or nothing. */
export function parseInput(text, { decimal = '.', date1904 = false } = {}) {
  if (text === '') return { kind: 'empty' };
  if (text.startsWith("'")) return { kind: 'text', value: text.slice(1) };
  if (text.startsWith('=') && text.length > 1) return { kind: 'formula', value: text.slice(1) };
  if (/^(true|false)$/i.test(text.trim())) return { kind: 'bool', value: /^true$/i.test(text.trim()) };
  const trimmed = text.trim();
  const iso = /^(\d{4})-(\d{1,2})-(\d{1,2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?$/.exec(trimmed);
  if (iso) {
    const [y, m, d, hh, mm, ss] = iso.slice(1).map(part => Number(part || 0));
    if (m >= 1 && m <= 12 && d >= 1 && d <= 31 && y >= 1900 && y <= 9999 && hh < 24 && mm < 60 && ss < 60) {
      const serial = dateSerial(y, m, d, date1904) + (hh * 3600 + mm * 60 + ss) / 86400;
      return { kind: 'number', value: serial, format: iso[4] ? 'yyyy-mm-dd hh:mm' : 'yyyy-mm-dd' };
    }
  }
  const time = /^(\d{1,2}):(\d{2})(?::(\d{2}))?$/.exec(trimmed);
  if (time && Number(time[1]) < 24 && Number(time[2]) < 60) {
    return { kind: 'number', value: (Number(time[1]) * 3600 + Number(time[2]) * 60 + Number(time[3] || 0)) / 86400, format: time[3] ? 'hh:mm:ss' : 'hh:mm' };
  }
  let plain = trimmed.replace(/\s/g, '');
  const percent = plain.endsWith('%');
  if (percent) plain = plain.slice(0, -1);
  if (decimal === ',') {
    if (/^[-+]?\d{1,3}(\.\d{3})+(,\d+)?$/.test(plain)) plain = plain.replace(/\./g, '');
    plain = plain.replace(',', '.');
  } else if (/^[-+]?\d{1,3}(,\d{3})+(\.\d+)?$/.test(plain)) plain = plain.replace(/,/g, '');
  if (/^[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[-+]?\d+)?$/i.test(plain)) {
    const value = Number(plain);
    if (Number.isFinite(value) && Math.abs(value) < 1e308) {
      if (percent) return { kind: 'number', value: value / 100, format: plain.includes('.') ? '0.00%' : '0%' };
      return { kind: 'number', value };
    }
  }
  return { kind: 'text', value: text };
}

// --- Editor -------------------------------------------------------------------------------------

const ICONS = {
  undo: '<path d="M4 7h7a3 3 0 0 1 0 6H8"/><path d="M6.5 4.5 4 7l2.5 2.5"/>',
  redo: '<path d="M12 7H5a3 3 0 0 0 0 6h3"/><path d="M9.5 4.5 12 7 9.5 9.5"/>',
  bold: '<path d="M5 3h4a2.5 2.5 0 0 1 0 5H5zM5 8h4.5a2.5 2.5 0 0 1 0 5H5z"/>',
  italic: '<path d="M7 3h5M4 13h5M10 3 6 13"/>',
  left: '<path d="M3 4h10M3 7h6M3 10h10M3 13h6"/>',
  center: '<path d="M3 4h10M5 7h6M3 10h10M5 13h6"/>',
  right: '<path d="M3 4h10M7 7h6M3 10h10M7 13h6"/>',
  fill: '<path d="M3 9 8 4l4 4-5 5z"/><path d="M12.5 10.5s1.5 1.6 1.5 2.5a1.5 1.5 0 0 1-3 0c0-.9 1.5-2.5 1.5-2.5z"/>',
  color: '<path d="M4.5 12 8 3l3.5 9M6 9h4"/>',
  save: '<path d="M3 3h8l2 2v8H3z"/><path d="M5 3v3h5V3M5 13V9h6v4"/>',
};

function icon(name) {
  return `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name]}</svg>`;
}

/** Shorter ways to show a General number: fewer decimals, then scientific notation. */
function* generalFits(value) {
  const { decimal } = localeSeparators(typeof navigator !== 'undefined' ? navigator.language : 'en-US');
  const local = text => (decimal === '.' ? text : text.replace('.', decimal));
  if (Math.abs(value) < 1e11) for (let places = 9; places >= 0; places--) yield local(String(Number(value.toFixed(places))));
  for (let digits = 5; digits >= 0; digits--) {
    yield local(value.toExponential(digits).replace(/\.?0+e/, 'e').replace('e', 'E').replace(/E([-+])(\d)$/, 'E$10$2'));
  }
}

const SHEET_FONT = 'Calibri, Carlito, "Segoe UI", Arial, sans-serif';

const FORMATS = [
  ['General', 'General'], ['0.00', 'Number'], ['#,##0.00', 'Thousands'], ['0%', 'Percent'], ['0.00%', 'Percent 0.00'],
  ['#,##0.00 "€"', 'Euro'], ['"$"#,##0.00', 'Dollar'], ['yyyy-mm-dd', 'Date'], ['yyyy-mm-dd hh:mm', 'Date and time'], ['hh:mm', 'Time'], ['@', 'Text'],
];

/** Mount the editor; onSave receives the updated OOXML package as Uint8Array. */
export function mountXlsxEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openXlsx(bytes);
  const locale = typeof navigator !== 'undefined' ? navigator.language : 'en-US';
  const separators = localeSeparators(locale);
  const formatOptions = { ...separators, date1904: model.date1904 };
  const visibleSheets = model.sheets.map((sheet, index) => index).filter(index => !model.sheets[index].hidden);
  let active = model.sheets[model.activeSheet]?.hidden ? visibleSheets[0] ?? 0 : model.activeSheet;
  const selections = new Map();
  let anchor = { row: 1, col: 1 }, focus = { row: 1, col: 1 };
  let editing = null, dragging = null, saving = false, destroyed = false, frame = 0, geo = null;
  const history = [];
  let historyAt = 0, savedAt = 0, clipboard = null;

  const root = document.createElement('section');
  root.className = 'xlsx-editor';
  const toolbar = document.createElement('div');
  toolbar.className = 'office-toolbar xlsx-toolbar';
  toolbar.setAttribute('role', 'toolbar');
  toolbar.setAttribute('aria-label', 'Spreadsheet tools');
  const status = document.createElement('span');
  status.className = 'office-status';
  status.setAttribute('role', 'status');
  const formulaRow = document.createElement('div');
  formulaRow.className = 'xlsx-formula-row';
  const address = document.createElement('span');
  address.className = 'xlsx-address';
  const fx = document.createElement('span');
  fx.className = 'xlsx-fx';
  fx.textContent = 'fx';
  const formulaInput = document.createElement('input');
  formulaInput.className = 'xlsx-formula';
  formulaInput.setAttribute('aria-label', 'Cell contents');
  formulaInput.autocomplete = 'off';
  formulaInput.spellcheck = false;
  formulaRow.append(address, fx, formulaInput);
  const scroll = document.createElement('div');
  scroll.className = 'xlsx-scroll';
  scroll.tabIndex = 0;
  scroll.setAttribute('role', 'grid');
  scroll.setAttribute('aria-label', 'Spreadsheet cells');
  const canvas = document.createElement('div');
  canvas.className = 'xlsx-canvas';
  const layer = document.createElement('div');
  const selectionBox = document.createElement('div');
  selectionBox.className = 'xlsx-selection';
  const activeBox = document.createElement('div');
  activeBox.className = 'xlsx-active';
  const cellEditor = document.createElement('textarea');
  cellEditor.className = 'xlsx-cell-editor';
  cellEditor.hidden = true;
  cellEditor.spellcheck = false;
  cellEditor.setAttribute('aria-label', 'Edit cell');
  // The cells a formula being typed points at (clicked, dragged over, or reached with arrows).
  const pointBox = document.createElement('div');
  pointBox.className = 'xlsx-point';
  pointBox.hidden = true;
  canvas.append(layer, selectionBox, activeBox, pointBox, cellEditor);
  scroll.append(canvas);
  const footer = document.createElement('div');
  footer.className = 'xlsx-footer';
  const tabs = document.createElement('div');
  tabs.className = 'xlsx-tabs';
  tabs.setAttribute('role', 'tablist');
  tabs.setAttribute('aria-label', 'Sheets');
  const summary = document.createElement('span');
  summary.className = 'xlsx-summary';
  footer.append(tabs, summary);
  const note = document.createElement('p');
  note.className = 'xlsx-note';
  const live = document.createElement('span');
  live.className = 'xlsx-live';
  live.setAttribute('aria-live', 'polite');
  // Function suggestions while a formula is typed, or the arguments of the call it is in.
  const assist = document.createElement('div');
  assist.className = 'xlsx-assist';
  assist.hidden = true;
  root.append(toolbar, formulaRow, scroll, footer, note, live, assist);
  container.replaceChildren(root);

  const sheet = () => model.sheets[active];
  const canWrite = () => !readonly && !saving && !sheet().protected;
  const dirty = () => historyAt !== savedAt;

  // Toolbar
  const buttons = {};
  function tool(name, label, action, { toggle = false } = {}) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'office-tool';
    b.innerHTML = icon(name);
    b.title = label;
    b.setAttribute('aria-label', label);
    if (toggle) b.setAttribute('aria-pressed', 'false');
    b.addEventListener('mousedown', event => event.preventDefault());
    b.addEventListener('click', action);
    toolbar.append(b);
    buttons[name] = b;
    return b;
  }
  const divider = () => { const d = document.createElement('span'); d.className = 'office-divider'; toolbar.append(d); };
  function colorTool(name, label, action) {
    const wrap = document.createElement('label');
    wrap.className = 'office-tool office-color';
    wrap.title = label;
    wrap.innerHTML = icon(name);
    const input = document.createElement('input');
    input.type = 'color';
    input.setAttribute('aria-label', label);
    input.value = name === 'fill' ? '#ffff00' : '#c00000';
    const swatch = document.createElement('span');
    swatch.className = 'office-swatch';
    swatch.style.background = input.value;
    input.addEventListener('input', () => { swatch.style.background = input.value; });
    input.addEventListener('change', () => action(input.value));
    wrap.append(input, swatch);
    toolbar.append(wrap);
    buttons[name] = input;
  }
  const formatSelect = document.createElement('select');
  if (!readonly) {
    tool('undo', 'Undo (Ctrl+Z)', () => step(-1));
    tool('redo', 'Redo (Ctrl+Y)', () => step(1));
    divider();
    tool('bold', 'Bold (Ctrl+B)', () => formatSelection({ bold: !activeStyle().bold }), { toggle: true });
    tool('italic', 'Italic (Ctrl+I)', () => formatSelection({ italic: !activeStyle().italic }), { toggle: true });
    colorTool('color', 'Text colour', value => formatSelection({ color: value }));
    colorTool('fill', 'Fill colour', value => formatSelection({ fill: value }));
    const noFill = document.createElement('button');
    noFill.type = 'button';
    noFill.className = 'office-tool office-text-tool';
    noFill.textContent = 'No fill';
    noFill.addEventListener('mousedown', event => event.preventDefault());
    noFill.addEventListener('click', () => formatSelection({ fill: null }));
    toolbar.append(noFill);
    buttons.nofill = noFill;
    divider();
    tool('left', 'Align left', () => formatSelection({ align: activeStyle().align === 'left' ? null : 'left' }), { toggle: true });
    tool('center', 'Align centre', () => formatSelection({ align: activeStyle().align === 'center' ? null : 'center' }), { toggle: true });
    tool('right', 'Align right', () => formatSelection({ align: activeStyle().align === 'right' ? null : 'right' }), { toggle: true });
    divider();
    formatSelect.className = 'office-select';
    formatSelect.setAttribute('aria-label', 'Number format');
    formatSelect.title = 'Number format';
    for (const [code, label] of [['', 'Format…'], ...FORMATS]) {
      const option = document.createElement('option');
      option.value = code;
      option.textContent = label;
      formatSelect.append(option);
    }
    formatSelect.addEventListener('change', () => {
      if (formatSelect.value) formatSelection({ numFmt: formatSelect.value });
      scroll.focus({ preventScroll: true });
    });
    toolbar.append(formatSelect);
  }
  const spacer = document.createElement('span');
  spacer.className = 'office-spacer';
  toolbar.append(spacer, status);
  let saveButton = null;
  if (!readonly) {
    saveButton = document.createElement('button');
    saveButton.type = 'button';
    saveButton.className = 'office-save';
    saveButton.innerHTML = `${icon('save')}<span>Save</span>`;
    saveButton.title = 'Save (Ctrl+S)';
    saveButton.addEventListener('click', () => persist().catch(() => {}));
    toolbar.append(saveButton);
  }

  function updateToolbar() {
    if (readonly) { status.textContent = 'Read only'; return; }
    const style = activeStyle();
    const writable = canWrite();
    for (const [name, b] of Object.entries(buttons)) b.disabled = !writable && name !== 'undo' && name !== 'redo';
    formatSelect.disabled = !writable || !model.styles;
    buttons.undo.disabled = saving || historyAt === 0;
    buttons.redo.disabled = saving || historyAt === history.length;
    buttons.bold.setAttribute('aria-pressed', String(!!style.bold));
    buttons.italic.setAttribute('aria-pressed', String(!!style.italic));
    for (const side of ['left', 'center', 'right']) buttons[side].setAttribute('aria-pressed', String(style.align === side));
    const known = FORMATS.find(([code]) => code === style.numFmt);
    formatSelect.value = known ? known[0] : '';
    if (!model.styles) for (const name of ['bold', 'italic', 'left', 'center', 'right', 'color', 'fill', 'nofill']) buttons[name].disabled = true;
    saveButton.disabled = saving || !dirty() || !onSave;
    status.textContent = saving ? 'Saving…' : dirty() ? 'Unsaved changes' : 'Saved';
  }

  function updateNote() {
    const notes = [];
    if (readonly) notes.push('Read only.');
    else if (sheet().protected) notes.push('This sheet is protected, so it is read only here.');
    const failed = [...model.failed].filter(key => key.startsWith(`${active}!`)).length;
    if (failed) notes.push(`${failed} formula${failed === 1 ? ' uses' : 's use'} functions this editor can't calculate; ${failed === 1 ? 'it keeps its' : 'they keep their'} saved value${failed === 1 ? '' : 's'} until the file is opened in a spreadsheet app.`);
    note.textContent = notes.join(' ');
    note.hidden = !notes.length;
  }

  // Sheet tabs
  function drawTabs() {
    tabs.replaceChildren();
    for (const index of visibleSheets) {
      const tab = document.createElement('button');
      tab.type = 'button';
      tab.className = 'xlsx-tab';
      tab.textContent = model.sheets[index].name;
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-selected', String(index === active));
      tab.addEventListener('click', () => showSheet(index));
      tabs.append(tab);
    }
  }
  function showSheet(index) {
    if (index === active) return;
    commitEdit();
    selections.set(active, { anchor, focus, top: scroll.scrollTop, left: scroll.scrollLeft });
    active = index;
    const saved = selections.get(index) || { anchor: { row: 1, col: 1 }, focus: { row: 1, col: 1 }, top: 0, left: 0 };
    anchor = saved.anchor; focus = saved.focus;
    geo = null;
    drawTabs();
    layout();
    scroll.scrollTop = saved.top; scroll.scrollLeft = saved.left;
    selectionChanged();
    updateNote();
  }

  // Geometry
  function colPx(s, col) {
    for (const range of s.cols) if (range.min <= col && col <= range.max) return range.hidden ? 0 : range.px ?? s.defaultColPx;
    return s.defaultColPx;
  }
  function rowPx(s, row) {
    if (s.hiddenRows.has(row)) return 0;
    const pt = s.rowHeights.get(row);
    return pt ? Math.max(4, Math.round(pt * 4 / 3)) : s.defaultRowPx;
  }
  function layout() {
    const s = sheet();
    // A whole-column or whole-row selection doesn't grow the grid to the sheet's end.
    const near = (value, last) => (value >= last ? 0 : value);
    const rows = Math.min(LAST_ROW, Math.max(100, s.maxRow + 40, anchor.row + 20, near(focus.row, LAST_ROW) + 20));
    const cols = Math.min(LAST_COL, Math.max(26, s.maxCol + 6, anchor.col + 4, near(focus.col, LAST_COL) + 4));
    if (geo && geo.rows === rows && geo.cols === cols && geo.sheet === s) return;
    const colX = new Float64Array(cols + 2);
    colX[1] = HEAD_W;
    for (let c = 1; c <= cols; c++) colX[c + 1] = colX[c] + colPx(s, c);
    const rowY = new Float64Array(rows + 2);
    rowY[1] = HEAD_H;
    for (let r = 1; r <= rows; r++) rowY[r + 1] = rowY[r] + rowPx(s, r);
    geo = { sheet: s, rows, cols, colX, rowY };
    canvas.style.width = `${colX[cols + 1]}px`;
    canvas.style.height = `${rowY[rows + 1]}px`;
  }
  function find(edges, count, value) {
    let lo = 1, hi = count;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (edges[mid] <= value) lo = mid; else hi = mid - 1;
    }
    return lo;
  }
  const rect = (r1, c1, r2, c2) => ({
    left: geo.colX[c1], top: geo.rowY[r1],
    width: geo.colX[Math.min(c2, geo.cols) + 1] - geo.colX[c1], height: geo.rowY[Math.min(r2, geo.rows) + 1] - geo.rowY[r1],
  });

  // Display
  function activeStyle() {
    const cell = sheet().cells.get(refName(anchor.row, anchor.col));
    return cellStyle(model, cell?.getAttribute('s'));
  }
  function shown(row, col) {
    const s = sheet();
    const cell = s.cells.get(refName(row, col));
    if (!cell) return null;
    const value = valueAt(model, active, row, col);
    const style = cellStyle(model, cell.getAttribute('s'));
    const formatted = formatValue(value, style.numFmt, formatOptions);
    let align = style.align;
    if (!align || align === 'general') align = typeof value === 'number' ? 'right' : typeof value === 'boolean' || value instanceof CellError ? 'center' : 'left';
    if (align === 'centerContinuous') align = 'center';
    if (!['left', 'center', 'right'].includes(align)) align = 'left';
    return { value, style, text: formatted.text, color: formatted.color, align };
  }

  function render() {
    frame = 0;
    if (destroyed) return;
    layout();
    const s = sheet();
    const top = scroll.scrollTop, left = scroll.scrollLeft;
    const height = scroll.clientHeight || 600, width = scroll.clientWidth || 800;
    const r1 = find(geo.rowY, geo.rows, top + HEAD_H), r2 = Math.min(geo.rows, find(geo.rowY, geo.rows, top + height) + 1);
    const c1 = find(geo.colX, geo.cols, left + HEAD_W), c2 = Math.min(geo.cols, find(geo.colX, geo.cols, left + width) + 1);
    const fragment = document.createDocumentFragment();
    const right = geo.colX[c2 + 1], bottom = geo.rowY[r2 + 1];
    for (let r = r1; r <= r2 + 1; r++) {
      const line = document.createElement('div');
      line.className = 'xlsx-hline';
      line.style.cssText = `top:${geo.rowY[r] - 1}px;left:${HEAD_W}px;width:${right - HEAD_W}px`;
      fragment.append(line);
    }
    for (let c = c1; c <= c2 + 1; c++) {
      const line = document.createElement('div');
      line.className = 'xlsx-vline';
      line.style.cssText = `left:${geo.colX[c] - 1}px;top:${HEAD_H}px;height:${bottom - HEAD_H}px`;
      fragment.append(line);
    }
    const drawn = new Set();
    const drawCell = (row, col, merge) => {
      const info = shown(row, col);
      if (!info) return;
      const { style } = info;
      const box = merge ? rect(merge.r1, merge.c1, merge.r2, merge.c2) : rect(row, col, row, col);
      if (!box.width || !box.height) return;
      const el = document.createElement('div');
      el.className = 'xlsx-cell';
      let width = box.width;
      const px = (style.size || 11) * 4 / 3 * FONT_SCALE;
      const font = `${style.italic ? 'italic ' : ''}${style.bold ? '700 ' : ''}${px.toFixed(1)}px ${style.font ? `"${style.font.replace(/["\\]/g, '')}", ` : ''}${SHEET_FONT}`;
      const needed = info.text && !style.wrap ? textWidth(info.text, font) + 8 : 0;
      // Text spills into empty neighbours on the right, as in spreadsheet apps.
      if (!merge && !style.wrap && info.align === 'left' && typeof info.value === 'string' && info.text) {
        let next = col + 1;
        while (width < needed && next <= Math.min(geo.cols, c2 + 4) && !shown(row, next)?.text && !s.covered.has(refName(row, next))) {
          width += geo.colX[next + 1] - geo.colX[next];
          next++;
        }
        if (width > box.width) el.classList.add('is-spilling');
      }
      let text = info.text, drawFont = font;
      // A number a little too wide shrinks to fit (fonts other than Calibri run wider); one far too
      // wide shows ### rather than a misleading part of itself.
      // General numbers lose decimals, then go scientific, before giving up, as in Excel.
      if (typeof info.value === 'number' && !style.wrap && needed > width && /^general$/i.test(style.numFmt.trim())) {
        for (const candidate of generalFits(info.value)) {
          if (textWidth(candidate, font) + 8 <= width) { text = candidate; break; }
        }
      }
      if (typeof info.value === 'number' && !style.wrap && text === info.text && needed > width) {
        const scale = (width - 6) / (needed - 6);
        if (scale >= 0.8) drawFont = font.replace(/[\d.]+px/, `${(px * scale).toFixed(1)}px`);
        else text = '#'.repeat(Math.max(1, Math.floor((width - 8) / textWidth('#', font))));
      }
      el.textContent = text;
      const css = [`left:${box.left}px`, `top:${box.top}px`, `width:${width}px`, `height:${box.height}px`, `justify-content:${info.align === 'right' ? 'flex-end' : info.align === 'center' ? 'center' : 'flex-start'}`, `text-align:${info.align}`];
      if (style.fill) css.push(`background:${style.fill}`);
      if (info.color || style.color) css.push(`color:${info.color || style.color}`);
      const lines = [style.underline && 'underline', style.strike && 'line-through'].filter(Boolean);
      if (lines.length) css.push(`text-decoration:${lines.join(' ')}`);
      css.push(`font:${drawFont}`);
      if (style.wrap) css.push('white-space:pre-wrap;overflow-wrap:anywhere');
      css.push(`align-items:${style.valign === 'top' ? 'flex-start' : style.valign === 'center' ? 'center' : 'flex-end'}`);
      if (style.indent) css.push(`padding-left:${4 + style.indent * 9}px`);
      el.style.cssText = css.join(';');
      fragment.append(el);
    };
    for (let r = r1; r <= r2; r++) {
      if (!rowPx(s, r)) continue;
      for (let c = c1; c <= c2; c++) {
        const ref = refName(r, c);
        const merge = s.covered.get(ref);
        if (merge) {
          const key = `${merge.r1}:${merge.c1}`;
          if (drawn.has(key)) continue;
          drawn.add(key);
          const box = rect(merge.r1, merge.c1, merge.r2, merge.c2);
          const blank = document.createElement('div');
          blank.className = 'xlsx-merge';
          blank.style.cssText = `left:${box.left}px;top:${box.top}px;width:${box.width}px;height:${box.height}px`;
          fragment.append(blank);
          drawCell(merge.r1, merge.c1, merge);
          continue;
        }
        drawCell(r, c, null);
      }
    }
    // Headers stay pinned to the top and left edges.
    const range = selectedRange();
    for (let c = c1; c <= c2; c++) {
      const head = document.createElement('div');
      head.className = 'xlsx-col-head';
      if (c >= range.c1 && c <= range.c2) head.classList.add('is-active');
      head.textContent = columnName(c);
      head.style.cssText = `left:${geo.colX[c]}px;top:${top}px;width:${geo.colX[c + 1] - geo.colX[c]}px;height:${HEAD_H}px`;
      fragment.append(head);
    }
    for (let r = r1; r <= r2; r++) {
      const h = geo.rowY[r + 1] - geo.rowY[r];
      if (!h) continue;
      const head = document.createElement('div');
      head.className = 'xlsx-row-head';
      if (r >= range.r1 && r <= range.r2) head.classList.add('is-active');
      head.textContent = String(r);
      head.style.cssText = `left:${left}px;top:${geo.rowY[r]}px;width:${HEAD_W}px;height:${h}px`;
      fragment.append(head);
    }
    const corner = document.createElement('div');
    corner.className = 'xlsx-corner';
    corner.title = 'Select all';
    corner.style.cssText = `left:${left}px;top:${top}px;width:${HEAD_W}px;height:${HEAD_H}px`;
    fragment.append(corner);
    layer.replaceChildren(fragment);
    placeSelection();
    placePoint();
  }
  const schedule = () => { if (!frame) frame = requestAnimationFrame(render); };

  // Selection
  function selectedRange() {
    return {
      r1: Math.min(anchor.row, focus.row), r2: Math.max(anchor.row, focus.row),
      c1: Math.min(anchor.col, focus.col), c2: Math.max(anchor.col, focus.col),
    };
  }
  function expandForMerges(range) {
    const s = sheet();
    if (!s.merges.length) return range;
    let changed = true;
    while (changed) {
      changed = false;
      for (const m of s.merges) {
        if (m.r2 < range.r1 || m.r1 > range.r2 || m.c2 < range.c1 || m.c1 > range.c2) continue;
        if (m.r1 < range.r1 || m.r2 > range.r2 || m.c1 < range.c1 || m.c2 > range.c2) {
          range = { r1: Math.min(range.r1, m.r1), r2: Math.max(range.r2, m.r2), c1: Math.min(range.c1, m.c1), c2: Math.max(range.c2, m.c2) };
          changed = true;
        }
      }
    }
    return range;
  }
  function placeSelection() {
    if (!geo) return;
    const range = expandForMerges(selectedRange());
    const box = rect(range.r1, range.c1, Math.min(range.r2, geo.rows), Math.min(range.c2, geo.cols));
    selectionBox.style.cssText = `left:${box.left - 1}px;top:${box.top - 1}px;width:${box.width + 1}px;height:${box.height + 1}px`;
    selectionBox.hidden = range.r1 === range.r2 && range.c1 === range.c2 && !sheet().covered.has(refName(anchor.row, anchor.col));
    const merge = sheet().covered.get(refName(anchor.row, anchor.col));
    const cellBox = merge ? rect(merge.r1, merge.c1, merge.r2, merge.c2) : rect(anchor.row, anchor.col, anchor.row, anchor.col);
    activeBox.style.cssText = `left:${cellBox.left - 1}px;top:${cellBox.top - 1}px;width:${cellBox.width + 1}px;height:${cellBox.height + 1}px`;
  }
  function editText(row, col) {
    const s = sheet();
    const cell = s.cells.get(refName(row, col));
    if (!cell) return '';
    const formula = formulaOf(s, cell);
    if (formula != null) return `=${formula}`;
    const value = cachedValue(model, cell);
    if (value == null) return '';
    if (value instanceof CellError) return value.code;
    if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE';
    if (typeof value === 'number') {
      const code = cellStyle(model, cell.getAttribute('s')).numFmt;
      if (isDateFormat(code)) {
        try {
          const p = serialParts(value, model.date1904);
          const date = `${p.year}-${String(p.month).padStart(2, '0')}-${String(p.day).padStart(2, '0')}`;
          const time = `${String(p.hours).padStart(2, '0')}:${String(p.minutes).padStart(2, '0')}${p.seconds ? `:${String(p.seconds).padStart(2, '0')}` : ''}`;
          if (value < 1 && value >= 0) return time;
          return value % 1 ? `${date} ${time}` : date;
        } catch { /* Out-of-range dates edit as numbers. */ }
      }
      const shown = /%/.test(code.replace(/"[^"]*"/g, '')) ? `${formatGeneral(Number((value * 100).toPrecision(15)), separators)}%` : formatGeneral(value, separators);
      return shown;
    }
    if (typeof value === 'string' && parseInput(value, separators).kind !== 'text') return `'${value}`;
    return String(value);
  }
  function selectionChanged() {
    const ref = refName(anchor.row, anchor.col);
    const range = selectedRange();
    address.textContent = range.r1 === range.r2 && range.c1 === range.c2 ? ref : `${refName(range.r1, range.c1)}:${refName(Math.min(range.r2, LAST_ROW), Math.min(range.c2, LAST_COL))}`;
    if (!editing) formulaInput.value = editText(anchor.row, anchor.col);
    formulaInput.readOnly = !canWrite();
    const info = shown(anchor.row, anchor.col);
    live.textContent = `${ref}${info?.text ? `, ${info.text}` : ', empty'}`;
    updateSummary();
    updateToolbar();
    schedule();
  }
  function updateSummary() {
    const range = selectedRange();
    if (range.r1 === range.r2 && range.c1 === range.c2) { summary.textContent = ''; return; }
    let count = 0, numbers = 0, sum = 0;
    const s = sheet();
    for (const [ref] of s.cells) {
      const { row, col } = parseRef(ref);
      if (row < range.r1 || row > range.r2 || col < range.c1 || col > range.c2) continue;
      const value = valueAt(model, active, row, col);
      if (value == null || value === '') continue;
      count++;
      if (typeof value === 'number') { numbers++; sum += value; }
    }
    const show = value => formatGeneral(Number(value.toPrecision(12)), separators);
    summary.textContent = numbers ? `Average: ${show(sum / numbers)}   Count: ${count}   Sum: ${show(sum)}` : count ? `Count: ${count}` : '';
  }
  function scrollIntoView(row, col) {
    layout();
    const box = rect(row, col, row, col);
    const viewW = scroll.clientWidth, viewH = scroll.clientHeight;
    if (box.left - HEAD_W < scroll.scrollLeft) scroll.scrollLeft = box.left - HEAD_W;
    else if (box.left + box.width > scroll.scrollLeft + viewW) scroll.scrollLeft = box.left + box.width - viewW;
    if (box.top - HEAD_H < scroll.scrollTop) scroll.scrollTop = box.top - HEAD_H;
    else if (box.top + box.height > scroll.scrollTop + viewH) scroll.scrollTop = box.top + box.height - viewH;
  }
  function select(row, col, extend = false) {
    row = Math.max(1, Math.min(LAST_ROW, row));
    col = Math.max(1, Math.min(LAST_COL, col));
    if (extend) focus = { row, col };
    else {
      const merge = sheet().covered.get(refName(row, col));
      anchor = merge ? { row: merge.r1, col: merge.c1 } : { row, col };
      focus = { ...anchor };
    }
    layout();
    scrollIntoView(extend ? focus.row : anchor.row, extend ? focus.col : anchor.col);
    selectionChanged();
  }
  function move(dr, dc, extend = false) {
    const from = extend ? focus : anchor;
    let row = from.row + dr, col = from.col + dc;
    const s = sheet();
    // Skip hidden rows and columns, and step over merged cells.
    while (dr && row >= 1 && row < LAST_ROW && !rowPx(s, row)) row += dr;
    while (dc && col >= 1 && col < LAST_COL && !colPx(s, col)) col += dc;
    const merge = !extend && s.covered.get(refName(from.row, from.col));
    if (merge) {
      if (dr > 0) row = merge.r2 + 1;
      if (dc > 0) col = merge.c2 + 1;
    }
    select(row, col, extend);
  }
  function filled(row, col) {
    const value = valueAt(model, active, row, col);
    return value != null && value !== '';
  }
  /** Ctrl+arrow: to the edge of the current block of data, or the next one. */
  function jump(dr, dc, extend) {
    const from = extend ? focus : anchor;
    const s = sheet();
    const limitRow = Math.max(s.maxRow, from.row), limitCol = Math.max(s.maxCol, from.col);
    let row = from.row, col = from.col;
    const inside = (r, c) => r >= 1 && c >= 1 && r <= limitRow && c <= limitCol;
    if (filled(row, col) && inside(row + dr, col + dc) && filled(row + dr, col + dc)) {
      while (inside(row + dr, col + dc) && filled(row + dr, col + dc)) { row += dr; col += dc; }
    } else {
      do { row += dr; col += dc; } while (inside(row, col) && !filled(row, col));
      if (!inside(row, col)) { row = Math.max(1, Math.min(row, limitRow)); col = Math.max(1, Math.min(col, limitCol)); }
    }
    select(row, col, extend);
  }

  // Edits, with undo
  function transaction(work) {
    const s = sheet();
    const before = new Map();
    const touch = ref => { if (!before.has(ref)) before.set(ref, s.cells.get(ref)?.cloneNode(true) || null); };
    work(touch);
    const items = [...before].map(([ref, copy]) => ({ ref, before: copy, after: s.cells.get(ref)?.cloneNode(true) || null }))
      .filter(item => (item.before ? serialize(item.before) : '') !== (item.after ? serialize(item.after) : ''));
    if (!items.length) return;
    if (savedAt > historyAt) savedAt = -1;
    history.splice(historyAt);
    history.push({ sheet: active, items });
    historyAt = history.length;
    if (history.length > 200) { history.shift(); historyAt--; savedAt--; }
    changed();
  }
  function changed() {
    model.edited = true;
    model.touched.add(sheet());
    geo = null;
    recalc(model);
    onChange(dirty());
    updateNote();
    selectionChanged();
  }
  function step(direction) {
    if (saving || editing) return;
    const entry = direction < 0 ? history[historyAt - 1] : history[historyAt];
    if (!entry) return;
    if (entry.sheet !== active) showSheet(entry.sheet);
    const s = sheet();
    for (const item of entry.items) placeCell(s, item.ref, direction < 0 ? item.before : item.after);
    historyAt += direction;
    const first = parseRef(entry.items[0].ref);
    anchor = { row: first.row, col: first.col }; focus = { ...anchor };
    model.touched.add(s);
    changed();
    scrollIntoView(anchor.row, anchor.col);
  }
  /** Turns a shared formula group into plain formulas, so one of its cells can change alone. */
  function unshare(s, cell, touch) {
    const f = direct(cell, 'f');
    if (f?.getAttribute('t') !== 'shared') return;
    const si = f.getAttribute('si');
    for (const [ref, member] of s.cells) {
      const mf = direct(member, 'f');
      if (mf?.getAttribute('t') !== 'shared' || mf.getAttribute('si') !== si) continue;
      touch(ref);
      const formula = formulaOf(s, member);
      mf.textContent = formula ?? '';
      for (const name of ['t', 'si', 'ref']) mf.removeAttribute(name);
    }
    s.shared.delete(si);
  }
  function writeInput(s, row, col, text, touch, styleIndex = null) {
    const ref = refName(row, col);
    const existing = s.cells.get(ref);
    const parsed = parseInput(text, { ...separators, date1904: model.date1904 });
    if (parsed.kind === 'empty' && !existing && styleIndex == null) return;
    touch(ref);
    if (existing) unshare(s, existing, touch);
    const cell = cellElement(s, row, col);
    if (styleIndex != null) {
      if (styleIndex) cell.setAttribute('s', String(styleIndex)); else cell.removeAttribute('s');
    }
    for (const child of [...cell.children]) child.remove();
    cell.removeAttribute('t');
    const doc = s.xml;
    if (parsed.kind === 'formula') {
      const f = doc.createElementNS(MAIN, 'f');
      f.textContent = parsed.value;
      cell.append(f);
    } else if (parsed.kind === 'number') {
      const v = doc.createElementNS(MAIN, 'v');
      v.textContent = String(parsed.value);
      cell.append(v);
      const current = cellStyle(model, cell.getAttribute('s'));
      const keep = parsed.format && (isDateFormat(parsed.format) ? isDateFormat(current.numFmt) : /%/.test(current.numFmt));
      if (parsed.format && !keep && model.styles && styleIndex == null) cell.setAttribute('s', String(restyle(model, cell.getAttribute('s'), { numFmt: parsed.format })));
    } else if (parsed.kind === 'bool') {
      cell.setAttribute('t', 'b');
      const v = doc.createElementNS(MAIN, 'v');
      v.textContent = parsed.value ? '1' : '0';
      cell.append(v);
    } else if (parsed.kind === 'text') {
      cell.setAttribute('t', 'inlineStr');
      const is = doc.createElementNS(MAIN, 'is');
      const t = doc.createElementNS(MAIN, 't');
      if (/^\s|\s$|\n/.test(parsed.value)) t.setAttributeNS(XML, 'xml:space', 'preserve');
      t.textContent = parsed.value;
      is.append(t);
      cell.append(is);
    } else if (!cell.getAttribute('s') || cell.getAttribute('s') === '0') {
      cell.remove();
      s.cells.delete(ref);
    }
  }
  /** The cells to change for the selection: whole rows or columns stop at the used area. */
  function selectionCells({ create }) {
    const s = sheet();
    const range = expandForMerges(selectedRange());
    const r2 = Math.min(range.r2, Math.max(s.maxRow, range.r1)), c2 = Math.min(range.c2, Math.max(s.maxCol, range.c1));
    const list = [];
    if (create && (r2 - range.r1 + 1) * (c2 - range.c1 + 1) <= MAX_FORMAT_CELLS) {
      for (let r = range.r1; r <= r2; r++) for (let c = range.c1; c <= c2; c++) {
        const merge = s.covered.get(refName(r, c));
        if (merge && (merge.r1 !== r || merge.c1 !== c)) continue;
        list.push({ row: r, col: c });
      }
      return list;
    }
    for (const ref of s.cells.keys()) {
      const { row, col } = parseRef(ref);
      if (row >= range.r1 && row <= r2 && col >= range.c1 && col <= c2) list.push({ row, col });
    }
    return list;
  }
  function formatSelection(change) {
    if (!canWrite() || !model.styles) return;
    commitEdit();
    const s = sheet();
    transaction(touch => {
      for (const { row, col } of selectionCells({ create: true })) {
        const ref = refName(row, col);
        touch(ref);
        const cell = cellElement(s, row, col);
        const index = restyle(model, cell.getAttribute('s'), change);
        if (index) cell.setAttribute('s', String(index)); else cell.removeAttribute('s');
      }
    });
    scroll.focus({ preventScroll: true });
  }
  function clearSelection() {
    if (!canWrite()) return;
    const s = sheet();
    transaction(touch => {
      for (const { row, col } of selectionCells({ create: false })) {
        const cell = s.cells.get(refName(row, col));
        if (cell && (cell.children.length || cell.hasAttribute('t'))) writeInput(s, row, col, '', touch);
      }
    });
  }

  // In-cell editing
  function startEdit(initial = null) {
    if (!canWrite() || editing) return;
    const merge = sheet().covered.get(refName(anchor.row, anchor.col));
    const box = merge ? rect(merge.r1, merge.c1, merge.r2, merge.c2) : rect(anchor.row, anchor.col, anchor.row, anchor.col);
    const original = editText(anchor.row, anchor.col);
    editing = { row: anchor.row, col: anchor.col, original, typed: initial != null, sheet: active };
    const style = activeStyle();
    cellEditor.value = initial ?? original;
    cellEditor.style.cssText = `left:${box.left}px;top:${box.top}px;min-width:${Math.max(box.width, 60)}px;height:${Math.max(box.height, 22)}px;${style.bold ? 'font-weight:700;' : ''}${style.italic ? 'font-style:italic;' : ''}`;
    cellEditor.hidden = false;
    fitEditor();
    cellEditor.focus({ preventScroll: true });
    cellEditor.setSelectionRange(cellEditor.value.length, cellEditor.value.length);
    formulaInput.value = cellEditor.value;
  }
  function fitEditor() {
    cellEditor.style.width = '0px';
    cellEditor.style.width = `${Math.min(600, cellEditor.scrollWidth + 8)}px`;
  }
  function commitEdit(dr = 0, dc = 0) {
    if (!editing) return;
    const source = editing.bar ? formulaInput : cellEditor;
    const { row, col, original, sheet: index } = editing;
    const value = source.value;
    editing = null;
    cellEditor.hidden = true;
    hideAssist();
    placePoint();
    if (value !== original && index === active) {
      const s = sheet();
      transaction(touch => writeInput(s, row, col, value, touch));
    }
    if (dr || dc) move(dr, dc); else selectionChanged();
    scroll.focus({ preventScroll: true });
  }
  function cancelEdit() {
    if (!editing) return;
    editing = null;
    cellEditor.hidden = true;
    hideAssist();
    placePoint();
    formulaInput.value = editText(anchor.row, anchor.col);
    scroll.focus({ preventScroll: true });
  }
  // Typing a formula: suggestions, argument hints, and references put in by pointing at cells.
  // Shortest first, so the common ones (SUM before SUBSTITUTE) come first.
  const FUNCTION_NAMES = Object.keys(FUNCTION_HELP).sort((a, b) => a.length - b.length || a.localeCompare(b));
  let suggestions = [], suggested = 0, suggestionMoved = false;
  const editorInput = () => (document.activeElement === formulaInput || editing?.bar ? formulaInput : cellEditor);
  /** Copies what `input` holds to the other place the edit shows. */
  function syncEditors(input) {
    if (input === cellEditor) formulaInput.value = cellEditor.value;
    else if (!editing?.bar) cellEditor.value = formulaInput.value;
    if (!cellEditor.hidden) fitEditor();
  }
  function hideAssist() {
    assist.hidden = true;
    suggestions = [];
    suggested = 0;
    suggestionMoved = false;
  }
  function updateAssist() {
    const input = editorInput();
    if (!editing || document.activeElement !== input || input.selectionStart !== input.selectionEnd) { hideAssist(); return; }
    const caret = input.selectionStart;
    const typed = formulaWordAt(input.value, caret);
    const word = typed ? typed.word.toUpperCase() : '';
    const found = typed ? FUNCTION_NAMES.filter(name => name.startsWith(word) && !(name === word && input.value[caret] === '(')) : [];
    if (found.join() !== suggestions.join()) { suggested = 0; suggestionMoved = false; }
    suggestions = found;
    const call = suggestions.length ? null : callAt(input.value, caret);
    const help = call && FUNCTION_HELP[call.name];
    if (!suggestions.length && !help) { assist.hidden = true; return; }
    if (suggestions.length) {
      assist.setAttribute('role', 'listbox');
      assist.replaceChildren(...suggestions.map((name, index) => {
        const item = document.createElement('div');
        item.className = 'xlsx-assist-item';
        item.setAttribute('role', 'option');
        item.setAttribute('aria-selected', String(index === suggested));
        const label = document.createElement('span');
        label.className = 'xlsx-assist-name';
        label.textContent = name;
        const about = document.createElement('span');
        about.className = 'xlsx-assist-about';
        about.textContent = FUNCTION_HELP[name][1];
        item.append(label, about);
        item.addEventListener('pointerdown', event => {
          event.preventDefault();
          suggested = index;
          acceptSuggestion();
        });
        return item;
      }));
    } else {
      assist.setAttribute('role', 'tooltip');
      const [args, about] = help;
      const parts = args ? args.split(', ') : [];
      // Past the last named argument, the one before "…" repeats.
      let current = Math.min(call.arg, parts.length - 1);
      if (parts[current] === '…') current--;
      const line = document.createElement('div');
      line.className = 'xlsx-assist-call';
      line.append(`${call.name}(`);
      parts.forEach((part, index) => {
        if (index) line.append(', ');
        const shown = document.createElement(index === current ? 'b' : 'span');
        shown.textContent = part;
        line.append(shown);
      });
      line.append(')');
      const text = document.createElement('p');
      text.textContent = about;
      line.append(text);
      assist.replaceChildren(line);
    }
    assist.hidden = false;
    const box = input.getBoundingClientRect();
    const width = assist.offsetWidth, height = assist.offsetHeight;
    assist.style.left = `${Math.max(8, Math.min(box.left, window.innerWidth - width - 8))}px`;
    assist.style.top = `${box.bottom + 4 + height > window.innerHeight - 8 ? Math.max(8, box.top - height - 4) : box.bottom + 4}px`;
    assist.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' });
  }
  function acceptSuggestion() {
    const input = editorInput();
    const caret = input.selectionStart;
    const typed = formulaWordAt(input.value, caret);
    const name = suggestions[suggested];
    if (!typed || !name) return;
    const paren = input.value[caret] === '(';
    input.setRangeText(paren ? name : `${name}(`, typed.start, caret, 'end');
    if (paren) input.setSelectionRange(input.selectionEnd + 1, input.selectionEnd + 1);
    syncEditors(input);
    hideAssist();
    updateAssist();
  }
  /** Keys for the suggestions: arrows choose, Tab (or Enter, once one was chosen) puts it in. */
  function assistKey(event) {
    if (assist.hidden || !suggestions.length) return false;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      suggested = (suggested + (event.key === 'ArrowDown' ? 1 : -1) + suggestions.length) % suggestions.length;
      suggestionMoved = true;
      updateAssist();
    } else if (event.key === 'Tab' || (event.key === 'Enter' && suggestionMoved)) {
      acceptSuggestion();
    } else if (event.key === 'Escape') {
      event.stopPropagation();
      hideAssist();
    } else {
      return false;
    }
    event.preventDefault();
    return true;
  }

  /** The reference last put in by pointing, while the caret is still just after it. */
  function activePoint() {
    const point = editing?.point;
    if (!point) return null;
    const input = editorInput();
    return input.selectionStart === point.end && input.selectionEnd === point.end && input.value.slice(point.start, point.end) === point.text ? point : null;
  }
  const canPoint = () => editing && editing.sheet === active && (activePoint() || canPointAt(editorInput().value, editorInput().selectionStart));
  /** Puts the reference of a cell (or, extending, of the range from where pointing started) at
   * the caret, in place of the one pointed at before. */
  function pointTo(row, col, extend = false) {
    const input = editorInput();
    const before = activePoint();
    row = Math.max(1, Math.min(LAST_ROW, row));
    col = Math.max(1, Math.min(LAST_COL, col));
    const merge = !extend && sheet().covered.get(refName(row, col));
    if (merge) { row = merge.r1; col = merge.c1; }
    const from = extend && before ? before.from : { row, col };
    const range = { r1: Math.min(from.row, row), c1: Math.min(from.col, col), r2: Math.max(from.row, row), c2: Math.max(from.col, col) };
    const text = range.r1 === range.r2 && range.c1 === range.c2 ? refName(row, col) : `${refName(range.r1, range.c1)}:${refName(range.r2, range.c2)}`;
    const start = before ? before.start : input.selectionStart;
    input.setRangeText(text, start, before ? before.end : input.selectionEnd, 'end');
    editing.point = { start, end: start + text.length, text, from, cell: { row, col }, range };
    syncEditors(input);
    input.focus({ preventScroll: true });
    placePoint();
    scrollIntoView(row, col);
    updateAssist();
  }
  function placePoint() {
    const point = editing && editing.sheet === active ? activePoint() : null;
    if (!point || !geo) { pointBox.hidden = true; return; }
    const { r1, c1, r2, c2 } = point.range;
    const box = rect(Math.min(r1, geo.rows), Math.min(c1, geo.cols), Math.min(r2, geo.rows), Math.min(c2, geo.cols));
    pointBox.style.cssText = `left:${box.left - 1}px;top:${box.top - 1}px;width:${box.width + 1}px;height:${box.height + 1}px`;
    pointBox.hidden = false;
  }
  for (const input of [cellEditor, formulaInput]) {
    input.addEventListener('keyup', event => { if (event.key !== 'Escape') { updateAssist(); placePoint(); } });
    input.addEventListener('click', () => { updateAssist(); placePoint(); });
    input.addEventListener('blur', () => setTimeout(() => { if (document.activeElement !== cellEditor && document.activeElement !== formulaInput) hideAssist(); }, 0));
  }

  cellEditor.addEventListener('input', () => { formulaInput.value = cellEditor.value; fitEditor(); placePoint(); updateAssist(); });
  cellEditor.addEventListener('keydown', event => {
    if (assistKey(event)) return;
    const arrow = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[event.key];
    // Arrows put a reference in, as clicking does, where one can go in a formula typed afresh
    // (or move the one just put in).
    if (arrow && !event.altKey && !event.ctrlKey && !event.metaKey && (editing?.typed || activePoint()) && canPoint()) {
      event.preventDefault();
      const from = activePoint()?.cell || { row: editing.row, col: editing.col };
      pointTo(from.row + arrow[0], from.col + arrow[1], event.shiftKey && !!activePoint());
      return;
    }
    if (event.key === 'Enter' && event.altKey) {
      event.preventDefault();
      cellEditor.setRangeText('\n', cellEditor.selectionStart, cellEditor.selectionEnd, 'end');
      formulaInput.value = cellEditor.value;
      return;
    }
    if (event.key === 'Enter') { event.preventDefault(); commitEdit(event.shiftKey ? -1 : 1, 0); }
    else if (event.key === 'Tab') { event.preventDefault(); commitEdit(0, event.shiftKey ? -1 : 1); }
    else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); cancelEdit(); }
    else if (editing?.typed && event.key.startsWith('Arrow') && !cellEditor.value.startsWith('=')) {
      event.preventDefault();
      const [dr, dc] = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[event.key];
      commitEdit(dr, dc);
    }
  });
  cellEditor.addEventListener('blur', () => { if (editing && !destroyed) setTimeout(() => { if (editing && document.activeElement !== cellEditor && document.activeElement !== formulaInput) commitEdit(); }, 0); });

  formulaInput.addEventListener('focus', () => {
    if (!canWrite() || editing) return;
    editing = { row: anchor.row, col: anchor.col, original: editText(anchor.row, anchor.col), typed: false, sheet: active, bar: true };
  });
  formulaInput.addEventListener('input', () => {
    if (editing && !editing.bar) { cellEditor.value = formulaInput.value; fitEditor(); }
    placePoint();
    updateAssist();
  });
  formulaInput.addEventListener('keydown', event => {
    if (!editing) return;
    if (assistKey(event)) return;
    if (event.key === 'Enter') { event.preventDefault(); commitEdit(event.shiftKey ? -1 : 1, 0); }
    else if (event.key === 'Tab') { event.preventDefault(); commitEdit(0, event.shiftKey ? -1 : 1); }
    else if (event.key === 'Escape') { event.preventDefault(); cancelEdit(); }
  });
  formulaInput.addEventListener('blur', () => {
    setTimeout(() => { if (editing?.bar && document.activeElement !== formulaInput) commitEdit(); }, 0);
  });

  // Pointer
  function pointAt(event) {
    const box = canvas.getBoundingClientRect();
    const x = event.clientX - box.left, y = event.clientY - box.top;
    const head = { col: x - scroll.scrollLeft < HEAD_W, row: y - scroll.scrollTop < HEAD_H };
    return {
      x, y, head,
      row: find(geo.rowY, geo.rows, Math.max(HEAD_H, y)),
      col: find(geo.colX, geo.cols, Math.max(HEAD_W, x)),
    };
  }
  canvas.addEventListener('pointerdown', event => {
    if (event.button !== 0 || event.target === cellEditor) return;
    layout();
    const p = pointAt(event);
    // Typing a formula where a reference can go: the cell clicked goes into it.
    if (editing && !p.head.row && !p.head.col && canPoint()) {
      event.preventDefault();
      pointTo(p.row, p.col, event.shiftKey);
      dragging = 'point';
      canvas.setPointerCapture?.(event.pointerId);
      return;
    }
    if (editing) commitEdit();
    if (p.head.col && p.head.row) {
      anchor = { row: 1, col: 1 }; focus = { row: LAST_ROW, col: LAST_COL };
      dragging = null;
    } else if (p.head.row) {
      if (!event.shiftKey) anchor = { row: 1, col: p.col };
      focus = { row: LAST_ROW, col: p.col };
      dragging = 'col';
    } else if (p.head.col) {
      if (!event.shiftKey) anchor = { row: p.row, col: 1 };
      focus = { row: p.row, col: LAST_COL };
      dragging = 'row';
    } else {
      select(p.row, p.col, event.shiftKey);
      dragging = 'cell';
    }
    canvas.setPointerCapture?.(event.pointerId);
    event.preventDefault();
    scroll.focus({ preventScroll: true });
    selectionChanged();
  });
  canvas.addEventListener('pointermove', event => {
    if (!dragging) return;
    const p = pointAt(event);
    if (dragging === 'point') {
      const point = activePoint();
      if (point && (point.cell.row !== p.row || point.cell.col !== p.col)) pointTo(p.row, p.col, true);
      return;
    }
    if (dragging === 'cell') focus = { row: p.row, col: p.col };
    else if (dragging === 'col') focus = { row: LAST_ROW, col: p.col };
    else focus = { row: p.row, col: LAST_COL };
    selectionChanged();
  });
  const stopDrag = () => { dragging = null; };
  // Pointing keeps the keyboard in the formula.
  canvas.addEventListener('mousedown', event => { if (dragging === 'point') event.preventDefault(); });
  canvas.addEventListener('pointerup', stopDrag);
  canvas.addEventListener('pointercancel', stopDrag);
  canvas.addEventListener('dblclick', event => {
    if (event.target === cellEditor) return;
    const p = pointAt(event);
    if (!p.head.row && !p.head.col) startEdit();
  });

  // Keyboard
  scroll.addEventListener('keydown', event => {
    if (event.target !== scroll) return;
    const mod = event.ctrlKey || event.metaKey;
    const key = event.key;
    // The file list's own keys (Delete to trash, select all, copy files…) stay out of the grid.
    // Escape still closes the preview, and the page's ⌘⇧ shortcuts run before this.
    if (key !== 'Escape') event.stopPropagation();
    const arrows = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
    if (arrows[key]) {
      event.preventDefault();
      const [dr, dc] = arrows[key];
      if (mod) jump(dr, dc, event.shiftKey); else move(dr, dc, event.shiftKey);
    } else if (key === 'Enter') { event.preventDefault(); move(event.shiftKey ? -1 : 1, 0); }
    else if (key === 'Tab') { event.preventDefault(); move(0, event.shiftKey ? -1 : 1); }
    else if (key === 'Home') { event.preventDefault(); if (mod) select(1, 1); else select(anchor.row, 1, event.shiftKey); }
    else if (key === 'End' && mod) { event.preventDefault(); select(Math.max(1, sheet().maxRow), Math.max(1, sheet().maxCol)); }
    else if (key === 'PageDown' || key === 'PageUp') {
      event.preventDefault();
      const rows = Math.max(1, Math.floor(scroll.clientHeight / (sheet().defaultRowPx || 20)) - 1);
      move(key === 'PageDown' ? rows : -rows, 0, event.shiftKey);
    } else if (key === 'F2') { event.preventDefault(); startEdit(); }
    else if (key === 'Delete' || key === 'Backspace') { event.preventDefault(); clearSelection(); }
    else if (mod && key.toLowerCase() === 'a') { event.preventDefault(); anchor = { row: 1, col: 1 }; focus = { row: LAST_ROW, col: LAST_COL }; selectionChanged(); }
    else if (mod && key.toLowerCase() === 'b') { event.preventDefault(); formatSelection({ bold: !activeStyle().bold }); }
    else if (mod && key.toLowerCase() === 'i') { event.preventDefault(); formatSelection({ italic: !activeStyle().italic }); }
    else if (!mod && !event.altKey && key.length === 1) {
      if (!canWrite()) return;
      event.preventDefault();
      startEdit(key);
    }
  });
  // Capturing, so these work while the grid (which stops its keys bubbling) has focus.
  root.addEventListener('keydown', event => {
    const mod = event.ctrlKey || event.metaKey;
    if (!mod) return;
    const key = event.key.toLowerCase();
    if (key === 's') {
      event.preventDefault(); event.stopPropagation();
      if (editing) commitEdit();
      persist().catch(() => {});
    } else if ((key === 'z' || key === 'y') && !editing && event.target !== formulaInput) {
      event.preventDefault(); event.stopPropagation();
      step(key === 'y' || event.shiftKey ? 1 : -1);
    }
  }, true);

  // Clipboard
  const gridFocused = () => document.activeElement === scroll;
  function copyText() {
    const s = sheet();
    const range = selectedRange();
    const r2 = Math.min(range.r2, Math.max(s.maxRow, range.r1)), c2 = Math.min(range.c2, Math.max(s.maxCol, range.c1));
    const lines = [], cells = [];
    for (let r = range.r1; r <= r2; r++) {
      const line = [], cellRow = [];
      for (let c = range.c1; c <= c2; c++) {
        const text = shown(r, c)?.text ?? '';
        line.push(/[\t\n"]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text);
        const cell = s.cells.get(refName(r, c));
        cellRow.push({ input: editText(r, c), style: cell?.getAttribute('s') || null, formula: formulaOf(s, cell) });
      }
      lines.push(line.join('\t'));
      cells.push(cellRow);
    }
    const text = lines.join('\n');
    clipboard = { text, cells, row: range.r1, col: range.c1 };
    return text;
  }
  function parseClipboard(text) {
    const rows = [];
    let row = [], field = '', quoted = false;
    for (let i = 0; i < text.length; i++) {
      const char = text[i];
      if (quoted) {
        if (char === '"' && text[i + 1] === '"') { field += '"'; i++; }
        else if (char === '"') quoted = false;
        else field += char;
      } else if (char === '"' && field === '') quoted = true;
      else if (char === '\t') { row.push(field); field = ''; }
      else if (char === '\n' || char === '\r') {
        if (char === '\r' && text[i + 1] === '\n') i++;
        row.push(field); rows.push(row); row = []; field = '';
      } else field += char;
    }
    if (field !== '' || row.length) { row.push(field); rows.push(row); }
    return rows;
  }
  document.addEventListener('copy', onCopy);
  document.addEventListener('cut', onCut);
  document.addEventListener('paste', onPaste);
  function onCopy(event) {
    if (!gridFocused()) return;
    event.preventDefault();
    event.clipboardData.setData('text/plain', copyText());
  }
  function onCut(event) {
    if (!gridFocused()) return;
    onCopy(event);
    clearSelection();
  }
  function onPaste(event) {
    if (!gridFocused() || !canWrite()) return;
    event.preventDefault();
    const text = event.clipboardData.getData('text/plain');
    const s = sheet();
    const range = selectedRange();
    const internal = clipboard && clipboard.text === text.replace(/\r\n/g, '\n').replace(/\n$/, '') ? clipboard : null;
    const grid = internal ? internal.cells : parseClipboard(text.replace(/\r\n$|\n$/, '')).map(row => row.map(input => ({ input })));
    if (!grid.length) return;
    const height = grid.length, width = Math.max(...grid.map(row => row.length));
    // One copied cell fills the whole selection.
    const fill = height === 1 && width === 1 && (range.r1 !== range.r2 || range.c1 !== range.c2);
    const rows = fill ? Math.min(range.r2, Math.max(s.maxRow, range.r1)) - range.r1 + 1 : height;
    const cols = fill ? Math.min(range.c2, Math.max(s.maxCol, range.c1)) - range.c1 + 1 : width;
    if (rows * cols > MAX_FORMAT_CELLS) return;
    transaction(touch => {
      for (let r = 0; r < rows; r++) {
        for (let c = 0; c < cols; c++) {
          const item = fill ? grid[0][0] : grid[r][c];
          if (!item) continue;
          const row = range.r1 + r, col = range.c1 + c;
          let input = item.input ?? '';
          if (internal && item.formula != null) {
            const source = { row: internal.row + (fill ? 0 : r), col: internal.col + (fill ? 0 : c) };
            input = `=${shiftFormula(item.formula, row - source.row, col - source.col)}`;
          }
          writeInput(s, row, col, input, touch, internal ? Number(item.style || 0) : null);
        }
      }
    });
    focus = { row: range.r1 + rows - 1, col: range.c1 + cols - 1 };
    selectionChanged();
  }

  // Saving
  async function persist() {
    if (readonly || !dirty() || saving || !onSave) return;
    commitEdit();
    saving = true;
    const target = historyAt;
    updateToolbar();
    try {
      await onSave(model.export());
      savedAt = target;
      onChange(dirty());
    } catch (error) {
      status.textContent = error?.message || 'Could not save';
      throw error;
    } finally {
      saving = false;
      if (!destroyed) {
        updateToolbar();
        if (dirty() && status.textContent === 'Saved') status.textContent = 'Unsaved changes';
      }
    }
  }

  scroll.addEventListener('scroll', schedule, { passive: true });
  const resize = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(schedule) : null;
  resize?.observe(scroll);
  drawTabs();
  layout();
  updateNote();
  selectionChanged();
  render();
  return {
    destroy() {
      destroyed = true;
      resize?.disconnect();
      document.removeEventListener('copy', onCopy);
      document.removeEventListener('cut', onCut);
      document.removeEventListener('paste', onPaste);
      if (frame) cancelAnimationFrame(frame);
      root.remove();
    },
    save: persist,
    get dirty() { return dirty(); },
  };
}
