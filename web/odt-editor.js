/**
 * Reusable, preservation-first OpenDocument Text editor. See odt-editor.md.
 *
 * The page is one editable surface: the browser does the typing, selection, Enter and Backspace,
 * and each change is written back into content.xml. Every paragraph and list keeps the XML node it
 * came from, so attributes and styles survive edits; content the editor can't edit (images, fields,
 * notes, bookmarks, indexes) shows as locked pieces that are written back unchanged.
 */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';

const OFFICE = 'urn:oasis:names:tc:opendocument:xmlns:office:1.0';
const TEXT = 'urn:oasis:names:tc:opendocument:xmlns:text:1.0';
const STYLE = 'urn:oasis:names:tc:opendocument:xmlns:style:1.0';
const FO = 'urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0';
const TABLE = 'urn:oasis:names:tc:opendocument:xmlns:table:1.0';
const DRAW = 'urn:oasis:names:tc:opendocument:xmlns:drawing:1.0';
const XLINK = 'http://www.w3.org/1999/xlink';
const SVG = 'urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0';
const children = node => [...node.children];
const named = (node, ns, local) => children(node).filter(child => child.namespaceURI === ns && child.localName === local);
const create = (doc, ns, prefix, local) => doc.createElementNS(ns, `${prefix}:${local}`);
const is = (node, ns, ...locals) => node?.nodeType === 1 && node.namespaceURI === ns && locals.includes(node.localName);

function hasValidMimetypeHeader(bytes) {
  if (bytes.length < 30) return false;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, true) !== 0x04034b50 || view.getUint16(8, true) !== 0) return false;
  const nameLength = view.getUint16(26, true), extraLength = view.getUint16(28, true);
  const start = 30 + nameLength + extraLength, size = view.getUint32(18, true);
  if (extraLength !== 0 || start + size > bytes.length || strFromU8(bytes.subarray(30, 30 + nameLength)) !== 'mimetype') return false;
  return strFromU8(bytes.subarray(start, start + size)) === 'application/vnd.oasis.opendocument.text';
}

function parse(source, label) {
  if (/<!DOCTYPE|<!ENTITY/i.test(source)) throw new Error(`${label} contains unsupported XML entities.`);
  const xml = new DOMParser().parseFromString(source, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error(`${label} is invalid.`);
  return xml;
}

export function openOdt(bytes) {
  if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
  if (bytes.length > 25 * 1024 * 1024) throw new Error('Documents up to 25 MB are supported.');
  if (!hasValidMimetypeHeader(bytes)) throw new Error('ODT package has an invalid mimetype header.');
  let total = 0, count = 0;
  const parts = unzipSync(bytes, { filter(part) {
    total += part.originalSize;
    if (++count > 4096 || total > 100 * 1024 * 1024) throw new Error('Document package is too large.');
    if (/^META-INF\/.*signatures.*\.xml$/i.test(part.name)) throw new Error('Digitally signed documents cannot be edited.');
    return true;
  }});
  if (!parts.mimetype || strFromU8(parts.mimetype) !== 'application/vnd.oasis.opendocument.text' || !parts['content.xml']) {
    throw new Error('This is not an OpenDocument Text file.');
  }
  const xml = parse(strFromU8(parts['content.xml']), 'Document XML');
  if (xml.getElementsByTagNameNS(OFFICE, 'text').length === 0) throw new Error('Document body is missing.');
  const styleDocs = [];
  if (parts['styles.xml']) styleDocs.push(parse(strFromU8(parts['styles.xml']), 'Document styles'));
  const model = {
    xml, parts, styleDocs,
    get body() { return xml.getElementsByTagNameNS(OFFICE, 'text')[0]; },
    styleByName: new Map(), listStyles: new Map(),
    export() {
      parts['content.xml'] = strToU8(new XMLSerializer().serializeToString(xml));
      // ODF requires the mimetype entry first and stored without compression.
      const ordered = { mimetype: [parts.mimetype, { level: 0 }] };
      for (const [name, data] of Object.entries(parts)) if (name !== 'mimetype') ordered[name] = data;
      return zipSync(ordered, { level: 6 });
    },
  };
  indexStyles(model);
  return model;
}

/** Style lookups by name: styles.xml first, then the document's own (automatic) styles. */
function indexStyles(model) {
  model.styleByName.clear();
  model.listStyles.clear();
  for (const doc of [...model.styleDocs, model.xml]) {
    for (const style of doc.getElementsByTagNameNS(STYLE, 'style')) {
      const name = style.getAttributeNS(STYLE, 'name');
      if (name) model.styleByName.set(`${style.getAttributeNS(STYLE, 'family')}:${name}`, style);
    }
    for (const style of doc.getElementsByTagNameNS(TEXT, 'list-style')) {
      const name = style.getAttributeNS(STYLE, 'name');
      if (name) model.listStyles.set(name, style);
    }
  }
}

function styleProp(model, family, name, propsName, ns, attr, depth = 0) {
  if (!name || depth > 12) return null;
  const style = model.styleByName.get(`${family}:${name}`);
  if (!style) return null;
  const props = named(style, STYLE, propsName)[0];
  const value = props?.getAttributeNS(ns, attr);
  if (value) return value;
  return styleProp(model, family, style.getAttributeNS(STYLE, 'parent-style-name'), propsName, ns, attr, depth + 1);
}

const TEXT_PROPS = [
  ['bold', FO, 'font-weight'], ['italic', FO, 'font-style'], ['underline', STYLE, 'text-underline-style'],
  ['strike', STYLE, 'text-line-through-style'], ['color', FO, 'color'], ['size', FO, 'font-size'],
  ['background', FO, 'background-color'], ['position', STYLE, 'text-position'], ['font', STYLE, 'font-name'],
];

/** CSS for a text or paragraph style's character formatting. */
function textCss(model, family, name) {
  const css = {};
  const get = (ns, attr) => styleProp(model, family, name, 'text-properties', ns, attr);
  const weight = get(FO, 'font-weight');
  if (weight) css.fontWeight = weight === 'bold' ? '700' : weight === 'normal' ? '400' : weight;
  const italic = get(FO, 'font-style');
  if (italic) css.fontStyle = italic === 'italic' || italic === 'oblique' ? 'italic' : 'normal';
  const lines = [];
  const underline = get(STYLE, 'text-underline-style');
  if (underline && underline !== 'none') lines.push('underline');
  const strike = get(STYLE, 'text-line-through-style');
  if (strike && strike !== 'none') lines.push('line-through');
  if (underline || strike) css.textDecorationLine = lines.join(' ') || 'none';
  const color = get(FO, 'color');
  if (/^#[0-9a-f]{6}$/i.test(color || '')) css.color = color;
  const size = get(FO, 'font-size');
  if (/^\d+(\.\d+)?(pt|px|cm|mm|in|%)$/.test(size || '')) css.fontSize = size;
  const background = get(FO, 'background-color');
  if (/^#[0-9a-f]{6}$/i.test(background || '')) css.backgroundColor = background;
  const position = get(STYLE, 'text-position');
  if (/^super/.test(position || '') || /^\d/.test(position || '') && parseFloat(position) > 0) css.verticalAlign = 'super';
  else if (/^sub/.test(position || '') || /^-\d/.test(position || '')) css.verticalAlign = 'sub';
  if (css.verticalAlign) css.fontSize ||= '0.75em';
  const font = get(STYLE, 'font-name') || get(FO, 'font-family');
  if (font && /^[\w .'-]+$/.test(font)) css.fontFamily = `'${font.replace(/'/g, '')}', Georgia, serif`;
  return css;
}

function paragraphCss(model, name) {
  const css = textCss(model, 'paragraph', name);
  const get = attr => styleProp(model, 'paragraph', name, 'paragraph-properties', FO, attr);
  const align = get('text-align');
  if (align) css.textAlign = { start: 'left', end: 'right', left: 'left', right: 'right', center: 'center', justify: 'justify' }[align] || '';
  for (const [attr, prop] of [['margin-left', 'marginLeft'], ['margin-right', 'marginRight'], ['text-indent', 'textIndent'], ['margin-top', 'marginTop'], ['margin-bottom', 'marginBottom']]) {
    const value = get(attr);
    if (/^-?\d+(\.\d+)?(cm|mm|in|pt|px)$/.test(value || '')) css[prop] = value;
  }
  const background = get('background-color');
  if (/^#[0-9a-f]{6}$/i.test(background || '')) css.backgroundColor = background;
  return css;
}

function lengthPx(value) {
  const match = /^(-?[\d.]+)(cm|mm|in|pt|px|pc)?$/.exec(value || '');
  if (!match) return null;
  const n = Number(match[1]);
  return Math.round(n * ({ cm: 37.8, mm: 3.78, in: 96, pt: 4 / 3, px: 1, pc: 16 }[match[2] || 'px']));
}

// Elements of a flow (the body, a section, a cell) that hold no visible content; kept as they are.
const HIDDEN_FLOW = new Set(['sequence-decls', 'variable-decls', 'user-field-decls', 'dde-connection-decls', 'tracked-changes', 'forms', 'named-expressions', 'database-ranges', 'soft-page-break', 'calculation-settings', 'content-validations', 'label-ranges', 'data-pilot-tables']);
const ICONS = {
  undo: '<path d="M4 7h7a3 3 0 0 1 0 6H8"/><path d="M6.5 4.5 4 7l2.5 2.5"/>',
  redo: '<path d="M12 7H5a3 3 0 0 0 0 6h3"/><path d="M9.5 4.5 12 7 9.5 9.5"/>',
  bold: '<path d="M5 3h4a2.5 2.5 0 0 1 0 5H5zM5 8h4.5a2.5 2.5 0 0 1 0 5H5z"/>',
  italic: '<path d="M7 3h5M4 13h5M10 3 6 13"/>',
  underline: '<path d="M5 3v4a3 3 0 0 0 6 0V3M4 13.5h8"/>',
  strike: '<path d="M3 8h10M11 4.5C10.5 3.6 9.4 3 8 3 6.3 3 5 3.9 5 5.2c0 1 .7 1.6 1.7 2M5 11c.5 1.2 1.6 2 3.1 2 1.9 0 3.1-1 3.1-2.3 0-.6-.2-1.1-.6-1.5"/>',
  color: '<path d="M4.5 12 8 3l3.5 9M6 9h4"/>',
  highlight: '<path d="m9.5 3 3.5 3.5-5.5 5.5H4v-3.5z"/><path d="M3 14h10"/>',
  left: '<path d="M3 4h10M3 7h6M3 10h10M3 13h6"/>',
  center: '<path d="M3 4h10M5 7h6M3 10h10M5 13h6"/>',
  right: '<path d="M3 4h10M7 7h6M3 10h10M7 13h6"/>',
  justify: '<path d="M3 4h10M3 7h10M3 10h10M3 13h10"/>',
  bullets: '<circle cx="3.5" cy="4.5" r=".9" fill="currentColor"/><circle cx="3.5" cy="8" r=".9" fill="currentColor"/><circle cx="3.5" cy="11.5" r=".9" fill="currentColor"/><path d="M6.5 4.5H13M6.5 8H13M6.5 11.5H13"/>',
  numbers: '<path d="M2.6 3.3 3.6 3v3M2.5 6h2M2.5 9.3c.2-.5.7-.7 1.1-.6.6.1.8.7.4 1.2L2.5 11.8h2M6.5 4.5H13M6.5 8H13M6.5 11.5H13"/>',
  clear: '<path d="M4 3h8M8 3l-2.5 10M3 13h5"/><path d="m10 9 4 4m0-4-4 4"/>',
  save: '<path d="M3 3h8l2 2v8H3z"/><path d="M5 3v3h5V3M5 13V9h6v4"/>',
};
const icon = name => `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name]}</svg>`;
const SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];
const BLOCK_TAGS = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'DIV', 'BLOCKQUOTE', 'PRE']);

/** Mounts a browser editor. onSave receives Uint8Array and resolves after persistence. */
export function mountOdtEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openOdt(bytes);
  const doc = model.xml;
  let destroyed = false, saving = false, nextId = 1, syncTimer = 0, snapshotTimer = 0, pending = false;
  const byId = new Map();
  const idOf = new WeakMap();
  const atoms = new Map();
  const rendered = new WeakMap();
  const imageUrls = new Map();
  const autoStyles = new Map();
  let serial = 0;

  const serialize = () => new XMLSerializer().serializeToString(doc);
  const history = [serialize()];
  let historyIndex = 0, savedXml = history[0];
  const dirty = () => pending || history[historyIndex] !== savedXml;

  const root = document.createElement('section');
  root.className = 'odt-editor document-editor';
  const toolbar = document.createElement('div');
  toolbar.className = 'office-toolbar';
  toolbar.setAttribute('role', 'toolbar');
  toolbar.setAttribute('aria-label', 'Document tools');
  const status = document.createElement('span');
  status.className = 'office-status';
  status.setAttribute('role', 'status');
  const scroller = document.createElement('div');
  scroller.className = 'odt-scroller';
  const page = document.createElement('div');
  page.className = 'docx-page odt-page';
  page.setAttribute('role', 'textbox');
  page.setAttribute('aria-multiline', 'true');
  page.setAttribute('aria-label', 'Document');
  page.spellcheck = true;
  scroller.append(page);
  root.append(toolbar, scroller);
  container.replaceChildren(root);

  // --- Rendering: XML to editable HTML ---

  function idFor(node) {
    let id = idOf.get(node);
    if (!id) { id = String(nextId++); idOf.set(node, id); }
    byId.set(id, node);
    return id;
  }
  // What the editor drew, to tell new formatting from the document's own. Kept as data: the page's
  // Content-Security-Policy blocks parsing style text back (and the app owns data-css).
  const TRACKED = ['fontWeight', 'fontStyle', 'textDecorationLine', 'textDecoration', 'color', 'backgroundColor', 'fontSize'];
  function applyCss(el, css) {
    for (const [key, value] of Object.entries(css)) el.style[key] = value;
    el.dataset.drawn = JSON.stringify(Object.fromEntries(TRACKED.map(prop => [prop, el.style[prop]])));
  }
  function atom(node, display) {
    const id = String(nextId++);
    atoms.set(id, node);
    display.dataset.atom = id;
    display.contentEditable = 'false';
    display.classList.add('odt-atom');
    return display;
  }
  function imageFor(frame) {
    const image = named(frame, DRAW, 'image')[0];
    const href = image?.getAttributeNS(XLINK, 'href') || '';
    if (!image || !model.parts[href]) return null;
    if (!imageUrls.has(href)) {
      const type = /\.png$/i.test(href) ? 'image/png' : /\.jpe?g$/i.test(href) ? 'image/jpeg' : /\.gif$/i.test(href) ? 'image/gif' : /\.svg$/i.test(href) ? 'image/svg+xml' : /\.webp$/i.test(href) ? 'image/webp' : '';
      if (!type) return null;
      imageUrls.set(href, URL.createObjectURL(new Blob([model.parts[href]], { type })));
    }
    const img = document.createElement('img');
    img.src = imageUrls.get(href);
    img.alt = named(frame, SVG, 'title')[0]?.textContent || frame.getAttributeNS(DRAW, 'name') || '';
    const width = lengthPx(frame.getAttributeNS(SVG, 'width')), height = lengthPx(frame.getAttributeNS(SVG, 'height'));
    if (width) img.style.width = `${width}px`;
    if (height && width) img.style.aspectRatio = `${width} / ${height}`;
    img.draggable = false;
    return img;
  }
  function renderInline(node, target, family = 'text') {
    for (const child of node.childNodes) {
      if (child.nodeType === 3) {
        // XML whitespace collapses; real spaces are written as <text:s/>.
        const text = child.textContent.replace(/[\t\n\r ]+/g, ' ');
        if (text) target.append(document.createTextNode(text));
        continue;
      }
      if (child.nodeType !== 1) continue;
      if (is(child, TEXT, 's')) { target.append(document.createTextNode(' '.repeat(Math.min(1000, Number(child.getAttributeNS(TEXT, 'c') || 1))))); continue; }
      if (is(child, TEXT, 'tab')) { target.append(document.createTextNode('\t')); continue; }
      if (is(child, TEXT, 'line-break')) { target.append(document.createElement('br')); continue; }
      if (is(child, TEXT, 'soft-page-break')) continue;
      if (is(child, TEXT, 'span')) {
        const span = document.createElement('span');
        const name = child.getAttributeNS(TEXT, 'style-name');
        if (name) span.dataset.style = name;
        applyCss(span, textCss(model, 'text', name));
        renderInline(child, span, family);
        target.append(span);
        continue;
      }
      if (is(child, TEXT, 'a')) {
        const link = document.createElement('a');
        link.dataset.odf = idFor(child);
        link.title = child.getAttributeNS(XLINK, 'href') || '';
        const name = child.getAttributeNS(TEXT, 'style-name');
        if (name) applyCss(link, textCss(model, 'text', name));
        renderInline(child, link, family);
        target.append(link);
        continue;
      }
      if (is(child, DRAW, 'frame')) {
        const img = imageFor(child);
        const holder = document.createElement('span');
        holder.className = img ? 'odt-image' : 'odt-object';
        if (img) holder.append(img); else holder.textContent = '▣';
        holder.title = img ? 'Image' : 'Embedded object (kept as it is)';
        target.append(atom(child, holder));
        continue;
      }
      if (is(child, TEXT, 'note')) {
        const mark = document.createElement('sup');
        mark.className = 'odt-note';
        mark.textContent = named(child, TEXT, 'note-citation')[0]?.textContent || '*';
        mark.title = named(child, TEXT, 'note-body')[0]?.textContent.trim().slice(0, 300) || 'Note';
        target.append(atom(child, mark));
        continue;
      }
      if (is(child, OFFICE, 'annotation')) {
        const mark = document.createElement('span');
        mark.className = 'odt-comment';
        mark.textContent = '💬';
        mark.title = [...child.getElementsByTagNameNS(TEXT, 'p')].map(p => p.textContent).join('\n').slice(0, 300) || 'Comment';
        target.append(atom(child, mark));
        continue;
      }
      // Bookmarks, reference marks, change marks and the like: invisible, kept in place.
      if (!child.textContent.trim() && !child.children.length || /^(bookmark|reference-mark|toc-mark|alphabetical-index-mark|user-index-mark|change|annotation-end)/.test(child.localName)) {
        const mark = document.createElement('span');
        mark.className = 'odt-marker';
        target.append(atom(child, mark));
        continue;
      }
      // Fields (page numbers, dates, titles…) show their text but can't be typed into.
      const field = document.createElement('span');
      field.className = 'odt-field';
      field.textContent = child.textContent.replace(/[\t\n\r ]+/g, ' ');
      field.title = `Field: ${child.localName.replace(/-/g, ' ')}`;
      target.append(atom(child, field));
    }
  }
  function renderParagraph(node, tag = null) {
    const heading = is(node, TEXT, 'h');
    const level = Math.max(1, Math.min(6, Number(node.getAttributeNS(TEXT, 'outline-level') || 1)));
    const el = document.createElement(tag || (heading ? `h${level}` : 'p'));
    el.dataset.odf = idFor(node);
    const name = node.getAttributeNS(TEXT, 'style-name');
    applyCss(el, paragraphCss(model, name));
    renderInline(node, el, 'paragraph');
    if (!el.childNodes.length || el.lastChild?.nodeName === 'BR') el.append(document.createElement('br'));
    return el;
  }
  function listKind(list) {
    // The list's own style, or the one it inherits from an enclosing list.
    for (let node = list; node && !is(node, OFFICE, 'text'); node = node.parentNode) {
      if (!is(node, TEXT, 'list')) continue;
      const name = node.getAttributeNS(TEXT, 'style-name');
      const style = name && model.listStyles.get(name);
      if (!style) continue;
      let depth = 0;
      for (let p = list; p && p !== node; p = p.parentNode) if (is(p, TEXT, 'list')) depth++;
      const levels = children(style);
      const level = levels.find(item => Number(item.getAttributeNS(TEXT, 'level')) === depth + 1) || levels[0];
      return level?.localName === 'list-level-style-number' ? 'ol' : 'ul';
    }
    return 'ul';
  }
  function renderList(node) {
    const list = document.createElement(listKind(node));
    list.dataset.odf = idFor(node);
    for (const item of children(node)) {
      if (!is(item, TEXT, 'list-item', 'list-header')) continue;
      const li = document.createElement('li');
      li.dataset.item = idFor(item);
      if (is(item, TEXT, 'list-header')) li.classList.add('odt-list-header');
      const blocks = children(item);
      const paragraphs = blocks.filter(child => is(child, TEXT, 'p', 'h'));
      const simple = paragraphs.length <= 1 && blocks.every(child => is(child, TEXT, 'p', 'h', 'list'));
      if (simple) {
        // One paragraph per item renders inline, so Enter makes a new item, as in word processors.
        const paragraph = paragraphs[0];
        if (paragraph) {
          li.dataset.odf = idFor(paragraph);
          applyCss(li, paragraphCss(model, paragraph.getAttributeNS(TEXT, 'style-name')));
          renderInline(paragraph, li, 'paragraph');
        }
        if (!li.childNodes.length) li.append(document.createElement('br'));
        for (const nested of blocks.filter(child => is(child, TEXT, 'list'))) li.append(renderList(nested));
      } else {
        li.dataset.flow = '1';
        renderFlow(item, li);
      }
      list.append(li);
    }
    return list;
  }
  function renderTable(node) {
    const wrap = document.createElement('div');
    wrap.className = 'odt-table';
    wrap.contentEditable = 'false';
    wrap.dataset.odf = idFor(node);
    const table = document.createElement('table');
    const body = document.createElement('tbody');
    const rows = [];
    const collect = parent => {
      for (const child of children(parent)) {
        if (is(child, TABLE, 'table-row')) rows.push(child);
        else if (is(child, TABLE, 'table-header-rows', 'table-rows', 'table-row-group')) collect(child);
      }
    };
    collect(node);
    for (const row of rows) {
      const tr = document.createElement('tr');
      for (const cell of children(row)) {
        if (!is(cell, TABLE, 'table-cell', 'covered-table-cell')) continue;
        if (is(cell, TABLE, 'covered-table-cell')) continue;
        const td = document.createElement('td');
        const span = Number(cell.getAttributeNS(TABLE, 'number-columns-spanned') || 1);
        const rowSpan = Number(cell.getAttributeNS(TABLE, 'number-rows-spanned') || 1);
        if (span > 1) td.colSpan = Math.min(span, 1000);
        if (rowSpan > 1) td.rowSpan = Math.min(rowSpan, 1000);
        const repeat = Math.min(64, Number(cell.getAttributeNS(TABLE, 'number-columns-repeated') || 1));
        td.dataset.odf = idFor(cell);
        td.dataset.flow = '1';
        td.contentEditable = readonly ? 'false' : 'true';
        renderFlow(cell, td);
        if (!td.childNodes.length) td.append(document.createElement('br'));
        tr.append(td);
        // Repeated cells are copies; only the first is editable.
        for (let i = 1; i < repeat; i++) {
          const copy = td.cloneNode(true);
          copy.contentEditable = 'false';
          delete copy.dataset.odf;
          delete copy.dataset.flow;
          copy.classList.add('odt-repeat');
          tr.append(copy);
        }
      }
      body.append(tr);
    }
    table.append(body);
    wrap.append(table);
    return wrap;
  }
  function renderFlow(node, target) {
    for (const child of children(node)) {
      if (is(child, TEXT, 'p', 'h')) target.append(renderParagraph(child));
      else if (is(child, TEXT, 'list')) target.append(renderList(child));
      else if (is(child, TABLE, 'table')) target.append(renderTable(child));
      else if (is(child, TEXT, 'section')) {
        const section = document.createElement('div');
        section.className = 'odt-section';
        section.dataset.odf = idFor(child);
        section.dataset.flow = '1';
        renderFlow(child, section);
        target.append(section);
      } else if (HIDDEN_FLOW.has(child.localName)) continue;
      else {
        // Indexes, tables of contents, drawings: shown, not edited.
        const block = document.createElement('div');
        block.className = 'odt-locked';
        const frame = is(child, DRAW, 'frame') ? imageFor(child) : null;
        if (frame) block.append(frame);
        else block.textContent = child.textContent.replace(/\s+/g, ' ').trim().slice(0, 2000) || child.localName.replace(/-/g, ' ');
        block.title = 'This part is kept as it is and can be edited in a word processor.';
        target.append(atom(child, block));
      }
    }
  }
  function render(keepSelection = true) {
    const offset = keepSelection ? caretOffset() : null;
    byId.clear();
    atoms.clear();
    page.replaceChildren();
    renderFlow(model.body, page);
    if (!page.childNodes.length) {
      const paragraph = create(doc, TEXT, 'text', 'p');
      model.body.append(paragraph);
      page.append(renderParagraph(paragraph));
    }
    page.querySelectorAll('[data-odf]').forEach(el => rendered.set(el, el.innerHTML));
    page.contentEditable = readonly || saving ? 'false' : 'true';
    page.querySelectorAll('.odt-table td[data-flow]').forEach(td => { td.contentEditable = readonly || saving ? 'false' : 'true'; });
    if (offset) restoreCaret(offset);
    updateTools();
  }

  // --- Syncing: edited HTML back to XML ---

  const cssOf = text => {
    try { return JSON.parse(text || '{}'); } catch { return {}; }
  };
  function formatting(el, inherited) {
    const next = { ...inherited, over: { ...inherited.over } };
    const tag = el.nodeName;
    if (tag === 'B' || tag === 'STRONG') next.over.bold = true;
    if (tag === 'I' || tag === 'EM') next.over.italic = true;
    if (tag === 'U') next.over.underline = true;
    if (tag === 'S' || tag === 'STRIKE' || tag === 'DEL') next.over.strike = true;
    if (tag === 'SUP') next.over.position = 'super';
    if (tag === 'SUB') next.over.position = 'sub';
    if (tag === 'FONT' && el.getAttribute('color')) next.over.color = hex(el.getAttribute('color'));
    if (el.dataset.style !== undefined) {
      // An explicitly styled span wins over formatting around it, as in CSS.
      next.base = el.dataset.style || null;
      const own = textCss(model, 'text', next.base);
      const css = { bold: 'fontWeight', italic: 'fontStyle', underline: 'textDecorationLine', strike: 'textDecorationLine', color: 'color', size: 'fontSize', background: 'backgroundColor', position: 'verticalAlign' };
      for (const key of Object.keys(next.over)) if (own[css[key]] !== undefined) delete next.over[key];
    }
    // Only what changed from how the editor drew the element counts as new formatting.
    const now = el.style, before = cssOf(el.dataset.drawn);
    const changed = prop => (now[prop] || '') !== (before[prop] || '');
    if (changed('fontWeight')) next.over.bold = /^(bold|[6-9]00)$/.test(now.fontWeight);
    if (changed('fontStyle')) next.over.italic = now.fontStyle === 'italic';
    if (changed('textDecorationLine') || changed('textDecoration')) {
      const lines = `${now.textDecorationLine} ${now.textDecoration}`;
      next.over.underline = /underline/.test(lines);
      next.over.strike = /line-through/.test(lines);
    }
    if (changed('color') && now.color) next.over.color = hex(now.color);
    if (changed('backgroundColor')) next.over.background = now.backgroundColor ? hex(now.backgroundColor) : 'transparent';
    if (changed('fontSize') && /pt$/.test(now.fontSize)) next.over.size = now.fontSize;
    return next;
  }
  function hex(color) {
    const match = /rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?/.exec(color || '');
    if (match) return match[4] === '0' ? 'transparent' : `#${[match[1], match[2], match[3]].map(n => Number(n).toString(16).padStart(2, '0')).join('')}`;
    return /^#[0-9a-f]{6}$/i.test(color || '') ? color.toLowerCase() : null;
  }
  function automaticStyles() {
    let styles = named(doc.documentElement, OFFICE, 'automatic-styles')[0];
    if (!styles) {
      styles = create(doc, OFFICE, 'office', 'automatic-styles');
      const after = named(doc.documentElement, OFFICE, 'master-styles')[0] || named(doc.documentElement, OFFICE, 'body')[0];
      doc.documentElement.insertBefore(styles, after || null);
    }
    return styles;
  }
  const automatic = (family, name) => {
    const style = model.styleByName.get(`${family}:${name}`);
    return style && style.ownerDocument === doc && style.parentNode?.localName === 'automatic-styles' ? style : null;
  };
  function newStyleName(prefix) {
    let name;
    do { name = `${prefix}${++serial}`; } while (model.styleByName.has(`text:${name}`) || model.styleByName.has(`paragraph:${name}`) || model.listStyles.has(name));
    return name;
  }
  /** An automatic style: `base`'s formatting plus `props`; automatic styles can't be parents. */
  function derivedStyle(family, base, propsName, props) {
    const key = `${family}|${base || ''}|${JSON.stringify(props)}`;
    if (autoStyles.has(key)) return autoStyles.get(key);
    const style = create(doc, STYLE, 'style', 'style');
    const name = newStyleName(family === 'text' ? 'OwnNAS_T' : 'OwnNAS_P');
    style.setAttributeNS(STYLE, 'style:name', name);
    style.setAttributeNS(STYLE, 'style:family', family);
    const source = base && automatic(family, base);
    if (source) {
      for (const attr of source.attributes) if (attr.localName !== 'name') style.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
      for (const child of children(source)) style.append(child.cloneNode(true));
    } else if (base) style.setAttributeNS(STYLE, 'style:parent-style-name', base);
    let target = named(style, STYLE, propsName)[0];
    if (!target) {
      target = create(doc, STYLE, 'style', propsName);
      // paragraph-properties comes before text-properties.
      if (propsName === 'paragraph-properties') style.insertBefore(target, style.firstChild); else style.append(target);
    }
    for (const [ns, prefix, attr, value] of props) {
      if (value == null) target.removeAttributeNS(ns, attr);
      else target.setAttributeNS(ns, `${prefix}:${attr}`, value);
    }
    automaticStyles().append(style);
    model.styleByName.set(`${family}:${name}`, style);
    autoStyles.set(key, name);
    return name;
  }
  function textStyle(base, over) {
    const props = [];
    if ('bold' in over) for (const [ns, prefix, attr] of [[FO, 'fo', 'font-weight'], [STYLE, 'style', 'font-weight-asian'], [STYLE, 'style', 'font-weight-complex']]) props.push([ns, prefix, attr, over.bold ? 'bold' : 'normal']);
    if ('italic' in over) for (const [ns, prefix, attr] of [[FO, 'fo', 'font-style'], [STYLE, 'style', 'font-style-asian'], [STYLE, 'style', 'font-style-complex']]) props.push([ns, prefix, attr, over.italic ? 'italic' : 'normal']);
    if ('underline' in over) {
      props.push([STYLE, 'style', 'text-underline-style', over.underline ? 'solid' : 'none']);
      props.push([STYLE, 'style', 'text-underline-width', over.underline ? 'auto' : null]);
      props.push([STYLE, 'style', 'text-underline-color', over.underline ? 'font-color' : null]);
    }
    if ('strike' in over) props.push([STYLE, 'style', 'text-line-through-style', over.strike ? 'solid' : 'none']);
    if (over.color) props.push([FO, 'fo', 'color', over.color]);
    if (over.background) props.push([FO, 'fo', 'background-color', over.background]);
    if (over.size) for (const [ns, prefix, attr] of [[FO, 'fo', 'font-size'], [STYLE, 'style', 'font-size-asian'], [STYLE, 'style', 'font-size-complex']]) props.push([ns, prefix, attr, over.size]);
    if (over.position) props.push([STYLE, 'style', 'text-position', over.position === 'super' ? 'super 58%' : 'sub 58%']);
    if (!props.length) return base;
    props.sort((a, b) => a[2].localeCompare(b[2]));
    return derivedStyle('text', base, 'text-properties', props);
  }
  /**
   * The browser moves a paragraph's own formatting onto its pieces when it formats part of it;
   * formatting that only repeats the paragraph's (and that the piece's own style doesn't change)
   * is left out.
   */
  const CSS_OF = { bold: 'fontWeight', italic: 'fontStyle', underline: 'textDecorationLine', strike: 'textDecorationLine', color: 'color', size: 'fontSize', background: 'backgroundColor' };
  function plain(ctx, defaults) {
    const own = ctx.base ? textCss(model, 'text', ctx.base) : {};
    const over = { ...ctx.over };
    for (const [key, value] of Object.entries(over)) {
      const prop = CSS_OF[key];
      if (!prop || own[prop] !== undefined) continue;
      const inherited = defaults[prop];
      const same = key === 'bold' ? value === (inherited === '700')
        : key === 'italic' ? value === (inherited === 'italic')
        : key === 'underline' ? value === /underline/.test(inherited || '')
        : key === 'strike' ? value === /line-through/.test(inherited || '')
        : inherited !== undefined && String(value).toLowerCase() === String(inherited).toLowerCase();
      if (same) delete over[key];
    }
    return over;
  }
  function writeText(target, text, state) {
    // Spaces after the first in a run, or at a paragraph's start, need <text:s>.
    let buffer = '';
    const flush = () => { if (buffer) { target.append(doc.createTextNode(buffer)); buffer = ''; } };
    for (let i = 0; i < text.length; i++) {
      const char = text[i];
      if (char === '\t') { flush(); target.append(create(doc, TEXT, 'text', 'tab')); state.space = false; state.start = false; continue; }
      if (char === '\n') { flush(); target.append(create(doc, TEXT, 'text', 'line-break')); state.space = false; continue; }
      if (char === ' ') {
        let run = 1;
        while (text[i + run] === ' ') run++;
        let literal = state.start || state.space ? 0 : 1;
        buffer += literal ? ' ' : '';
        const extra = run - literal;
        if (extra > 0) {
          flush();
          const s = create(doc, TEXT, 'text', 's');
          if (extra > 1) s.setAttributeNS(TEXT, 'text:c', String(extra));
          target.append(s);
        }
        i += run - 1;
        state.space = true;
        state.start = false;
        continue;
      }
      buffer += char;
      state.space = false;
      state.start = false;
    }
    flush();
  }
  function emitInline(htmlNode, odfParent, ctx, state) {
    for (const child of htmlNode.childNodes) {
      if (child.nodeType === 3) {
        const text = child.textContent.replace(/​/g, '');
        if (!text) continue;
        const name = textStyle(ctx.base, plain(ctx, state.defaults));
        let target = odfParent;
        if (name) {
          const last = odfParent.lastChild;
          if (is(last, TEXT, 'span') && last.getAttributeNS(TEXT, 'style-name') === name) target = last;
          else {
            target = create(doc, TEXT, 'text', 'span');
            target.setAttributeNS(TEXT, 'text:style-name', name);
            odfParent.append(target);
          }
        }
        writeText(target, text, state);
        continue;
      }
      if (child.nodeType !== 1) continue;
      if (child.dataset.atom) {
        const original = atoms.get(child.dataset.atom);
        if (original) odfParent.append(original.cloneNode(true));
        state.space = false; state.start = false;
        continue;
      }
      if (child.nodeName === 'UL' || child.nodeName === 'OL') continue;
      if (child.nodeName === 'BR') {
        // The browser keeps a <br> at a paragraph's end so an empty last line shows; it isn't text.
        if (child !== state.trailing) {
          odfParent.append(create(doc, TEXT, 'text', 'line-break'));
          state.space = false;
        }
        continue;
      }
      if (child.nodeName === 'A' && child.dataset.odf && byId.get(child.dataset.odf)) {
        const original = byId.get(child.dataset.odf);
        const link = create(doc, TEXT, 'text', 'a');
        for (const attr of original.attributes) link.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
        odfParent.append(link);
        emitInline(child, link, formatting(child, ctx), state);
        continue;
      }
      if (BLOCK_TAGS.has(child.nodeName)) {
        // A block the browser nested inside a paragraph: its text joins the paragraph.
        emitInline(child, odfParent, ctx, state);
        continue;
      }
      emitInline(child, odfParent, formatting(child, ctx), state);
    }
  }
  function syncParagraph(el, node) {
    if (rendered.get(el) === el.innerHTML && idOf.get(node) === el.dataset.odf) return;
    for (const child of [...node.childNodes]) child.remove();
    let trailing = null;
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
    for (let item = walker.nextNode(); item; item = walker.nextNode()) {
      if (item.nodeName === 'BR') trailing = item;
      else if (item.nodeType === 3 ? item.textContent.replace(/\u200b/g, '') : item.dataset?.atom) trailing = null;
      if (item.nodeName === 'UL' || item.nodeName === 'OL') break;
    }
    const defaults = textCss(model, 'paragraph', node.getAttributeNS(TEXT, 'style-name'));
    emitInline(el, node, { base: null, over: {} }, { start: true, space: false, trailing, defaults });
    rendered.set(el, el.innerHTML);
  }
  function freshParagraph(like) {
    const node = create(doc, TEXT, 'text', like && is(like, TEXT, 'h') ? 'h' : 'p');
    if (like) for (const attr of like.attributes) {
      if (attr.namespaceURI === 'http://www.w3.org/2000/xmlns/' || attr.localName === 'id' || attr.localName === 'xml-id') continue;
      node.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
    }
    return node;
  }
  function claim(el, used, fallback) {
    const original = el.dataset.odf && byId.get(el.dataset.odf);
    if (original && !used.has(original)) { used.add(original); return original; }
    const node = fallback(original);
    used.add(node);
    el.dataset.odf = idFor(node);
    return node;
  }
  /** Writes an edited flow (the page, a section, a cell, a list item) back into its XML node. */
  function syncFlow(htmlParent, odfParent, used) {
    // Wrappers the browser makes around blocks are dissolved first.
    for (const child of [...htmlParent.children]) {
      if (child.nodeName === 'DIV' && !child.dataset.odf && !child.dataset.atom && [...child.children].some(item => BLOCK_TAGS.has(item.nodeName) || item.nodeName === 'UL' || item.nodeName === 'OL')) {
        child.replaceWith(...child.childNodes);
      }
    }
    const out = [];
    const keep = children(odfParent).filter(child => HIDDEN_FLOW.has(child.localName) || is(child, TEXT, 'number') || is(child, TABLE, 'table-column', 'table-columns', 'table-header-columns'));
    let loose = null;
    for (const child of [...htmlParent.childNodes]) {
      // Text or inline pieces sitting directly in the flow join one new paragraph.
      if (child.nodeType === 3 || (child.nodeType === 1 && !BLOCK_TAGS.has(child.nodeName) && !['UL', 'OL'].includes(child.nodeName) && !child.dataset.atom && !child.classList.contains('odt-table') && !child.dataset.flow)) {
        if (child.nodeType === 3 && !child.textContent.trim() && !loose) continue;
        if (!loose) {
          loose = document.createElement('p');
          child.before(loose);
        }
        loose.append(child);
        continue;
      }
      if (child.nodeType !== 1) continue;
      if (loose) {
        const node = claim(loose, used, () => create(doc, TEXT, 'text', 'p'));
        syncParagraph(loose, node);
        out.push(node);
        loose = null;
      }
      if (child.dataset.atom) {
        const original = atoms.get(child.dataset.atom);
        if (original && !used.has(original)) { used.add(original); out.push(original); }
        continue;
      }
      if (child.classList.contains('odt-table')) {
        const table = child.dataset.odf && byId.get(child.dataset.odf);
        if (table && !used.has(table)) {
          used.add(table);
          for (const td of child.querySelectorAll('td[data-flow]')) {
            if (td.closest('.odt-table') !== child) continue;
            const cell = byId.get(td.dataset.odf);
            if (cell) syncFlow(td, cell, used);
          }
          out.push(table);
        }
        continue;
      }
      if (child.dataset.flow && child.classList.contains('odt-section')) {
        const section = byId.get(child.dataset.odf);
        if (section && !used.has(section)) { used.add(section); syncFlow(child, section, used); out.push(section); }
        continue;
      }
      if (child.nodeName === 'UL' || child.nodeName === 'OL') {
        out.push(syncList(child, used));
        continue;
      }
      if (/^H[1-6]$/.test(child.nodeName) || child.nodeName === 'P' || child.nodeName === 'DIV' || child.nodeName === 'BLOCKQUOTE' || child.nodeName === 'PRE') {
        let node = claim(child, used, like => freshParagraph(like));
        const level = /^H([1-6])$/.exec(child.nodeName)?.[1];
        if (level && !is(node, TEXT, 'h')) node = retag(node, 'h', level);
        else if (!level && is(node, TEXT, 'h')) node = retag(node, 'p');
        syncParagraph(child, node);
        out.push(node);
      }
    }
    if (loose) {
      const node = claim(loose, used, () => create(doc, TEXT, 'text', 'p'));
      syncParagraph(loose, node);
      out.push(node);
    }
    if (!out.some(node => is(node, TEXT, 'p', 'h', 'list') || is(node, TABLE, 'table')) && (is(odfParent, TABLE, 'table-cell') || is(odfParent, OFFICE, 'text') || is(odfParent, TEXT, 'list-item'))) {
      out.push(create(doc, TEXT, 'text', 'p'));
    }
    const current = children(odfParent);
    if (current.length !== keep.length + out.length || [...keep, ...out].some((node, i) => current[i] !== node)) {
      for (const child of current) child.remove();
      for (const node of [...keep, ...out]) odfParent.append(node);
    }
  }
  function retag(node, local, level) {
    const next = create(doc, TEXT, 'text', local);
    for (const attr of node.attributes) {
      if (attr.namespaceURI === TEXT && attr.localName === 'outline-level') continue;
      next.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
    }
    if (local === 'h') next.setAttributeNS(TEXT, 'text:outline-level', String(level));
    while (node.firstChild) next.append(node.firstChild);
    node.parentNode?.replaceChild(next, node);
    const id = idOf.get(node);
    if (id) { idOf.set(next, id); byId.set(id, next); }
    return next;
  }
  function syncList(listEl, used) {
    const list = claim(listEl, used, like => {
      const fresh = create(doc, TEXT, 'text', 'list');
      if (like) for (const attr of like.attributes) if (attr.localName !== 'id') fresh.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
      return fresh;
    });
    const items = [];
    const keep = children(list).filter(child => !is(child, TEXT, 'list-item', 'list-header'));
    for (const li of [...listEl.children]) {
      if (li.nodeName !== 'LI') {
        if (li.nodeName === 'UL' || li.nodeName === 'OL') {
          // A list nested directly in a list belongs in an item of its own.
          const item = create(doc, TEXT, 'text', 'list-item');
          item.append(syncList(li, used));
          items.push(item);
        }
        continue;
      }
      const original = li.dataset.item && byId.get(li.dataset.item);
      let item;
      if (original && !used.has(original)) { item = original; used.add(item); }
      else {
        item = create(doc, TEXT, 'text', 'list-item');
        used.add(item);
        li.dataset.item = idFor(item);
      }
      if (li.dataset.flow) {
        syncFlow(li, item, used);
      } else {
        const paragraph = claim(li, used, like => freshParagraph(like));
        syncParagraph(li, paragraph);
        const nested = [...li.children].filter(child => child.nodeName === 'UL' || child.nodeName === 'OL').map(child => syncList(child, used));
        for (const child of children(item)) child.remove();
        item.append(paragraph, ...nested);
      }
      items.push(item);
    }
    for (const child of children(list)) child.remove();
    for (const node of [...keep, ...items]) list.append(node);
    return list;
  }
  function sync() {
    clearTimeout(syncTimer);
    syncTimer = 0;
    if (readonly) return;
    syncFlow(page, model.body, new Set());
  }

  // --- History ---

  function changed({ immediate = false } = {}) {
    pending = true;
    onChange(true);
    status.textContent = 'Unsaved changes';
    clearTimeout(snapshotTimer);
    if (immediate) snapshot();
    else snapshotTimer = setTimeout(snapshot, 500);
    updateTools();
  }
  function snapshot() {
    clearTimeout(snapshotTimer);
    if (syncTimer) sync();
    pending = false;
    const current = serialize();
    if (current !== history[historyIndex]) {
      history.splice(historyIndex + 1);
      history.push(current);
      historyIndex = history.length - 1;
      let size = history.reduce((sum, entry) => sum + entry.length * 2, 0);
      while (history.length > 2 && (history.length > 100 || size > 30 * 1024 * 1024)) { size -= history.shift().length * 2; historyIndex--; }
    }
    onChange(dirty());
    status.textContent = dirty() ? 'Unsaved changes' : 'Saved';
    updateTools();
  }
  function restore(index) {
    if (readonly || saving || index < 0 || index >= history.length) return;
    const fresh = parse(history[index], 'Document XML');
    doc.replaceChild(doc.importNode(fresh.documentElement, true), doc.documentElement);
    historyIndex = index;
    pending = false;
    autoStyles.clear();
    indexStyles(model);
    render();
    onChange(dirty());
    status.textContent = dirty() ? 'Unsaved changes' : 'Saved';
  }
  function undo() { if (pending) snapshot(); restore(historyIndex - 1); }
  function redo() { if (pending) snapshot(); restore(historyIndex + 1); }

  // --- Caret ---

  function caretOffset() {
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !page.contains(selection.anchorNode)) return null;
    const measure = (node, offset) => {
      const range = document.createRange();
      range.setStart(page, 0);
      try { range.setEnd(node, offset); } catch { return 0; }
      return range.toString().length;
    };
    const range = selection.getRangeAt(0);
    return { start: measure(range.startContainer, range.startOffset), end: measure(range.endContainer, range.endOffset) };
  }
  function restoreCaret({ start, end }) {
    const locate = target => {
      const walker = document.createTreeWalker(page, NodeFilter.SHOW_TEXT);
      let seen = 0, node, last = null;
      while ((node = walker.nextNode())) {
        last = node;
        if (seen + node.length >= target) return [node, target - seen];
        seen += node.length;
      }
      return last ? [last, last.length] : [page, 0];
    };
    try {
      const range = document.createRange();
      range.setStart(...locate(start));
      range.setEnd(...locate(end));
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    } catch { /* The caret stays where the browser put it. */ }
  }

  // --- Joining paragraphs ---
  // The browser joins paragraphs by rebuilding their text with copied inline styles, losing which
  // document style each piece had. The editor joins them itself, moving the nodes as they are.

  const INLINE_BLOCK = 'p, h1, h2, h3, h4, h5, h6, li';
  function blockAt(node) {
    const el = (node?.nodeType === 1 ? node : node?.parentElement)?.closest(INLINE_BLOCK);
    return el && page.contains(el) && !el.dataset.flow ? el : null;
  }
  const nestedList = block => [...block.children].find(child => child.nodeName === 'UL' || child.nodeName === 'OL') || null;
  function textBetween(block, startNode, startOffset, endNode, endOffset) {
    const range = document.createRange();
    try { range.setStart(startNode, startOffset); range.setEnd(endNode, endOffset); } catch { return 'x'; }
    const piece = range.cloneContents();
    return range.toString().replace(/\u200b/g, '') + (piece.querySelector?.('[data-atom], img') ? 'x' : '');
  }
  const atStart = (range, block) => textBetween(block, block, 0, range.startContainer, range.startOffset) === '';
  function atEnd(range, block) {
    const list = nestedList(block);
    const end = list ? [block, [...block.childNodes].indexOf(list)] : [block, block.childNodes.length];
    return textBetween(block, range.endContainer, range.endOffset, ...end) === '';
  }
  function lastItem(list) {
    let item = [...list.children].filter(child => child.nodeName === 'LI').pop();
    while (item && nestedList(item)) {
      const deeper = [...nestedList(item).children].filter(child => child.nodeName === 'LI').pop();
      if (!deeper) break;
      item = deeper;
    }
    return item || null;
  }
  function previousBlock(block) {
    if (block.nodeName === 'LI') {
      const previous = block.previousElementSibling;
      if (previous?.nodeName !== 'LI') return null;
      return nestedList(previous) ? lastItem(nestedList(previous)) || previous : previous;
    }
    const previous = block.previousElementSibling;
    if (!previous) return null;
    if (/^(P|H[1-6])$/.test(previous.nodeName)) return previous;
    if (previous.nodeName === 'UL' || previous.nodeName === 'OL') return lastItem(previous);
    return null;
  }
  function nextBlock(block) {
    if (block.nodeName === 'LI') {
      const nested = nestedList(block);
      if (nested) return [...nested.children].find(child => child.nodeName === 'LI') || null;
      const next = block.nextElementSibling;
      return next?.nodeName === 'LI' ? next : null;
    }
    const next = block.nextElementSibling;
    if (!next) return null;
    if (/^(P|H[1-6])$/.test(next.nodeName)) return next;
    if (next.nodeName === 'UL' || next.nodeName === 'OL') return [...next.children].find(child => child.nodeName === 'LI') || null;
    return null;
  }
  const placeholder = block => block.childNodes.length === 1 && block.firstChild.nodeName === 'BR';
  /** Moves `second`'s text to the end of `first` and removes it; returns where they meet. */
  function join(first, second) {
    const firstList = nestedList(first);
    if (placeholder(first)) first.firstChild.remove();
    else if (!firstList && first.lastChild?.nodeName === 'BR') first.lastChild.remove();
    const meeting = firstList ? [first, [...first.childNodes].indexOf(firstList)] : [first, first.childNodes.length];
    const moving = [...second.childNodes].filter(child => child.nodeName !== 'UL' && child.nodeName !== 'OL');
    const lists = [...second.childNodes].filter(child => child.nodeName === 'UL' || child.nodeName === 'OL');
    if (!placeholder(second)) for (const child of moving) first.insertBefore(child, firstList);
    for (const list of lists) (first.nodeName === 'LI' ? first : first.parentNode).insertBefore(list, first.nodeName === 'LI' ? null : first.nextSibling);
    let parent = second.parentElement;
    second.remove();
    while (parent && parent !== page && (parent.nodeName === 'UL' || parent.nodeName === 'OL') && !parent.children.length) {
      const up = parent.parentElement;
      parent.remove();
      parent = up;
    }
    if (!first.childNodes.length || first.lastChild === firstList && first.childNodes.length === 1) first.insertBefore(document.createElement('br'), firstList);
    return meeting;
  }
  function caretAt([node, offset]) {
    const range = document.createRange();
    range.setStart(node, Math.min(offset, node.childNodes.length));
    range.collapse(true);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  }
  function edited() {
    clearTimeout(syncTimer);
    syncTimer = setTimeout(sync, 150);
    changed();
  }
  function joinInstead(event) {
    const type = event.inputType;
    const selection = window.getSelection();
    if (!selection?.rangeCount) return;
    const range = selection.getRangeAt(0);
    const first = blockAt(range.startContainer), last = blockAt(range.endContainer);
    if (!range.collapsed) {
      if (!first || !last || first === last || !/^(delete|insertText|insertReplacementText|insertParagraph|insertLineBreak)/.test(type)) return;
      event.preventDefault();
      if (first.closest('td') !== last.closest('td')) return;
      range.deleteContents();
      // Blocks wholly inside the selection are gone; the last block's remainder joins the first.
      const meeting = last.isConnected ? join(first, last) : [range.startContainer, range.startOffset];
      caretAt(meeting);
      if ((type === 'insertText' || type === 'insertReplacementText') && event.data) {
        const text = document.createTextNode(event.data);
        const at = window.getSelection().getRangeAt(0);
        at.insertNode(text);
        caretAt([text.parentNode, [...text.parentNode.childNodes].indexOf(text) + 1]);
      }
      edited();
      if (type === 'insertParagraph') setTimeout(() => document.execCommand('insertParagraph'), 0);
      return;
    }
    if (!first) return;
    if ((type === 'deleteContentBackward' || type === 'deleteWordBackward') && atStart(range, first)) {
      const previous = previousBlock(first);
      if (previous) {
        event.preventDefault();
        caretAt(join(previous, first));
        edited();
      } else if (first.nodeName === 'LI' && !first.previousElementSibling) {
        // Backspace at the start of a list's first item takes it out of the list.
        event.preventDefault();
        sync();
        const node = byId.get(first.dataset.odf);
        const list = node?.parentNode?.parentNode;
        if (node && list && is(list, TEXT, 'list')) toggleList(listKind(list) === 'ol');
      } else if (first.previousElementSibling && !/^(P|H[1-6]|UL|OL)$/.test(first.previousElementSibling.nodeName)) {
        // After a table or a locked part there is nothing to join; an empty paragraph just goes.
        event.preventDefault();
        if (placeholder(first) || !first.textContent) { const before = first.previousElementSibling; first.remove(); edited(); before?.scrollIntoView?.({ block: 'nearest' }); }
      }
      return;
    }
    if ((type === 'deleteContentForward' || type === 'deleteWordForward') && atEnd(range, first)) {
      const next = nextBlock(first);
      if (next) {
        event.preventDefault();
        caretAt(join(first, next));
        edited();
      } else if (first.nextElementSibling && !/^(P|H[1-6]|UL|OL)$/.test(first.nextElementSibling.nodeName)) event.preventDefault();
    }
  }

  // --- Commands ---

  function selectedBlocks() {
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !page.contains(selection.anchorNode)) return [];
    const range = selection.getRangeAt(0);
    const blockOf = node => {
      for (let el = node.nodeType === 1 ? node : node.parentElement; el && el !== page; el = el.parentElement) {
        if (el.dataset?.odf && (BLOCK_TAGS.has(el.nodeName) || el.nodeName === 'LI')) return el;
      }
      return null;
    };
    const blocks = [...page.querySelectorAll('p[data-odf], h1[data-odf], h2[data-odf], h3[data-odf], h4[data-odf], h5[data-odf], h6[data-odf], li[data-odf], div[data-odf]:not([data-flow]):not(.odt-table)')]
      .filter(el => range.intersectsNode(el));
    const first = blockOf(range.startContainer);
    if (first && !blocks.includes(first)) blocks.unshift(first);
    return blocks;
  }
  function editable() {
    return !readonly && !saving;
  }
  /**
   * Runs one of the browser's editing commands. They write formatting as style attributes, which
   * the app's Content-Security-Policy doesn't apply; what a command wrote is applied again through
   * the CSSOM, which it allows. Style text the browser adds on its own (to keep a merged
   * paragraph's old look) is scrubbed after each edit instead, so it never becomes formatting.
   */
  let executing = false;
  function exec(command, value = null) {
    scrub();
    const before = new Map([...page.querySelectorAll('[style]')].map(el => [el, el.getAttribute('style')]));
    try { document.execCommand('styleWithCSS', false, true); } catch { /* Older engines use tags. */ }
    executing = true;
    try { document.execCommand(command, false, value); } finally { executing = false; }
    for (const el of page.querySelectorAll('[style]')) {
      const text = el.getAttribute('style');
      if (before.get(el) !== text) applyStyleText(el, text);
    }
  }
  /**
   * Style text the CSP left unapplied: on a copy of an element the editor drew (Enter splits
   * spans by copying them) it is the editor's own and is applied; anywhere else it is dropped.
   */
  function scrub() {
    for (const el of page.querySelectorAll('[style]')) {
      const css = el.style.cssText, text = el.getAttribute('style');
      if (text === css) continue;
      if (el.dataset.drawn !== undefined) applyStyleText(el, text);
      else if (css) el.style.cssText = css;
      else el.removeAttribute('style');
    }
  }
  function applyStyleText(el, text) {
    for (const declaration of text.split(';')) {
      const colon = declaration.indexOf(':');
      const prop = declaration.slice(0, colon).trim().toLowerCase();
      if (colon < 0 || !/^[a-z-]+$/.test(prop)) continue;
      el.style.setProperty(prop, declaration.slice(colon + 1).replace(/!important/i, '').trim());
    }
  }
  function inlineCommand(command, value = null) {
    if (!editable()) return;
    page.focus({ preventScroll: true });
    exec(command, value);
    sync();
    changed({ immediate: true });
  }
  function fontSize(points) {
    if (!editable() || !points) return;
    page.focus({ preventScroll: true });
    // The browser only knows sizes 1-7; mark with 7, then set the real size.
    exec('fontSize', '7');
    for (const el of page.querySelectorAll('[style*="xxx-large"], font[size="7"]')) {
      el.style.fontSize = `${points}pt`;
      el.removeAttribute('size');
    }
    sync();
    changed({ immediate: true });
  }
  function blockCommand(work) {
    if (!editable()) return;
    sync();
    const offset = caretOffset();
    const nodes = selectedBlocks().map(el => byId.get(el.dataset.odf)).filter(Boolean);
    if (!nodes.length) return;
    work(nodes);
    render(false);
    if (offset) restoreCaret(offset);
    page.focus({ preventScroll: true });
    changed({ immediate: true });
  }
  function align(value) {
    blockCommand(nodes => {
      for (const node of nodes) {
        const current = node.getAttributeNS(TEXT, 'style-name') || null;
        node.setAttributeNS(TEXT, 'text:style-name', derivedStyle('paragraph', current, 'paragraph-properties', [[FO, 'fo', 'text-align', value]]));
      }
    });
  }
  function commonStyle(...names) {
    return names.find(name => model.styleByName.get(`paragraph:${name}`)?.ownerDocument !== doc) || null;
  }
  function paragraphKind(level) {
    blockCommand(nodes => {
      for (let node of nodes) {
        const style = level === '0'
          ? commonStyle('Text_20_body', 'Standard', 'Default_20_Paragraph_20_Style')
          : commonStyle(`Heading_20_${level}`, `Heading${level}`, `Heading_${level}`);
        if (level === '0') { if (is(node, TEXT, 'h')) node = retag(node, 'p'); }
        else node = retag(node, 'h', level);
        if (style) node.setAttributeNS(TEXT, 'text:style-name', style);
        else node.removeAttributeNS(TEXT, 'style-name');
      }
    });
  }
  function listStyle(numbered) {
    const name = numbered ? 'OwnNAS_Numbers' : 'OwnNAS_Bullets';
    if (model.listStyles.has(name)) return name;
    const style = create(doc, TEXT, 'text', 'list-style');
    style.setAttributeNS(STYLE, 'style:name', name);
    for (let level = 1; level <= 10; level++) {
      const entry = create(doc, TEXT, 'text', numbered ? 'list-level-style-number' : 'list-level-style-bullet');
      entry.setAttributeNS(TEXT, 'text:level', String(level));
      if (numbered) {
        entry.setAttributeNS(STYLE, 'style:num-suffix', '.');
        entry.setAttributeNS(STYLE, 'style:num-format', ['1', 'a', 'i'][(level - 1) % 3]);
      } else entry.setAttributeNS(TEXT, 'text:bullet-char', ['•', '◦', '▪'][(level - 1) % 3]);
      const props = create(doc, STYLE, 'style', 'list-level-properties');
      props.setAttributeNS(TEXT, 'text:list-level-position-and-space-mode', 'label-alignment');
      const label = create(doc, STYLE, 'style', 'list-level-label-alignment');
      label.setAttributeNS(TEXT, 'text:label-followed-by', 'listtab');
      label.setAttributeNS(TEXT, 'text:list-tab-stop-position', `${(0.635 * (level + 1)).toFixed(3)}cm`);
      label.setAttributeNS(FO, 'fo:text-indent', '-0.635cm');
      label.setAttributeNS(FO, 'fo:margin-left', `${(0.635 * (level + 1)).toFixed(3)}cm`);
      props.append(label);
      entry.append(props);
      style.append(entry);
    }
    automaticStyles().append(style);
    model.listStyles.set(name, style);
    return name;
  }
  function toggleList(numbered) {
    blockCommand(nodes => {
      const kind = numbered ? 'ol' : 'ul';
      const items = nodes.map(node => node.parentNode).filter(parent => is(parent, TEXT, 'list-item'));
      const allListed = items.length === nodes.length;
      const lists = [...new Set(items.map(item => item.parentNode))];
      if (allListed && lists.every(list => listKind(list) === kind)) {
        // Turning a list off: its paragraphs go back into the flow where the list was.
        for (const node of nodes) {
          const item = node.parentNode, list = item.parentNode;
          const after = children(list).slice(children(list).indexOf(item) + 1);
          if (after.length) {
            const rest = list.cloneNode(false);
            for (const other of after) rest.append(other);
            list.after(rest);
          }
          list.after(node);
          for (const child of children(item)) if (child !== node) node.after(child);
          item.remove();
          if (!children(list).some(child => is(child, TEXT, 'list-item', 'list-header'))) list.remove();
        }
        return;
      }
      if (allListed) {
        // Switching between bullets and numbers restyles the lists.
        for (const list of lists) {
          let top = list;
          while (is(top.parentNode?.parentNode, TEXT, 'list')) top = top.parentNode.parentNode;
          top.setAttributeNS(TEXT, 'text:style-name', listStyle(numbered));
        }
        return;
      }
      const name = listStyle(numbered);
      let current = null;
      for (const node of nodes) {
        if (is(node.parentNode, TEXT, 'list-item')) { current = null; continue; }
        if (!current || current.nextSibling !== node) {
          current = create(doc, TEXT, 'text', 'list');
          current.setAttributeNS(TEXT, 'text:style-name', name);
          node.before(current);
        }
        const item = create(doc, TEXT, 'text', 'list-item');
        current.append(item);
        item.append(node);
      }
    });
  }

  // --- Toolbar ---

  const controls = {};
  function tool(name, label, action, toggle = false) {
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
    controls[name] = b;
    return b;
  }
  const divider = () => { const d = document.createElement('span'); d.className = 'office-divider'; toolbar.append(d); };
  function picker(label, options, action) {
    const select = document.createElement('select');
    select.className = 'office-select';
    select.setAttribute('aria-label', label);
    select.title = label;
    for (const [value, text] of options) {
      const option = document.createElement('option');
      option.value = value;
      option.textContent = text;
      select.append(option);
    }
    // Keep the text selection while the menu is used.
    let saved = null;
    select.addEventListener('mousedown', () => { saved = caretOffset(); });
    select.addEventListener('change', () => {
      if (saved) restoreCaret(saved);
      action(select.value);
    });
    toolbar.append(select);
    return select;
  }
  function colorTool(name, label, initial, action) {
    const wrap = document.createElement('label');
    wrap.className = 'office-tool office-color';
    wrap.title = label;
    wrap.innerHTML = icon(name);
    const input = document.createElement('input');
    input.type = 'color';
    input.value = initial;
    input.setAttribute('aria-label', label);
    const swatch = document.createElement('span');
    swatch.className = 'office-swatch';
    swatch.style.background = initial;
    let saved = null;
    wrap.addEventListener('mousedown', () => { saved = caretOffset(); });
    input.addEventListener('input', () => { swatch.style.background = input.value; });
    input.addEventListener('change', () => { if (saved) restoreCaret(saved); action(input.value); });
    wrap.append(input, swatch);
    toolbar.append(wrap);
    controls[name] = input;
  }
  let blockSelect = null, sizeSelect = null, saveButton = null;
  if (!readonly) {
    tool('undo', 'Undo (Ctrl+Z)', undo);
    tool('redo', 'Redo (Ctrl+Y)', redo);
    divider();
    blockSelect = picker('Paragraph style', [['0', 'Normal text'], ['1', 'Heading 1'], ['2', 'Heading 2'], ['3', 'Heading 3'], ['4', 'Heading 4']], paragraphKind);
    sizeSelect = picker('Font size', [['', 'Size'], ...SIZES.map(n => [String(n), String(n)])], value => fontSize(value));
    divider();
    tool('bold', 'Bold (Ctrl+B)', () => inlineCommand('bold'), true);
    tool('italic', 'Italic (Ctrl+I)', () => inlineCommand('italic'), true);
    tool('underline', 'Underline (Ctrl+U)', () => inlineCommand('underline'), true);
    tool('strike', 'Strikethrough', () => inlineCommand('strikeThrough'), true);
    colorTool('color', 'Text colour', '#c00000', value => inlineCommand('foreColor', value));
    colorTool('highlight', 'Highlight', '#fff176', value => inlineCommand('hiliteColor', value));
    divider();
    tool('left', 'Align left', () => align('start'), true);
    tool('center', 'Align centre', () => align('center'), true);
    tool('right', 'Align right', () => align('end'), true);
    tool('justify', 'Justify', () => align('justify'), true);
    divider();
    tool('bullets', 'Bulleted list', () => toggleList(false), true);
    tool('numbers', 'Numbered list', () => toggleList(true), true);
    tool('clear', 'Clear formatting', () => inlineCommand('removeFormat'));
  }
  const spacer = document.createElement('span');
  spacer.className = 'office-spacer';
  toolbar.append(spacer, status);
  if (!readonly) {
    saveButton = document.createElement('button');
    saveButton.type = 'button';
    saveButton.className = 'office-save';
    saveButton.innerHTML = `${icon('save')}<span>Save</span>`;
    saveButton.title = 'Save (Ctrl+S)';
    saveButton.addEventListener('click', save);
    toolbar.append(saveButton);
  }
  status.textContent = readonly ? 'Read only' : 'Saved';

  function updateTools() {
    if (readonly || destroyed) return;
    for (const control of [...Object.values(controls), blockSelect, sizeSelect]) control.disabled = saving;
    controls.undo.disabled = saving || (historyIndex === 0 && !pending);
    controls.redo.disabled = saving || historyIndex === history.length - 1 || pending;
    saveButton.disabled = saving || !dirty() || !onSave;
    const selection = window.getSelection?.();
    if (!selection?.rangeCount || !page.contains(selection.anchorNode)) return;
    const el = selection.anchorNode.nodeType === 1 ? selection.anchorNode : selection.anchorNode.parentElement;
    if (!el) return;
    const css = getComputedStyle(el);
    controls.bold.setAttribute('aria-pressed', String(Number(css.fontWeight) >= 600));
    controls.italic.setAttribute('aria-pressed', String(css.fontStyle === 'italic'));
    const lines = [];
    for (let node = el; node && node !== page; node = node.parentElement) lines.push(getComputedStyle(node).textDecorationLine);
    controls.underline.setAttribute('aria-pressed', String(lines.some(line => line.includes('underline'))));
    controls.strike.setAttribute('aria-pressed', String(lines.some(line => line.includes('line-through'))));
    const block = el.closest('p, h1, h2, h3, h4, h5, h6, li');
    if (block && page.contains(block)) {
      const alignment = getComputedStyle(block).textAlign;
      const side = { start: 'left', left: 'left', center: 'center', right: 'right', end: 'right', justify: 'justify' }[alignment] || 'left';
      for (const name of ['left', 'center', 'right', 'justify']) controls[name].setAttribute('aria-pressed', String(name === side));
      const heading = /^H([1-6])$/.exec(block.nodeName);
      blockSelect.value = heading ? String(Math.min(4, Number(heading[1]))) : '0';
      const list = block.closest('ul, ol');
      controls.bullets.setAttribute('aria-pressed', String(list?.nodeName === 'UL'));
      controls.numbers.setAttribute('aria-pressed', String(list?.nodeName === 'OL'));
    }
    const points = Math.round(parseFloat(css.fontSize) * 0.75);
    sizeSelect.value = SIZES.includes(points) ? String(points) : '';
  }

  // --- Events ---

  try { document.execCommand('defaultParagraphSeparator', false, 'p'); } catch { /* Not supported everywhere. */ }
  page.addEventListener('input', () => {
    if (readonly || saving) return;
    if (!executing) scrub();
    clearTimeout(syncTimer);
    syncTimer = setTimeout(sync, 150);
    changed();
  });
  page.addEventListener('beforeinput', event => {
    const type = event.inputType;
    if (type === 'historyUndo') { event.preventDefault(); undo(); return; }
    if (type === 'historyRedo') { event.preventDefault(); redo(); return; }
    if (type === 'insertFromDrop' || type === 'deleteByDrag') { event.preventDefault(); return; }
    if (editable()) joinInstead(event);
  });
  page.addEventListener('paste', event => {
    if (!editable()) return;
    event.preventDefault();
    event.stopPropagation();
    // Plain text only: lines become paragraphs.
    const text = (event.clipboardData?.getData('text/plain') || '').replace(/\r\n?/g, '\n');
    text.split('\n').forEach((line, i) => {
      if (i) document.execCommand('insertParagraph');
      if (line) document.execCommand('insertText', false, line);
    });
  });
  page.addEventListener('drop', event => { event.preventDefault(); event.stopPropagation(); });
  page.addEventListener('keydown', event => {
    if (event.key === 'Tab' && editable()) {
      event.preventDefault();
      const inList = window.getSelection()?.anchorNode?.parentElement?.closest('li');
      if (inList && page.contains(inList)) document.execCommand(event.shiftKey ? 'outdent' : 'indent');
      else if (!event.shiftKey) document.execCommand('insertText', false, '\t');
    }
  });
  root.addEventListener('keydown', event => {
    const mod = event.ctrlKey || event.metaKey;
    if (!mod) return;
    const key = event.key.toLowerCase();
    if (key === 's') { event.preventDefault(); event.stopPropagation(); save(); }
    else if (key === 'z' && !readonly) { event.preventDefault(); event.stopPropagation(); if (event.shiftKey) redo(); else undo(); }
    else if (key === 'y' && !readonly) { event.preventDefault(); event.stopPropagation(); redo(); }
    else if (['b', 'i', 'u'].includes(key) && !event.shiftKey && !event.altKey && page.contains(event.target)) {
      // The browser's own shortcuts would write formatting the page's CSP ignores.
      event.preventDefault(); event.stopPropagation();
      inlineCommand({ b: 'bold', i: 'italic', u: 'underline' }[key]);
    }
  }, true);
  // Keys typed in the document are not file-list shortcuts.
  page.addEventListener('keydown', event => { if (event.key !== 'Escape') event.stopPropagation(); });
  const onSelection = () => { if (!destroyed && page.contains(document.getSelection()?.anchorNode)) updateTools(); };
  document.addEventListener('selectionchange', onSelection);

  /** Styles the editor made that no text uses any more (left by undone or retyped edits). */
  function pruneStyles() {
    const used = new Set();
    for (const el of doc.getElementsByTagNameNS('*', '*')) {
      for (const attr of el.attributes) if (attr.localName === 'style-name' || attr.localName === 'parent-style-name') used.add(attr.value);
    }
    const styles = named(doc.documentElement, OFFICE, 'automatic-styles')[0];
    for (const style of styles ? children(styles) : []) {
      const name = style.getAttributeNS(STYLE, 'name');
      if (!/^OwnNAS_[TP]\d+$/.test(name) || used.has(name)) continue;
      style.remove();
      model.styleByName.delete(`${style.getAttributeNS(STYLE, 'family')}:${name}`);
      for (const [key, value] of autoStyles) if (value === name) autoStyles.delete(key);
    }
  }
  async function save() {
    if (readonly || saving || destroyed || !onSave) return;
    if (pending || syncTimer) snapshot();
    if (!dirty()) return;
    pruneStyles();
    saving = true;
    const snapshotXml = history[historyIndex];
    page.contentEditable = 'false';
    updateTools();
    status.textContent = 'Saving…';
    try {
      await onSave(model.export());
      if (!destroyed) { savedXml = snapshotXml; onChange(dirty()); status.textContent = dirty() ? 'Unsaved changes' : 'Saved'; }
    } catch (err) {
      if (!destroyed) status.textContent = `Save failed: ${err.message}`;
    } finally {
      saving = false;
      if (!destroyed) {
        page.contentEditable = readonly ? 'false' : 'true';
        updateTools();
      }
    }
  }

  render(false);
  return {
    export: () => { if (pending || syncTimer) snapshot(); pruneStyles(); return model.export(); },
    save, undo, redo,
    get dirty() { return dirty(); },
    destroy() {
      destroyed = true;
      clearTimeout(syncTimer);
      clearTimeout(snapshotTimer);
      document.removeEventListener('selectionchange', onSelection);
      for (const url of imageUrls.values()) URL.revokeObjectURL(url);
      root.remove();
    },
  };
}
