/** Framework-independent, preservation-first DOCX editor. See docx-editor.md. */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';
const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const children = (node, name) => [...node.children].filter(n => n.namespaceURI === W && n.localName === name);
const el = (doc, name) => doc.createElementNS(W, `w:${name}`);
export function openDocx(bytes) {
  if (bytes.length > 25 * 1024 * 1024) throw new Error('Documents up to 25 MB are supported.');
  let total = 0, count = 0;
  const parts = unzipSync(bytes, { filter(part) {
    total += part.originalSize;
    if (++count > 4096 || total > 100 * 1024 * 1024) throw new Error('Document package is too large.');
    if (part.name.startsWith('_xmlsignatures/')) throw new Error('Digitally signed documents cannot be edited.');
    return true;
  }});
  if (!parts['word/document.xml'] || !parts['[Content_Types].xml']) throw new Error('This is not a DOCX document.');
  if (parts['word/settings.xml']) {
    const settings = new DOMParser().parseFromString(strFromU8(parts['word/settings.xml']), 'application/xml');
    const protection = settings.getElementsByTagNameNS(W, 'documentProtection')[0];
    if (protection && ['1', 'true', 'on'].includes(protection.getAttributeNS(W, 'enforcement'))) throw new Error('Protected documents cannot be edited.');
  }
  const source = strFromU8(parts['word/document.xml']);
  if (/<!DOCTYPE|<!ENTITY/i.test(source)) throw new Error('Document XML entities are unsupported.');
  const xml = new DOMParser().parseFromString(source, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error('Document XML is invalid.');
  const body = xml.getElementsByTagNameNS(W, 'body')[0];
  if (!body) throw new Error('Document body is missing.');
  return { xml, body, export() {
    parts['word/document.xml'] = strToU8(new XMLSerializer().serializeToString(xml));
    return zipSync(parts, { level: 6 });
  }};
}

/** Mount an editor. onSave receives Uint8Array and resolves after persistence. */
export function mountDocxEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openDocx(bytes);
  const paragraphs = new WeakMap(), runs = new WeakMap();
  let activeRun = null, destroyed = false, saving = false, lastTyping = null;
  const serialize = () => new XMLSerializer().serializeToString(model.xml);
  const history = [serialize()]; let historyIndex = 0, savedXml = history[0];
  const isDirty = () => history[historyIndex] !== savedXml;
  const root = document.createElement('section'); root.className = 'docx-editor';
  const toolbar = document.createElement('div'); toolbar.className = 'docx-toolbar'; toolbar.setAttribute('aria-label', 'Document tools');
  const status = document.createElement('span'); status.className = 'docx-status'; status.setAttribute('role', 'status');
  const page = document.createElement('div'); page.className = 'docx-page';
  const note = document.createElement('p'); note.className = 'muted';
  note.textContent = 'Click text to edit. Formatting applies to the active text run; alignment and headings apply to its paragraph. Layout is approximate. Unsupported content is preserved in the file.';
  root.append(toolbar, note, page); container.replaceChildren(root);
  let undoButton, redoButton, saveButton, sizeSelect, headingSelect, colorInput;
  const formatButtons = new Map();
  function button(label, action) {
    const b = document.createElement('button'); b.type = 'button'; b.textContent = label;
    b.addEventListener('mousedown', e => e.preventDefault());
    b.addEventListener('click', action); toolbar.append(b); return b;
  }
  function select(label, options, action) {
    const input = document.createElement('select'); input.setAttribute('aria-label', label);
    for (const [value, name] of options) { const o = document.createElement('option'); o.value = value; o.textContent = name; input.append(o); }
    input.addEventListener('change', () => action(input.value)); toolbar.append(input); return input;
  }
  function activeParagraph() { let p = activeRun; while (p && p.localName !== 'p') p = p.parentElement; return p; }
  function property(parent, name, order) {
    let n = children(parent, name)[0];
    if (!n) {
      n = el(model.xml, name);
      const before = [...parent.children].find(child => order.indexOf(child.localName) > order.indexOf(name));
      parent.insertBefore(n, before || null);
    }
    return n;
  }
  const runOrder = ['rStyle','rFonts','b','bCs','i','iCs','caps','smallCaps','strike','dstrike','outline','shadow','emboss','imprint','noProof','snapToGrid','vanish','webHidden','color','spacing','w','kern','position','sz','szCs','highlight','u','effect','bdr','shd','fitText','vertAlign','rtl','cs','em','lang','eastAsianLayout','specVanish','oMath','rPrChange'];
  const pOrder = ['pStyle','keepNext','keepLines','pageBreakBefore','framePr','widowControl','numPr','suppressLineNumbers','pBdr','shd','tabs','suppressAutoHyphens','kinsoku','wordWrap','overflowPunct','topLinePunct','autoSpaceDE','autoSpaceDN','bidi','adjustRightInd','snapToGrid','spacing','ind','contextualSpacing','mirrorIndents','suppressOverlap','jc','textDirection','textAlignment','textboxTightWrap','outlineLvl','divId','cnfStyle','rPr','sectPr','pPrChange'];
  function props(node, kind) { let n = children(node, kind)[0]; if (!n) { n = el(model.xml, kind); node.prepend(n); } return n; }
  const enabled = n => n && !['0','false','off','none'].includes(n.getAttributeNS(W, 'val'));
  function changed(typingNode = null) {
    const xml = serialize(); if (xml === history[historyIndex]) return;
    const now = Date.now(), coalesce = typingNode && lastTyping?.node === typingNode && now - lastTyping.time < 600 && historyIndex === history.length - 1 && historyIndex > 0 && history[historyIndex] !== savedXml;
    history.splice(historyIndex + 1);
    if (coalesce) history[historyIndex] = xml;
    else { history.push(xml); historyIndex++; }
    let size = history.reduce((sum, entry) => sum + entry.length * 2, 0);
    while (history.length > 2 && (history.length > 50 || size > 20 * 1024 * 1024)) { size -= history.shift().length * 2; historyIndex--; }
    lastTyping = typingNode ? { node: typingNode, time: now } : null;
    onChange(isDirty()); status.textContent = isDirty() ? 'Unsaved changes' : 'Saved'; updateTools();
  }
  function format(name, value) {
    if (!activeRun || saving || readonly) return;
    const rPr = props(activeRun, 'rPr'), wasEnabled = enabled(children(rPr, name)[0]);
    const n = property(rPr, name, runOrder);
    if (value === undefined) value = wasEnabled ? (name === 'u' ? 'none' : '0') : (name === 'u' ? 'single' : '1');
    n.setAttributeNS(W, 'w:val', value);
    if (name === 'color') { n.removeAttributeNS(W, 'themeColor'); n.removeAttributeNS(W, 'themeTint'); n.removeAttributeNS(W, 'themeShade'); }
    changed(); render(); focusRun();
  }
  function align(value) {
    const p = activeParagraph(); if (!p || saving || readonly) return;
    property(props(p, 'pPr'), 'jc', pOrder).setAttributeNS(W, 'w:val', value); changed(); render(); focusRun();
  }
  function heading(level) {
    const p = activeParagraph(); if (!p || saving || readonly) return;
    const pPr = props(p, 'pPr');
    // Direct outline/font formatting works without assuming a Heading style exists in the source package.
    if (level === '0') children(pPr, 'outlineLvl')[0]?.remove();
    else property(pPr, 'outlineLvl', pOrder).setAttributeNS(W, 'w:val', String(Number(level) - 1));
    for (const r of children(p, 'r')) {
      if (![...r.children].every(n => ['rPr','t'].includes(n.localName))) continue;
      const rPr = props(r, 'rPr');
      property(rPr, 'sz', runOrder).setAttributeNS(W, 'w:val', ({ 0: '24', 1: '48', 2: '36', 3: '28' })[level]);
      property(rPr, 'b', runOrder).setAttributeNS(W, 'w:val', level === '0' ? '0' : '1');
    }
    changed(); render(); focusRun();
  }
  function nodePath(node) {
    const path = []; while (node && node !== model.xml.documentElement) { path.unshift([...node.parentElement.children].indexOf(node)); node = node.parentElement; } return path;
  }
  function restore(index) {
    if (readonly || saving || index < 0 || index >= history.length) return;
    const path = activeRun ? nodePath(activeRun) : null;
    const xml = new DOMParser().parseFromString(history[index], 'application/xml');
    model.xml.replaceChild(model.xml.importNode(xml.documentElement, true), model.xml.documentElement);
    model.body = model.xml.getElementsByTagNameNS(W, 'body')[0];
    activeRun = path?.reduce((n, i) => n?.children[i], model.xml.documentElement);
    if (activeRun?.localName !== 'r') activeRun = null;
    historyIndex = index; lastTyping = null; onChange(isDirty()); status.textContent = isDirty() ? 'Unsaved changes' : 'Saved'; render(); focusRun();
  }
  function focusRun() { runs.get(activeRun)?.focus(); }
  function insertParagraph(split = null) {
    if (readonly || saving) return;
    const p = el(model.xml, 'p'), r = el(model.xml, 'r'), t = el(model.xml, 't'); r.append(t); p.append(r);
    const current = activeParagraph();
    if (split && current && [...current.children].every(n => ['pPr','r'].includes(n.localName)) && children(current, 'r').every(run => [...run.children].every(n => ['rPr','t'].includes(n.localName)))) {
      const selection = window.getSelection();
      if (selection.rangeCount && split.span.contains(selection.anchorNode) && selection.isCollapsed) {
        const range = selection.getRangeAt(0).cloneRange(); range.selectNodeContents(split.span); range.setEnd(selection.anchorNode, selection.anchorOffset);
        const offset = range.toString().length;
        const oldRun = activeRun, oldText = split.textNode;
        t.textContent = oldText.textContent.slice(offset); oldText.textContent = oldText.textContent.slice(0, offset);
        t.setAttributeNS('http://www.w3.org/XML/1998/namespace', 'xml:space', 'preserve'); oldText.setAttributeNS('http://www.w3.org/XML/1998/namespace', 'xml:space', 'preserve');
        const rPr = children(oldRun, 'rPr')[0]; if (rPr) r.prepend(rPr.cloneNode(true));
        const textNodes = [...oldRun.children], textIndex = textNodes.indexOf(oldText);
        for (const tail of textNodes.slice(textIndex + 1)) r.append(tail);
        const runNodes = [...current.children], runIndex = runNodes.indexOf(oldRun);
        for (const tail of runNodes.slice(runIndex + 1)) p.append(tail);
        const pPr = children(current, 'pPr')[0];
        if (pPr) {
          p.prepend(pPr.cloneNode(true));
          // The section boundary belongs after the trailing half of the paragraph.
          children(pPr, 'sectPr')[0]?.remove();
        }
      }
    }
    if (current) current.after(p); else model.body.insertBefore(p, children(model.body, 'sectPr')[0] || null);
    activeRun = r; changed(); render(); focusRun();
  }
  if (!readonly) {
    undoButton = button('Undo', () => restore(historyIndex - 1)); redoButton = button('Redo', () => restore(historyIndex + 1));
    for (const [label, prop] of [['Bold','b'], ['Italic','i'], ['Underline','u']]) formatButtons.set(prop, button(label, () => format(prop)));
    sizeSelect = select('Font size', [['','Font size'], ...[8,10,11,12,14,16,18,24,36,48,72].map(n => [String(n), `${n} pt`])], value => { if (value) format('sz', String(Number(value) * 2)); });
    const label = document.createElement('label'); label.className = 'docx-color'; label.textContent = 'Text color';
    colorInput = document.createElement('input'); colorInput.type = 'color'; colorInput.value = '#202020'; colorInput.setAttribute('aria-label', 'Text color');
    colorInput.addEventListener('change', () => format('color', colorInput.value.slice(1).toUpperCase())); label.append(colorInput); toolbar.append(label);
    headingSelect = select('Paragraph style', [['0','Normal'], ['1','Heading 1'], ['2','Heading 2'], ['3','Heading 3']], heading);
    for (const [label, value] of [['Align left','left'], ['Align center','center'], ['Align right','right'], ['Justify','both']]) button(label, () => align(value));
    button('Add paragraph', () => insertParagraph()); saveButton = button('Save DOCX', save);
  }
  toolbar.append(status); status.textContent = readonly ? 'Read only' : 'Ready';
  function updateTools() {
    if (readonly) return;
    toolbar.querySelectorAll('button, select, input').forEach(n => n.disabled = saving);
    undoButton.disabled = saving || historyIndex === 0; redoButton.disabled = saving || historyIndex === history.length - 1;
    saveButton.disabled = saving || !isDirty() || !onSave;
    const rPr = activeRun && children(activeRun, 'rPr')[0];
    for (const [name, button] of formatButtons) { button.disabled = saving || !activeRun; button.setAttribute('aria-pressed', String(!!enabled(rPr && children(rPr, name)[0]))); }
    sizeSelect.disabled = colorInput.disabled = headingSelect.disabled = saving || !activeRun;
    sizeSelect.value = rPr ? String(Number(children(rPr, 'sz')[0]?.getAttributeNS(W, 'val')) / 2) : '';
    const color = rPr && children(rPr, 'color')[0]?.getAttributeNS(W, 'val'); colorInput.value = /^[0-9a-f]{6}$/i.test(color || '') ? `#${color}` : '#202020';
    const pPr = activeParagraph() && children(activeParagraph(), 'pPr')[0];
    const outline = pPr && children(pPr, 'outlineLvl')[0]?.getAttributeNS(W, 'val'); headingSelect.value = outline != null ? String(Number(outline) + 1) : '0';
  }
  async function save() {
    if (readonly || saving || destroyed || !isDirty() || !onSave) return;
    saving = true; const snapshot = history[historyIndex]; lastTyping = null;
    page.querySelectorAll('[contenteditable]').forEach(n => n.setAttribute('contenteditable', 'false')); updateTools(); status.textContent = 'Saving…';
    try { await onSave(model.export()); if (!destroyed) { savedXml = snapshot; onChange(isDirty()); status.textContent = 'Saved'; } }
    catch (err) { if (!destroyed) status.textContent = `Save failed: ${err.message}`; }
    finally { saving = false; if (!destroyed) render(); }
  }
  function renderNode(node, target, locked = false) {
    if (node.namespaceURI !== W) return;
    const name = node.localName; locked ||= ['del','ins','sdt','fldSimple'].includes(name);
    if (name === 'p') {
      locked ||= node.getElementsByTagNameNS(W, 'fldChar').length > 0;
      const p = document.createElement('p'); paragraphs.set(node, p);
      const pPr = children(node, 'pPr')[0];
      const jc = pPr && children(pPr, 'jc')[0]?.getAttributeNS(W, 'val');
      if (['left','right','center','both'].includes(jc)) p.style.textAlign = jc === 'both' ? 'justify' : jc;
      const outline = pPr && children(pPr, 'outlineLvl')[0]?.getAttributeNS(W, 'val');
      if (outline != null && Number(outline) < 6) { p.setAttribute('role', 'heading'); p.setAttribute('aria-level', String(Number(outline) + 1)); }
      target.append(p); [...node.children].forEach(n => renderNode(n, p, locked)); if (!p.childNodes.length) p.append(document.createElement('br'));
    } else if (['tbl','tr','tc'].includes(name)) {
      const element = document.createElement({ tbl:'table', tr:'tr', tc:'td' }[name]); target.append(element); [...node.children].forEach(n => renderNode(n, element, locked));
    } else if (name === 'r') {
      const rPr = children(node, 'rPr')[0], safe = !locked && [...node.children].every(n => ['rPr','t'].includes(n.localName));
      for (const child of node.children) {
        if (child.localName === 't') {
          const span = document.createElement('span'); span.textContent = child.textContent; span.className = 'docx-run';
          for (const [prop, style, value] of [['b','fontWeight','bold'], ['i','fontStyle','italic'], ['u','textDecoration','underline']]) if (enabled(rPr && children(rPr, prop)[0])) span.style[style] = value;
          const size = rPr && Number(children(rPr, 'sz')[0]?.getAttributeNS(W, 'val')); if (size >= 2 && size <= 288) span.style.fontSize = `${size / 2}pt`;
          const color = rPr && children(rPr, 'color')[0]?.getAttributeNS(W, 'val'); if (/^[0-9a-f]{6}$/i.test(color || '')) span.style.color = `#${color}`;
          if (safe && !readonly) {
            if (!runs.has(node) || !runs.get(node).isConnected) runs.set(node, span);
            span.setAttribute('contenteditable', saving ? 'false' : 'true'); span.setAttribute('role','textbox'); span.setAttribute('aria-label','Document text');
            span.addEventListener('focus', () => { activeRun = node; updateTools(); });
            span.addEventListener('beforeinput', e => {
              if (e.inputType === 'insertParagraph') { e.preventDefault(); activeRun = node; insertParagraph({ span, textNode: child }); }
              if (e.inputType === 'insertLineBreak') e.preventDefault();
              if (e.inputType === 'historyUndo' || e.inputType === 'historyRedo') { e.preventDefault(); restore(historyIndex + (e.inputType === 'historyUndo' ? -1 : 1)); }
            });
            span.addEventListener('drop', e => { e.preventDefault(); e.stopPropagation(); });
            span.addEventListener('paste', e => {
              e.preventDefault(); e.stopPropagation(); const text = e.clipboardData.getData('text/plain').replace(/[\r\n]+/g,' '), selection = window.getSelection();
              if (!selection.rangeCount) return; const range = selection.getRangeAt(0); if (!span.contains(range.commonAncestorContainer)) return;
              range.deleteContents(); const t = document.createTextNode(text); range.insertNode(t); range.setStartAfter(t); range.collapse(true); selection.removeAllRanges(); selection.addRange(range); span.dispatchEvent(new Event('input'));
            });
            span.addEventListener('input', () => { child.textContent = span.textContent; child.setAttributeNS('http://www.w3.org/XML/1998/namespace','xml:space','preserve'); changed(child); });
          }
          target.append(span);
        } else if (child.localName === 'tab') target.append(document.createTextNode('\t'));
        else if (child.localName === 'br') target.append(document.createElement('br'));
        else if (child.localName !== 'rPr') { const marker = document.createElement('span'); marker.className = 'muted'; marker.textContent = ' [preserved content] '; target.append(marker); }
      }
    } else if (!['pPr','tblPr','tblGrid','trPr','tcPr','sectPr'].includes(name)) [...node.children].forEach(n => renderNode(n, target, locked));
  }
  function render() { page.replaceChildren(); [...model.body.children].forEach(n => renderNode(n, page)); updateTools(); }
  root.addEventListener('keydown', e => {
    if (!e.ctrlKey && !e.metaKey) return;
    const key = e.key.toLowerCase();
    if (key === 's') { e.preventDefault(); e.stopPropagation(); save(); }
    if (!readonly && (key === 'z' || key === 'y')) { e.preventDefault(); e.stopPropagation(); restore(historyIndex + (key === 'y' || e.shiftKey ? 1 : -1)); }
    if (!readonly && ['b','i','u'].includes(key) && activeRun) { e.preventDefault(); e.stopPropagation(); format(key); }
  });
  render();
  return { export: () => model.export(), save, undo: () => restore(historyIndex - 1), redo: () => restore(historyIndex + 1), get dirty() { return isDirty(); }, destroy() { destroyed = true; root.remove(); } };
}
