/**
 * Preservation-first XLSX workbook editor. It edits cell XML in place and keeps every other part of
 * the package, shows cells with their number formats, fonts, fills, column widths and merges, and
 * recalculates the formulas it understands (see sheet-formula.js). Formulas it can't calculate keep
 * their saved values and are recalculated by the spreadsheet app on open.
 */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';
import {
  BUILTIN_FORMATS, CellError, FUNCTION_HELP, Unsupported, callAt, canPointAt, columnName, dateSerial, evaluate,
  formatGeneral, formatValue, formulaWordAt, fromFileFormula, toFileFormula, localeSeparators, moveFormula, moveSpan, moveSqref, parseFormula, parseRef,
  refName, serialParts, shiftFormula,
} from './sheet-formula.js';

const MAIN = 'http://schemas.openxmlformats.org/spreadsheetml/2006/main';
const DOC_REL = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PKG_REL = 'http://schemas.openxmlformats.org/package/2006/relationships';
const CONTENT_TYPES = 'http://schemas.openxmlformats.org/package/2006/content-types';
const DRAWING = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const SHEET_DRAWING = 'http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing';
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
const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent || '');
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
    const border = kids(direct(styles.root, 'borders'), 'border')[Number(xf.getAttribute('borderId') || 0)];
    if (border) {
      for (const side of ['left', 'right', 'top', 'bottom']) {
        const node = direct(border, side) || direct(border, side === 'left' ? 'start' : side === 'right' ? 'end' : '');
        const kind = node?.getAttribute('style');
        if (kind && kind !== 'none') (style.borders ||= {})[side] = { kind, color: colorOf(direct(node, 'color'), styles.theme) || '#000000' };
      }
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

/** The style index for `base` with `change` applied: bold, italic, underline, strike, color, size,
 * font, fill, align, valign, wrap, borders ({ left, right, top, bottom }) or numFmt. */
function restyle(model, base, change) {
  const styles = model.styles;
  if (!styles) return base;
  const doc = styles.xml;
  const xfs = section(styles, 'cellXfs');
  const list = kids(xfs, 'xf');
  const xf = (list[Number(base) || 0] || list[0])?.cloneNode(true) || doc.createElementNS(MAIN, 'xf');
  if (['bold', 'italic', 'color', 'underline', 'strike', 'size', 'font'].some(key => key in change)) {
    const fonts = section(styles, 'fonts');
    const font = (kids(fonts, 'font')[Number(xf.getAttribute('fontId') || 0)] || kids(fonts, 'font')[0])?.cloneNode(true) || doc.createElementNS(MAIN, 'font');
    if ('bold' in change) setFontChild(font, 'b', change.bold && (() => {}));
    if ('italic' in change) setFontChild(font, 'i', change.italic && (() => {}));
    if ('color' in change) setFontChild(font, 'color', change.color && (node => node.setAttribute('rgb', `FF${change.color.slice(1).toUpperCase()}`)));
    if ('underline' in change) setFontChild(font, 'u', change.underline && (() => {}));
    if ('strike' in change) setFontChild(font, 'strike', change.strike && (() => {}));
    if ('size' in change) setFontChild(font, 'sz', node => node.setAttribute('val', String(change.size)));
    if ('font' in change) {
      setFontChild(font, 'name', node => node.setAttribute('val', change.font));
      // A theme font ("scheme") would win over the name.
      setFontChild(font, 'scheme', null);
      setFontChild(font, 'family', null);
    }
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
  for (const [key, attr, on] of [['align', 'horizontal', change.align], ['valign', 'vertical', change.valign], ['wrap', 'wrapText', change.wrap ? '1' : null]]) {
    if (!(key in change)) continue;
    let align = direct(xf, 'alignment');
    if (!align && on) {
      align = doc.createElementNS(MAIN, 'alignment');
      xf.insertBefore(align, xf.firstChild);
    }
    if (align) {
      if (on) align.setAttribute(attr, on);
      else align.removeAttribute(attr);
      if (!align.attributes.length) align.remove();
    }
    xf.setAttribute('applyAlignment', '1');
  }
  if ('borders' in change) {
    // Sides to draw (true), clear (false), or keep (left out).
    const borders = section(styles, 'borders');
    const old = kids(borders, 'border')[Number(xf.getAttribute('borderId') || 0)];
    const border = doc.createElementNS(MAIN, 'border');
    for (const side of ['left', 'right', 'top', 'bottom', 'diagonal']) {
      const want = change.borders[side];
      const keep = old && direct(old, side);
      let node;
      if (want === undefined && keep) node = keep.cloneNode(true);
      else {
        node = doc.createElementNS(MAIN, side);
        if (want) {
          node.setAttribute('style', 'thin');
          const color = doc.createElementNS(MAIN, 'color');
          color.setAttribute('auto', '1');
          node.append(color);
        }
      }
      border.append(node);
    }
    xf.setAttribute('borderId', String(addUnique(borders, 'border', border)));
    xf.setAttribute('applyBorder', '1');
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

/** The model of one worksheet from its XML: cells, rows, column widths and merged cells. */
function readSheet(name, path, xml, hidden, counter = { cells: 0 }) {
  const root = xml.documentElement;
  const sheetData = direct(root, 'sheetData');
  if (!sheetData) throw new Error('Worksheet data is missing.');
  const sheet = {
    name, path, xml, root, sheetData, cells: new Map(), rows: new Map(), shared: new Map(),
    maxRow: 0, maxCol: 0, rowHeights: new Map(), hiddenRows: new Set(), cols: [], merges: [], covered: new Map(),
    hidden,
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
      if (++counter.cells > MAX_CELLS) throw new Error('This workbook contains too many cells to edit in the browser.');
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
  const pane = direct(direct(direct(root, 'sheetViews'), 'sheetView'), 'pane');
  const frozen = ['frozen', 'frozenSplit'].includes(pane?.getAttribute('state'));
  sheet.frozen = {
    rows: frozen ? Math.max(0, Math.round(Number(pane.getAttribute('ySplit')) || 0)) : 0,
    cols: frozen ? Math.max(0, Math.round(Number(pane.getAttribute('xSplit')) || 0)) : 0,
  };
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
  return sheet;
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
  const counter = { cells: 0 };
  for (const sheetNode of sheetNodes) {
    const relId = sheetNode.getAttributeNS(DOC_REL, 'id') || sheetNode.getAttribute('r:id');
    const relationship = relationships.get(relId);
    if (!relationship?.type.endsWith('/worksheet')) continue;
    const path = normalizePart(relationship.target);
    if (!parts[path]) continue;
    const name = sheetNode.getAttribute('name') || `Sheet ${sheets.length + 1}`;
    const xml = parseXml(strFromU8(parts[path]), `Worksheet ${name}`);
    const sheet = readSheet(name, path, xml, ['hidden', 'veryHidden'].includes(sheetNode.getAttribute('state')), counter);
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
    markDynamicArrays(model);
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
  if (model.workbookTouched) parts['xl/workbook.xml'] = strToU8(serialize(model.workbookXml));
  for (const sheet of model.touched) parts[sheet.path] = strToU8(serialize(sheet.xml));
  if (model.stylesTouched && model.styles) parts['xl/styles.xml'] = strToU8(serialize(model.styles.xml));
  return zipSync(parts, { level: 6 });
}

/** Saves formulas that spill as dynamic arrays, as Excel does: an array formula over the cells
 * it fills, marked by the workbook's metadata as dynamic so Excel resizes it as it recalculates. */
function markDynamicArrays(model) {
  let mark;
  model.sheets.forEach((sheet, index) => {
    const spilled = new Map((model.spills?.get(index) || []).map(spill => [refName(spill.r1, spill.c1), spill]));
    for (const [ref, cell] of sheet.cells) {
      const f = direct(cell, 'f');
      if (!f || f.getAttribute('t') === 'dataTable' || !spills(cell, formulaOf(sheet, cell) || '')) continue;
      const spill = spilled.get(ref);
      const range = spill ? `${ref}:${refName(spill.r2, spill.c2)}` : ref;
      if (f.getAttribute('t') === 'array' && f.getAttribute('ref') === range && cell.hasAttribute('cm')) continue;
      mark ??= dynamicArrayMark(model);
      if (f.getAttribute('t') === 'shared') continue;
      f.setAttribute('t', 'array');
      f.setAttribute('ref', range);
      if (mark) cell.setAttribute('cm', mark);
      model.touched.add(sheet);
    }
  });
}

/** The cell metadata index that marks a dynamic array, adding the metadata part if the workbook
 * has none. Null when it has metadata of its own without that mark (the formula is then saved as
 * a fixed-size array). */
function dynamicArrayMark(model) {
  const { parts } = model;
  const rel = [...model.relsXml.getElementsByTagNameNS(PKG_REL, 'Relationship')].find(item => (item.getAttribute('Type') || '').endsWith('/sheetMetadata'));
  if (rel) {
    let path;
    try { path = normalizePart(rel.getAttribute('Target') || ''); } catch { return null; }
    if (!parts[path]) return null;
    const xml = parseXml(strFromU8(parts[path]), 'Metadata');
    const types = kids(direct(xml.documentElement, 'metadataTypes'), 'metadataType');
    const type = types.findIndex(item => item.getAttribute('name') === 'XLDAPR') + 1;
    if (!type) return null;
    const blocks = kids(direct(xml.documentElement, 'cellMetadata'), 'bk');
    const block = blocks.findIndex(bk => kids(bk, 'rc').some(rc => rc.getAttribute('t') === String(type))) + 1;
    return block ? String(block) : null;
  }
  parts['xl/metadata.xml'] = strToU8(`<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<metadata xmlns="${MAIN}" xmlns:xda="http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray"><metadataTypes count="1"><metadataType name="XLDAPR" minSupportedVersion="120000" copy="1" pasteAll="1" pasteValues="1" merge="1" splitFirst="1" rowColShift="1" clearFormats="1" clearComments="1" assign="1" coerce="1" cellMeta="1"/></metadataTypes><futureMetadata name="XLDAPR" count="1"><bk><extLst><ext uri="{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}"><xda:dynamicArrayProperties fDynamic="1" fCollapsed="0"/></ext></extLst></bk></futureMetadata><cellMetadata count="1"><bk><rc t="1" v="0"/></bk></cellMetadata></metadata>`);
  const ids = new Set([...model.relsXml.getElementsByTagNameNS(PKG_REL, 'Relationship')].map(item => item.getAttribute('Id')));
  let id = 1;
  while (ids.has(`rId${id}`)) id++;
  const link = model.relsXml.createElementNS(PKG_REL, 'Relationship');
  link.setAttribute('Id', `rId${id}`);
  link.setAttribute('Type', 'http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata');
  link.setAttribute('Target', 'metadata.xml');
  model.relsXml.documentElement.append(link);
  parts['xl/_rels/workbook.xml.rels'] = strToU8(serialize(model.relsXml));
  const types = parseXml(strFromU8(parts['[Content_Types].xml']), 'Content types');
  const override = types.createElementNS(CONTENT_TYPES, 'Override');
  override.setAttribute('PartName', '/xl/metadata.xml');
  override.setAttribute('ContentType', 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml');
  types.documentElement.append(override);
  parts['[Content_Types].xml'] = strToU8(serialize(types));
  return '1';
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
  const spilled = spillAt(model, index, row, col);
  if (spilled !== undefined && (!cell || spilled.member)) return spilled.value;
  if (!cell) return null;
  const key = `${index}!${ref}`;
  return model.computed.has(key) ? model.computed.get(key) : cachedValue(model, cell);
}

/** A formula that can return several cells, spilling them below and to the right: one of the
 * newer array functions, or one saved as a dynamic array. Older array formulas ({=…}) keep the
 * values the file saved. */
function spills(cell, formula) {
  return cell.hasAttribute('cm') || /\b(?:_xlfn\.)?(?:_xlws\.)?(?:UNIQUE|SORT|FILTER)\s*\(/i.test(formula);
}

/** The cells of a dynamic array saved by a spreadsheet app hold its last values: they are the
 * array's own, not in its way. */
function arrayMember(sheet, row, col) {
  for (const spill of sheet.arrays || []) {
    if (row >= spill.r1 && row <= spill.r2 && col >= spill.c1 && col <= spill.c2 && !(row === spill.r1 && col === spill.c1)) return true;
  }
  return false;
}

/** The value a spilling formula puts at a cell: { value, member } or undefined. */
function spillAt(model, index, row, col) {
  for (const spill of model.spills?.get(index) || []) {
    if (row < spill.r1 || row > spill.r2 || col < spill.c1 || col > spill.c2 || (row === spill.r1 && col === spill.c1)) continue;
    return { value: spill.rows[row - spill.r1][col - spill.c1], member: true };
  }
  return undefined;
}

/** Recalculates every formula the engine understands; the others keep their saved values. */
function recalc(model, { onlyMissing = false } = {}) {
  const byName = new Map(model.sheets.map((sheet, index) => [sheet.name.toLowerCase(), index]));
  for (const sheet of model.sheets) {
    // Ranges dynamic arrays saved by spreadsheet apps cover.
    sheet.arrays = [];
    for (const cell of sheet.cells.values()) {
      const f = direct(cell, 'f');
      if (f?.getAttribute('t') !== 'array' || !cell.hasAttribute('cm')) continue;
      const [a, b] = (f.getAttribute('ref') || '').split(':').map(parseRef);
      if (a) sheet.arrays.push({ r1: a.row, c1: a.col, r2: (b || a).row, c2: (b || a).col });
    }
  }
  // A formula reading cells another one spills into needs that one's results first: when a pass
  // finds spills, a second pass runs knowing them.
  function pass(known) {
    const results = new Map(), busy = new Set(), failed = new Set(), found = new Map();
    const view = { ...model, spills: known };
    function cellValue(index, row, col) {
      const sheet = model.sheets[index];
      const ref = refName(row, col);
      const cell = sheet.cells.get(ref);
      const spilled = spillAt(view, index, row, col);
      if (spilled !== undefined && (!cell || arrayMember(sheet, row, col))) return spilled.value;
      if (!cell) return null;
      const f = direct(cell, 'f');
      const text = f && f.getAttribute('t') !== 'dataTable' ? formulaOf(sheet, cell) : null;
      const formula = text != null && (f.getAttribute('t') !== 'array' || spills(cell, text)) ? text : null;
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
        }, { array: spills(cell, formula) });
        if (value?.rows) value = spill(index, row, col, value.rows);
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
    /** Spills rows from a formula at row, col, unless other cells are in the way (#SPILL!). */
    function spill(index, row, col, rows) {
      const sheet = model.sheets[index];
      const r2 = row + rows.length - 1, c2 = col + rows[0].length - 1;
      if (r2 > LAST_ROW || c2 > LAST_COL) return new CellError('#SPILL!');
      for (let r = row; r <= r2; r++) {
        for (let c = col; c <= c2; c++) {
          if (r === row && c === col) continue;
          const other = sheet.cells.get(refName(r, c));
          const empty = !other || (!direct(other, 'f') && !direct(other, 'v') && !direct(other, 'is'));
          if (!empty && !arrayMember(sheet, r, c)) return new CellError('#SPILL!');
          if (sheet.covered.has(refName(r, c))) return new CellError('#SPILL!');
        }
      }
      for (const other of found.get(index) || []) {
        if (!(r2 < other.r1 || row > other.r2 || c2 < other.c1 || col > other.c2)) return new CellError('#SPILL!');
      }
      if (!found.has(index)) found.set(index, []);
      found.get(index).push({ r1: row, c1: col, r2, c2, rows });
      return rows[0][0];
    }
    model.sheets.forEach((sheet, index) => {
      for (const [ref, cell] of sheet.cells) {
        if (!direct(cell, 'f')) continue;
        if (onlyMissing && direct(cell, 'v') && !spills(cell, formulaOf(sheet, cell) || '')) continue;
        const { row, col } = parseRef(ref);
        const value = cellValue(index, row, col);
        if (onlyMissing) model.computed.set(`${index}!${ref}`, value);
      }
    });
    return { results, failed, found };
  }
  let outcome = pass(new Map());
  if (outcome.found.size) outcome = pass(outcome.found);
  model.spills = outcome.found;
  if (!onlyMissing) {
    model.computed = outcome.results;
    model.failed = outcome.failed;
  } else for (const key of outcome.failed) model.failed.add(key);
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

// --- Rows and columns ---------------------------------------------------------------------------

/** The parts a worksheet links to, as { type, path }, the type being the relationship's last word. */
function sheetLinks(model, sheet) {
  const rels = model.parts[sheet.path.replace(/([^/]+)$/, '_rels/$1.rels')];
  if (!rels) return [];
  const xml = parseXml(strFromU8(rels), 'Worksheet relationships');
  const folder = sheet.path.replace(/\/[^/]+$/, '');
  return [...xml.getElementsByTagNameNS(PKG_REL, 'Relationship')]
    .filter(rel => rel.getAttribute('TargetMode') !== 'External')
    .map(rel => {
      try { return { type: (rel.getAttribute('Type') || '').split('/').pop(), path: normalizePart(rel.getAttribute('Target') || '', folder) }; } catch { return null; }
    })
    .filter(Boolean);
}

/** Why rows or columns cannot be inserted into or deleted from a sheet, or null when they can. */
export function structureBlocked(model, sheet) {
  const types = new Set(sheetLinks(model, sheet).map(link => link.type));
  if (types.has('table')) return 'This sheet has tables; rows and columns cannot be inserted or deleted in it here yet.';
  if (types.has('pivotTable')) return 'This sheet has pivot tables; rows and columns cannot be inserted or deleted in it here yet.';
  if (types.has('comments') || types.has('threadedComment')) return 'This sheet has comments; rows and columns cannot be inserted or deleted in it here yet.';
  return null;
}

/** Turns every shared formula in the workbook into plain ones, so each can move on its own. */
function unshareAll(model) {
  for (const sheet of model.sheets) {
    const plain = [];
    for (const cell of sheet.cells.values()) {
      const f = direct(cell, 'f');
      if (f?.getAttribute('t') !== 'shared') continue;
      const formula = formulaOf(sheet, cell);
      if (formula != null) plain.push([f, formula]);
    }
    for (const [f, formula] of plain) {
      f.textContent = formula;
      for (const name of ['t', 'si', 'ref']) f.removeAttribute(name);
    }
    if (plain.length) { sheet.shared.clear(); model.touched.add(sheet); }
  }
}

/** Inserts `count` rows or columns (`axis` "row" or "col") before `at` in sheet `index`, or deletes
 * -count of them from `at` on, moving the cells after them and everything that refers to cells:
 * formulas in every sheet, defined names, merged cells, ranges of formatting and validation,
 * column widths, and pictures and charts. */
export function restructure(model, index, axis, at, count) {
  const sheet = model.sheets[index];
  const blocked = structureBlocked(model, sheet);
  if (blocked) throw new Error(blocked);
  const last = axis === 'row' ? LAST_ROW : LAST_COL;
  if (count > 0 && (axis === 'row' ? sheet.maxRow : sheet.maxCol) + count > last) {
    throw new Error(`There is no room to insert ${axis === 'row' ? 'rows' : 'columns'}: cells would be pushed off the end of the sheet.`);
  }
  unshareAll(model);
  const { root } = sheet;
  for (const row of kids(sheet.sheetData, 'row')) {
    if (axis === 'row') {
      const r = Number(row.getAttribute('r'));
      const span = moveSpan(r, r, at, count);
      if (!span) { row.remove(); continue; }
      if (span[0] === r) continue;
      row.setAttribute('r', String(span[0]));
      for (const cell of kids(row, 'c')) cell.setAttribute('r', refName(span[0], parseRef(cell.getAttribute('r')).col));
    } else {
      // Which columns a row uses is only a hint, and no longer right.
      row.removeAttribute('spans');
      for (const cell of kids(row, 'c')) {
        const { row: r, col } = parseRef(cell.getAttribute('r'));
        const span = moveSpan(col, col, at, count);
        if (!span) cell.remove();
        else if (span[0] !== col) cell.setAttribute('r', refName(r, span[0]));
      }
    }
  }
  if (axis === 'col') {
    const cols = direct(root, 'cols');
    for (const col of kids(cols, 'col')) {
      const span = moveSpan(Number(col.getAttribute('min')), Number(col.getAttribute('max')), at, count);
      if (!span || span[0] > LAST_COL) { col.remove(); continue; }
      col.setAttribute('min', String(span[0]));
      col.setAttribute('max', String(Math.min(LAST_COL, span[1])));
    }
    if (cols && !kids(cols, 'col').length) cols.remove();
  }
  // Formulas, in every sheet (conditional formatting and validation have them too).
  model.sheets.forEach((other, i) => {
    const change = { sheet: sheet.name, here: i === index, axis, at, count };
    let moved = false;
    for (const name of ['f', 'formula', 'formula1', 'formula2']) {
      for (const node of other.root.getElementsByTagNameNS(MAIN, name)) {
        if (!node.textContent) continue;
        const next = moveFormula(node.textContent, change);
        if (next !== node.textContent) { node.textContent = next; moved = true; }
      }
    }
    if (moved) model.touched.add(other);
  });
  for (const node of model.workbookXml.getElementsByTagNameNS(MAIN, 'definedName')) {
    const next = moveFormula(node.textContent, { sheet: sheet.name, here: false, axis, at, count });
    if (next !== node.textContent) { node.textContent = next; model.workbookTouched = true; }
  }
  // Ranges in the sheet: merged cells, formatting, validation, links, filters, the selection.
  const single = ref => !ref.includes(':') || ref.split(':')[0] === ref.split(':')[1];
  for (const node of [...root.getElementsByTagNameNS(MAIN, '*')]) {
    if (node.localName === 'dimension' || sheet.sheetData.contains(node)) continue;
    for (const attr of ['sqref', 'ref', 'activeCell', 'topLeftCell']) {
      if (!node.hasAttribute(attr) || !node.isConnected) continue;
      const next = moveSqref(node.getAttribute(attr), axis, at, count);
      if (next && !(node.localName === 'mergeCell' && single(next))) node.setAttribute(attr, next);
      else if (node.localName === 'selection' || node.localName === 'pane') node.setAttribute(attr, 'A1');
      else node.remove();
    }
  }
  for (const [listName, itemName] of [['mergeCells', 'mergeCell'], ['dataValidations', 'dataValidation'], ['hyperlinks', 'hyperlink']]) {
    const list = direct(root, listName);
    if (!list) continue;
    const left = kids(list, itemName).length;
    if (!left) list.remove();
    else if (list.hasAttribute('count')) list.setAttribute('count', String(left));
  }
  for (const node of kids(root, 'conditionalFormatting')) if (!kids(node, 'cfRule').length) node.remove();
  // Arrays formulas in the cells: their range moves too.
  for (const f of sheet.sheetData.getElementsByTagNameNS(MAIN, 'f')) {
    if (f.getAttribute('t') !== 'array' || !f.hasAttribute('ref')) continue;
    const next = moveSqref(f.getAttribute('ref'), axis, at, count);
    if (next) f.setAttribute('ref', next); else f.removeAttribute('ref');
  }
  // Pictures and charts are anchored to cells (counted from 0).
  for (const link of sheetLinks(model, sheet)) {
    if (link.type !== 'drawing' || !model.parts[link.path]) continue;
    const drawing = parseXml(strFromU8(model.parts[link.path]), 'Drawing');
    for (const marker of [...drawing.getElementsByTagNameNS(SHEET_DRAWING, 'from'), ...drawing.getElementsByTagNameNS(SHEET_DRAWING, 'to')]) {
      const node = kids(marker, axis, SHEET_DRAWING)[0];
      if (!node) continue;
      const position = Number(node.textContent) + 1;
      const span = moveSpan(position, position, at, count);
      node.textContent = String((span ? span[0] : at) - 1);
    }
    model.parts[link.path] = strToU8(serialize(drawing));
  }
  rereadSheet(sheet);
  updateDimension(sheet);
  model.touched.add(sheet);
}

/** Reads a sheet's model again from its XML, after its structure changed. */
function rereadSheet(sheet) {
  Object.assign(sheet, readSheet(sheet.name, sheet.path, sheet.xml, sheet.hidden));
}

/** Changes the <col> ranges of columns c1…c2 with `change(col)`, splitting ranges that reach
 * outside them and adding ranges for columns that had none. */
export function setColumns(sheet, c1, c2, change, { existingOnly = false, reread = true } = {}) {
  const { root, xml } = sheet;
  let cols = direct(root, 'cols');
  if (!cols) {
    // <cols> comes right before <sheetData>.
    cols = xml.createElementNS(MAIN, 'cols');
    root.insertBefore(cols, sheet.sheetData);
  }
  const make = (min, max, from) => {
    const col = from ? from.cloneNode(true) : xml.createElementNS(MAIN, 'col');
    col.setAttribute('min', String(min));
    col.setAttribute('max', String(max));
    return col;
  };
  const pieces = [], inside = [];
  for (const col of kids(cols, 'col')) {
    const min = Number(col.getAttribute('min')), max = Number(col.getAttribute('max'));
    if (max < c1 || min > c2) { pieces.push(col); continue; }
    if (min < c1) pieces.push(make(min, c1 - 1, col));
    const middle = make(Math.max(min, c1), Math.min(max, c2), col);
    inside.push(middle);
    if (max > c2) pieces.push(make(c2 + 1, max, col));
  }
  inside.sort((a, b) => Number(a.getAttribute('min')) - Number(b.getAttribute('min')));
  let next = c1;
  for (const col of existingOnly ? [] : [...inside]) {
    const min = Number(col.getAttribute('min'));
    if (min > next) inside.push(make(next, min - 1, null));
    next = Number(col.getAttribute('max')) + 1;
  }
  if (!existingOnly && next <= c2) inside.push(make(next, c2, null));
  for (const col of inside) {
    if (!col.hasAttribute('width')) col.setAttribute('width', String(Math.round(sheet.defaultColPx / 7 * 100) / 100));
    change(col);
  }
  pieces.push(...inside);
  pieces.sort((a, b) => Number(a.getAttribute('min')) - Number(b.getAttribute('min')));
  cols.replaceChildren(...pieces);
  if (!pieces.length) cols.remove();
  if (reread) rereadSheet(sheet);
}

/** Changes the <row> elements of rows r1…r2 with `change(row)`, adding ones that are missing. */
export function setRows(sheet, r1, r2, change, { existingOnly = false } = {}) {
  if (existingOnly) {
    for (const [r, row] of sheet.rows) if (r >= r1 && r <= r2) change(row);
  } else {
    for (let r = r1; r <= r2; r++) change(rowElement(sheet, r));
  }
  rereadSheet(sheet);
}

/** The order <worksheet>'s children must come in. */
const WORKSHEET_ORDER = ['sheetPr', 'dimension', 'sheetViews', 'sheetFormatPr', 'cols', 'sheetData', 'sheetCalcPr', 'sheetProtection',
  'protectedRanges', 'scenarios', 'autoFilter', 'sortState', 'dataConsolidate', 'customSheetViews', 'mergeCells', 'phoneticPr',
  'conditionalFormatting', 'dataValidations', 'hyperlinks', 'printOptions', 'pageMargins', 'pageSetup', 'headerFooter', 'rowBreaks',
  'colBreaks', 'customProperties', 'cellWatches', 'ignoredErrors', 'smartTags', 'drawing', 'legacyDrawing', 'legacyDrawingHF',
  'drawingHF', 'picture', 'oleObjects', 'controls', 'webPublishItems', 'tableParts', 'extLst'];

/** A child of the worksheet element, added in its place if missing. */
function worksheetChild(sheet, name) {
  let node = direct(sheet.root, name);
  if (node) return node;
  node = sheet.xml.createElementNS(MAIN, name);
  const after = WORKSHEET_ORDER.slice(WORKSHEET_ORDER.indexOf(name) + 1);
  sheet.root.insertBefore(node, [...sheet.root.children].find(child => after.includes(child.localName)) || null);
  return node;
}

/** Merges a range into one cell (keeping the top-left value, as Excel does), taking in merged
 * cells inside it; or with `merge` false, unmerges the merged cells it touches. */
export function mergeRange(sheet, range, merge) {
  const list = worksheetChild(sheet, 'mergeCells');
  const overlaps = item => {
    const [a, b] = (item.getAttribute('ref') || '').split(':').map(parseRef);
    return a && b && !(Math.max(a.row, b.row) < range.r1 || Math.min(a.row, b.row) > range.r2 || Math.max(a.col, b.col) < range.c1 || Math.min(a.col, b.col) > range.c2);
  };
  for (const item of kids(list, 'mergeCell')) {
    if (!overlaps(item)) continue;
    if (merge) {
      const [a, b] = item.getAttribute('ref').split(':').map(parseRef);
      const inside = Math.min(a.row, b.row) >= range.r1 && Math.max(a.row, b.row) <= range.r2 && Math.min(a.col, b.col) >= range.c1 && Math.max(a.col, b.col) <= range.c2;
      if (!inside) throw new Error('The selection overlaps part of merged cells: select all of them, or unmerge them first.');
    }
    item.remove();
  }
  if (merge) {
    for (const [ref, cell] of [...sheet.cells]) {
      const { row, col } = parseRef(ref);
      if (row < range.r1 || row > range.r2 || col < range.c1 || col > range.c2 || (row === range.r1 && col === range.c1)) continue;
      // Other cells keep their formatting, not their contents.
      for (const child of [...cell.children]) child.remove();
      cell.removeAttribute('t');
    }
    const item = sheet.xml.createElementNS(MAIN, 'mergeCell');
    item.setAttribute('ref', `${refName(range.r1, range.c1)}:${refName(range.r2, range.c2)}`);
    list.append(item);
  }
  const left = kids(list, 'mergeCell').length;
  if (left) list.setAttribute('count', String(left)); else list.remove();
  rereadSheet(sheet);
}

/** Keeps the first `rows` rows and `cols` columns in view while the rest scrolls (0 and 0 unfreezes). */
export function freezePanes(sheet, rows, cols) {
  const views = worksheetChild(sheet, 'sheetViews');
  let view = kids(views, 'sheetView')[0];
  if (!view) {
    view = sheet.xml.createElementNS(MAIN, 'sheetView');
    view.setAttribute('workbookViewId', '0');
    views.append(view);
  }
  for (const pane of kids(view, 'pane')) pane.remove();
  // Selections belong to panes; the one for the top-left pane (no "pane") stays.
  for (const selection of kids(view, 'selection')) if (selection.hasAttribute('pane')) selection.remove();
  if (rows || cols) {
    const pane = sheet.xml.createElementNS(MAIN, 'pane');
    if (cols) pane.setAttribute('xSplit', String(cols));
    if (rows) pane.setAttribute('ySplit', String(rows));
    pane.setAttribute('topLeftCell', refName(rows + 1, cols + 1));
    pane.setAttribute('activePane', rows && cols ? 'bottomRight' : rows ? 'bottomLeft' : 'topRight');
    pane.setAttribute('state', 'frozen');
    view.insertBefore(pane, view.firstChild);
  }
  rereadSheet(sheet);
}

// --- Filling ------------------------------------------------------------------------------------

const FILL_LISTS = [
  ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'],
  ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'],
  ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'],
  ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'],
];

/** How dragging the fill handle continues a run of cells: given what they hold, in order, as
 * { kind: "number" | "text" | …, value, date }, returns k => the entry k steps from the first
 * (k may be negative, for filling up or left), or null when the cells are to be copied. */
export function fillSeries(entries) {
  const n = entries.length;
  if (!n) return null;
  const round = value => Number(value.toPrecision(15));
  if (entries.every(entry => entry.kind === 'number')) {
    const first = entries[0].value;
    // One number is copied, except a date, which goes on a day at a time.
    if (n === 1) return entries[0].date ? k => ({ kind: 'number', value: first + k }) : null;
    const step = (entries[n - 1].value - first) / (n - 1);
    return k => ({ kind: 'number', value: round(first + step * k) });
  }
  if (!entries.every(entry => entry.kind === 'text')) return null;
  const texts = entries.map(entry => entry.value);
  for (const list of FILL_LISTS) {
    const positions = texts.map(text => list.findIndex(item => item.toLowerCase() === text.toLowerCase()));
    if (positions.some(position => position < 0)) continue;
    const step = n === 1 ? 1 : positions[1] - positions[0];
    if (positions.some((position, i) => ((position - positions[0] - step * i) % list.length + list.length) % list.length)) return null;
    const sample = texts[0];
    const cased = item => (sample === sample.toUpperCase() ? item.toUpperCase() : sample === sample.toLowerCase() ? item.toLowerCase() : item);
    return k => ({ kind: 'text', value: cased(list[(((positions[0] + step * k) % list.length) + list.length) % list.length]) });
  }
  // Text ending in a number ("Item 1", "Q1") counts on.
  const numbered = texts.map(text => /^(.*?)(\d+)$/.exec(text));
  if (numbered.every(match => match && match[1] === numbered[0][1])) {
    const numbers = numbered.map(match => Number(match[2]));
    const step = n === 1 ? 1 : (numbers[n - 1] - numbers[0]) / (n - 1);
    if (!Number.isInteger(step) || numbers.some((number, i) => number !== numbers[0] + step * i)) return null;
    const width = numbered[0][2].startsWith('0') ? numbered[0][2].length : 0;
    return k => {
      const number = numbers[0] + step * k;
      return number < 0 ? null : { kind: 'text', value: numbered[0][1] + String(number).padStart(width, '0') };
    };
  }
  return null;
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
  freeze: '<path d="M2.5 2.5h11v11h-11z"/><path d="M2.5 6.5h11M6.5 2.5v11" stroke-width="2.4"/>',
  underline: '<path d="M5 3v4a3 3 0 0 0 6 0V3M4 13h8"/>',
  strike: '<path d="M3 8h10M10.5 4.5C10 3.6 9 3 7.8 3 6.3 3 5 3.9 5 5.2c0 .9.5 1.5 1.3 1.9M5.5 11.3c.5.9 1.5 1.7 2.9 1.7 1.6 0 2.8-.9 2.8-2.3 0-.6-.2-1.1-.6-1.5"/>',
  wrap: '<path d="M3 4h10M3 8h8a2 2 0 0 1 0 4H8"/><path d="M9.5 10.5 8 12l1.5 1.5M3 12h3"/>',
  merge: '<path d="M2.5 3.5h11v9h-11z"/><path d="M5 8h6M6.5 6.5 5 8l1.5 1.5M9.5 6.5 11 8l-1.5 1.5"/>',
  border: '<path d="M2.5 2.5h11v11h-11zM8 2.5v11M2.5 8h11" stroke-dasharray="1.5 1.5"/><path d="M2.5 13.5h11"/>',
};

const FONTS = ['Calibri', 'Aptos', 'Arial', 'Cambria', 'Consolas', 'Courier New', 'Georgia', 'Segoe UI', 'Tahoma', 'Times New Roman', 'Trebuchet MS', 'Verdana'];
const SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];

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

/** CSS for an Excel border style. */
const BORDER_CSS = {
  thin: '1px solid', hair: '1px solid', medium: '2px solid', thick: '3px solid', double: '3px double', dashed: '1px dashed',
  dotted: '1px dotted', mediumDashed: '2px dashed', dashDot: '1px dashed', mediumDashDot: '2px dashed', dashDotDot: '1px dotted',
  mediumDashDotDot: '2px dotted', slantDashDot: '2px dashed',
};

/** The CSS font a cell's text is drawn in. */
function cellFont(style) {
  const px = (style.size || 11) * 4 / 3 * FONT_SCALE;
  return `${style.italic ? 'italic ' : ''}${style.bold ? '700 ' : ''}${px.toFixed(1)}px ${style.font ? `"${style.font.replace(/["\\]/g, '')}", ` : ''}${SHEET_FONT}`;
}

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
  let historyAt = 0, savedAt = 0, clipboard = null, message = '';

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
  const fx = document.createElement('button');
  fx.type = 'button';
  fx.className = 'xlsx-fx';
  fx.textContent = 'fx';
  fx.title = 'Functions';
  fx.setAttribute('aria-label', 'Functions');
  fx.setAttribute('aria-haspopup', 'dialog');
  fx.setAttribute('aria-expanded', 'false');
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
  // The fill handle at the selection's corner, the cells a drag of it fills, and the line a
  // column or row edge being dragged goes to.
  const fillHandle = document.createElement('div');
  fillHandle.className = 'xlsx-fill-handle';
  fillHandle.title = 'Drag to fill; double-click to fill down';
  const fillBox = document.createElement('div');
  fillBox.className = 'xlsx-fill';
  fillBox.hidden = true;
  const sizeGuide = document.createElement('div');
  sizeGuide.className = 'xlsx-size-guide';
  sizeGuide.hidden = true;
  canvas.append(layer, selectionBox, activeBox, pointBox, fillBox, fillHandle, sizeGuide, cellEditor);
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
  const menu = document.createElement('div');
  menu.className = 'menu xlsx-menu';
  menu.setAttribute('role', 'menu');
  menu.hidden = true;
  // Every function, with how to use it and a quick filter; picking one puts it in the formula.
  const functionsPanel = document.createElement('div');
  functionsPanel.className = 'xlsx-functions';
  functionsPanel.setAttribute('role', 'dialog');
  functionsPanel.setAttribute('aria-label', 'Functions');
  functionsPanel.hidden = true;
  const functionsHead = document.createElement('div');
  functionsHead.className = 'xlsx-functions-head';
  const functionsTitle = document.createElement('strong');
  functionsTitle.textContent = 'Functions';
  const functionsClose = document.createElement('button');
  functionsClose.type = 'button';
  functionsClose.className = 'xlsx-functions-close';
  functionsClose.textContent = '✕';
  functionsClose.setAttribute('aria-label', 'Close');
  functionsHead.append(functionsTitle, functionsClose);
  const functionsFilter = document.createElement('input');
  functionsFilter.type = 'search';
  functionsFilter.className = 'xlsx-functions-filter';
  functionsFilter.placeholder = 'Filter: name or what it does';
  functionsFilter.setAttribute('aria-label', 'Filter functions');
  functionsFilter.autocomplete = 'off';
  functionsFilter.spellcheck = false;
  const functionsHelp = document.createElement('details');
  functionsHelp.className = 'xlsx-functions-help';
  const helpSummary = document.createElement('summary');
  helpSummary.textContent = 'How formulas work';
  const helpList = document.createElement('ul');
  for (const parts of [
    ['A formula starts with ', ['=A1+B1'], '. Operators: ', ['+ - * /'], ', ', ['^'], ' for powers, ', ['&'], ' to join text, and ', ['= <> < > <= >='], ' to compare.'],
    ['A function takes its arguments in brackets, separated by commas: ', ['=ROUND(A1, 2)'], '. Arguments in [square brackets] can be left out; … means more can follow.'],
    ['A range is two corners with a colon: ', ['A1:B10'], '. A whole column is ', ['A:A'], ', another sheet ', ['Data!A1'], '.'],
    ['$ keeps a reference from moving when the formula is copied or filled: ', ['$A$1'], ' keeps both, ', ['A$1'], ' just the row.'],
    ['Text goes in double quotes: ', ['"Paid"'], '. So do conditions: ', ['=COUNTIF(B2:B20, ">100")'], '.'],
    ['While typing, click a cell to put it in (drag for a range), and Tab picks the function suggested.'],
  ]) {
    const item = document.createElement('li');
    for (const part of parts) {
      if (typeof part === 'string') { item.append(part); continue; }
      const code = document.createElement('code');
      code.textContent = part[0];
      item.append(code);
    }
    helpList.append(item);
  }
  functionsHelp.append(helpSummary, helpList);
  const functionsCount = document.createElement('p');
  functionsCount.className = 'xlsx-functions-count';
  functionsCount.setAttribute('aria-live', 'polite');
  const functionsList = document.createElement('div');
  functionsList.className = 'xlsx-functions-list';
  functionsList.setAttribute('role', 'listbox');
  functionsList.setAttribute('aria-label', 'Functions');
  functionsPanel.append(functionsHead, functionsFilter, functionsHelp, functionsCount, functionsList);
  root.append(toolbar, formulaRow, scroll, footer, note, live, assist, menu, functionsPanel);
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
  function selectTool(label, options, action) {
    const select = document.createElement('select');
    select.className = 'office-select';
    select.title = label;
    select.setAttribute('aria-label', label);
    for (const [value, text] of options) {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = text;
      select.append(option);
    }
    select.addEventListener('change', () => {
      if (select.value !== '') action(select.value);
      scroll.focus({ preventScroll: true });
    });
    toolbar.append(select);
    return select;
  }
  /** Makes sure a select can show `value`, adding it as an option if it is not one. */
  function showValue(select, value) {
    if (value !== '' && ![...select.options].some(option => option.value === value)) {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = value;
      select.append(option);
    }
    select.value = value;
  }
  let fontSelect = null, sizeSelect = null, valignSelect = null;
  if (!readonly) {
    tool('undo', 'Undo (Ctrl+Z)', () => step(-1));
    tool('redo', 'Redo (Ctrl+Y)', () => step(1));
    divider();
    tool('bold', 'Bold (Ctrl+B)', () => formatSelection({ bold: !activeStyle().bold }), { toggle: true });
    tool('italic', 'Italic (Ctrl+I)', () => formatSelection({ italic: !activeStyle().italic }), { toggle: true });
    tool('underline', 'Underline (Ctrl+U)', () => formatSelection({ underline: !activeStyle().underline }), { toggle: true });
    tool('strike', 'Strikethrough (Ctrl+5)', () => formatSelection({ strike: !activeStyle().strike }), { toggle: true });
    fontSelect = selectTool('Font', [['', 'Font'], ...FONTS.map(name => [name, name])], value => formatSelection({ font: value }));
    fontSelect.classList.add('xlsx-font-select');
    sizeSelect = selectTool('Font size', SIZES.map(size => [String(size), String(size)]), value => formatSelection({ size: Number(value) }));
    sizeSelect.classList.add('xlsx-size-select');
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
    valignSelect = selectTool('Vertical alignment', [['bottom', 'Bottom'], ['center', 'Middle'], ['top', 'Top']], value => formatSelection({ valign: value === 'bottom' ? null : value }));
    tool('wrap', 'Wrap text', () => formatSelection({ wrap: !activeStyle().wrap }), { toggle: true });
    tool('merge', 'Merge and centre, or unmerge', toggleMerge, { toggle: true });
    tool('border', 'Borders', () => {
      const box = buttons.border.getBoundingClientRect();
      openMenu(box.left, box.bottom + 4, [
        ['All borders', () => borderSelection('all')], ['Outside borders', () => borderSelection('outside')],
        ['Bottom border', () => borderSelection('bottom')], ['Top border', () => borderSelection('top')],
        ['Left border', () => borderSelection('left')], ['Right border', () => borderSelection('right')], '-',
        ['No borders', () => borderSelection('none')],
      ]);
    });
    buttons.border.setAttribute('aria-haspopup', 'menu');
    divider();
    tool('freeze', 'Freeze panes', () => {
      const box = buttons.freeze.getBoundingClientRect();
      const f = sheet().frozen || { rows: 0, cols: 0 };
      const at = { rows: anchor.row - 1, cols: anchor.col - 1 };
      openMenu(box.left, box.bottom + 4, [
        ['Freeze top row', () => freeze(1, 0)],
        ['Freeze first column', () => freeze(0, 1)],
        [at.rows || at.cols ? `Freeze above and left of ${refName(anchor.row, anchor.col)}` : 'Freeze above and left of the selection', () => freeze(at.rows, at.cols)],
        ...(f.rows || f.cols ? ['-', ['Unfreeze', () => freeze(0, 0)]] : []),
      ]);
    });
    buttons.freeze.setAttribute('aria-haspopup', 'menu');
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
    buttons.underline.setAttribute('aria-pressed', String(!!style.underline));
    buttons.strike.setAttribute('aria-pressed', String(!!style.strike));
    buttons.wrap.setAttribute('aria-pressed', String(!!style.wrap));
    buttons.merge.setAttribute('aria-pressed', String(sheet().covered.has(refName(anchor.row, anchor.col))));
    showValue(fontSelect, style.font || '');
    showValue(sizeSelect, String(style.size || 11));
    valignSelect.value = style.valign === 'center' || style.valign === 'top' ? style.valign : 'bottom';
    for (const select of [fontSelect, sizeSelect, valignSelect]) select.disabled = !writable || !model.styles;
    for (const side of ['left', 'center', 'right']) buttons[side].setAttribute('aria-pressed', String(style.align === side));
    const known = FORMATS.find(([code]) => code === style.numFmt);
    formatSelect.value = known ? known[0] : '';
    if (!model.styles) for (const name of ['bold', 'italic', 'underline', 'strike', 'left', 'center', 'right', 'wrap', 'border', 'color', 'fill', 'nofill']) buttons[name].disabled = true;
    saveButton.disabled = saving || !dirty() || !onSave;
    status.textContent = saving ? 'Saving…' : dirty() ? 'Unsaved changes' : 'Saved';
  }

  function updateNote() {
    const notes = [];
    if (readonly) notes.push('Read only.');
    else if (sheet().protected) notes.push('This sheet is protected, so it is read only here.');
    const failed = [...model.failed].filter(key => key.startsWith(`${active}!`)).length;
    if (message) notes.push(message);
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
  const frozen = () => {
    const f = sheet().frozen || { rows: 0, cols: 0 };
    return { rows: Math.min(f.rows, geo ? geo.rows - 1 : f.rows), cols: Math.min(f.cols, geo ? geo.cols - 1 : f.cols) };
  };
  /** Where a range is drawn: inside the frozen rows or columns, it moves with the scroll. */
  function onScreen(r1, c1, r2, c2) {
    const box = rect(r1, c1, r2, c2);
    const f = frozen();
    if (f.rows && r2 <= f.rows) box.top += scroll.scrollTop;
    if (f.cols && c2 <= f.cols) box.left += scroll.scrollLeft;
    return box;
  }

  // Display
  function activeStyle() {
    const cell = sheet().cells.get(refName(anchor.row, anchor.col));
    return cellStyle(model, cell?.getAttribute('s'));
  }
  function shown(row, col) {
    const s = sheet();
    const cell = s.cells.get(refName(row, col));
    const spilled = spillAt(model, active, row, col);
    if (!cell && spilled === undefined) return null;
    const value = valueAt(model, active, row, col);
    const style = cellStyle(model, cell?.getAttribute('s'));
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
    // Frozen rows and columns stay in place: drawn over the rest, shifted by the scroll.
    const { rows: fr, cols: fc } = frozen();
    const frozenH = geo.rowY[fr + 1] - HEAD_H, frozenW = geo.colX[fc + 1] - HEAD_W;
    const r1 = Math.max(fr + 1, find(geo.rowY, geo.rows, top + HEAD_H + frozenH)), r2 = Math.min(geo.rows, find(geo.rowY, geo.rows, top + height) + 1);
    const c1 = Math.max(fc + 1, find(geo.colX, geo.cols, left + HEAD_W + frozenW)), c2 = Math.min(geo.cols, find(geo.colX, geo.cols, left + width) + 1);
    const fragment = document.createDocumentFragment();
    let dx = 0, dy = 0, pinned = false;
    const lines = (rowFrom, rowTo, colFrom, colTo) => {
      const x1 = geo.colX[colFrom], x2 = geo.colX[colTo + 1], y1 = geo.rowY[rowFrom], y2 = geo.rowY[rowTo + 1];
      for (let r = rowFrom; r <= rowTo + 1; r++) {
        const line = document.createElement('div');
        line.className = pinned ? 'xlsx-hline is-frozen' : 'xlsx-hline';
        line.style.cssText = `top:${geo.rowY[r] - 1 + dy}px;left:${x1 + dx}px;width:${x2 - x1}px`;
        fragment.append(line);
      }
      for (let c = colFrom; c <= colTo + 1; c++) {
        const line = document.createElement('div');
        line.className = pinned ? 'xlsx-vline is-frozen' : 'xlsx-vline';
        line.style.cssText = `left:${geo.colX[c] - 1 + dx}px;top:${y1 + dy}px;height:${y2 - y1}px`;
        fragment.append(line);
      }
    };
    const drawn = new Set();
    const drawCell = (row, col, merge) => {
      const info = shown(row, col);
      if (!info) return;
      const { style } = info;
      const box = merge ? rect(merge.r1, merge.c1, merge.r2, merge.c2) : rect(row, col, row, col);
      box.left += dx;
      box.top += dy;
      if (!box.width || !box.height) return;
      const el = document.createElement('div');
      el.className = pinned ? 'xlsx-cell is-frozen' : 'xlsx-cell';
      let width = box.width;
      const px = (style.size || 11) * 4 / 3 * FONT_SCALE;
      const font = cellFont(style);
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
      if (style.borders) {
        const edge = document.createElement('div');
        edge.className = pinned ? 'xlsx-border is-frozen' : 'xlsx-border';
        edge.style.cssText = [`left:${box.left - 1}px`, `top:${box.top - 1}px`, `width:${box.width + 1}px`, `height:${box.height + 1}px`,
          ...Object.entries(style.borders).map(([side, { kind, color }]) => `border-${side}:${BORDER_CSS[kind] || '1px solid'} ${color}`)].join(';');
        fragment.append(edge);
      }
    };
    const area = (rowFrom, rowTo, colFrom, colTo) => {
      if (rowFrom > rowTo || colFrom > colTo) return;
      if (pinned) {
        // The frozen part hides what scrolls under it.
        const band = document.createElement('div');
        band.className = 'xlsx-frozen';
        const x1 = geo.colX[colFrom], y1 = geo.rowY[rowFrom];
        band.style.cssText = `left:${x1 + dx}px;top:${y1 + dy}px;width:${geo.colX[colTo + 1] - x1}px;height:${geo.rowY[rowTo + 1] - y1}px`;
        fragment.append(band);
      }
      lines(rowFrom, rowTo, colFrom, colTo);
      for (let r = rowFrom; r <= rowTo; r++) {
        if (!rowPx(s, r)) continue;
        for (let c = colFrom; c <= colTo; c++) {
          const ref = refName(r, c);
          const merge = s.covered.get(ref);
          if (merge) {
            const key = `${merge.r1}:${merge.c1}`;
            if (drawn.has(key)) continue;
            drawn.add(key);
            const box = rect(merge.r1, merge.c1, merge.r2, merge.c2);
            const blank = document.createElement('div');
            blank.className = pinned ? 'xlsx-merge is-frozen' : 'xlsx-merge';
            blank.style.cssText = `left:${box.left + dx}px;top:${box.top + dy}px;width:${box.width}px;height:${box.height}px`;
            fragment.append(blank);
            drawCell(merge.r1, merge.c1, merge);
            continue;
          }
          drawCell(r, c, null);
        }
      }
    };
    area(r1, r2, c1, c2);
    pinned = true;
    dy = top; dx = 0;
    area(1, fr, c1, c2);
    dy = 0; dx = left;
    area(r1, r2, 1, fc);
    dy = top; dx = left;
    area(1, fr, 1, fc);
    pinned = false; dx = 0; dy = 0;
    // The edge of the frozen part, as in spreadsheet apps.
    if (fr) {
      const line = document.createElement('div');
      line.className = 'xlsx-freeze-line';
      line.style.cssText = `left:${left}px;top:${geo.rowY[fr + 1] + top - 1}px;width:${width}px;height:2px`;
      fragment.append(line);
    }
    if (fc) {
      const line = document.createElement('div');
      line.className = 'xlsx-freeze-line';
      line.style.cssText = `left:${geo.colX[fc + 1] + left - 1}px;top:${top}px;width:2px;height:${height}px`;
      fragment.append(line);
    }
    // Headers stay pinned to the top and left edges.
    const range = selectedRange();
    for (const c of [...Array.from({ length: Math.max(0, c2 - c1 + 1) }, (_, i) => c1 + i), ...Array.from({ length: fc }, (_, i) => i + 1)]) {
      const w = geo.colX[c + 1] - geo.colX[c];
      if (!w) continue;
      const head = document.createElement('div');
      head.className = c <= fc ? 'xlsx-col-head is-frozen' : 'xlsx-col-head';
      if (c >= range.c1 && c <= range.c2) head.classList.add('is-active');
      head.textContent = columnName(c);
      head.style.cssText = `left:${geo.colX[c] + (c <= fc ? left : 0)}px;top:${top}px;width:${w}px;height:${HEAD_H}px`;
      fragment.append(head);
    }
    for (const r of [...Array.from({ length: Math.max(0, r2 - r1 + 1) }, (_, i) => r1 + i), ...Array.from({ length: fr }, (_, i) => i + 1)]) {
      const h = geo.rowY[r + 1] - geo.rowY[r];
      if (!h) continue;
      const head = document.createElement('div');
      head.className = r <= fr ? 'xlsx-row-head is-frozen' : 'xlsx-row-head';
      if (r >= range.r1 && r <= range.r2) head.classList.add('is-active');
      head.textContent = String(r);
      head.style.cssText = `left:${left}px;top:${geo.rowY[r] + (r <= fr ? top : 0)}px;width:${HEAD_W}px;height:${h}px`;
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
    if (editing && !cellEditor.hidden) {
      const box = onScreen(editing.row, editing.col, editing.row, editing.col);
      cellEditor.style.left = `${box.left}px`;
      cellEditor.style.top = `${box.top}px`;
    }
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
    const box = onScreen(range.r1, range.c1, Math.min(range.r2, geo.rows), Math.min(range.c2, geo.cols));
    selectionBox.style.cssText = `left:${box.left - 1}px;top:${box.top - 1}px;width:${box.width + 1}px;height:${box.height + 1}px`;
    selectionBox.hidden = range.r1 === range.r2 && range.c1 === range.c2 && !sheet().covered.has(refName(anchor.row, anchor.col));
    const merge = sheet().covered.get(refName(anchor.row, anchor.col));
    const cellBox = merge ? onScreen(merge.r1, merge.c1, merge.r2, merge.c2) : onScreen(anchor.row, anchor.col, anchor.row, anchor.col);
    activeBox.style.cssText = `left:${cellBox.left - 1}px;top:${cellBox.top - 1}px;width:${cellBox.width + 1}px;height:${cellBox.height + 1}px`;
    const whole = range.r2 >= LAST_ROW || range.c2 >= LAST_COL;
    fillHandle.hidden = !canWrite() || !!editing || whole;
    const corner = range.r1 === range.r2 && range.c1 === range.c2 ? cellBox : box;
    fillHandle.style.cssText = `left:${corner.left + corner.width - 4}px;top:${corner.top + corner.height - 4}px`;
  }
  function editText(row, col) {
    const s = sheet();
    const cell = s.cells.get(refName(row, col));
    if (!cell) return '';
    const formula = formulaOf(s, cell);
    if (formula != null) return `=${fromFileFormula(formula)}`;
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
    // Frozen rows and columns are always in view, and cover the start of the rest.
    const f = frozen();
    const frozenW = geo.colX[f.cols + 1] - HEAD_W, frozenH = geo.rowY[f.rows + 1] - HEAD_H;
    if (col > f.cols) {
      if (box.left - HEAD_W - frozenW < scroll.scrollLeft) scroll.scrollLeft = box.left - HEAD_W - frozenW;
      else if (box.left + box.width > scroll.scrollLeft + viewW) scroll.scrollLeft = box.left + box.width - viewW;
    }
    if (row > f.rows) {
      if (box.top - HEAD_H - frozenH < scroll.scrollTop) scroll.scrollTop = box.top - HEAD_H - frozenH;
      else if (box.top + box.height > scroll.scrollTop + viewH) scroll.scrollTop = box.top + box.height - viewH;
    }
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
    record({ sheet: active, items });
    changed();
  }
  function record(entry) {
    if (savedAt > historyAt) savedAt = -1;
    history.splice(historyAt);
    history.push(entry);
    historyAt = history.length;
    if (history.length > 200) { history.shift(); historyAt--; savedAt--; }
  }
  /** A change to rows, columns, or their sizes, with undo: the XML it changes is kept as it was
   * before and after (`transaction` keeps cells alone). False when it could not be made. */
  function structural(work) {
    if (!canWrite()) return false;
    commitEdit();
    const drawings = sheetLinks(model, sheet()).filter(link => link.type === 'drawing').map(link => link.path);
    const capture = () => ({
      sheets: model.sheets.map(s => serialize(s.xml)),
      workbook: serialize(model.workbookXml),
      parts: Object.fromEntries(drawings.map(path => [path, model.parts[path]])),
    });
    const before = capture();
    try {
      work(sheet());
    } catch (error) {
      restore(before);
      geo = null;
      notice(error.message);
      selectionChanged();
      return false;
    }
    const after = capture();
    const only = (mine, other) => ({
      sheets: mine.sheets.map((text, i) => (text === other.sheets[i] ? null : text)),
      workbook: mine.workbook === other.workbook ? null : mine.workbook,
      parts: Object.fromEntries(Object.entries(mine.parts).filter(([path, bytes]) => bytes !== other.parts[path])),
    });
    const change = { before: only(before, after), after: only(after, before) };
    if (change.after.sheets.every(text => text == null) && change.after.workbook == null && !Object.keys(change.after.parts).length) return true;
    record({ sheet: active, snapshot: change });
    changed();
    return true;
  }
  /** Puts back sheets, the workbook, and drawings as `structural` kept them. */
  function restore(state) {
    state.sheets.forEach((text, index) => {
      if (text == null) return;
      const s = model.sheets[index];
      s.xml = parseXml(text, `Worksheet ${s.name}`);
      rereadSheet(s);
      model.touched.add(s);
    });
    if (state.workbook != null) {
      model.workbookXml = parseXml(state.workbook, 'Workbook XML');
      model.workbookTouched = true;
    }
    Object.assign(model.parts, state.parts);
  }
  function notice(text) {
    message = text;
    live.textContent = text;
    updateNote();
  }
  function changed() {
    message = '';
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
    if (entry.snapshot) {
      restore(direction < 0 ? entry.snapshot.before : entry.snapshot.after);
      historyAt += direction;
      changed();
      return;
    }
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
    writeParsed(s, row, col, parseInput(text, { ...separators, date1904: model.date1904 }), touch, styleIndex);
  }
  /** Writes an entry as `parseInput` reads one ({ kind, value, format }) into a cell. */
  function writeParsed(s, row, col, parsed, touch, styleIndex = null) {
    const ref = refName(row, col);
    const existing = s.cells.get(ref);
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
      f.textContent = toFileFormula(parsed.value);
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
        const wanted = typeof change === 'function' ? change(row, col) : change;
        if (!wanted) continue;
        const ref = refName(row, col);
        touch(ref);
        const cell = cellElement(s, row, col);
        const index = restyle(model, cell.getAttribute('s'), wanted);
        if (index) cell.setAttribute('s', String(index)); else cell.removeAttribute('s');
      }
    });
    scroll.focus({ preventScroll: true });
  }
  /** Borders on the selection: "all", "outside", "none", or one side. */
  function borderSelection(kind) {
    const s = sheet();
    const range = expandForMerges(selectedRange());
    const r2 = Math.min(range.r2, Math.max(s.maxRow, range.r1)), c2 = Math.min(range.c2, Math.max(s.maxCol, range.c1));
    formatSelection((row, col) => {
      const merge = s.covered.get(refName(row, col));
      const bottom = merge ? merge.r2 : row, right = merge ? merge.c2 : col;
      if (kind === 'all') return { borders: { left: true, right: true, top: true, bottom: true } };
      if (kind === 'none') return { borders: { left: false, right: false, top: false, bottom: false } };
      const sides = {};
      if ((kind === 'outside' || kind === 'left') && col === range.c1) sides.left = true;
      if ((kind === 'outside' || kind === 'right') && right >= c2) sides.right = true;
      if ((kind === 'outside' || kind === 'top') && row === range.r1) sides.top = true;
      if ((kind === 'outside' || kind === 'bottom') && bottom >= r2) sides.bottom = true;
      return Object.keys(sides).length ? { borders: sides } : null;
    });
  }
  function freeze(rows, cols) {
    if (rows >= LAST_ROW || cols >= LAST_COL) return;
    if (structural(s => freezePanes(s, rows, cols))) {
      scroll.scrollTop = 0;
      scroll.scrollLeft = 0;
      schedule();
    }
  }
  /** Merges the selection and centres it, or unmerges merged cells in it. */
  function toggleMerge() {
    const range = selectedRange();
    const s = sheet();
    const merged = s.merges.some(m => !(m.r2 < range.r1 || m.r1 > range.r2 || m.c2 < range.c1 || m.c1 > range.c2));
    if (!merged && range.r1 === range.r2 && range.c1 === range.c2) return;
    if (!merged && (range.r2 >= LAST_ROW || range.c2 >= LAST_COL)) { notice('Whole rows or columns cannot be merged.'); return; }
    const lost = !merged && [...s.cells].some(([ref, cell]) => {
      const { row, col } = parseRef(ref);
      return row >= range.r1 && row <= range.r2 && col >= range.c1 && col <= range.c2 && !(row === range.r1 && col === range.c1) && (direct(cell, 'v') || direct(cell, 'f') || direct(cell, 'is'));
    });
    const done = structural(sheetNow => {
      mergeRange(sheetNow, range, !merged);
      if (!merged && model.styles) {
        const cell = cellElement(sheetNow, range.r1, range.c1);
        cell.setAttribute('s', String(restyle(model, cell.getAttribute('s'), { align: 'center' })));
      }
    });
    if (done && lost) notice('Merged: only the top-left cell kept its contents (Undo brings the others back).');
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
    const box = merge ? onScreen(merge.r1, merge.c1, merge.r2, merge.c2) : onScreen(anchor.row, anchor.col, anchor.row, anchor.col);
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
    const box = onScreen(Math.min(r1, geo.rows), Math.min(c1, geo.cols), Math.min(r2, geo.rows), Math.min(c2, geo.cols));
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
  // Focus going to the list of functions keeps the edit open: a function picked there goes into it.
  const keepsEdit = () => [cellEditor, formulaInput].includes(document.activeElement) || functionsPanel.contains(document.activeElement);
  cellEditor.addEventListener('blur', () => { if (editing && !destroyed) setTimeout(() => { if (editing && !keepsEdit()) commitEdit(); }, 0); });

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
    setTimeout(() => { if (editing?.bar && !keepsEdit()) commitEdit(); }, 0);
  });

  // The list of functions
  const CATEGORIES = ['Math', 'Statistics', 'Logic', 'Text', 'Date and time', 'Lookup', 'Information'];
  let functionsBack = null, functionsShown = [], functionsActive = 0;
  function openFunctions() {
    closeMenu();
    hideAssist();
    // Where the edit was, to put the function there.
    const input = editing ? editorInput() : null;
    functionsBack = input ? { input, start: input.selectionStart, end: input.selectionEnd } : null;
    functionsPanel.style.top = `${formulaRow.offsetTop + formulaRow.offsetHeight + 4}px`;
    functionsPanel.hidden = false;
    fx.setAttribute('aria-expanded', 'true');
    functionsFilter.value = '';
    renderFunctions();
    functionsFilter.focus({ preventScroll: true });
  }
  function closeFunctions(refocus = true) {
    if (functionsPanel.hidden) return;
    functionsPanel.hidden = true;
    fx.setAttribute('aria-expanded', 'false');
    if (!refocus) return;
    if (editing && functionsBack) {
      functionsBack.input.focus({ preventScroll: true });
      functionsBack.input.setSelectionRange(functionsBack.start, functionsBack.end);
    } else {
      scroll.focus({ preventScroll: true });
    }
  }
  /** The functions matching the filter: by name first, then by what they do. */
  function matchingFunctions(query) {
    const all = Object.keys(FUNCTION_HELP);
    const q = query.trim().toLowerCase();
    if (!q) return all.sort((a, b) => CATEGORIES.indexOf(FUNCTION_HELP[a][2]) - CATEGORIES.indexOf(FUNCTION_HELP[b][2]) || a.localeCompare(b));
    const score = name => {
      const [, about, category] = FUNCTION_HELP[name];
      const lower = name.toLowerCase();
      if (lower.startsWith(q)) return 0;
      if (lower.includes(q)) return 1;
      return `${about} ${category}`.toLowerCase().includes(q) ? 2 : -1;
    };
    return all.map(name => [name, score(name)]).filter(([, s]) => s >= 0)
      .sort((a, b) => a[1] - b[1] || a[0].length - b[0].length || a[0].localeCompare(b[0])).map(([name]) => name);
  }
  function renderFunctions() {
    const query = functionsFilter.value;
    functionsShown = matchingFunctions(query);
    functionsActive = 0;
    const total = Object.keys(FUNCTION_HELP).length;
    functionsCount.textContent = !query.trim() ? `${total} functions${canWrite() ? '. Pick one to put it in the cell.' : ''}`
      : functionsShown.length ? `${functionsShown.length} of ${total}` : 'No function matches.';
    const items = [];
    let group = null;
    functionsShown.forEach((name, index) => {
      const [args, about, category, example] = FUNCTION_HELP[name];
      if (!query.trim() && category !== group) {
        group = category;
        const heading = document.createElement('div');
        heading.className = 'xlsx-functions-group';
        heading.textContent = category;
        items.push(heading);
      }
      const item = document.createElement('div');
      item.className = 'xlsx-function';
      item.id = `xlsx-function-${name.replace(/\W/g, '-')}`;
      item.setAttribute('role', 'option');
      item.setAttribute('aria-selected', String(index === functionsActive));
      const signature = document.createElement('code');
      signature.className = 'xlsx-function-sig';
      signature.textContent = `${name}(${args})`;
      item.append(signature);
      if (query.trim()) {
        const tag = document.createElement('span');
        tag.className = 'xlsx-function-tag';
        tag.textContent = category;
        item.append(tag);
      }
      const text = document.createElement('p');
      text.textContent = about;
      const sample = document.createElement('p');
      sample.className = 'xlsx-function-example';
      const code = document.createElement('code');
      code.textContent = example;
      sample.append('Example: ', code);
      item.append(text, sample);
      // The filter keeps the keyboard.
      item.addEventListener('pointerdown', event => event.preventDefault());
      item.addEventListener('click', () => insertFunction(name));
      items.push(item);
    });
    functionsList.replaceChildren(...items);
    functionsList.scrollTop = 0;
    markActiveFunction();
  }
  function markActiveFunction() {
    const name = functionsShown[functionsActive];
    for (const item of functionsList.querySelectorAll('.xlsx-function')) {
      const on = name != null && item.id === `xlsx-function-${name.replace(/\W/g, '-')}`;
      item.setAttribute('aria-selected', String(on));
      if (on) { item.scrollIntoView?.({ block: 'nearest' }); functionsFilter.setAttribute('aria-activedescendant', item.id); }
    }
  }
  /** Puts `NAME(` into the formula being typed (in place of a name begun), or starts one. */
  function insertFunction(name) {
    if (!canWrite()) return;
    const back = functionsBack;
    closeFunctions(false);
    if (editing && back) {
      const { input } = back;
      input.focus({ preventScroll: true });
      if (!input.value.startsWith('=')) {
        input.value = `=${name}(`;
        input.setSelectionRange(input.value.length, input.value.length);
      } else {
        const typed = back.start === back.end ? formulaWordAt(input.value, back.start) : null;
        input.setRangeText(`${name}(`, typed ? typed.start : back.start, back.end, 'end');
      }
      syncEditors(input);
    } else {
      if (editing) commitEdit();
      startEdit(`=${name}(`);
    }
    placePoint();
    updateAssist();
  }
  fx.addEventListener('pointerdown', event => event.preventDefault());
  fx.addEventListener('click', () => (functionsPanel.hidden ? openFunctions() : closeFunctions()));
  functionsClose.addEventListener('click', () => closeFunctions());
  functionsFilter.addEventListener('input', renderFunctions);
  functionsPanel.addEventListener('keydown', event => {
    // The page's own keys (Escape closes the preview) stay out.
    event.stopPropagation();
    if (event.key === 'Escape') { event.preventDefault(); closeFunctions(); }
    else if ((event.key === 'ArrowDown' || event.key === 'ArrowUp') && functionsShown.length) {
      event.preventDefault();
      functionsActive = (functionsActive + (event.key === 'ArrowDown' ? 1 : -1) + functionsShown.length) % functionsShown.length;
      markActiveFunction();
    } else if (event.key === 'Enter' && event.target === functionsFilter && functionsShown.length) {
      event.preventDefault();
      insertFunction(functionsShown[functionsActive]);
    }
  });
  const outsideFunctions = event => {
    if (!functionsPanel.hidden && !functionsPanel.contains(event.target) && event.target !== fx) closeFunctions();
  };
  document.addEventListener('pointerdown', outsideFunctions, true);

  // Pointer
  function pointAt(event) {
    const box = canvas.getBoundingClientRect();
    const x = event.clientX - box.left, y = event.clientY - box.top;
    const head = { col: x - scroll.scrollLeft < HEAD_W, row: y - scroll.scrollTop < HEAD_H };
    // Over frozen rows or columns, the place is as if not scrolled.
    const f = frozen();
    const inRows = f.rows && y - scroll.scrollTop < geo.rowY[f.rows + 1], inCols = f.cols && x - scroll.scrollLeft < geo.colX[f.cols + 1];
    const px = inCols ? x - scroll.scrollLeft : x, py = inRows ? y - scroll.scrollTop : y;
    return {
      x: px, y: py, head,
      row: find(geo.rowY, geo.rows, Math.max(HEAD_H, py)),
      col: find(geo.colX, geo.cols, Math.max(HEAD_W, px)),
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
    if (event.target === fillHandle) {
      dragging = { kind: 'fill', source: expandForMerges(selectedRange()), target: null };
      canvas.setPointerCapture?.(event.pointerId);
      event.preventDefault();
      return;
    }
    // The edge of a column or row header: drag to resize.
    const edge = !canWrite() || (p.head.row && p.head.col) ? 0 : p.head.row ? colEdge(p.x) : p.head.col ? rowEdge(p.y) : 0;
    if (edge) {
      const axis = p.head.row ? 'col' : 'row';
      const edges = axis === 'col' ? geo.colX : geo.rowY;
      dragging = { kind: 'size', axis, index: edge, start: axis === 'col' ? p.x : p.y, px: edges[edge + 1] - edges[edge] };
      placeGuide(dragging, dragging.start);
      canvas.setPointerCapture?.(event.pointerId);
      event.preventDefault();
      scroll.focus({ preventScroll: true });
      return;
    }
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
    if (!geo) return;
    const p = pointAt(event);
    if (!dragging) {
      // Over a header's edge, show that it can be dragged.
      const cursor = !canWrite() || (p.head.row && p.head.col) ? '' : p.head.row && colEdge(p.x) ? 'col-resize' : p.head.col && rowEdge(p.y) ? 'row-resize' : '';
      if (canvas.style.cursor !== cursor) canvas.style.cursor = cursor;
      return;
    }
    if (dragging.kind === 'size') {
      placeGuide(dragging, Math.max(dragging.start - dragging.px, dragging.axis === 'col' ? p.x : p.y));
      return;
    }
    if (dragging.kind === 'fill') {
      dragging.target = fillTarget(dragging.source, p);
      const area = dragging.target || dragging.source;
      const box = onScreen(area.r1, area.c1, area.r2, area.c2);
      fillBox.style.cssText = `left:${box.left - 1}px;top:${box.top - 1}px;width:${box.width + 1}px;height:${box.height + 1}px`;
      fillBox.hidden = !dragging.target;
      return;
    }
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
  // Pointing keeps the keyboard in the formula.
  canvas.addEventListener('mousedown', event => { if (dragging === 'point') event.preventDefault(); });
  canvas.addEventListener('pointerup', event => {
    const drag = dragging;
    dragging = null;
    sizeGuide.hidden = true;
    fillBox.hidden = true;
    if (drag?.kind === 'size') {
      const p = pointAt(event);
      const px = Math.max(0, Math.round(drag.px + (drag.axis === 'col' ? p.x : p.y) - drag.start));
      if (px !== drag.px) setSize(drag.axis, drag.index, px);
    } else if (drag?.kind === 'fill' && drag.target) {
      fill(drag.source, drag.target);
    }
  });
  canvas.addEventListener('pointercancel', () => {
    dragging = null;
    sizeGuide.hidden = true;
    fillBox.hidden = true;
  });
  canvas.addEventListener('dblclick', event => {
    if (event.target === cellEditor) return;
    if (event.target === fillHandle) { fillToNeighbour(); return; }
    const p = pointAt(event);
    if (p.head.row && p.head.col) return;
    if (p.head.row) { const col = colEdge(p.x); if (col) fitColumns(...spanOf('col', col)); return; }
    if (p.head.col) { const row = rowEdge(p.y); if (row) fitRows(...spanOf('row', row)); return; }
    startEdit();
  });
  canvas.addEventListener('contextmenu', event => {
    if (event.target === cellEditor) return;
    event.preventDefault();
    if (editing) commitEdit();
    layout();
    const p = pointAt(event);
    const range = selectedRange();
    const wholeRows = range.c1 === 1 && range.c2 >= LAST_COL, wholeCols = range.r1 === 1 && range.r2 >= LAST_ROW;
    const inRows = p.row >= range.r1 && p.row <= range.r2, inCols = p.col >= range.c1 && p.col <= range.c2;
    // Right-clicking outside the selection selects what was clicked first.
    if (p.head.row && p.head.col) {
      anchor = { row: 1, col: 1 }; focus = { row: LAST_ROW, col: LAST_COL };
    } else if (p.head.row) {
      if (!(wholeCols && inCols)) { anchor = { row: 1, col: p.col }; focus = { row: LAST_ROW, col: p.col }; }
    } else if (p.head.col) {
      if (!(wholeRows && inRows)) { anchor = { row: p.row, col: 1 }; focus = { row: p.row, col: LAST_COL }; }
    } else if (!(inRows && inCols)) {
      select(p.row, p.col);
    }
    scroll.focus({ preventScroll: true });
    selectionChanged();
    openMenu(event.clientX, event.clientY);
  });

  // Rows and columns: sizes, inserting, deleting, hiding
  /** The column whose right edge is at x (within a few pixels), or 0. */
  function colEdge(x) {
    const col = find(geo.colX, geo.cols, x);
    if (geo.colX[col + 1] - x <= 4) return col;
    if (x - geo.colX[col] <= 4 && col > 1) return col - 1;
    return 0;
  }
  function rowEdge(y) {
    const row = find(geo.rowY, geo.rows, y);
    if (geo.rowY[row + 1] - y <= 3) return row;
    if (y - geo.rowY[row] <= 3 && row > 1) return row - 1;
    return 0;
  }
  function placeGuide(drag, at) {
    const edges = drag.axis === 'col' ? geo.colX : geo.rowY;
    const position = edges[drag.index] + Math.max(0, drag.px + at - drag.start);
    sizeGuide.style.cssText = drag.axis === 'col'
      ? `left:${position - 1}px;top:${scroll.scrollTop}px;width:2px;height:${scroll.clientHeight}px`
      : `top:${position - 1}px;left:${scroll.scrollLeft}px;height:2px;width:${scroll.clientWidth}px`;
    sizeGuide.hidden = false;
  }
  /** The columns (or rows) a header action applies to: all those selected whole, when `index` is
   * one of them, or just `index`. */
  function spanOf(axis, index) {
    const range = selectedRange();
    if (axis === 'col' && range.r1 === 1 && range.r2 >= LAST_ROW && index >= range.c1 && index <= range.c2) return [range.c1, Math.min(range.c2, Math.max(sheet().maxCol, range.c1))];
    if (axis === 'row' && range.c1 === 1 && range.c2 >= LAST_COL && index >= range.r1 && index <= range.r2) return [range.r1, Math.min(range.r2, Math.max(sheet().maxRow, range.r1))];
    return [index, index];
  }
  const MAX_ROW_CHANGES = 10000;
  /** Sets the size of a column or row (and the others selected with it); 0 hides them. */
  function setSize(axis, index, px) {
    const [lo, hi] = spanOf(axis, index);
    structural(s => {
      if (axis === 'col') {
        setColumns(s, lo, hi, col => {
          if (!px) { col.setAttribute('hidden', '1'); return; }
          col.setAttribute('width', String(Math.round(px / 7 * 100) / 100));
          col.setAttribute('customWidth', '1');
          col.removeAttribute('hidden');
        });
      } else {
        if (hi - lo >= MAX_ROW_CHANGES) throw new Error(`Rows can be resized at most ${MAX_ROW_CHANGES} at a time.`);
        setRows(s, lo, hi, row => {
          if (!px) { row.setAttribute('hidden', '1'); return; }
          row.setAttribute('ht', String(Math.round(px * 0.75 * 100) / 100));
          row.setAttribute('customHeight', '1');
          row.removeAttribute('hidden');
        });
      }
    });
  }
  /** Columns as wide as their widest text (wrapped and merged cells aside). */
  function fitColumns(lo, hi) {
    const s = sheet();
    const widths = new Map();
    for (const ref of s.cells.keys()) {
      const { row, col } = parseRef(ref);
      if (col < lo || col > hi || s.covered.has(ref) || s.hiddenRows.has(row)) continue;
      const info = shown(row, col);
      if (!info?.text || info.style.wrap) continue;
      widths.set(col, Math.max(widths.get(col) || 0, Math.ceil(textWidth(info.text, cellFont(info.style)) / FONT_SCALE) + 10));
    }
    structural(s => {
      for (let col = lo; col <= hi; col++) {
        const px = Math.min(1800, widths.get(col) || s.defaultColPx);
        setColumns(s, col, col, node => {
          node.setAttribute('width', String(Math.round(px / 7 * 100) / 100));
          node.setAttribute('customWidth', '1');
          node.removeAttribute('hidden');
        }, { reread: false });
      }
      rereadSheet(s);
    });
  }
  /** Rows back to the height of their text (the sheet's default height). */
  function fitRows(lo, hi) {
    structural(s => setRows(s, lo, hi, row => { row.removeAttribute('ht'); row.removeAttribute('customHeight'); }, { existingOnly: true }));
  }
  function hideOrShow(axis, hide) {
    const range = selectedRange();
    if (axis === 'col') {
      structural(s => setColumns(s, range.c1, range.c2, col => {
        if (hide) { col.setAttribute('hidden', '1'); return; }
        col.removeAttribute('hidden');
        if (!Number(col.getAttribute('width'))) col.setAttribute('width', String(Math.round(s.defaultColPx / 7 * 100) / 100));
      }, { existingOnly: !hide }));
    } else {
      const hi = range.r2;
      structural(s => {
        if (hide && hi - range.r1 >= MAX_ROW_CHANGES) throw new Error(`Rows can be hidden at most ${MAX_ROW_CHANGES} at a time.`);
        setRows(s, range.r1, hi, row => {
          if (hide) { row.setAttribute('hidden', '1'); return; }
          row.removeAttribute('hidden');
          if (row.getAttribute('ht') === '0') { row.removeAttribute('ht'); row.removeAttribute('customHeight'); }
        }, { existingOnly: !hide });
      });
    }
  }
  function insertOrDelete(axis, at, count) {
    structural(() => restructure(model, active, axis, at, count));
  }
  function anyHidden(axis) {
    const range = selectedRange();
    const s = sheet();
    if (axis === 'col') return s.cols.some(col => (col.hidden || col.px === 0) && col.max >= range.c1 && col.min <= range.c2);
    return [...s.hiddenRows].some(row => row >= range.r1 && row <= range.r2) || [...s.rowHeights].some(([row, pt]) => !pt && row >= range.r1 && row <= range.r2);
  }

  // The right-click menu
  function menuItems() {
    const range = selectedRange();
    const rows = range.r2 - range.r1 + 1, cols = range.c2 - range.c1 + 1;
    const wholeRows = range.c1 === 1 && range.c2 >= LAST_COL, wholeCols = range.r1 === 1 && range.r2 >= LAST_ROW;
    const count = (n, word) => (n === 1 ? `${word}` : `${n} ${word}s`);
    const items = [];
    if (!wholeCols) {
      items.push([`Insert ${count(rows, 'row')} above`, () => insertOrDelete('row', range.r1, rows)]);
      items.push([`Insert ${count(rows, 'row')} below`, () => insertOrDelete('row', range.r2 + 1, rows)]);
    }
    if (!wholeRows) {
      items.push([`Insert ${count(cols, 'column')} left`, () => insertOrDelete('col', range.c1, cols)]);
      items.push([`Insert ${count(cols, 'column')} right`, () => insertOrDelete('col', range.c2 + 1, cols)]);
    }
    items.push('-');
    if (!wholeCols) items.push([`Delete ${count(rows, 'row')}`, () => insertOrDelete('row', range.r1, -rows)]);
    if (!wholeRows) items.push([`Delete ${count(cols, 'column')}`, () => insertOrDelete('col', range.c1, -cols)]);
    items.push('-');
    if (wholeCols) {
      if (!wholeRows) items.push([`Hide ${count(cols, 'column')}`, () => hideOrShow('col', true)]);
      if (anyHidden('col')) items.push(['Unhide columns', () => hideOrShow('col', false)]);
      items.push([cols === 1 ? 'Fit width to contents' : 'Fit widths to contents', () => fitColumns(...spanOf('col', range.c1))]);
    }
    if (wholeRows) {
      if (!wholeCols) items.push([`Hide ${count(rows, 'row')}`, () => hideOrShow('row', true)]);
      if (anyHidden('row')) items.push(['Unhide rows', () => hideOrShow('row', false)]);
      items.push([rows === 1 ? 'Reset height' : 'Reset heights', () => fitRows(...spanOf('row', range.r1))]);
    }
    if (!wholeRows && !wholeCols) {
      if (rows > 1) items.push([`Fill down${IS_MAC ? ' (⌘D)' : ' (Ctrl+D)'}`, fillDown]);
      items.push(['Clear contents', clearSelection]);
    }
    return items;
  }
  function openMenu(x, y, list = menuItems()) {
    const items = list.filter((item, i, all) => item !== '-' || (i > 0 && all[i - 1] !== '-' && i < all.length - 1));
    menu.replaceChildren(...items.map(item => {
      if (item === '-') {
        const line = document.createElement('div');
        line.className = 'menu-separator';
        return line;
      }
      const [label, action] = item;
      const button = document.createElement('button');
      button.type = 'button';
      button.setAttribute('role', 'menuitem');
      button.textContent = label;
      button.disabled = !canWrite();
      button.addEventListener('click', () => {
        closeMenu();
        action();
        scroll.focus({ preventScroll: true });
      });
      return button;
    }));
    menu.hidden = false;
    menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - menu.offsetWidth - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - menu.offsetHeight - 8))}px`;
    menu.querySelector('button:not(:disabled)')?.focus({ preventScroll: true });
  }
  function closeMenu() {
    menu.hidden = true;
  }
  menu.addEventListener('keydown', event => {
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    const at = buttons.indexOf(document.activeElement);
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeMenu(); scroll.focus({ preventScroll: true }); }
    else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      buttons[(at + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
    }
    event.stopPropagation();
  });
  const outsideMenu = event => { if (!menu.hidden && !menu.contains(event.target)) closeMenu(); };
  document.addEventListener('pointerdown', outsideMenu, true);
  scroll.addEventListener('scroll', closeMenu);

  // Filling
  /** The range a drag of the fill handle from `source` to point `p` fills, or null. */
  function fillTarget(source, p) {
    const below = p.row - source.r2, above = source.r1 - p.row, right = p.col - source.c2, left = source.c1 - p.col;
    const down = Math.max(below, above, 0), across = Math.max(right, left, 0);
    if (!down && !across) return null;
    if (down >= across) return below > 0 ? { ...source, r2: p.row } : { ...source, r1: p.row };
    return right > 0 ? { ...source, c2: p.col } : { ...source, c1: p.col };
  }
  /** What a cell holds, for `fillSeries`. */
  function entryOf(cell) {
    if (!cell) return { kind: 'empty' };
    if (direct(cell, 'f')) return { kind: 'formula' };
    const value = cachedValue(model, cell);
    if (typeof value === 'number') return { kind: 'number', value, date: isDateFormat(cellStyle(model, cell.getAttribute('s')).numFmt) };
    if (typeof value === 'string') return { kind: 'text', value };
    return { kind: 'other' };
  }
  /** Copies a cell to row, col, its formula's relative references moved by dRow and dCol. */
  function copyCell(s, source, row, col, dRow, dCol) {
    const ref = refName(row, col);
    if (!source) {
      s.cells.get(ref)?.remove();
      s.cells.delete(ref);
      return;
    }
    const copy = source.cloneNode(true);
    copy.setAttribute('r', ref);
    const f = direct(copy, 'f');
    if (f) {
      const formula = formulaOf(s, source) ?? f.textContent;
      for (const name of ['t', 'si', 'ref']) f.removeAttribute(name);
      f.textContent = shiftFormula(formula, dRow, dCol);
      for (const node of [...copy.children]) if (node.localName === 'v') node.remove();
    }
    placeCell(s, ref, copy);
    s.maxRow = Math.max(s.maxRow, row);
    s.maxCol = Math.max(s.maxCol, col);
    updateDimension(s);
  }
  /** Fills `target` (the source range grown one way) from `source`: series go on (numbers, dates,
   * "Item 1", weekdays, months), and anything else is copied, formulas moving as they go.
   * `copy` copies always, as Ctrl+D does. */
  function fill(source, target, { copy = false } = {}) {
    if (!canWrite()) return;
    commitEdit();
    const s = sheet();
    const down = target.r1 !== source.r1 || target.r2 !== source.r2;
    const forward = down ? target.r2 > source.r2 : target.c2 > source.c2;
    const length = down ? source.r2 - source.r1 + 1 : source.c2 - source.c1 + 1;
    const extra = down ? target.r2 - target.r1 + 1 - length : target.c2 - target.c1 + 1 - length;
    const lines = down ? source.c2 - source.c1 + 1 : source.r2 - source.r1 + 1;
    if (extra <= 0) return;
    if (extra * lines > MAX_FORMAT_CELLS) { notice('That is too many cells to fill at once.'); return; }
    if (s.merges.some(m => !(m.r2 < target.r1 || m.r1 > target.r2 || m.c2 < target.c1 || m.c1 > target.c2))) {
      notice('Cells cannot be filled across merged cells.');
      return;
    }
    transaction(touch => {
      for (let line = 0; line < lines; line++) {
        const at = i => (down ? { row: source.r1 + i, col: source.c1 + line } : { row: source.r1 + line, col: source.c1 + i });
        const cells = Array.from({ length }, (_, i) => { const { row, col } = at(i); return s.cells.get(refName(row, col)) || null; });
        const series = copy ? null : fillSeries(cells.map(entryOf));
        for (let step = 0; step < extra; step++) {
          const k = forward ? length + step : -1 - step;
          const { row, col } = at(k);
          const from = ((k % length) + length) % length;
          const ref = refName(row, col);
          touch(ref);
          const existing = s.cells.get(ref);
          if (existing) unshare(s, existing, touch);
          const value = series?.(k);
          if (value) writeParsed(s, row, col, value, touch, Number(cells[from]?.getAttribute('s') || 0));
          else copyCell(s, cells[from], row, col, down ? k - from : 0, down ? 0 : k - from);
        }
      }
    });
    anchor = { row: target.r1, col: target.c1 };
    focus = { row: target.r2, col: target.c2 };
    selectionChanged();
  }
  /** Double-clicking the fill handle: down as far as the column beside has data. */
  function fillToNeighbour() {
    const source = expandForMerges(selectedRange());
    const side = source.c1 > 1 && filled(source.r2 + 1, source.c1 - 1) ? source.c1 - 1 : filled(source.r2 + 1, source.c2 + 1) ? source.c2 + 1 : 0;
    if (!side) return;
    let end = source.r2 + 1;
    while (end < LAST_ROW && filled(end + 1, side)) end++;
    fill(source, { ...source, r2: end });
  }
  /** Ctrl+D: the first row of the selection copied down through it. */
  function fillDown() {
    const range = expandForMerges(selectedRange());
    if (range.r2 >= LAST_ROW || range.c2 >= LAST_COL || range.r1 === range.r2) return;
    fill({ ...range, r2: range.r1 }, range, { copy: true });
  }

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
    else if (mod && key.toLowerCase() === 'd') { event.preventDefault(); fillDown(); }
    else if (mod && key.toLowerCase() === 'u') { event.preventDefault(); formatSelection({ underline: !activeStyle().underline }); }
    else if (mod && key === '5') { event.preventDefault(); formatSelection({ strike: !activeStyle().strike }); }
    else if (key === 'ContextMenu' || (key === 'F10' && event.shiftKey)) {
      event.preventDefault();
      const box = rect(anchor.row, anchor.col, anchor.row, anchor.col);
      const at = canvas.getBoundingClientRect();
      openMenu(at.left + box.left + box.width / 2, at.top + box.top + box.height / 2);
    }
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
      document.removeEventListener('pointerdown', outsideMenu, true);
      document.removeEventListener('pointerdown', outsideFunctions, true);
      if (frame) cancelAnimationFrame(frame);
      root.remove();
    },
    save: persist,
    get dirty() { return dirty(); },
  };
}
