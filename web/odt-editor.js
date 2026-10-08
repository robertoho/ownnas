/** Reusable, preservation-first OpenDocument Text editor. See odt-editor.md. */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';

const OFFICE = 'urn:oasis:names:tc:opendocument:xmlns:office:1.0';
const TEXT = 'urn:oasis:names:tc:opendocument:xmlns:text:1.0';
const STYLE = 'urn:oasis:names:tc:opendocument:xmlns:style:1.0';
const FO = 'urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0';
const TABLE = 'urn:oasis:names:tc:opendocument:xmlns:table:1.0';
const children = node => [...node.children];
const named = (node, ns, local) => children(node).filter(child => child.namespaceURI === ns && child.localName === local);
const create = (doc, ns, prefix, local) => doc.createElementNS(ns, `${prefix}:${local}`);
function hasValidMimetypeHeader(bytes) {
  if (bytes.length < 30) return false;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(0, true) !== 0x04034b50 || view.getUint16(8, true) !== 0) return false;
  const nameLength = view.getUint16(26, true), extraLength = view.getUint16(28, true);
  const start = 30 + nameLength + extraLength, size = view.getUint32(18, true);
  if (extraLength !== 0 || start + size > bytes.length || strFromU8(bytes.subarray(30, 30 + nameLength)) !== 'mimetype') return false;
  return strFromU8(bytes.subarray(start, start + size)) === 'application/vnd.oasis.opendocument.text';
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
  const source = strFromU8(parts['content.xml']);
  if (/<!DOCTYPE|<!ENTITY/i.test(source)) throw new Error('Document XML entities are unsupported.');
  const xml = new DOMParser().parseFromString(source, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error('Document XML is invalid.');
  const body = xml.getElementsByTagNameNS(OFFICE, 'text')[0];
  if (!body) throw new Error('Document body is missing.');
  const styleDocs = [xml];
  if (parts['styles.xml']) {
    const styleSource = strFromU8(parts['styles.xml']);
    if (/<!DOCTYPE|<!ENTITY/i.test(styleSource)) throw new Error('Document styles contain unsupported XML entities.');
    const styles = new DOMParser().parseFromString(styleSource, 'application/xml');
    if (styles.getElementsByTagName('parsererror').length) throw new Error('Document styles are invalid.');
    styleDocs.push(styles);
  }
  const styleByName = new Map();
  for (const doc of styleDocs) for (const style of doc.getElementsByTagNameNS(STYLE, 'style')) {
    const name = style.getAttributeNS(STYLE, 'name');
    if (name) styleByName.set(name, style);
  }
  function propertyValue(styleName, family, propName, attributeNs, attributeName, depth = 0) {
    if (!styleName || depth > 12) return null;
    const style = styleByName.get(styleName);
    if (!style || (family && style.getAttributeNS(STYLE, 'family') !== family)) return null;
    const direct = named(style, STYLE, propName)[0];
    const value = direct?.getAttributeNS(attributeNs, attributeName);
    if (value != null) return value;
    return propertyValue(style.getAttributeNS(STYLE, 'parent-style-name'), family, propName, attributeNs, attributeName, depth + 1);
  }
  const baseStyles = [...styleByName.values()].filter(style => style.ownerDocument !== xml);
  return { xml, body, parts, styleByName, baseStyles, propertyValue, export() {
    parts['content.xml'] = strToU8(new XMLSerializer().serializeToString(xml));
    // ODF requires the mimetype entry first and stored without compression.
    const ordered = { mimetype: { data: parts.mimetype, level: 0 } };
    for (const [name, data] of Object.entries(parts)) if (name !== 'mimetype') ordered[name] = data;
    return zipSync(ordered, { level: 6 });
  }};
}

/** Mounts a browser editor. onSave receives Uint8Array and resolves after persistence. */
export function mountOdtEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openOdt(bytes);
  const hasTextNode = node => [...node.childNodes].some(child => child.nodeType === Node.TEXT_NODE ? child.textContent.length > 0 : hasTextNode(child));
  for (const paragraph of [...model.xml.getElementsByTagNameNS(TEXT, 'p'), ...model.xml.getElementsByTagNameNS(TEXT, 'h')]) {
    if (!hasTextNode(paragraph)) paragraph.append(document.createTextNode(''));
  }
  const textSpans = new WeakMap();
  let activeText = null, destroyed = false, saving = false, lastTyping = null, styleSerial = 0;
  const serialize = () => new XMLSerializer().serializeToString(model.xml);
  const history = [serialize()]; let historyIndex = 0, savedXml = history[0];
  const dirty = () => history[historyIndex] !== savedXml;
  const root = document.createElement('section'); root.className = 'odt-editor document-editor';
  const toolbar = document.createElement('div'); toolbar.className = 'docx-toolbar'; toolbar.setAttribute('aria-label', 'Document tools');
  const status = document.createElement('span'); status.className = 'docx-status'; status.setAttribute('role', 'status');
  const page = document.createElement('div'); page.className = 'docx-page odt-page';
  const note = document.createElement('p'); note.className = 'muted';
  note.textContent = 'Edit text, then save to keep the ODT format. Layout is approximate; unsupported content and package parts are preserved.';
  root.append(toolbar, note, page); container.replaceChildren(root);
  let undoButton, redoButton, saveButton, sizeSelect, colorInput;
  const formatButtons = new Map();
  function button(label, action) {
    const b = document.createElement('button'); b.type = 'button'; b.textContent = label;
    b.addEventListener('mousedown', e => e.preventDefault()); b.addEventListener('click', action); toolbar.append(b); return b;
  }
  function select(label, options, action) {
    const input = document.createElement('select'); input.setAttribute('aria-label', label);
    for (const [value, name] of options) { const o = document.createElement('option'); o.value = value; o.textContent = name; input.append(o); }
    input.addEventListener('change', () => action(input.value)); toolbar.append(input); return input;
  }
  function changed(typingNode = null, rerender = true) {
    const current = serialize(); if (current === history[historyIndex]) return;
    const now = Date.now(), coalesce = typingNode && lastTyping?.node === typingNode && now - lastTyping.time < 600 && historyIndex === history.length - 1 && historyIndex > 0 && history[historyIndex] !== savedXml;
    history.splice(historyIndex + 1);
    if (coalesce) history[historyIndex] = current; else { history.push(current); historyIndex++; }
    let size = history.reduce((sum, entry) => sum + entry.length * 2, 0);
    while (history.length > 2 && (history.length > 50 || size > 20 * 1024 * 1024)) { size -= history.shift().length * 2; historyIndex--; }
    lastTyping = typingNode ? { node: typingNode, time: now } : null;
    onChange(dirty()); status.textContent = dirty() ? 'Unsaved changes' : 'Saved';
    if (rerender) render(); else updateTools();
  }
  function automaticStyles() {
    let styles = model.xml.getElementsByTagNameNS(OFFICE, 'automatic-styles')[0];
    if (!styles) {
      styles = create(model.xml, OFFICE, 'office', 'automatic-styles');
      const bodyNode = model.xml.getElementsByTagNameNS(OFFICE, 'body')[0];
      model.xml.documentElement.insertBefore(styles, bodyNode || null);
    }
    return styles;
  }
  function textStyle(node, properties) {
    const parentSpan = node.parentElement?.namespaceURI === TEXT && node.parentElement.localName === 'span' ? node.parentElement : null;
    const inherited = parentSpan?.getAttributeNS(TEXT, 'style-name') || '';
    let name; do { name = `OwnNAS_T${++styleSerial}`; } while (model.styleByName.has(name));
    const style = create(model.xml, STYLE, 'style', 'style');
    style.setAttributeNS(STYLE, 'style:name', name);
    style.setAttributeNS(STYLE, 'style:family', 'text');
    if (inherited) style.setAttributeNS(STYLE, 'style:parent-style-name', inherited);
    const props = create(model.xml, STYLE, 'style', 'text-properties');
    for (const [ns, prefix, key, value] of properties) props.setAttributeNS(ns, `${prefix}:${key}`, value);
    style.append(props); automaticStyles().append(style); model.styleByName.set(name, style);
    const span = create(model.xml, TEXT, 'text', 'span'); span.setAttributeNS(TEXT, 'text:style-name', name);
    node.parentNode.insertBefore(span, node); span.append(node);
    return span;
  }
  function format(prop, value) {
    if (!activeText || readonly || saving) return;
    const inheritedSpan = activeText.parentElement?.namespaceURI === TEXT && activeText.parentElement.localName === 'span' ? activeText.parentElement : null;
    const inheritedName = inheritedSpan?.getAttributeNS(TEXT, 'style-name') || '';
    const current = inheritedName;
    let key, ns, prefix;
    if (prop === 'bold') { key = 'font-weight'; ns = FO; prefix = 'fo'; if (value === undefined) value = model.propertyValue(current, 'text', 'text-properties', FO, key) === 'bold' ? 'normal' : 'bold'; }
    if (prop === 'italic') { key = 'font-style'; ns = FO; prefix = 'fo'; if (value === undefined) value = model.propertyValue(current, 'text', 'text-properties', FO, key) === 'italic' ? 'normal' : 'italic'; }
    if (prop === 'underline') { key = 'text-underline-style'; ns = STYLE; prefix = 'style'; if (value === undefined) value = model.propertyValue(current, 'text', 'text-properties', STYLE, key) === 'solid' ? 'none' : 'solid'; }
    if (prop === 'color') { key = 'color'; ns = FO; prefix = 'fo'; }
    if (prop === 'size') { key = 'font-size'; ns = FO; prefix = 'fo'; }
    textStyle(activeText, [[ns, prefix, key, value]]);
    changed(); textSpans.get(activeText)?.focus();
  }
  function paragraphStyle(node, key, value) {
    if (readonly || saving || !node) return;
    let name; do { name = `OwnNAS_P${++styleSerial}`; } while (model.styleByName.has(name));
    const style = create(model.xml, STYLE, 'style', 'style'); style.setAttributeNS(STYLE, 'style:name', name); style.setAttributeNS(STYLE, 'style:family', 'paragraph');
    const props = create(model.xml, STYLE, 'style', 'paragraph-properties'); props.setAttributeNS(FO, `fo:${key}`, value); style.append(props); automaticStyles().append(style);
    model.styleByName.set(name, style); node.setAttributeNS(TEXT, 'text:style-name', name); changed();
  }
  function insertParagraph() {
    if (readonly || saving) return;
    let current = activeText?.parentElement;
    while (current && !(current.namespaceURI === TEXT && ['p','h'].includes(current.localName))) current = current.parentElement;
    const paragraph = create(model.xml, TEXT, 'text', 'p'), textNode = document.createTextNode('');
    paragraph.append(textNode);
    if (current?.parentNode) current.after(paragraph); else model.body.append(paragraph);
    activeText = textNode; changed(); textSpans.get(textNode)?.focus();
  }
  function setHeading(level) {
    let paragraph = activeText?.parentElement;
    while (paragraph && !(paragraph.namespaceURI === TEXT && ['p','h'].includes(paragraph.localName))) paragraph = paragraph.parentElement;
    if (!paragraph) return;
    if (level === '0') {
      const p = create(model.xml, TEXT, 'text', 'p');
      for (const attr of [...paragraph.attributes]) if (!(attr.namespaceURI === TEXT && attr.localName === 'style-name')) p.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
      paragraph.parentNode.replaceChild(p, paragraph); while (paragraph.firstChild) p.append(paragraph.firstChild);
    } else {
      const h = create(model.xml, TEXT, 'text', 'h'); h.setAttributeNS(TEXT, 'text:outline-level', level);
      for (const attr of [...paragraph.attributes]) if (!(attr.namespaceURI === TEXT && attr.localName === 'outline-level')) h.setAttributeNS(attr.namespaceURI, attr.name, attr.value);
      paragraph.parentNode.replaceChild(h, paragraph); while (paragraph.firstChild) h.append(paragraph.firstChild);
    }
    changed();
  }
  function restore(index) {
    if (readonly || saving || index < 0 || index >= history.length) return;
    const xml = new DOMParser().parseFromString(history[index], 'application/xml');
    model.xml.replaceChild(model.xml.importNode(xml.documentElement, true), model.xml.documentElement);
    model.body = model.xml.getElementsByTagNameNS(OFFICE, 'text')[0]; model.styleByName.clear();
    for (const style of model.baseStyles) { const name = style.getAttributeNS(STYLE, 'name'); if (name) model.styleByName.set(name, style); }
    for (const style of model.xml.getElementsByTagNameNS(STYLE, 'style')) { const name = style.getAttributeNS(STYLE, 'name'); if (name) model.styleByName.set(name, style); }
    historyIndex = index; lastTyping = null; activeText = null; onChange(dirty()); status.textContent = dirty() ? 'Unsaved changes' : 'Saved'; render();
  }
  if (!readonly) {
    undoButton = button('Undo', () => restore(historyIndex - 1)); redoButton = button('Redo', () => restore(historyIndex + 1));
    for (const [label, prop] of [['Bold','bold'], ['Italic','italic'], ['Underline','underline']]) formatButtons.set(prop, button(label, () => format(prop)));
    sizeSelect = select('Font size', [['','Font size'], ...[8,10,11,12,14,16,18,24,36,48,72].map(n => [String(n), `${n} pt`])], v => { if (v) format('size', `${v}pt`); });
    const label = document.createElement('label'); label.className = 'docx-color'; label.textContent = 'Text color'; colorInput = document.createElement('input'); colorInput.type = 'color'; colorInput.value = '#202020'; colorInput.setAttribute('aria-label', 'Text color'); colorInput.addEventListener('change', () => format('color', colorInput.value)); label.append(colorInput); toolbar.append(label);
    select('Paragraph style', [['0','Normal'], ['1','Heading 1'], ['2','Heading 2'], ['3','Heading 3']], setHeading);
    for (const [labelText, value] of [['Align left','start'], ['Align center','center'], ['Align right','end'], ['Justify','justify']]) button(labelText, () => {
      let p = activeText?.parentElement; while (p && !(p.namespaceURI === TEXT && ['p','h'].includes(p.localName))) p = p.parentElement;
      paragraphStyle(p, 'text-align', value);
    });
    button('Add paragraph', insertParagraph); saveButton = button('Save ODT', save);
  }
  toolbar.append(status); status.textContent = readonly ? 'Read only' : 'Ready';
  function updateTools() {
    if (readonly) return;
    toolbar.querySelectorAll('button, select, input').forEach(n => n.disabled = saving);
    undoButton.disabled = saving || historyIndex === 0; redoButton.disabled = saving || historyIndex === history.length - 1;
    saveButton.disabled = saving || !dirty() || !onSave;
    for (const b of formatButtons.values()) b.disabled = saving || !activeText;
    sizeSelect.disabled = colorInput.disabled = saving || !activeText;
  }
  async function save() {
    if (readonly || saving || destroyed || !dirty() || !onSave) return;
    saving = true; const snapshot = history[historyIndex]; lastTyping = null; page.querySelectorAll('[contenteditable]').forEach(n => n.setAttribute('contenteditable','false')); updateTools(); status.textContent = 'Saving…';
    try { await onSave(model.export()); if (!destroyed) { savedXml = snapshot; onChange(dirty()); status.textContent = 'Saved'; } }
    catch (err) { if (!destroyed) status.textContent = `Save failed: ${err.message}`; }
    finally { saving = false; if (!destroyed) render(); }
  }
  function renderNode(node, target, inheritedStyle = '') {
    if (node.nodeType === Node.TEXT_NODE) {
      if (node.parentElement?.namespaceURI !== TEXT) return;
      const span = document.createElement('span'); span.className = 'docx-run'; span.textContent = node.textContent;
      const styleName = node.parentElement?.getAttributeNS(TEXT, 'style-name') || inheritedStyle;
      if (model.propertyValue(styleName, 'text', 'text-properties', FO, 'font-weight') === 'bold') span.style.fontWeight = 'bold';
      if (model.propertyValue(styleName, 'text', 'text-properties', FO, 'font-style') === 'italic') span.style.fontStyle = 'italic';
      if (model.propertyValue(styleName, 'text', 'text-properties', STYLE, 'text-underline-style') === 'solid') span.style.textDecoration = 'underline';
      const color = model.propertyValue(styleName, 'text', 'text-properties', FO, 'color'); if (/^#[0-9a-f]{6}$/i.test(color || '')) span.style.color = color;
      const size = model.propertyValue(styleName, 'text', 'text-properties', FO, 'font-size'); if (/^\d+(\.\d+)?(pt|px|em|%)$/.test(size || '')) span.style.fontSize = size;
      if (!readonly) {
        textSpans.set(node, span); span.setAttribute('contenteditable', saving ? 'false' : 'true'); span.setAttribute('role','textbox'); span.setAttribute('aria-label','Document text');
        span.addEventListener('focus', () => { activeText = node; updateTools(); });
        span.addEventListener('beforeinput', e => { if (e.inputType === 'insertParagraph') { e.preventDefault(); activeText = node; insertParagraph(); } else if (e.inputType === 'insertLineBreak') e.preventDefault(); });
        span.addEventListener('drop', e => { e.preventDefault(); e.stopPropagation(); });
        span.addEventListener('input', () => { node.textContent = span.textContent; node.parentElement?.setAttributeNS('http://www.w3.org/XML/1998/namespace','xml:space','preserve'); changed(node, false); });
        span.addEventListener('paste', e => { e.preventDefault(); e.stopPropagation(); const text = e.clipboardData.getData('text/plain').replace(/[\r\n]+/g,' '), selection = window.getSelection(); if (!selection.rangeCount) return; const range = selection.getRangeAt(0); if (!span.contains(range.commonAncestorContainer)) return; range.deleteContents(); const textNode = document.createTextNode(text); range.insertNode(textNode); range.setStartAfter(textNode); range.collapse(true); selection.removeAllRanges(); selection.addRange(range); span.dispatchEvent(new Event('input')); });
      }
      target.append(span); return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE || (node.namespaceURI !== TEXT && node.namespaceURI !== TABLE)) return;
    const name = node.localName;
    if (node.namespaceURI === TABLE) {
      const tableTag = { table: 'table', 'table-row': 'tr', 'table-cell': 'td', 'covered-table-cell': 'td', 'table-header-rows': 'tbody', 'table-rows': 'tbody', 'table-column': null, 'table-columns': null }[name];
      if (tableTag === null || tableTag === undefined) { for (const child of node.childNodes) renderNode(child, target); return; }
      const tableElement = document.createElement(tableTag); tableElement.className = `odt-${name}`; target.append(tableElement);
      for (const child of node.childNodes) renderNode(child, tableElement);
      return;
    }
    if (name === 's') { const count = Math.min(1000, Number(node.getAttributeNS(TEXT,'c') || 1)); target.append(document.createTextNode('\u00a0'.repeat(count))); return; }
    if (name === 'tab') { target.append(document.createTextNode('\t')); return; }
    if (name === 'line-break') { target.append(document.createElement('br')); return; }
    let tag = null;
    if (['p','h'].includes(name)) tag = 'p';
    else if (name === 'list') tag = 'ul';
    else if (name === 'list-item') tag = 'li';
    else if (name === 'table') tag = 'table';
    else if (name === 'table-row') tag = 'tr';
    else if (name === 'table-cell' || name === 'covered-table-cell') tag = 'td';
    else if (name === 'a') tag = 'span';
    const element = tag ? document.createElement(tag) : document.createElement('span');
    if (name === 'h') { const level = Number(node.getAttributeNS(TEXT,'outline-level') || 1); element.setAttribute('role','heading'); element.setAttribute('aria-level', String(Math.max(1, Math.min(6, level)))); }
    if (['table','table-row','table-cell','covered-table-cell'].includes(name)) element.classList.add('odt-'+name);
    const styleName = node.getAttributeNS(TEXT, 'style-name');
    if (['p','h'].includes(name)) {
      const align = model.propertyValue(styleName, 'paragraph', 'paragraph-properties', FO, 'text-align');
      if (['start','left','end','right','center','justify'].includes(align || '')) element.style.textAlign = ({start:'left',end:'right'})[align] || align;
      if (!node.childNodes.length) element.append(document.createElement('br'));
    }
    target.append(element);
    for (const child of node.childNodes) renderNode(child, element, styleName || inheritedStyle);
  }
  function render() {
    page.replaceChildren();
    for (const child of model.body.childNodes) renderNode(child, page);
    updateTools();
  }
  function saveKey(e) { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); e.stopPropagation(); save(); } else if ((e.ctrlKey || e.metaKey) && !readonly) { const key = e.key.toLowerCase(); if (key === 'z' || key === 'y') { e.preventDefault(); e.stopPropagation(); restore(historyIndex + (key === 'y' || e.shiftKey ? 1 : -1)); } } }
  root.addEventListener('keydown', saveKey);
  render();
  return { export: () => model.export(), save, undo: () => restore(historyIndex - 1), redo: () => restore(historyIndex + 1), get dirty() { return dirty(); }, destroy() { destroyed = true; root.remove(); } };
}
