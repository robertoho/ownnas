/** Framework-independent, preservation-first DOCX editor. See docx-editor.md. */
import { unzipSync, zipSync, strFromU8, strToU8 } from './vendor/three/addons/libs/fflate.module.js';

const W = 'http://schemas.openxmlformats.org/wordprocessingml/2006/main';
const R = 'http://schemas.openxmlformats.org/officeDocument/2006/relationships';
const PKG_REL = 'http://schemas.openxmlformats.org/package/2006/relationships';
const CT = 'http://schemas.openxmlformats.org/package/2006/content-types';
const XML_NS = 'http://www.w3.org/XML/1998/namespace';
const WP = 'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing';
const A = 'http://schemas.openxmlformats.org/drawingml/2006/main';
const PIC = 'http://schemas.openxmlformats.org/drawingml/2006/picture';
const REL = {
  styles: `${R}/styles`, numbering: `${R}/numbering`, image: `${R}/image`, hyperlink: `${R}/hyperlink`,
  header: `${R}/header`, footer: `${R}/footer`, footnotes: `${R}/footnotes`, comments: `${R}/comments`,
};
const WML_ROOT = `xmlns:w="${W}" xmlns:r="${R}"`;

const kids = (node, name) => [...node.children].filter(n => n.namespaceURI === W && n.localName === name);
const kid = (node, name) => (node ? kids(node, name)[0] : undefined);
const attr = (node, name) => node?.getAttributeNS(W, name) ?? null;
const on = n => !!n && !['0', 'false', 'off', 'none'].includes(attr(n, 'val'));
const parseXml = text => {
  if (/<!DOCTYPE|<!ENTITY/i.test(text)) throw new Error('Document XML entities are unsupported.');
  const xml = new DOMParser().parseFromString(text, 'application/xml');
  if (xml.getElementsByTagName('parsererror').length) throw new Error('Document XML is invalid.');
  return xml;
};
const toXml = xml => new XMLSerializer().serializeToString(xml);
const PREAMBLE = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n';
const resolvePart = target => {
  const segments = [];
  for (const piece of (target.startsWith('/') ? target.slice(1) : `word/${target}`).split('/')) {
    if (piece === '..') segments.pop(); else if (piece && piece !== '.') segments.push(piece);
  }
  return segments.join('/');
};

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
    const settings = parseXml(strFromU8(parts['word/settings.xml']));
    const protection = settings.getElementsByTagNameNS(W, 'documentProtection')[0];
    if (protection && ['1', 'true', 'on'].includes(attr(protection, 'enforcement'))) throw new Error('Protected documents cannot be edited.');
  }
  const xml = parseXml(strFromU8(parts['word/document.xml']));
  const body = xml.getElementsByTagNameNS(W, 'body')[0];
  if (!body) throw new Error('Document body is missing.');

  // Parts this editor may change are parsed once and written back only if they were.
  const loaded = new Map(), touched = new Set();
  const load = name => {
    if (!loaded.has(name)) loaded.set(name, parts[name] ? parseXml(strFromU8(parts[name])) : null);
    return loaded.get(name);
  };
  const RELS = 'word/_rels/document.xml.rels', TYPES = '[Content_Types].xml';
  const relations = () => {
    let rels = load(RELS);
    if (!rels) { rels = parseXml(`<Relationships xmlns="${PKG_REL}"/>`); loaded.set(RELS, rels); }
    return rels;
  };
  const model = {
    xml, body, parts,
    part: load,
    touch(name) { touched.add(name); },
    relation(id) {
      const found = [...(load(RELS)?.documentElement.children ?? [])].find(n => n.getAttribute('Id') === id);
      return found ? { type: found.getAttribute('Type'), target: found.getAttribute('Target'), external: found.getAttribute('TargetMode') === 'External' } : null;
    },
    relationOfType(type) {
      const found = [...(load(RELS)?.documentElement.children ?? [])].find(n => n.getAttribute('Type') === type);
      return found ? resolvePart(found.getAttribute('Target')) : null;
    },
    addRelation(type, target, external = false) {
      const rels = relations(), root = rels.documentElement;
      const used = new Set([...root.children].map(n => n.getAttribute('Id')));
      let number = used.size + 1;
      while (used.has(`rId${number}`)) number++;
      const rel = rels.createElementNS(PKG_REL, 'Relationship');
      rel.setAttribute('Id', `rId${number}`); rel.setAttribute('Type', type); rel.setAttribute('Target', target);
      if (external) rel.setAttribute('TargetMode', 'External');
      root.append(rel); touched.add(RELS);
      return `rId${number}`;
    },
    declareType(partOrExtension, mime) {
      const types = load(TYPES), root = types.documentElement, isPart = partOrExtension.startsWith('/');
      const present = [...root.children].some(n => (isPart ? n.getAttribute('PartName') === partOrExtension : n.getAttribute('Extension')?.toLowerCase() === partOrExtension));
      if (present) return;
      const entry = types.createElementNS(root.namespaceURI || CT, isPart ? 'Override' : 'Default');
      entry.setAttribute(isPart ? 'PartName' : 'Extension', partOrExtension); entry.setAttribute('ContentType', mime);
      root.append(entry); touched.add(TYPES);
    },
    /** A WordprocessingML part, made and connected to the document if the package lacks one. */
    ensurePart(type, name, rootName, mime) {
      const existing = model.relationOfType(type);
      if (existing && load(existing)) return existing;
      const part = `word/${name}`;
      loaded.set(part, parseXml(`<w:${rootName} ${WML_ROOT}/>`));
      model.declareType(`/${part}`, mime);
      model.addRelation(type, name);
      touched.add(part);
      return part;
    },
    export() {
      for (const name of touched) if (loaded.get(name)) parts[name] = strToU8(PREAMBLE + toXml(loaded.get(name)).replace(/^<\?xml[^>]*\?>\s*/, ''));
      parts['word/document.xml'] = strToU8(toXml(xml));
      return zipSync(parts, { level: 6 });
    },
  };
  return model;
}

const RUN_ORDER = ['rStyle','rFonts','b','bCs','i','iCs','caps','smallCaps','strike','dstrike','outline','shadow','emboss','imprint','noProof','snapToGrid','vanish','webHidden','color','spacing','w','kern','position','sz','szCs','highlight','u','effect','bdr','shd','fitText','vertAlign','rtl','cs','em','lang','eastAsianLayout','specVanish','oMath'];
const PARA_ORDER = ['pStyle','keepNext','keepLines','pageBreakBefore','framePr','widowControl','numPr','suppressLineNumbers','pBdr','shd','tabs','suppressAutoHyphens','kinsoku','wordWrap','overflowPunct','topLinePunct','autoSpaceDE','autoSpaceDN','bidi','adjustRightInd','snapToGrid','spacing','ind','contextualSpacing','mirrorIndents','suppressOverlap','jc','textDirection','textAlignment','textboxTightWrap','outlineLvl','divId','cnfStyle','rPr','sectPr','pPrChange'];
const HIGHLIGHTS = { yellow:'#ffff00', green:'#00ff00', cyan:'#00ffff', magenta:'#ff00ff', blue:'#0000ff', red:'#ff0000', darkBlue:'#000080', darkCyan:'#008080', darkGreen:'#008000', darkMagenta:'#800080', darkRed:'#800000', darkYellow:'#808000', darkGray:'#808080', lightGray:'#c0c0c0', black:'#000000', white:'#ffffff' };
const SANS = new Set(['Arial','Calibri','Helvetica','Segoe UI','Tahoma','Trebuchet MS','Verdana','Aptos','Carlito','Liberation Sans']);
const MONO = new Set(['Consolas','Courier New','Menlo','Liberation Mono']);
const FONTS = ['Aptos','Arial','Calibri','Cambria','Consolas','Courier New','Garamond','Georgia','Helvetica','Tahoma','Times New Roman','Trebuchet MS','Verdana'];
const SIZES = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36, 48, 72];
const STACK = name => `"${name}", ${SANS.has(name) ? 'Arial, sans-serif' : MONO.has(name) ? 'Menlo, monospace' : 'Georgia, serif'}`;
/** Styles offered in every document; written into styles.xml the first time one is used. */
const BUILT_IN = {
  Title: { name: 'Title', r: '<w:sz w:val="56"/>', p: '<w:spacing w:after="80"/>' },
  Subtitle: { name: 'Subtitle', r: '<w:color w:val="595959"/><w:sz w:val="28"/>', p: '<w:spacing w:after="160"/>' },
  Heading1: { name: 'heading 1', label: 'Heading 1', r: '<w:b/><w:sz w:val="40"/>', p: '<w:keepNext/><w:spacing w:before="360" w:after="80"/><w:outlineLvl w:val="0"/>' },
  Heading2: { name: 'heading 2', label: 'Heading 2', r: '<w:b/><w:sz w:val="32"/>', p: '<w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="1"/>' },
  Heading3: { name: 'heading 3', label: 'Heading 3', r: '<w:b/><w:sz w:val="28"/>', p: '<w:keepNext/><w:spacing w:before="200" w:after="60"/><w:outlineLvl w:val="2"/>' },
  Heading4: { name: 'heading 4', label: 'Heading 4', r: '<w:b/><w:i/><w:sz w:val="24"/>', p: '<w:keepNext/><w:spacing w:before="160" w:after="40"/><w:outlineLvl w:val="3"/>' },
  Quote: { name: 'Quote', r: '<w:i/><w:color w:val="404040"/>', p: '<w:spacing w:before="160" w:after="160"/><w:ind w:left="720" w:right="720"/>' },
  Code: { name: 'Code', r: '<w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/>', p: '<w:spacing w:after="0"/>' },
};
const LISTS = {
  bullet: { nsid: '0A1B2C01', fmt: lvl => ['bullet', ['•', '◦', '▪'][lvl % 3]] },
  number: { nsid: '0A1B2C02', fmt: lvl => [['decimal', 'lowerLetter', 'lowerRoman'][lvl % 3], `%${lvl + 1}.`] },
};
const ROMAN = [[1000,'m'],[900,'cm'],[500,'d'],[400,'cd'],[100,'c'],[90,'xc'],[50,'l'],[40,'xl'],[10,'x'],[9,'ix'],[5,'v'],[4,'iv'],[1,'i']];
const roman = n => { let out = ''; for (const [value, text] of ROMAN) while (n >= value) { out += text; n -= value; } return out; };
const letters = n => { let out = ''; while (n > 0) { n--; out = String.fromCharCode(97 + n % 26) + out; n = Math.floor(n / 26); } return out; };
const counterText = (n, fmt) => ({ lowerLetter: letters(n), upperLetter: letters(n).toUpperCase(), lowerRoman: roman(n), upperRoman: roman(n).toUpperCase(), none: '' }[fmt] ?? String(n));
// Word's bullets are usually private-use characters in Symbol or Wingdings.
const bulletGlyph = text => {
  const code = text.codePointAt(0);
  if (code === 0xF0B7 || code === 0xB7) return '•';
  if (code === 0xF0A7 || code === 0xF06E || code === 0xF0FC) return '▪';
  if (text === 'o') return '◦';
  return code >= 0xE000 && code <= 0xF8FF ? '•' : text;
};

const ICONS = {
  undo: '<path d="M4 7h7a3 3 0 0 1 0 6H8"/><path d="M6.5 4.5 4 7l2.5 2.5"/>',
  redo: '<path d="M12 7H5a3 3 0 0 0 0 6h3"/><path d="M9.5 4.5 12 7 9.5 9.5"/>',
  bold: '<path d="M5 3h4a2.5 2.5 0 0 1 0 5H5zM5 8h4.5a2.5 2.5 0 0 1 0 5H5z"/>',
  italic: '<path d="M7 3h5M4 13h5M10 3 6 13"/>',
  underline: '<path d="M5 3v4a3 3 0 0 0 6 0V3M4 13.5h8"/>',
  strike: '<path d="M3 8h10M11 4.5C10.5 3.6 9.4 3 8 3 6.3 3 5 3.9 5 5.2M5 11c.5 1.2 1.6 2 3.1 2 1.9 0 3.1-1 3.1-2.2"/>',
  sup: '<path d="m3 6 5 7M8 6l-5 7M10.5 4.2c.3-.8 2.5-.9 2.5.5 0 1-2.5 2-2.5 3.3H13"/>',
  sub: '<path d="m3 3 5 7M8 3l-5 7M10.5 10.2c.3-.8 2.5-.9 2.5.5 0 1-2.5 2-2.5 3.3H13"/>',
  color: '<path d="M4.5 12 8 3l3.5 9M6 9h4"/>',
  highlight: '<path d="m9.5 3 3.5 3.5-5.5 5.5H4v-3.5z"/><path d="M3 14h10"/>',
  clear: '<path d="M4 3h8M8 3l-2.5 10M3 13h5"/><path d="m10 9 4 4m0-4-4 4"/>',
  painter: '<path d="M3 3h8v3H3zM11 4.5h2v3H7.5v2M6.5 9.5h2V14h-2z"/>',
  left: '<path d="M3 4h10M3 7h6M3 10h10M3 13h6"/>',
  center: '<path d="M3 4h10M5 7h6M3 10h10M5 13h6"/>',
  right: '<path d="M3 4h10M7 7h6M3 10h10M7 13h6"/>',
  justify: '<path d="M3 4h10M3 7h10M3 10h10M3 13h10"/>',
  bullets: '<circle cx="3.5" cy="4.5" r=".9" fill="currentColor"/><circle cx="3.5" cy="8" r=".9" fill="currentColor"/><circle cx="3.5" cy="11.5" r=".9" fill="currentColor"/><path d="M6.5 4.5H13M6.5 8H13M6.5 11.5H13"/>',
  numbers: '<path d="M2.6 3.3 3.6 3v3M2.5 6h2M2.5 9.3c.2-.5.7-.7 1.1-.6.6.1.8.7.4 1.2L2.5 11.8h2M6.5 4.5H13M6.5 8H13M6.5 11.5H13"/>',
  outdent: '<path d="M3 3.5h10M8 6.5h5M8 9.5h5M3 12.5h10M5.5 6 3 8l2.500 2"/>',
  indent: '<path d="M3 3.5h10M8 6.5h5M8 9.5h5M3 12.5h10M3 6l2.500 2L3 10"/>',
  link: '<path d="M6.500 9.500a2.500 2.500 0 0 0 3.500 0l2.500-2.500A2.500 2.500 0 0 0 9 3.500l-.8.8M9.500 6.500a2.500 2.500 0 0 0-3.500 0L3.500 9A2.500 2.500 0 0 0 7 12.500l.8-.8"/>',
  image: '<rect x="2.500" y="3" width="11" height="10" rx="1"/><circle cx="6" cy="6.500" r="1"/><path d="m3 12 3.500-3.500 2 2 2-2L13 11"/>',
  table: '<rect x="2.500" y="3" width="11" height="10" rx="1"/><path d="M2.500 6.500h11M2.500 10h11M6.500 3v10M10 3v10"/>',
  pagebreak: '<path d="M4 2v3.500h8V2M4 14v-3.500h8V14M2 8h2M6 8h4M12 8h2"/>',
  find: '<circle cx="7" cy="7" r="3.800"/><path d="m10 10 3.500 3.500"/>',
  print: '<path d="M4.500 6V2.500h7V6M4.500 11.500H3V6.500h10v5h-1.500"/><path d="M4.500 9.500h7v4h-7z"/>',
  outline: '<path d="M3 4h2M3 8h2M3 12h2M7 4h6M9 8h4M9 12h4"/>',
  page: '<path d="M4 2.500h6l2.500 2.500v8.500H4z"/><path d="M10 2.500V5h2.500"/>',
  save: '<path d="M3 3h8l2 2v8H3z"/><path d="M5 3v3h5V3M5 13V9h6v4"/>',
  rowAbove: '<path d="M2.500 8h11v5h-11zM2.500 10.500h11M8 2.500v4M6 4.500h4"/>',
  rowBelow: '<path d="M2.500 3h11v5h-11zM2.500 5.500h11M8 9.500v4M6 11.500h4"/>',
  colLeft: '<path d="M8 2.500h5v11H8zM10.500 2.500v11M2.500 8h4M4.500 6v4"/>',
  colRight: '<path d="M3 2.500h5v11H3zM5.500 2.500v11M9.500 8h4M11.500 6v4"/>',
  delRow: '<path d="M2.500 5h11v6h-11zM6 6.500l4 3m0-3-4 3"/>',
  delCol: '<path d="M5 2.500h6v11H5zM6.500 6l3 4m0-4-3 4"/>',
  delTable: '<rect x="2.500" y="3" width="11" height="10" rx="1"/><path d="m5.500 5.500 5 5m0-5-5 5"/>',
  shade: '<path d="M2.500 3h11v10h-11z"/><path d="M2.500 3h11v10z" fill="currentColor" stroke="none" opacity=".35"/>',
};
const svg = name => {
  const holder = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const [key, value] of Object.entries({ viewBox: '0 0 16 16', width: '16', height: '16', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.4', 'stroke-linecap': 'round', 'stroke-linejoin': 'round', 'aria-hidden': 'true' })) holder.setAttribute(key, value);
  const parsed = new DOMParser().parseFromString(`<svg xmlns="http://www.w3.org/2000/svg">${ICONS[name]}</svg>`, 'image/svg+xml').documentElement;
  for (const child of [...parsed.children]) holder.append(document.importNode(child, true));
  return holder;
};
const IMAGE_TYPES = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', bmp: 'image/bmp', svg: 'image/svg+xml' };

/** Mount an editor. onSave receives Uint8Array and resolves after persistence. */
export function mountDocxEditor(container, bytes, { readonly = false, onChange = () => {}, onSave } = {}) {
  const model = openDocx(bytes);
  const doc = model.xml;
  let destroyed = false, saving = false, nextId = 1, typingAt = 0, syncTimer = 0;
  let pending = null, painter = null, listCache = null;
  /** XML behind the page: paragraphs and tables by data-p / data-b / data-tc, atoms by data-x. */
  const sources = new Map(), atoms = new Map(), signatures = new Map();
  /** Run formats by data-r: the rPr element, or null for plain text. */
  const runFormats = new Map([['0', null]]), formatKeys = new Map([['', '0']]);
  const imageUrls = new Map();

  const make = name => doc.createElementNS(W, `w:${name}`);
  const fragment = (text, target = doc) => {
    const holder = parseXml(`<x ${WML_ROOT} xmlns:wp="${WP}" xmlns:a="${A}" xmlns:pic="${PIC}">${text}</x>`).documentElement;
    return [...holder.children].map(n => target.importNode(n, true));
  };
  const history = [toXml(doc)];
  let historyIndex = 0, savedXml = history[0];
  const isDirty = () => history[historyIndex] !== savedXml;

  const el = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };
  const root = el('section', 'docx-editor');
  const toolbar = el('div', 'office-toolbar docx-toolbar');
  toolbar.setAttribute('role', 'toolbar'); toolbar.setAttribute('aria-label', 'Document tools');
  const tableBar = el('div', 'office-toolbar docx-table-bar'); tableBar.hidden = true;
  tableBar.setAttribute('role', 'toolbar'); tableBar.setAttribute('aria-label', 'Table tools');
  const findBar = el('div', 'office-toolbar docx-find-bar'); findBar.hidden = true;
  const workspace = el('div', 'docx-workspace');
  const outlinePanel = el('nav', 'docx-outline'); outlinePanel.hidden = true; outlinePanel.setAttribute('aria-label', 'Document outline');
  const scroller = el('div', 'docx-scroller');
  const sheet = el('div', 'docx-sheet');
  const header = el('div', 'docx-margin docx-header'); const footer = el('div', 'docx-margin docx-footer');
  const notes = el('div', 'docx-notes');
  const page = el('div', 'docx-page');
  page.setAttribute('role', 'textbox'); page.setAttribute('aria-multiline', 'true'); page.setAttribute('aria-label', 'Document text');
  page.spellcheck = true;
  const footerBar = el('div', 'docx-footerbar');
  const status = el('span', 'office-status docx-status'); status.setAttribute('role', 'status');
  const counts = el('span', 'office-status docx-counts');
  footerBar.append(counts);
  sheet.append(header, page, notes, footer); scroller.append(sheet); workspace.append(outlinePanel, scroller);
  root.append(toolbar, tableBar, findBar, workspace, footerBar);
  container.replaceChildren(root);

  // ---- Styles ------------------------------------------------------------------------------
  const stylesName = () => model.relationOfType(REL.styles);
  const stylesXml = () => { const name = stylesName(); return name ? model.part(name) : null; };
  const styleNode = id => (id ? [...(stylesXml()?.getElementsByTagNameNS(W, 'style') ?? [])].find(n => attr(n, 'styleId') === id) : undefined);
  function styleChain(id) {
    const chain = []; let at = styleNode(id), guard = 0;
    while (at && guard++ < 20) { chain.unshift(at); at = styleNode(attr(kid(at, 'basedOn'), 'val')); }
    return chain;
  }
  const defaultParaStyle = () => [...(stylesXml()?.getElementsByTagNameNS(W, 'style') ?? [])].find(n => attr(n, 'type') === 'paragraph' && ['1', 'true'].includes(attr(n, 'default')));
  const paraStyleId = pPr => attr(kid(pPr, 'pStyle'), 'val') ?? (defaultParaStyle() ? attr(defaultParaStyle(), 'styleId') : null);
  /** The nearest setting of a run property: run format, its character style, paragraph style, defaults. */
  function inherited(styleId, section, name) {
    for (const style of styleChain(styleId).reverse()) {
      const found = kid(kid(style, section), name);
      if (found) return found;
    }
    const defaults = stylesXml()?.getElementsByTagNameNS(W, 'docDefaults')[0];
    return kid(kid(kid(defaults, section === 'rPr' ? 'rPrDefault' : 'pPrDefault'), section), name);
  }
  function ensureStyle(id) {
    if (styleNode(id) || !BUILT_IN[id]) return;
    const name = model.ensurePart(REL.styles, 'styles.xml', 'styles', 'application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml');
    const xml = model.part(name), spec = BUILT_IN[id];
    const body = `<w:style w:type="paragraph" w:styleId="${id}"><w:name w:val="${spec.name}"/><w:basedOn w:val="${attr(defaultParaStyle(), 'styleId') ?? 'Normal'}"/><w:next w:val="${attr(defaultParaStyle(), 'styleId') ?? 'Normal'}"/><w:qFormat/><w:pPr>${spec.p}</w:pPr><w:rPr>${spec.r}</w:rPr></w:style>`;
    xml.documentElement.append(...fragment(body, xml));
    model.touch(name);
  }
  function styleChoices() {
    const choices = new Map([['', 'Normal']]);
    for (const [id, spec] of Object.entries(BUILT_IN)) choices.set(id, spec.label ?? spec.name);
    for (const style of stylesXml()?.getElementsByTagNameNS(W, 'style') ?? []) {
      if (attr(style, 'type') !== 'paragraph' || ['1', 'true'].includes(attr(style, 'default'))) continue;
      const id = attr(style, 'styleId');
      if (!choices.has(id) && kid(style, 'qFormat')) choices.set(id, attr(kid(style, 'name'), 'val') ?? id);
    }
    return choices;
  }

  // ---- Showing formats ---------------------------------------------------------------------
  const fontOf = rFonts => attr(rFonts, 'ascii') ?? attr(rFonts, 'hAnsi') ?? attr(rFonts, 'cs');
  function showRunProperties(style, rPr) {
    for (const [name, css, value] of [['b', 'fontWeight', ['bold', 'normal']], ['i', 'fontStyle', ['italic', 'normal']]]) {
      const n = kid(rPr, name); if (n) style[css] = value[on(n) ? 0 : 1];
    }
    const deco = [];
    const u = kid(rPr, 'u'); if (u && on(u)) deco.push('underline');
    const s = kid(rPr, 'strike') ?? kid(rPr, 'dstrike'); if (s && on(s)) deco.push('line-through');
    if (deco.length) style.textDecorationLine = deco.join(' ');
    const size = Number(attr(kid(rPr, 'sz'), 'val')); if (size >= 2 && size <= 3276) style.fontSize = `${size / 2}pt`;
    const color = attr(kid(rPr, 'color'), 'val'); if (/^[0-9a-f]{6}$/i.test(color || '')) style.color = `#${color}`;
    const font = fontOf(kid(rPr, 'rFonts')); if (font) style.fontFamily = STACK(font);
    const hl = attr(kid(rPr, 'highlight'), 'val'); if (HIGHLIGHTS[hl]) style.backgroundColor = HIGHLIGHTS[hl];
    const fill = attr(kid(rPr, 'shd'), 'fill'); if (/^[0-9a-f]{6}$/i.test(fill || '')) style.backgroundColor = `#${fill}`;
    const vert = attr(kid(rPr, 'vertAlign'), 'val');
    if (vert === 'superscript' || vert === 'subscript') { style.verticalAlign = vert === 'superscript' ? 'super' : 'sub'; style.fontSize = style.fontSize ? `calc(${style.fontSize} * 0.7)` : '0.7em'; }
    if (kid(rPr, 'caps') && on(kid(rPr, 'caps'))) style.textTransform = 'uppercase';
    if (kid(rPr, 'smallCaps') && on(kid(rPr, 'smallCaps'))) style.fontVariant = 'small-caps';
  }
  const formatKey = rPr => {
    const text = rPr && rPr.children.length ? toXml(rPr) : '';
    let key = formatKeys.get(text);
    if (!key) { key = String(nextId++); formatKeys.set(text, key); runFormats.set(key, rPr.cloneNode(true)); }
    return key;
  };
  function styleSpan(span, key) {
    span.dataset.r = key; span.removeAttribute('style');
    const rPr = runFormats.get(key);
    if (!rPr) return;
    const charStyle = attr(kid(rPr, 'rStyle'), 'val');
    for (const style of styleChain(charStyle)) showRunProperties(span.style, kid(style, 'rPr'));
    showRunProperties(span.style, rPr);
  }
  const twipsPx = value => `${Number(value) / 15}px`;
  function showParagraph(dom, node) {
    dom.removeAttribute('style'); dom.removeAttribute('role'); dom.removeAttribute('aria-level');
    const pPr = kid(node, 'pPr'), styleId = paraStyleId(pPr);
    for (const style of styleChain(styleId)) showRunProperties(dom.style, kid(style, 'rPr'));
    const look = name => kid(pPr, name) ?? inherited(styleId, 'pPr', name);
    const jc = attr(look('jc'), 'val');
    const align = { left: 'left', start: 'left', right: 'right', end: 'right', center: 'center', both: 'justify', distribute: 'justify' }[jc];
    if (align) dom.style.textAlign = align;
    const spacing = look('spacing');
    if (spacing) {
      if (attr(spacing, 'before') != null) dom.style.marginTop = twipsPx(attr(spacing, 'before'));
      if (attr(spacing, 'after') != null) dom.style.marginBottom = twipsPx(attr(spacing, 'after'));
      const line = Number(attr(spacing, 'line'));
      if (line > 0) dom.style.lineHeight = ['exact', 'atLeast'].includes(attr(spacing, 'lineRule')) ? twipsPx(line) : String(line / 240 * 1.2);
    }
    const number = listOf(pPr, styleId);
    let ind = look('ind');
    if (number && !kid(pPr, 'ind')) ind = number.ind ?? ind;
    const left = Number(attr(ind, 'left') ?? attr(ind, 'start') ?? 0), right = Number(attr(ind, 'right') ?? attr(ind, 'end') ?? 0);
    const hanging = Number(attr(ind, 'hanging') ?? 0), first = Number(attr(ind, 'firstLine') ?? 0);
    if (left) dom.style.marginLeft = twipsPx(left);
    if (right) dom.style.marginRight = twipsPx(right);
    if (first) dom.style.textIndent = twipsPx(first);
    dom.style.setProperty('--docx-hang', number ? `${Math.max(hanging, 240) / 15}px` : '0px');
    const fill = attr(look('shd'), 'fill'); if (/^[0-9a-f]{6}$/i.test(fill || '')) dom.style.backgroundColor = `#${fill}`;
    if (on(kid(pPr, 'pageBreakBefore'))) dom.classList.add('docx-break-before'); else dom.classList.remove('docx-break-before');
    const outline = attr(look('outlineLvl'), 'val');
    if (outline != null && Number(outline) < 6) { dom.setAttribute('role', 'heading'); dom.setAttribute('aria-level', String(Number(outline) + 1)); }
  }

  // ---- Numbered and bulleted lists ----------------------------------------------------------
  const numberingName = () => model.relationOfType(REL.numbering);
  function numberingIndex() {
    if (listCache) return listCache;
    const index = { nums: new Map(), abstracts: new Map() }, xml = numberingName() && model.part(numberingName());
    for (const n of xml?.getElementsByTagNameNS(W, 'abstractNum') ?? []) index.abstracts.set(attr(n, 'abstractNumId'), n);
    for (const n of xml?.getElementsByTagNameNS(W, 'num') ?? []) index.nums.set(attr(n, 'numId'), n);
    return (listCache = index);
  }
  function listOf(pPr, styleId) {
    let numPr = kid(pPr, 'numPr');
    for (const style of styleChain(styleId).reverse()) { if (numPr) break; numPr = kid(kid(style, 'pPr'), 'numPr'); }
    const numId = attr(kid(numPr, 'numId'), 'val');
    if (!numId || numId === '0') return null;
    const index = numberingIndex(), num = index.nums.get(numId);
    const abstractId = attr(kid(num, 'abstractNumId'), 'val'), abstract = index.abstracts.get(abstractId);
    if (!abstract) return null;
    const ilvl = Number(attr(kid(numPr, 'ilvl'), 'val') ?? 0);
    const lvl = kids(abstract, 'lvl').find(n => Number(attr(n, 'ilvl')) === ilvl);
    if (!lvl) return null;
    return { numId, abstractId, ilvl, abstract, fmt: attr(kid(lvl, 'numFmt'), 'val') ?? 'decimal', text: attr(kid(lvl, 'lvlText'), 'val') ?? '', start: Number(attr(kid(lvl, 'start'), 'val') ?? 1), ind: kid(kid(lvl, 'pPr'), 'ind') };
  }
  /** Works out every list marker, in document order. Called after anything that may change one. */
  function renumber() {
    const counters = new Map();
    for (const dom of page.querySelectorAll('p[data-p]')) {
      const node = sources.get(dom.dataset.p);
      const info = node && listOf(kid(node, 'pPr'), paraStyleId(kid(node, 'pPr')));
      if (!info) { delete dom.dataset.marker; continue; }
      let levels = counters.get(info.abstractId);
      if (!levels) counters.set(info.abstractId, levels = []);
      levels[info.ilvl] = (levels[info.ilvl] ?? info.start - 1) + 1;
      levels.length = info.ilvl + 1;
      if (info.fmt === 'bullet') { dom.dataset.marker = bulletGlyph(info.text || '•'); continue; }
      dom.dataset.marker = info.text.replace(/%(\d)/g, (_, digit) => {
        const level = Number(digit) - 1, at = kids(info.abstract, 'lvl').find(n => Number(attr(n, 'ilvl')) === level);
        return counterText(levels[level] ?? Number(attr(kid(at, 'start'), 'val') ?? 1), attr(kid(at, 'numFmt'), 'val') ?? 'decimal');
      });
    }
  }
  function listId(kind) {
    const name = model.ensurePart(REL.numbering, 'numbering.xml', 'numbering', 'application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml');
    const xml = model.part(name), root = xml.documentElement, spec = LISTS[kind];
    listCache = null;
    const abstracts = [...root.getElementsByTagNameNS(W, 'abstractNum')];
    let abstract = abstracts.find(n => attr(kid(n, 'nsid'), 'val') === spec.nsid);
    if (abstract) {
      const id = attr(abstract, 'abstractNumId');
      const num = [...root.getElementsByTagNameNS(W, 'num')].find(n => attr(kid(n, 'abstractNumId'), 'val') === id);
      if (num) return attr(num, 'numId');
    }
    const free = (list, key) => Math.max(0, ...list.map(n => Number(attr(n, key)) || 0)) + 1;
    const abstractId = free(abstracts, 'abstractNumId');
    const levels = Array.from({ length: 9 }, (_, lvl) => {
      const [fmt, text] = spec.fmt(lvl);
      return `<w:lvl w:ilvl="${lvl}"><w:start w:val="1"/><w:numFmt w:val="${fmt}"/><w:lvlText w:val="${text}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="${720 * (lvl + 1)}" w:hanging="360"/></w:pPr></w:lvl>`;
    }).join('');
    [abstract] = fragment(`<w:abstractNum w:abstractNumId="${abstractId}"><w:nsid w:val="${spec.nsid}"/><w:multiLevelType w:val="hybridMultilevel"/>${levels}</w:abstractNum>`, xml);
    const firstNum = root.getElementsByTagNameNS(W, 'num')[0];
    root.insertBefore(abstract, firstNum ?? null);
    const numId = free([...root.getElementsByTagNameNS(W, 'num')], 'numId');
    const [num] = fragment(`<w:num w:numId="${numId}"><w:abstractNumId w:val="${abstractId}"/></w:num>`, xml);
    const after = [...root.children].find(n => !['abstractNum', 'num', 'numPicBullet'].includes(n.localName));
    root.insertBefore(num, after ?? null);
    model.touch(name); listCache = null;
    return String(numId);
  }

  // ---- Page from XML ------------------------------------------------------------------------
  const register = (map, node) => { const id = String(nextId++); map.set(id, node); return id; };
  function makeAtom(nodes, visual) {
    visual.dataset.x = register(atoms, nodes);
    visual.setAttribute('contenteditable', 'false');
    return visual;
  }
  const textOf = node => [...node.getElementsByTagNameNS(W, 't'), ...node.getElementsByTagNameNS(W, 'delText')].sort((a, b) => (a.compareDocumentPosition(b) & 4 ? -1 : 1)).map(n => n.textContent).join('');
  function pictureUrl(id) {
    const rel = model.relation(id);
    if (!rel || rel.external) return null;
    const part = resolvePart(rel.target), mime = IMAGE_TYPES[part.split('.').pop().toLowerCase()];
    if (!mime || !model.parts[part]) return null;
    if (!imageUrls.has(part)) imageUrls.set(part, URL.createObjectURL(new Blob([model.parts[part]], { type: mime })));
    return imageUrls.get(part);
  }
  function drawingVisual(node) {
    const blip = node.getElementsByTagNameNS(A, 'blip')[0];
    const url = blip && pictureUrl(blip.getAttributeNS(R, 'embed'));
    if (!url) return el('span', 'docx-object', 'Picture or object (kept in the file)');
    const image = el('img', 'docx-image'); image.src = url;
    const extent = node.getElementsByTagNameNS(WP, 'extent')[0];
    if (extent) { image.style.width = `${Number(extent.getAttribute('cx')) / 9525}px`; }
    image.alt = node.getElementsByTagNameNS(WP, 'docPr')[0]?.getAttribute('descr') || 'Picture';
    return image;
  }
  /** One run child as its own run, so an atom can stand between pieces of text. */
  function runWith(run, child) {
    const copy = make('r'), rPr = kid(run, 'rPr');
    if (rPr) copy.append(rPr.cloneNode(true));
    copy.append(child.cloneNode(true));
    return copy;
  }
  function renderRun(run, target) {
    const key = formatKey(kid(run, 'rPr'));
    let span = null;
    const text = value => {
      if (!span) { span = el('span', 'docx-run'); styleSpan(span, key); target.append(span); }
      span.append(document.createTextNode(value));
    };
    for (const child of run.children) {
      if (child.namespaceURI !== W) { target.append(makeAtom([runWith(run, child)], el('span', 'docx-object', 'Object'))); span = null; continue; }
      switch (child.localName) {
        case 'rPr': break;
        case 't': text(child.textContent); break;
        case 'tab': text('\t'); break;
        case 'noBreakHyphen': text('‑'); break;
        case 'cr': case 'br':
          if (attr(child, 'type') === 'page' || attr(child, 'type') === 'column') {
            target.append(makeAtom([runWith(run, child)], el('span', 'docx-pagebreak', attr(child, 'type') === 'page' ? 'Page break' : 'Column break')));
          } else {
            const br = el('br'); br.dataset.lb = key; target.append(br);
          }
          span = null; break;
        case 'drawing': case 'pict': case 'object':
          target.append(makeAtom([runWith(run, child)], drawingVisual(child))); span = null; break;
        case 'footnoteReference': case 'endnoteReference': {
          const mark = el('sup', 'docx-noteref', attr(child, 'id'));
          target.append(makeAtom([runWith(run, child)], mark)); span = null; break;
        }
        case 'commentReference': {
          const mark = el('span', 'docx-comment-mark', '※'); mark.title = commentText(attr(child, 'id'));
          target.append(makeAtom([runWith(run, child)], mark)); span = null; break;
        }
        default: {
          const hidden = el('span'); hidden.hidden = true;
          target.append(makeAtom([runWith(run, child)], hidden)); span = null;
        }
      }
    }
  }
  function commentText(id) {
    const name = model.relationOfType(REL.comments), xml = name && model.part(name);
    const comment = [...(xml?.getElementsByTagNameNS(W, 'comment') ?? [])].find(n => attr(n, 'id') === id);
    return comment ? `${attr(comment, 'author') ?? 'Comment'}: ${textOf(comment)}` : 'Comment';
  }
  const fieldChar = (node, kind) => node.localName === 'r' && kids(node, 'fldChar').some(n => attr(n, 'fldCharType') === kind);
  function renderInline(items, target) {
    for (let at = 0; at < items.length; at++) {
      const node = items[at];
      if (node.namespaceURI !== W) { const hidden = el('span'); hidden.hidden = true; target.append(makeAtom([node], hidden)); continue; }
      const name = node.localName;
      if (name === 'pPr') continue;
      if (fieldChar(node, 'begin')) {
        // A field (page number, cross-reference…) stays whole: Word works out its text.
        let depth = 0, end = at;
        for (; end < items.length; end++) {
          if (fieldChar(items[end], 'begin')) depth++;
          if (fieldChar(items[end], 'end') && --depth === 0) break;
        }
        const nodes = items.slice(at, end + 1);
        const visual = el('span', 'docx-field'); let inResult = false;
        for (const part of nodes) {
          if (fieldChar(part, 'separate')) inResult = true; else if (inResult && part.localName === 'r') visual.append(document.createTextNode(textOf(part)));
        }
        if (!visual.textContent) visual.textContent = 'Field';
        target.append(makeAtom(nodes, visual)); at = end;
      } else if (name === 'r') renderRun(node, target);
      else if (name === 'hyperlink') {
        const link = el('a', 'docx-link');
        link.dataset.link = register(sources, node);
        const rel = model.relation(node.getAttributeNS(R, 'id'));
        link.title = rel?.target ?? (attr(node, 'anchor') ? `#${attr(node, 'anchor')}` : '');
        renderInline([...node.children], link); target.append(link);
      } else if (['ins', 'moveTo', 'del', 'moveFrom'].includes(name)) {
        const removed = name === 'del' || name === 'moveFrom';
        const visual = el(removed ? 'del' : 'ins', removed ? 'docx-removed' : 'docx-added', textOf(node));
        visual.title = `${removed ? 'Removed' : 'Added'} by ${attr(node, 'author') ?? 'someone'}`;
        target.append(makeAtom([node], visual));
      } else if (['fldSimple', 'sdt', 'smartTag', 'customXml'].includes(name)) {
        target.append(makeAtom([node], el('span', 'docx-field', textOf(node) || 'Field')));
      } else {
        const hidden = el('span'); hidden.hidden = true;
        target.append(makeAtom([node], hidden));
      }
    }
  }
  function renderParagraph(node, target) {
    const dom = el('p');
    dom.dataset.p = register(sources, node);
    showParagraph(dom, node);
    renderInline([...node.children], dom);
    if (!dom.childNodes.length || dom.lastChild.nodeName === 'BR') dom.append(el('br'));
    target.append(dom);
    signatures.set(dom.dataset.p, dom.innerHTML);
  }
  function renderTable(node, target) {
    const table = el('table');
    table.dataset.b = register(sources, node);
    const tblPr = kid(node, 'tblPr'), width = attr(kid(tblPr, 'tblW'), 'w');
    if (attr(kid(tblPr, 'tblW'), 'type') === 'dxa' && Number(width) > 0) table.style.width = twipsPx(width);
    const grid = kids(kid(node, 'tblGrid') ?? make('x'), 'gridCol');
    if (grid.length) {
      const cols = el('colgroup');
      for (const col of grid) { const c = el('col'); if (Number(attr(col, 'w')) > 0) c.style.width = twipsPx(attr(col, 'w')); cols.append(c); }
      table.append(cols);
    }
    const rows = kids(node, 'tr');
    rows.forEach((tr, rowIndex) => {
      const row = el('tr'); row.dataset.tr = register(sources, tr);
      let column = 0;
      for (const tc of kids(tr, 'tc')) {
        const tcPr = kid(tc, 'tcPr'), span = Number(attr(kid(tcPr, 'gridSpan'), 'val') ?? 1), merge = kid(tcPr, 'vMerge');
        if (merge && attr(merge, 'val') !== 'restart') { column += span; continue; }
        const cell = el('td'); cell.dataset.tc = register(sources, tc);
        if (span > 1) cell.colSpan = span;
        if (merge) {
          let down = 1;
          for (let next = rowIndex + 1; next < rows.length; next++) {
            let col = 0, found = null;
            for (const other of kids(rows[next], 'tc')) {
              if (col === column) { found = other; break; }
              col += Number(attr(kid(kid(other, 'tcPr'), 'gridSpan'), 'val') ?? 1);
            }
            const m = kid(kid(found, 'tcPr'), 'vMerge');
            if (m && attr(m, 'val') !== 'restart') down++; else break;
          }
          cell.rowSpan = down;
        }
        const fill = attr(kid(tcPr, 'shd'), 'fill'); if (/^[0-9a-f]{6}$/i.test(fill || '')) cell.style.backgroundColor = `#${fill}`;
        renderBlocks([...tc.children], cell);
        row.append(cell); column += span;
      }
      table.append(row);
    });
    target.append(table);
  }
  function renderBlocks(items, target) {
    let fieldDepth = 0;
    for (const node of items) {
      if (node.namespaceURI !== W) { const hidden = el('div'); hidden.hidden = true; target.append(makeAtom([node], hidden)); continue; }
      const name = node.localName;
      if (['sectPr', 'tcPr'].includes(name)) continue;
      if (name === 'p') {
        // A field running across paragraphs (a table of contents) can't be edited piece by piece.
        const marks = [...node.getElementsByTagNameNS(W, 'fldChar')].map(n => attr(n, 'fldCharType'));
        const before = fieldDepth;
        for (const mark of marks) fieldDepth += mark === 'begin' ? 1 : mark === 'end' ? -1 : 0;
        if (before > 0 || fieldDepth > 0) target.append(makeAtom([node], el('div', 'docx-locked', textOf(node) || ' ')));
        else renderParagraph(node, target);
      } else if (name === 'tbl') renderTable(node, target);
      else if (name === 'sdt') target.append(makeAtom([node], el('div', 'docx-locked', textOf(node) || 'Content control')));
      else { const hidden = el('div'); hidden.hidden = true; target.append(makeAtom([node], hidden)); }
    }
    if (target !== page && !target.querySelector(':scope > p')) {
      const p = make('p'); renderParagraph(p, target);
    }
  }
  function showPageGeometry() {
    const sectPr = kid(model.body, 'sectPr'), size = kid(sectPr, 'pgSz'), margins = kid(sectPr, 'pgMar');
    const width = Number(attr(size, 'w')) || 11906, height = Number(attr(size, 'h')) || 16838;
    sheet.style.width = `${width / 15}px`; sheet.style.minHeight = `${height / 15}px`;
    const side = (name, fallback) => `${(Number(attr(margins, name)) || fallback) / 15}px`;
    sheet.style.paddingTop = side('top', 1440); sheet.style.paddingBottom = side('bottom', 1440);
    sheet.style.paddingLeft = side('left', 1440); sheet.style.paddingRight = side('right', 1440);
    page.removeAttribute('style');
    const font = fontOf(inherited(null, 'rPr', 'rFonts')); if (font) page.style.fontFamily = STACK(font);
    const fontSize = Number(attr(inherited(null, 'rPr', 'sz'), 'val')); if (fontSize) page.style.fontSize = `${fontSize / 2}pt`;
  }
  const partText = (type, reference) => {
    const ref = kid(kid(model.body, 'sectPr'), reference);
    const rel = ref && model.relation(ref.getAttributeNS(R, 'id'));
    if (!rel) return '';
    const xml = model.part(resolvePart(rel.target));
    return xml ? [...xml.getElementsByTagNameNS(W, 'p')].map(textOf).join('\n').trim() : '';
  };
  function showMargins() {
    header.textContent = partText(REL.header, 'headerReference'); header.hidden = !header.textContent;
    footer.textContent = partText(REL.footer, 'footerReference'); footer.hidden = !footer.textContent;
    notes.replaceChildren();
    const name = model.relationOfType(REL.footnotes), xml = name && model.part(name);
    const shown = new Set([...model.body.getElementsByTagNameNS(W, 'footnoteReference')].map(n => attr(n, 'id')));
    for (const note of xml?.getElementsByTagNameNS(W, 'footnote') ?? []) {
      if (shown.has(attr(note, 'id'))) { const line = el('p', 'docx-note', `${attr(note, 'id')}. ${textOf(note).trim()}`); notes.append(line); }
    }
    notes.hidden = !notes.children.length;
  }
  function render() {
    sources.clear(); atoms.clear(); signatures.clear();
    page.replaceChildren();
    showPageGeometry();
    renderBlocks([...model.body.children], page);
    if (!page.querySelector('p[data-p]')) {
      const p = make('p'); model.body.insertBefore(p, kid(model.body, 'sectPr') ?? null); renderParagraph(p, page);
    }
    showMargins(); renumber(); showOutline(); showCounts(); updateTools();
  }

  // ---- XML from page ------------------------------------------------------------------------
  function buildInline(dom, target) {
    let run = null, runKey = null;
    const startRun = key => {
      if (run && runKey === key) return run;
      run = make('r'); runKey = key;
      const rPr = runFormats.get(key); if (rPr) run.append(doc.importNode(rPr, true));
      target.append(run);
      return run;
    };
    const addText = (value, key) => {
      if (!value) return;
      for (const piece of value.split(/(\t|‑)/)) {
        if (!piece) continue;
        const r = startRun(key);
        if (piece === '\t') r.append(make('tab'));
        else if (piece === '‑') r.append(make('noBreakHyphen'));
        else {
          const last = r.lastElementChild;
          if (last?.localName === 't') last.textContent += piece;
          else { const t = make('t'); t.setAttributeNS(XML_NS, 'xml:space', 'preserve'); t.textContent = piece; r.append(t); }
        }
      }
    };
    const walk = (node, key) => {
      for (const child of node.childNodes) {
        if (child.nodeType === 3) addText(child.nodeValue.replace(/​/g, ''), key);
        else if (child.nodeType !== 1) continue;
        else if (child.dataset.x) { run = null; for (const part of atoms.get(child.dataset.x) ?? []) target.append(part.cloneNode(true)); }
        else if (child.nodeName === 'BR') { if (child.dataset.lb !== undefined) startRun(child.dataset.lb || key).append(make('br')); }
        else if (child.dataset.link) {
          const original = sources.get(child.dataset.link), link = original.cloneNode(false);
          run = null; target.append(link);
          buildInline(child, link);
        } else walk(child, child.dataset.r ?? key);
      }
    };
    walk(dom, '0');
  }
  function syncParagraph(dom) {
    const node = sources.get(dom.dataset.p), signature = dom.innerHTML;
    if (signatures.get(dom.dataset.p) === signature) return node;
    for (const child of [...node.children]) if (child.localName !== 'pPr' || child.namespaceURI !== W) child.remove();
    buildInline(dom, node);
    signatures.set(dom.dataset.p, signature);
    return node;
  }
  function newParagraphFor(dom, like) {
    const node = make('p'), pPr = like && kid(like, 'pPr');
    if (pPr) { const copy = pPr.cloneNode(true); kid(copy, 'sectPr')?.remove(); node.append(copy); }
    dom.dataset.p = register(sources, node);
    signatures.delete(dom.dataset.p);
    return node;
  }
  function syncContainer(dom, parent) {
    const list = [], seen = new Set();
    // A browser can leave bare text or an element outside any paragraph: it gets one.
    for (const stray of [...dom.childNodes]) {
      const block = stray.nodeType === 1 && (stray.dataset.p || stray.dataset.b || stray.dataset.x || stray.nodeName === 'COLGROUP');
      if (block || (stray.nodeType === 3 && !stray.nodeValue.trim())) continue;
      const wrapper = el('p'); stray.replaceWith(wrapper); wrapper.append(stray);
    }
    for (const child of dom.children) {
      if (child.dataset.p || child.nodeName === 'P') {
        const id = child.dataset.p;
        if (!id || seen.has(id) || !sources.has(id)) newParagraphFor(child, id ? sources.get(id) : null);
        seen.add(child.dataset.p);
        list.push(syncParagraph(child));
      } else if (child.dataset.b) {
        for (const cell of child.querySelectorAll(':scope > tr > td')) syncContainer(cell, sources.get(cell.dataset.tc));
        list.push(sources.get(child.dataset.b));
      } else if (child.dataset.x) list.push(...(atoms.get(child.dataset.x) ?? []));
    }
    const fixed = n => n.namespaceURI === W && ['sectPr', 'tcPr'].includes(n.localName);
    const current = [...parent.children].filter(n => !fixed(n));
    if (current.length === list.length && current.every((n, i) => n === list[i])) return;
    for (const n of current) n.remove();
    const before = [...parent.children].find(n => n.localName === 'sectPr') ?? null;
    for (const n of list) parent.insertBefore(n, before);
  }
  function syncDom() {
    clearTimeout(syncTimer); syncTimer = 0;
    syncContainer(page, model.body);
  }

  // ---- Selection helpers --------------------------------------------------------------------
  const selection = () => window.getSelection();
  function selectionRange() {
    const s = selection();
    if (!s.rangeCount) return null;
    const range = s.getRangeAt(0);
    return page.contains(range.commonAncestorContainer) ? range : null;
  }
  const blockOf = node => (node?.nodeType === 1 ? node : node?.parentElement)?.closest('p[data-p]');
  const cellOf = node => (node?.nodeType === 1 ? node : node?.parentElement)?.closest('td[data-tc]');
  const editable = node => !(node.nodeType === 1 ? node : node.parentElement)?.closest('[data-x]');
  function textNodes(scope) {
    const out = [], walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) if (editable(walker.currentNode)) out.push(walker.currentNode);
    return out;
  }
  const selectedBlocks = range => {
    const blocks = [...page.querySelectorAll('p[data-p]')].filter(block => range.intersectsNode(block));
    return blocks.length ? blocks : [blockOf(range.startContainer)].filter(Boolean);
  };
  const setCaret = (node, offset) => {
    const range = document.createRange(); range.setStart(node, offset); range.collapse(true);
    const s = selection(); s.removeAllRanges(); s.addRange(range);
  };
  function caretAtBlock(block, offset) {
    let left = offset;
    for (const node of textNodes(block)) {
      if (left <= node.nodeValue.length) return setCaret(node, left);
      left -= node.nodeValue.length;
    }
    const breakAt = block.querySelector(':scope > br:last-child');
    if (breakAt) setCaret(block, [...block.childNodes].indexOf(breakAt)); else setCaret(block, block.childNodes.length);
  }
  const offsetInBlock = (block, node, offset) => {
    const range = document.createRange(); range.selectNodeContents(block); range.setEnd(node, offset);
    return range.toString().length;
  };
  function caretPlace() {
    const range = selectionRange(), block = range && blockOf(range.startContainer);
    if (!block) return null;
    return { index: [...page.querySelectorAll('p[data-p]')].indexOf(block), offset: offsetInBlock(block, range.startContainer, range.startOffset) };
  }
  function restorePlace(place) {
    if (!place) return;
    const blocks = page.querySelectorAll('p[data-p]'), block = blocks[Math.min(place.index, blocks.length - 1)];
    if (block) { page.focus({ preventScroll: true }); caretAtBlock(block, place.offset); }
  }
  /** Splits text so the range covers whole text nodes, each alone in its own run span. */
  function isolateSelected(range) {
    const { startContainer: sc, startOffset: so, endContainer: ec, endOffset: eo } = range;
    if (ec.nodeType === 3 && eo < ec.nodeValue.length) ec.splitText(eo);
    let first = sc;
    if (sc.nodeType === 3 && so > 0) first = sc.splitText(so);
    const end = ec === sc && sc.nodeType === 3 ? first : ec;
    const whole = document.createRange();
    if (first.nodeType === 3) whole.setStartBefore(first); else whole.setStart(first, so);
    if (end.nodeType === 3) whole.setEndAfter(end); else whole.setEnd(end, eo);
    const picked = textNodes(whole.commonAncestorContainer.nodeType === 3 ? whole.commonAncestorContainer.parentNode : whole.commonAncestorContainer)
      .filter(node => node.nodeValue && whole.intersectsNode(node) && (node === first || node === end || (whole.comparePoint(node, 0) === 0 && whole.comparePoint(node, node.nodeValue.length) === 0)));
    return picked.map(alone);
  }
  function alone(textNode) {
    let span = textNode.parentElement;
    if (!span.matches('span[data-r]')) {
      const wrap = el('span', 'docx-run'); styleSpan(wrap, '0');
      textNode.replaceWith(wrap); wrap.append(textNode);
      return wrap;
    }
    const nodes = [...span.childNodes], at = nodes.indexOf(textNode);
    if (at < nodes.length - 1) {
      const tail = span.cloneNode(false); tail.append(...nodes.slice(at + 1)); span.after(tail);
    }
    if (at > 0) {
      const mine = span.cloneNode(false); mine.append(textNode); span.after(mine); span = mine;
    }
    return span;
  }
  function selectSpans(spans) {
    if (!spans.length) return;
    const range = document.createRange();
    range.setStart(spans[0].firstChild, 0);
    const last = spans[spans.length - 1].lastChild;
    range.setEnd(last, last.nodeValue.length);
    const s = selection(); s.removeAllRanges(); s.addRange(range);
  }

  // ---- Changes and history ------------------------------------------------------------------
  function changed({ typing = false } = {}) {
    syncDom();
    const xml = toXml(doc);
    if (xml === history[historyIndex]) { updateTools(); return; }
    const now = Date.now(), coalesce = typing && now - typingAt < 800 && historyIndex === history.length - 1 && historyIndex > 0 && history[historyIndex] !== savedXml;
    history.splice(historyIndex + 1);
    if (coalesce) history[historyIndex] = xml; else { history.push(xml); historyIndex++; }
    typingAt = typing ? now : 0;
    let size = history.reduce((sum, entry) => sum + entry.length * 2, 0);
    while (history.length > 2 && (history.length > 50 || size > 20 * 1024 * 1024)) { size -= history.shift().length * 2; historyIndex--; }
    onChange(isDirty()); status.textContent = isDirty() ? 'Unsaved changes' : 'Saved';
    renumber(); showOutline(); showCounts(); updateTools();
  }
  function afterTyping() {
    onChange(true); status.textContent = 'Unsaved changes';
    clearTimeout(syncTimer);
    syncTimer = setTimeout(() => { if (!destroyed && !composing) changed({ typing: true }); }, 180);
    if (saveButton) saveButton.disabled = saving || !onSave;
  }
  function restore(index) {
    if (readonly || saving || index < 0 || index >= history.length) return;
    flush();
    if (index >= history.length) return;
    const place = caretPlace();
    const xml = parseXml(history[index]);
    doc.replaceChild(doc.importNode(xml.documentElement, true), doc.documentElement);
    model.body = doc.getElementsByTagNameNS(W, 'body')[0];
    historyIndex = index; typingAt = 0; pending = null;
    onChange(isDirty()); status.textContent = isDirty() ? 'Unsaved changes' : 'Saved';
    render(); restorePlace(place);
  }
  /** Anything typed but not yet recorded goes into the history now. */
  const flush = () => { if (syncTimer) changed({ typing: true }); };

  // ---- Character formatting -----------------------------------------------------------------
  function setProperty(rPr, name, value, attributes) {
    let n = kid(rPr, name);
    if (value === null) { n?.remove(); return; }
    if (!n) {
      n = make(name);
      const before = [...rPr.children].find(child => RUN_ORDER.indexOf(child.localName) > RUN_ORDER.indexOf(name));
      rPr.insertBefore(n, before ?? null);
    }
    for (const a of [...n.attributes]) n.removeAttributeNode(a);
    if (value !== undefined && value !== true) n.setAttributeNS(W, 'w:val', value);
    for (const [key, item] of Object.entries(attributes ?? {})) n.setAttributeNS(W, `w:${key}`, item);
  }
  function currentRunKey() {
    if (pending) return pending;
    const range = selectionRange();
    if (!range) return '0';
    let node = range.startContainer;
    if (node.nodeType === 1 && range.startOffset > 0) node = node.childNodes[range.startOffset - 1] ?? node;
    const element = node.nodeType === 1 ? node : node.parentElement;
    const span = element.closest('span[data-r]') ?? element.querySelector?.('span[data-r]');
    if (span) return span.dataset.r;
    const block = blockOf(node), inside = block && [...block.querySelectorAll('span[data-r]')].pop();
    return inside?.dataset.r ?? '0';
  }
  function runValue(key, name) {
    const rPr = runFormats.get(key), direct = kid(rPr, name);
    if (direct) return direct;
    const charStyle = attr(kid(rPr, 'rStyle'), 'val');
    for (const style of styleChain(charStyle).reverse()) { const n = kid(kid(style, 'rPr'), name); if (n) return n; }
    const block = blockOf(selectionRange()?.startContainer), node = block && sources.get(block.dataset.p);
    return inherited(node ? paraStyleId(kid(node, 'pPr')) : null, 'rPr', name);
  }
  /** Changes the format of the selection, or of what is typed next if nothing is selected. */
  function applyFormat(change) {
    if (readonly || saving) return;
    const range = selectionRange();
    if (!range) return;
    const edit = key => {
      const rPr = runFormats.get(key)?.cloneNode(true) ?? make('rPr');
      change(rPr);
      return formatKey(rPr);
    };
    if (range.collapsed) { pending = edit(currentRunKey()); page.focus({ preventScroll: true }); updateTools(); return; }
    flush();
    const spans = isolateSelected(range);
    for (const span of spans) styleSpan(span, edit(span.dataset.r));
    selectSpans(spans);
    changed();
  }
  function toggle(name, onValue, offValue) {
    const active = name === 'u' ? on(runValue(currentRunKey(), 'u')) : on(runValue(currentRunKey(), name));
    applyFormat(rPr => {
      if (!active) setProperty(rPr, name, onValue ?? true);
      else if (offValue !== undefined) setProperty(rPr, name, offValue);
      else setProperty(rPr, name, null);
      // A style may switch it on: off has to be said out loud then.
      if (active && name !== 'u' && !kid(rPr, name)) {
        const block = blockOf(selectionRange()?.startContainer), node = block && sources.get(block.dataset.p);
        if (on(inherited(node ? paraStyleId(kid(node, 'pPr')) : null, 'rPr', name))) setProperty(rPr, name, '0');
      }
    });
  }
  function toggleVert(value) {
    const active = attr(kid(runFormats.get(currentRunKey()), 'vertAlign'), 'val') === value;
    applyFormat(rPr => setProperty(rPr, 'vertAlign', active ? null : value));
  }
  const clearFormat = () => applyFormat(rPr => { for (const child of [...rPr.children]) if (child.localName !== 'lang') child.remove(); });

  // ---- Paragraph formatting -----------------------------------------------------------------
  function paragraphProperty(pPr, name) {
    let n = kid(pPr, name);
    if (!n) {
      n = make(name);
      const before = [...pPr.children].find(child => PARA_ORDER.indexOf(child.localName) > PARA_ORDER.indexOf(name));
      pPr.insertBefore(n, before ?? null);
    }
    return n;
  }
  const propertiesOf = node => { let pPr = kid(node, 'pPr'); if (!pPr) { pPr = make('pPr'); node.prepend(pPr); } return pPr; };
  function eachParagraph(change) {
    if (readonly || saving) return;
    const range = selectionRange();
    if (!range) return;
    flush();
    const place = saveRange(range);
    for (const block of selectedBlocks(range)) {
      const node = sources.get(block.dataset.p);
      change(propertiesOf(node), node, block);
      if (!kid(node, 'pPr')?.children.length) kid(node, 'pPr')?.remove();
      showParagraph(block, node);
    }
    changed(); renumber(); restoreRange(place);
  }
  const saveRange = range => {
    const startBlock = blockOf(range.startContainer), endBlock = blockOf(range.endContainer);
    return { startBlock, endBlock, start: startBlock && offsetInBlock(startBlock, range.startContainer, range.startOffset), end: endBlock && offsetInBlock(endBlock, range.endContainer, range.endOffset), collapsed: range.collapsed };
  };
  function restoreRange(place) {
    if (!place.startBlock?.isConnected) return;
    page.focus({ preventScroll: true });
    caretAtBlock(place.startBlock, place.start);
    if (place.collapsed || !place.endBlock?.isConnected) return;
    const anchor = selection().getRangeAt(0).cloneRange();
    caretAtBlock(place.endBlock, place.end);
    const tail = selection().getRangeAt(0);
    anchor.setEnd(tail.endContainer, tail.endOffset);
    selection().removeAllRanges(); selection().addRange(anchor);
  }
  const align = value => eachParagraph(pPr => { paragraphProperty(pPr, 'jc').setAttributeNS(W, 'w:val', value); });
  function setParagraphStyle(id) {
    if (id) ensureStyle(id);
    eachParagraph(pPr => {
      kid(pPr, 'outlineLvl')?.remove();
      if (id) paragraphProperty(pPr, 'pStyle').setAttributeNS(W, 'w:val', id); else kid(pPr, 'pStyle')?.remove();
    });
  }
  function setLineSpacing(value) {
    if (!value) return;
    eachParagraph(pPr => {
      const spacing = paragraphProperty(pPr, 'spacing');
      spacing.setAttributeNS(W, 'w:line', String(Math.round(Number(value) * 240))); spacing.setAttributeNS(W, 'w:lineRule', 'auto');
    });
  }
  function changeIndent(direction) {
    eachParagraph(pPr => {
      const info = listOf(pPr, paraStyleId(pPr));
      if (info && kid(pPr, 'numPr')) {
        const level = Math.max(0, Math.min(8, info.ilvl + direction));
        paragraphProperty(kid(pPr, 'numPr'), 'ilvl');
        kid(kid(pPr, 'numPr'), 'ilvl').setAttributeNS(W, 'w:val', String(level));
        return;
      }
      const ind = kid(pPr, 'ind'), left = Math.max(0, Number(attr(ind, 'left') ?? 0) + direction * 720);
      if (!left) { ind?.remove(); return; }
      paragraphProperty(pPr, 'ind').setAttributeNS(W, 'w:left', String(left));
    });
  }
  function toggleList(kind) {
    const range = selectionRange();
    if (!range) return;
    const first = selectedBlocks(range)[0], firstNode = first && sources.get(first.dataset.p);
    const info = firstNode && listOf(kid(firstNode, 'pPr'), paraStyleId(kid(firstNode, 'pPr')));
    const isSame = info && (info.fmt === 'bullet') === (kind === 'bullet');
    const id = isSame ? null : listId(kind);
    eachParagraph(pPr => {
      kid(pPr, 'numPr')?.remove();
      kid(pPr, 'ind')?.remove();
      if (!id) return;
      const [numPr] = fragment(`<w:numPr><w:ilvl w:val="0"/><w:numId w:val="${id}"/></w:numPr>`);
      pPr.insertBefore(numPr, [...pPr.children].find(child => PARA_ORDER.indexOf(child.localName) > PARA_ORDER.indexOf('numPr')) ?? null);
    });
  }

  // ---- Structure ----------------------------------------------------------------------------
  function deleteSelected() {
    const range = selectionRange();
    if (!range || range.collapsed) return;
    const startBlock = blockOf(range.startContainer), endBlock = blockOf(range.endContainer);
    const startCell = cellOf(range.startContainer), endCell = cellOf(range.endContainer);
    if (startCell !== endCell || !startBlock || !endBlock) {
      // Across a table's edge: the text goes, the table's structure stays.
      const picked = isolateSelected(range);
      for (const span of picked) span.textContent = '';
      for (const block of page.querySelectorAll('p[data-p]')) if (!block.textContent && !block.querySelector('[data-x], br')) block.append(el('br'));
      if (startBlock) caretAtBlock(startBlock, 0);
      return;
    }
    const offset = offsetInBlock(startBlock, range.startContainer, range.startOffset);
    range.deleteContents();
    if (startBlock !== endBlock) {
      for (const between of [...page.querySelectorAll('p[data-p], table, div[data-x]')].filter(n => n !== startBlock && n !== endBlock && (startBlock.compareDocumentPosition(n) & 4) && (endBlock.compareDocumentPosition(n) & 2))) {
        if (!between.closest('td') || between.closest('td') === startCell) between.remove();
      }
      mergeInto(startBlock, endBlock);
    }
    tidyBlock(startBlock);
    caretAtBlock(startBlock, offset);
  }
  /** Moves `next` into the end of `block`; a section end in `next` is kept. */
  function mergeInto(block, next) {
    const nextNode = sources.get(next.dataset.p), sect = kid(kid(nextNode, 'pPr'), 'sectPr');
    if (sect) paragraphProperty(propertiesOf(sources.get(block.dataset.p)), 'sectPr').replaceWith(sect.cloneNode(true));
    block.querySelector(':scope > br:last-child:not([data-lb])')?.remove();
    block.append(...next.childNodes); next.remove();
  }
  function tidyBlock(block) {
    for (const span of block.querySelectorAll('span[data-r]')) if (!span.childNodes.length) span.remove();
    if (!block.childNodes.length || (!block.textContent && !block.querySelector('[data-x]:not([hidden]), br'))) block.append(el('br'));
  }
  function splitBlock() {
    const range = selectionRange();
    if (!range) return;
    deleteSelected();
    const here = selectionRange(), block = blockOf(here?.startContainer);
    if (!block) return;
    const node = sources.get(block.dataset.p), info = listOf(kid(node, 'pPr'), paraStyleId(kid(node, 'pPr')));
    if (info && !block.textContent.trim() && kid(kid(node, 'pPr'), 'numPr')) { // Enter on an empty item ends the list.
      kid(kid(node, 'pPr'), 'numPr').remove(); showParagraph(block, node); changed(); renumber(); caretAtBlock(block, 0);
      return;
    }
    const key = currentRunKey(), tailRange = document.createRange();
    tailRange.setStart(here.startContainer, here.startOffset);
    tailRange.setEnd(block, block.childNodes.length);
    const tail = tailRange.extractContents();
    const fresh = el('p');
    const atEnd = !tail.textContent && !tail.querySelector?.('[data-x]:not([hidden])');
    const made = newParagraphFor(fresh, node);
    // The sectPr stays with the last paragraph of a section: the tail.
    const sect = kid(kid(node, 'pPr'), 'sectPr');
    if (sect) { sect.remove(); propertiesOf(made).append(sect); }
    const styleId = attr(kid(kid(made, 'pPr'), 'pStyle'), 'val');
    if (atEnd && styleId) {
      const next = attr(kid(styleNode(styleId), 'next'), 'val');
      if (next !== styleId) { kid(kid(made, 'pPr'), 'pStyle').remove(); if (next && next !== attr(defaultParaStyle(), 'styleId')) paragraphProperty(kid(made, 'pPr'), 'pStyle').setAttributeNS(W, 'w:val', next); }
    }
    fresh.append(tail);
    block.after(fresh);
    showParagraph(fresh, made);
    tidyBlock(block); tidyBlock(fresh);
    if (atEnd) { for (const n of [...fresh.childNodes]) if (n.nodeType === 1 && n.nodeName !== 'BR' && !n.textContent) n.remove(); if (!fresh.childNodes.length) fresh.append(el('br')); }
    caretAtBlock(fresh, 0);
    pending = key;
    changed();
  }
  function mergeBackward(block) {
    const previous = block.previousElementSibling;
    if (!previous?.dataset.p) return false;
    const length = previous.textContent.length;
    mergeInto(previous, block);
    tidyBlock(previous);
    caretAtBlock(previous, length);
    changed();
    return true;
  }
  function insertLineBreak() {
    deleteSelected();
    const range = selectionRange();
    if (!range) return;
    const br = el('br');
    br.dataset.lb = currentRunKey();
    range.insertNode(br);
    if (br.nextSibling?.nodeType === 3 && !br.nextSibling.nodeValue) br.nextSibling.remove();
    if (!br.nextSibling) br.after(el('br'));
    const after = document.createRange(); after.setStartAfter(br); after.collapse(true);
    selection().removeAllRanges(); selection().addRange(after);
    changed();
  }
  function insertTextHere(text, key) {
    const range = selectionRange();
    if (!range) return;
    const block = blockOf(range.startContainer);
    const trailing = block && range.startContainer === block && block.childNodes[range.startOffset]?.nodeName === 'BR' && !block.childNodes[range.startOffset].dataset.lb ? block.childNodes[range.startOffset] : null;
    const span = el('span', 'docx-run'); styleSpan(span, key); span.append(document.createTextNode(text));
    const at = range.startContainer.nodeType === 3 ? range.startContainer.parentElement : range.startContainer;
    const host = at.closest?.('span[data-r]');
    if (host) {
      // Into the middle of a run: it splits around the new one.
      const cut = document.createRange();
      cut.setStart(range.startContainer, range.startOffset); cut.setEnd(host, host.childNodes.length);
      const tail = host.cloneNode(false); tail.append(cut.extractContents());
      host.after(span);
      if (tail.textContent) span.after(tail);
      if (!host.textContent) host.remove();
    } else {
      range.insertNode(span);
      trailing?.remove();
    }
    setCaret(span.firstChild, text.length);
  }
  function insertAtom(xmlNodes, visual) {
    deleteSelected();
    const range = selectionRange();
    if (!range) return false;
    const atom = makeAtom(xmlNodes, visual), host = (range.startContainer.nodeType === 3 ? range.startContainer.parentElement : range.startContainer).closest?.('span[data-r]');
    if (host) {
      const cut = document.createRange();
      cut.setStart(range.startContainer, range.startOffset); cut.setEnd(host, host.childNodes.length);
      const tail = host.cloneNode(false); tail.append(cut.extractContents());
      host.after(atom);
      if (tail.textContent) atom.after(tail);
      if (!host.textContent) host.remove();
    } else range.insertNode(atom);
    const after = document.createRange(); after.setStartAfter(atom); after.collapse(true);
    selection().removeAllRanges(); selection().addRange(after);
    return true;
  }
  function insertPageBreak() {
    if (readonly || saving || !selectionRange()) return;
    const run = make('r'), br = make('br'); br.setAttributeNS(W, 'w:type', 'page'); run.append(br);
    if (insertAtom([run], el('span', 'docx-pagebreak', 'Page break'))) changed();
  }
  async function insertImage(file) {
    const extension = file.name.split('.').pop().toLowerCase();
    if (!IMAGE_TYPES[extension] || extension === 'svg') { status.textContent = 'Choose a PNG, JPEG, GIF, BMP or WebP picture'; return; }
    const data = new Uint8Array(await file.arrayBuffer());
    const url = URL.createObjectURL(new Blob([data], { type: IMAGE_TYPES[extension] }));
    const size = await new Promise(resolve => {
      const probe = new Image();
      probe.onload = () => resolve({ w: probe.naturalWidth, h: probe.naturalHeight });
      probe.onerror = () => resolve(null);
      probe.src = url;
    });
    if (!size) { URL.revokeObjectURL(url); status.textContent = 'That picture could not be read'; return; }
    let number = 1;
    while (model.parts[`word/media/image${number}.${extension}`]) number++;
    const part = `word/media/image${number}.${extension}`;
    model.parts[part] = data; imageUrls.set(part, url);
    model.declareType(extension, IMAGE_TYPES[extension]);
    const id = model.addRelation(REL.image, `media/image${number}.${extension}`);
    const room = Math.max(120, page.clientWidth), scale = Math.min(1, room / size.w);
    const cx = Math.round(size.w * scale * 9525), cy = Math.round(size.h * scale * 9525), docId = nextId++;
    const safeName = file.name.replace(/[<>&"']/g, '');
    const [run] = fragment(`<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="${cx}" cy="${cy}"/><wp:docPr id="${docId}" name="${safeName}"/><a:graphic><a:graphicData uri="${PIC}"><pic:pic><pic:nvPicPr><pic:cNvPr id="${docId}" name="${safeName}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="${id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="${cx}" cy="${cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>`);
    if (insertAtom([run], drawingVisual(run))) changed();
  }
  function resizeImage(image, width) {
    const atom = atoms.get(image.dataset.x), run = atom?.[0];
    const extent = run?.getElementsByTagNameNS(WP, 'extent')[0];
    if (!extent) return;
    const ratio = Number(extent.getAttribute('cy')) / Number(extent.getAttribute('cx')), cx = Math.round(width * 9525), cy = Math.round(cx * ratio);
    extent.setAttribute('cx', String(cx)); extent.setAttribute('cy', String(cy));
    for (const ext of run.getElementsByTagNameNS(A, 'ext')) { if (ext.hasAttribute('cx')) { ext.setAttribute('cx', String(cx)); ext.setAttribute('cy', String(cy)); } }
    image.style.width = `${width}px`;
  }
  // Links ---
  function currentLink() {
    const range = selectionRange(), node = range?.startContainer;
    return (node?.nodeType === 1 ? node : node?.parentElement)?.closest('a[data-link]') ?? null;
  }
  async function editLink() {
    if (readonly || saving) return;
    const range = selectionRange();
    if (!range) return;
    const saved = range.cloneRange(), existing = currentLink();
    const values = await ask('Link', [
      ['Address', 'url', existing?.title ?? 'https://'],
      ...(saved.collapsed && !existing ? [['Text to show', 'text', '']] : []),
    ], existing ? 'Remove link' : null);
    if (values === null) return;
    page.focus({ preventScroll: true });
    selection().removeAllRanges(); selection().addRange(saved);
    if (values === 'extra') { existing.replaceWith(...existing.childNodes); changed(); return; }
    const url = values.url.trim();
    if (!/^(https?:|mailto:|tel:)/i.test(url)) { status.textContent = 'A link starts with https://, http://, mailto: or tel:'; return; }
    const id = model.addRelation(REL.hyperlink, url, true);
    if (existing) {
      const node = sources.get(existing.dataset.link).cloneNode(false);
      node.setAttributeNS(R, 'r:id', id); existing.dataset.link = register(sources, node); existing.title = url;
      changed(); return;
    }
    const key = currentRunKey();
    if (saved.collapsed) insertTextHere(values.text.trim() || url, key);
    if (saved.collapsed) { const made = selection().anchorNode; const r = document.createRange(); r.selectNodeContents(made); selection().removeAllRanges(); selection().addRange(r); }
    const spans = isolateSelected(selection().getRangeAt(0));
    if (!spans.length) return;
    const groups = [];
    for (const span of spans) {
      const last = groups[groups.length - 1];
      if (last && last[last.length - 1].nextSibling === span) last.push(span); else groups.push([span]);
    }
    for (const group of groups) {
      if (group[0].closest('a[data-link]')) continue;
      const node = make('hyperlink'); node.setAttributeNS(R, 'r:id', id);
      const link = el('a', 'docx-link'); link.dataset.link = register(sources, node); link.title = url;
      group[0].before(link); link.append(...group);
    }
    const end = document.createRange(); end.setStartAfter(spans[spans.length - 1].closest('a[data-link]')); end.collapse(true);
    selection().removeAllRanges(); selection().addRange(end);
    changed();
  }
  // Tables ---
  async function insertTable() {
    if (readonly || saving) return;
    const range = selectionRange();
    if (!range) return;
    const saved = range.cloneRange();
    const values = await ask('Insert table', [['Rows', 'rows', '3', 'number'], ['Columns', 'cols', '3', 'number']]);
    if (!values) return;
    const rows = Math.max(1, Math.min(200, Number(values.rows) || 1)), cols = Math.max(1, Math.min(40, Number(values.cols) || 1));
    page.focus({ preventScroll: true }); selection().removeAllRanges(); selection().addRange(saved);
    flush();
    const block = blockOf(saved.startContainer);
    if (!block) return;
    const margins = kid(kid(model.body, 'sectPr'), 'pgMar'), size = kid(kid(model.body, 'sectPr'), 'pgSz');
    const room = (Number(attr(size, 'w')) || 11906) - (Number(attr(margins, 'left')) || 1440) - (Number(attr(margins, 'right')) || 1440);
    const width = Math.floor(room / cols);
    const borders = ['top', 'left', 'bottom', 'right', 'insideH', 'insideV'].map(side => `<w:${side} w:val="single" w:sz="4" w:space="0" w:color="auto"/>`).join('');
    const cell = `<w:tc><w:tcPr><w:tcW w:w="${width}" w:type="dxa"/></w:tcPr><w:p/></w:tc>`;
    const [table] = fragment(`<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/><w:tblBorders>${borders}</w:tblBorders></w:tblPr><w:tblGrid>${`<w:gridCol w:w="${width}"/>`.repeat(cols)}</w:tblGrid>${`<w:tr>${cell.repeat(cols)}</w:tr>`.repeat(rows)}</w:tbl>`);
    syncDom();
    const node = sources.get(block.dataset.p);
    node.after(table);
    // Word needs a paragraph after a table.
    if (!table.nextElementSibling || table.nextElementSibling.localName !== 'p') table.after(make('p'));
    const place = { index: [...page.querySelectorAll('p[data-p]')].indexOf(block) + 1, offset: 0 };
    render(); changed(); restorePlace(place);
  }
  function tableContext() {
    const cell = cellOf(selectionRange()?.startContainer);
    if (!cell) return null;
    const tc = sources.get(cell.dataset.tc), tr = tc.parentElement, tbl = tr.parentElement;
    let column = 0;
    for (const other of kids(tr, 'tc')) {
      if (other === tc) break;
      column += Number(attr(kid(kid(other, 'tcPr'), 'gridSpan'), 'val') ?? 1);
    }
    return { cell, tc, tr, tbl, column, width: Number(attr(kid(kid(tc, 'tcPr'), 'gridSpan'), 'val') ?? 1) };
  }
  const blankCell = model_ => {
    const copy = model_.cloneNode(false), tcPr = kid(model_, 'tcPr');
    if (tcPr) { const pr = tcPr.cloneNode(true); kid(pr, 'vMerge')?.remove(); copy.append(pr); }
    copy.append(make('p'));
    return copy;
  };
  function tableEdit(change) {
    if (readonly || saving) return;
    const context = tableContext();
    if (!context) return;
    flush(); syncDom();
    const place = caretPlace();
    if (change(context) === false) return;
    render(); changed(); restorePlace(place);
  }
  const addRow = below => tableEdit(({ tr }) => {
    const fresh = tr.cloneNode(false), trPr = kid(tr, 'trPr');
    if (trPr) fresh.append(trPr.cloneNode(true));
    for (const tc of kids(tr, 'tc')) fresh.append(blankCell(tc));
    tr.parentElement.insertBefore(fresh, below ? tr.nextSibling : tr);
  });
  const cellAt = (row, column) => {
    let col = 0;
    for (const tc of kids(row, 'tc')) {
      const span = Number(attr(kid(kid(tc, 'tcPr'), 'gridSpan'), 'val') ?? 1);
      if (column >= col && column < col + span) return tc;
      col += span;
    }
    return null;
  };
  const addColumn = right => tableEdit(({ tbl, column, width }) => {
    const at = right ? column + width - 1 : column;
    for (const row of kids(tbl, 'tr')) {
      const tc = cellAt(row, at);
      if (tc) row.insertBefore(blankCell(tc), right ? tc.nextSibling : tc);
    }
    const grid = kid(tbl, 'tblGrid'), cols = grid ? kids(grid, 'gridCol') : [];
    if (cols[at]) grid.insertBefore(cols[at].cloneNode(true), right ? cols[at].nextSibling : cols[at]);
  });
  const deleteRow = () => tableEdit(({ tbl, tr }) => { tr.remove(); if (!kids(tbl, 'tr').length) removeTable(tbl); });
  const deleteColumn = () => tableEdit(({ tbl, column, width }) => {
    for (const row of kids(tbl, 'tr')) for (let at = column + width - 1; at >= column; at--) cellAt(row, at)?.remove();
    const grid = kid(tbl, 'tblGrid');
    if (grid) kids(grid, 'gridCol').slice(column, column + width).forEach(n => n.remove());
    for (const row of kids(tbl, 'tr')) if (!kids(row, 'tc').length) row.remove();
    if (!kids(tbl, 'tr').length) removeTable(tbl);
  });
  const removeTable = tbl => {
    if (!tbl.nextElementSibling && !tbl.previousElementSibling) tbl.after(make('p'));
    tbl.remove();
  };
  const deleteTable = () => tableEdit(({ tbl }) => removeTable(tbl));
  function shadeCell(color) {
    tableEdit(({ tc }) => {
      let tcPr = kid(tc, 'tcPr');
      if (!tcPr) { tcPr = make('tcPr'); tc.prepend(tcPr); }
      kid(tcPr, 'shd')?.remove();
      const [shd] = fragment(`<w:shd w:val="clear" w:color="auto" w:fill="${color.slice(1).toUpperCase()}"/>`);
      const after = [...tcPr.children].find(n => ['noWrap', 'tcMar', 'textDirection', 'tcFitText', 'vAlign', 'hideMark'].includes(n.localName));
      tcPr.insertBefore(shd, after ?? null);
    });
  }
  function nextCell(direction) {
    const cell = cellOf(selectionRange()?.startContainer);
    if (!cell) return false;
    const cells = [...cell.closest('table').querySelectorAll('td[data-tc]')], at = cells.indexOf(cell) + direction;
    if (at < 0) return true;
    if (at >= cells.length) return true;
    const target = cells[at].querySelector('p[data-p]');
    if (target) caretAtBlock(target, 0);
    return true;
  }
  // Tracked changes ---
  const trackedNames = ['ins', 'del', 'moveFrom', 'moveTo', 'rPrChange', 'pPrChange', 'sectPrChange', 'tblPrChange', 'tcPrChange', 'trPrChange', 'tblGridChange', 'cellIns', 'cellDel', 'moveFromRangeStart', 'moveFromRangeEnd', 'moveToRangeStart', 'moveToRangeEnd'];
  const hasTracked = () => ['ins', 'del', 'moveFrom', 'moveTo'].some(name => model.body.getElementsByTagNameNS(W, name).length);
  function resolveTracked(accept) {
    if (readonly || saving) return;
    flush(); syncDom();
    const keep = accept ? ['ins', 'moveTo'] : ['del', 'moveFrom'], drop = accept ? ['del', 'moveFrom'] : ['ins', 'moveTo'];
    for (const name of trackedNames) {
      for (const node of [...model.body.getElementsByTagNameNS(W, name)]) {
        const isMark = node.parentElement?.localName === 'rPr'; // a paragraph mark's own change
        if (keep.includes(name) && !isMark) {
          for (const del of [...node.getElementsByTagNameNS(W, 'delText')]) { const t = make('t'); t.setAttributeNS(XML_NS, 'xml:space', 'preserve'); t.textContent = del.textContent; del.replaceWith(t); }
          node.replaceWith(...node.childNodes);
        } else if (drop.includes(name) && !isMark) node.remove();
        else if (!isMark || keep.includes(name) || drop.includes(name)) node.remove();
      }
    }
    render(); changed();
  }
  // Page setup ---
  async function pageSetup() {
    if (readonly || saving) return;
    const saved = selectionRange()?.cloneRange();
    let sectPr = kid(model.body, 'sectPr');
    const size = kid(sectPr, 'pgSz'), margins = kid(sectPr, 'pgMar');
    const w = Number(attr(size, 'w')) || 11906, h = Number(attr(size, 'h')) || 16838;
    const short = Math.min(w, h), paper = Math.abs(short - 12240) < 40 ? 'letter' : Math.abs(short - 11906) < 40 ? 'a4' : 'other';
    const cm = (name, fallback) => (((Number(attr(margins, name)) || fallback) / 1440) * 2.54).toFixed(2);
    const values = await ask('Page setup', [
      ['Paper', 'paper', paper, 'select', [['a4', 'A4'], ['letter', 'Letter'], ...(paper === 'other' ? [['other', 'As it is']] : [])]],
      ['Orientation', 'turn', w > h ? 'landscape' : 'portrait', 'select', [['portrait', 'Portrait'], ['landscape', 'Landscape']]],
      ['Top margin (cm)', 'top', cm('top', 1440), 'number'], ['Bottom margin (cm)', 'bottom', cm('bottom', 1440), 'number'],
      ['Left margin (cm)', 'left', cm('left', 1440), 'number'], ['Right margin (cm)', 'right', cm('right', 1440), 'number'],
    ]);
    if (!values) return;
    flush(); syncDom();
    if (!sectPr) { sectPr = make('sectPr'); model.body.append(sectPr); }
    const sizes = { a4: [11906, 16838], letter: [12240, 15840], other: [short, Math.max(w, h)] }[values.paper];
    const [pw, ph] = values.turn === 'landscape' ? [sizes[1], sizes[0]] : sizes;
    const pgSz = kid(sectPr, 'pgSz') ?? fragment('<w:pgSz/>')[0];
    pgSz.setAttributeNS(W, 'w:w', String(pw)); pgSz.setAttributeNS(W, 'w:h', String(ph));
    if (values.turn === 'landscape') pgSz.setAttributeNS(W, 'w:orient', 'landscape'); else pgSz.removeAttributeNS(W, 'orient');
    const pgMar = kid(sectPr, 'pgMar') ?? fragment('<w:pgMar w:header="708" w:footer="708" w:gutter="0"/>')[0];
    for (const side of ['top', 'bottom', 'left', 'right']) {
      const value = Number(values[side]);
      if (Number.isFinite(value) && value >= 0 && value < 30) pgMar.setAttributeNS(W, `w:${side}`, String(Math.round((value / 2.54) * 1440)));
    }
    // pgSz and pgMar come in that order, after any header and footer references.
    if (!pgSz.parentElement || !pgMar.parentElement) {
      const firstOther = [...sectPr.children].find(n => !['headerReference', 'footerReference', 'footnotePr', 'endnotePr', 'type', 'pgSz', 'pgMar'].includes(n.localName));
      if (!pgSz.parentElement) sectPr.insertBefore(pgSz, kid(sectPr, 'pgMar') ?? firstOther ?? null);
      if (!pgMar.parentElement) sectPr.insertBefore(pgMar, firstOther ?? null);
    }
    showPageGeometry(); changed();
    if (saved) { page.focus({ preventScroll: true }); selection().removeAllRanges(); selection().addRange(saved); }
  }

  // ---- Find and replace ---------------------------------------------------------------------
  let findInput, replaceInput, findCount, matchCase, lastMatch = null;
  function matches() {
    const needle = findInput.value;
    if (!needle) return [];
    const wanted = matchCase.checked ? needle : needle.toLocaleLowerCase(), found = [];
    for (const block of page.querySelectorAll('p[data-p]')) {
      const nodes = textNodes(block);
      const text = nodes.map(n => n.nodeValue).join(''), hay = matchCase.checked ? text : text.toLocaleLowerCase();
      for (let at = hay.indexOf(wanted); at >= 0; at = hay.indexOf(wanted, at + Math.max(1, wanted.length))) found.push({ block, at, length: needle.length });
    }
    return found;
  }
  function rangeOfMatch(match) {
    const range = document.createRange();
    let left = match.at, started = false, need = match.length;
    for (const node of textNodes(match.block)) {
      const size = node.nodeValue.length;
      if (!started && left <= size && (left < size || need === 0)) { range.setStart(node, left); started = true; need += left; }
      if (started) {
        if (need <= size) { range.setEnd(node, need); return range; }
        need -= size;
      } else left -= size;
    }
    return null;
  }
  function selectMatch(match) {
    const range = match && rangeOfMatch(match);
    if (!range) return;
    page.focus({ preventScroll: true });
    selection().removeAllRanges(); selection().addRange(range);
    (range.startContainer.parentElement ?? match.block).scrollIntoView({ block: 'center', behavior: 'instant' });
    lastMatch = match;
  }
  function findStep(direction = 1) {
    const found = matches();
    findCount.textContent = findInput.value ? (found.length ? `${found.length} found` : 'Not found') : '';
    if (!found.length) return;
    const blocks = [...page.querySelectorAll('p[data-p]')];
    const place = lastMatch && blocks.includes(lastMatch.block) ? [blocks.indexOf(lastMatch.block), lastMatch.at] : [-1, -1];
    const after = m => { const i = blocks.indexOf(m.block); return i > place[0] || (i === place[0] && m.at > place[1]); };
    const pick = direction > 0 ? (found.find(after) ?? found[0]) : ([...found].reverse().find(m => !after(m) && !(blocks.indexOf(m.block) === place[0] && m.at === place[1])) ?? found[found.length - 1]);
    selectMatch(pick);
    findCount.textContent = `${found.indexOf(pick) + 1} of ${found.length}`;
  }
  function replaceRange(range, text) {
    const start = range.startContainer, key = (start.nodeType === 3 ? start.parentElement : start).closest('span[data-r]')?.dataset.r ?? '0';
    selection().removeAllRanges(); selection().addRange(range);
    const block = blockOf(start), offset = offsetInBlock(block, start, range.startOffset);
    range.deleteContents();
    tidyBlock(block);
    if (text) { caretAtBlock(block, offset); block.querySelector(':scope > br:last-child:not([data-lb])')?.remove(); insertTextHere(text, key); }
  }
  function replaceOne() {
    if (readonly || saving) return;
    const range = selectionRange(), found = matches();
    const selected = range && found.find(m => { const r = rangeOfMatch(m); return r && r.compareBoundaryPoints(Range.START_TO_START, range) === 0 && r.compareBoundaryPoints(Range.END_TO_END, range) === 0; });
    if (selected) { replaceRange(range, replaceInput.value); changed(); }
    findStep(1);
  }
  function replaceAll() {
    if (readonly || saving) return;
    const found = matches().reverse();
    for (const match of found) { const range = rangeOfMatch(match); if (range) replaceRange(range, replaceInput.value); }
    if (found.length) changed();
    findCount.textContent = `${found.length} replaced`;
  }
  function openFind(withReplace) {
    findBar.hidden = false;
    findBar.classList.toggle('docx-find-only', !withReplace);
    const range = selectionRange();
    if (range && !range.collapsed && !range.toString().includes('\n')) findInput.value = range.toString();
    findInput.focus(); findInput.select();
  }

  // ---- A small dialog -----------------------------------------------------------------------
  /** Asks for values; resolves to them, to null when cancelled, or to 'extra' for the extra button. */
  function ask(title, fields, extra = null) {
    return new Promise(resolve => {
      const dialog = el('dialog', 'docx-dialog'), form = el('form');
      form.method = 'dialog';
      form.append(el('h3', '', title));
      const inputs = {};
      for (const [label, key, value, type, options] of fields) {
        const row = el('label', 'docx-dialog-row'); row.append(el('span', '', label));
        let input;
        if (type === 'select') {
          input = el('select');
          for (const [v, text] of options) { const o = el('option', '', text); o.value = v; input.append(o); }
        } else { input = el('input'); input.type = type || 'text'; if (type === 'number') { input.step = 'any'; input.min = '0'; } }
        input.value = value; inputs[key] = input; row.append(input); form.append(row);
      }
      const actions = el('div', 'docx-dialog-actions');
      const done = result => { dialog.close(); dialog.remove(); resolve(result); };
      if (extra) { const b = el('button', '', extra); b.type = 'button'; b.addEventListener('click', () => done('extra')); actions.append(b); }
      const cancel = el('button', '', 'Cancel'); cancel.type = 'button'; cancel.addEventListener('click', () => done(null));
      const ok = el('button', 'primary', 'OK'); ok.type = 'submit';
      actions.append(cancel, ok); form.append(actions); dialog.append(form);
      form.addEventListener('submit', event => { event.preventDefault(); done(Object.fromEntries(Object.entries(inputs).map(([k, n]) => [k, n.value]))); });
      dialog.addEventListener('cancel', event => { event.preventDefault(); done(null); });
      root.append(dialog); dialog.showModal();
      Object.values(inputs)[0]?.select?.();
    });
  }

  // ---- Toolbar ------------------------------------------------------------------------------
  let undoButton, redoButton, saveButton, styleSelect, fontSelect, sizeSelect, spacingSelect, colorInput, highlightInput, trackedAccept, trackedReject, painterButton;
  const formatButtons = new Map(), alignButtons = new Map(), listButtons = new Map();
  function tool(name, label, action, bar = toolbar) {
    const b = el('button', 'office-tool'); b.type = 'button'; b.title = label; b.setAttribute('aria-label', label);
    b.append(svg(name));
    b.addEventListener('mousedown', e => e.preventDefault());
    b.addEventListener('click', action); bar.append(b);
    return b;
  }
  const textTool = (label, action, bar = toolbar) => {
    const b = el('button', 'office-tool office-text-tool', label); b.type = 'button';
    b.addEventListener('mousedown', e => e.preventDefault());
    b.addEventListener('click', action); bar.append(b);
    return b;
  };
  const divider = (bar = toolbar) => bar.append(el('span', 'office-divider'));
  function choice(label, options, action) {
    const input = el('select', 'office-select'); input.setAttribute('aria-label', label); input.title = label;
    for (const [value, name] of options) { const o = el('option', '', name); o.value = value; input.append(o); }
    input.addEventListener('change', () => action(input.value));
    toolbar.append(input);
    return input;
  }
  function colorTool(name, label, start, action, bar = toolbar) {
    const wrap = el('label', 'office-tool office-color'); wrap.title = label;
    wrap.append(svg(name));
    const swatch = el('span', 'office-swatch'); swatch.style.backgroundColor = start; wrap.append(swatch);
    const input = el('input'); input.type = 'color'; input.value = start; input.setAttribute('aria-label', label);
    // A colour picker takes the focus: the selection has to come back with it.
    let saved = null;
    wrap.addEventListener('mousedown', () => { saved = selectionRange()?.cloneRange() ?? null; });
    input.addEventListener('change', () => {
      if (saved) { page.focus({ preventScroll: true }); selection().removeAllRanges(); selection().addRange(saved); }
      swatch.style.backgroundColor = input.value; action(input.value);
    });
    wrap.append(input); bar.append(wrap);
    return input;
  }
  function usePainter() {
    if (painter) painter = null;
    else {
      const range = selectionRange();
      painter = range ? currentRunKey() : null;
      const start = range?.startContainer, span = (start?.nodeType === 1 ? start : start?.parentElement)?.closest('span[data-r]');
      if (span) painter = span.dataset.r;
    }
    updateTools();
  }
  const fileInput = el('input'); fileInput.type = 'file'; fileInput.accept = 'image/png,image/jpeg,image/gif,image/bmp,image/webp'; fileInput.hidden = true;
  let imageRange = null;
  fileInput.addEventListener('change', () => {
    const file = fileInput.files[0]; fileInput.value = '';
    if (!file) return;
    if (imageRange) { page.focus({ preventScroll: true }); selection().removeAllRanges(); selection().addRange(imageRange); }
    insertImage(file).catch(err => { status.textContent = err.message; });
  });
  function buildToolbar() {
    if (!readonly) {
      undoButton = tool('undo', 'Undo', () => restore(historyIndex - 1)); redoButton = tool('redo', 'Redo', () => restore(historyIndex + 1));
      divider();
      styleSelect = choice('Paragraph style', [...styleChoices()], setParagraphStyle);
      fontSelect = choice('Font', [['', 'Font'], ...FONTS.map(f => [f, f])], value => { if (value) applyFormat(rPr => setProperty(rPr, 'rFonts', true, { ascii: value, hAnsi: value, cs: value })); });
      sizeSelect = choice('Font size', [['', 'Size'], ...SIZES.map(n => [String(n), String(n)])], value => { if (value) applyFormat(rPr => { setProperty(rPr, 'sz', String(Number(value) * 2)); setProperty(rPr, 'szCs', String(Number(value) * 2)); }); });
      divider();
      for (const [icon, label, prop] of [['bold', 'Bold', 'b'], ['italic', 'Italic', 'i'], ['underline', 'Underline', 'u'], ['strike', 'Strikethrough', 'strike']]) {
        formatButtons.set(prop, tool(icon, label, () => (prop === 'u' ? toggle('u', 'single') : toggle(prop))));
      }
      formatButtons.set('superscript', tool('sup', 'Superscript', () => toggleVert('superscript')));
      formatButtons.set('subscript', tool('sub', 'Subscript', () => toggleVert('subscript')));
      colorInput = colorTool('color', 'Text color', '#c00000', value => applyFormat(rPr => setProperty(rPr, 'color', value.slice(1).toUpperCase())));
      highlightInput = colorTool('highlight', 'Highlight', '#ffff00', value => applyFormat(rPr => {
        setProperty(rPr, 'highlight', null);
        const name = Object.entries(HIGHLIGHTS).find(([, hex]) => hex === value)?.[0];
        if (name) { setProperty(rPr, 'shd', null); setProperty(rPr, 'highlight', name); }
        else setProperty(rPr, 'shd', true, { val: 'clear', color: 'auto', fill: value.slice(1).toUpperCase() });
      }));
      painterButton = tool('painter', 'Copy formatting: then select text to apply it', usePainter);
      tool('clear', 'Clear formatting', clearFormat);
      divider();
      for (const [icon, label, value] of [['left', 'Align left', 'left'], ['center', 'Align center', 'center'], ['right', 'Align right', 'right'], ['justify', 'Justify', 'both']]) alignButtons.set(value, tool(icon, label, () => align(value)));
      spacingSelect = choice('Line spacing', [['', 'Spacing'], ['1', '1.0'], ['1.15', '1.15'], ['1.5', '1.5'], ['2', '2.0']], value => { setLineSpacing(value); spacingSelect.value = ''; });
      listButtons.set('bullet', tool('bullets', 'Bulleted list', () => toggleList('bullet')));
      listButtons.set('number', tool('numbers', 'Numbered list', () => toggleList('number')));
      tool('outdent', 'Decrease indent', () => changeIndent(-1)); tool('indent', 'Increase indent', () => changeIndent(1));
      divider();
      tool('link', 'Link', editLink);
      tool('image', 'Insert picture', () => { imageRange = selectionRange()?.cloneRange() ?? null; fileInput.click(); });
      tool('table', 'Insert table', insertTable);
      tool('pagebreak', 'Page break', insertPageBreak);
      tool('page', 'Page setup', pageSetup);
      divider();
    }
    tool('find', readonly ? 'Find' : 'Find and replace', () => openFind(!readonly));
    tool('outline', 'Outline', () => { outlinePanel.hidden = !outlinePanel.hidden; showOutline(); });
    tool('print', 'Print or save as PDF', printDocument);
    toolbar.append(el('span', 'office-spacer'), status);
    if (!readonly) {
      trackedAccept = textTool('Accept all changes', () => resolveTracked(true));
      trackedReject = textTool('Reject all changes', () => resolveTracked(false));
      saveButton = el('button', 'office-save', 'Save'); saveButton.type = 'button';
      saveButton.prepend(svg('save'));
      saveButton.addEventListener('mousedown', e => e.preventDefault());
      saveButton.addEventListener('click', save); toolbar.append(saveButton, fileInput);
      tool('rowAbove', 'Insert row above', () => addRow(false), tableBar);
      tool('rowBelow', 'Insert row below', () => addRow(true), tableBar);
      tool('colLeft', 'Insert column left', () => addColumn(false), tableBar);
      tool('colRight', 'Insert column right', () => addColumn(true), tableBar);
      divider(tableBar);
      tool('delRow', 'Delete row', deleteRow, tableBar); tool('delCol', 'Delete column', deleteColumn, tableBar); tool('delTable', 'Delete table', deleteTable, tableBar);
      divider(tableBar);
      colorTool('shade', 'Cell color', '#fff2cc', shadeCell, tableBar);
    }
    findInput = el('input', 'docx-find-input'); findInput.type = 'search'; findInput.placeholder = 'Find'; findInput.setAttribute('aria-label', 'Find');
    findInput.addEventListener('input', () => { lastMatch = null; findStep(1); });
    findInput.addEventListener('keydown', e => { if (e.key === 'Enter') { e.preventDefault(); findStep(e.shiftKey ? -1 : 1); } });
    replaceInput = el('input', 'docx-find-input docx-replace'); replaceInput.type = 'text'; replaceInput.placeholder = 'Replace with'; replaceInput.setAttribute('aria-label', 'Replace with');
    const caseLabel = el('label', 'docx-find-case'); matchCase = el('input'); matchCase.type = 'checkbox'; caseLabel.append(matchCase, el('span', '', 'Match case'));
    matchCase.addEventListener('change', () => { lastMatch = null; findStep(1); });
    findCount = el('span', 'office-status');
    findBar.append(findInput, textTool('Previous', () => findStep(-1), findBar), textTool('Next', () => findStep(1), findBar), caseLabel);
    if (!readonly) findBar.append(replaceInput, (() => { const b = textTool('Replace', replaceOne, findBar); b.classList.add('docx-replace'); return b; })(), (() => { const b = textTool('Replace all', replaceAll, findBar); b.classList.add('docx-replace'); return b; })());
    findBar.append(findCount, textTool('Close', () => { findBar.hidden = true; page.focus({ preventScroll: true }); }, findBar));
    status.textContent = readonly ? 'Read only' : 'Ready';
  }
  function updateTools() {
    if (readonly) return;
    const range = selectionRange();
    toolbar.querySelectorAll('button, select, input').forEach(n => { n.disabled = saving; });
    undoButton.disabled = saving || (historyIndex === 0 && !syncTimer); redoButton.disabled = saving || historyIndex === history.length - 1;
    saveButton.disabled = saving || !onSave || !(isDirty() || syncTimer);
    painterButton.setAttribute('aria-pressed', String(!!painter));
    const tracked = hasTracked();
    trackedAccept.hidden = trackedReject.hidden = !tracked;
    tableBar.hidden = !range || !cellOf(range.startContainer);
    if (!range) return;
    const key = currentRunKey();
    for (const [name, button] of formatButtons) {
      const pressed = name === 'superscript' || name === 'subscript' ? attr(kid(runFormats.get(key), 'vertAlign'), 'val') === name : on(runValue(key, name));
      button.setAttribute('aria-pressed', String(pressed));
    }
    const size = Number(attr(runValue(key, 'sz'), 'val')) / 2;
    sizeSelect.value = SIZES.includes(size) ? String(size) : '';
    const font = fontOf(runValue(key, 'rFonts'));
    fontSelect.value = FONTS.includes(font) ? font : '';
    const block = blockOf(range.startContainer), node = block && sources.get(block.dataset.p), pPr = kid(node, 'pPr');
    if (node) {
      const id = attr(kid(pPr, 'pStyle'), 'val') ?? '';
      if (id && ![...styleSelect.options].some(o => o.value === id)) { const o = el('option', '', attr(kid(styleNode(id), 'name'), 'val') ?? id); o.value = id; styleSelect.append(o); }
      styleSelect.value = id;
      const jc = attr(kid(pPr, 'jc') ?? inherited(paraStyleId(pPr), 'pPr', 'jc'), 'val') ?? 'left';
      for (const [value, button] of alignButtons) button.setAttribute('aria-pressed', String(jc === value));
      const info = listOf(pPr, paraStyleId(pPr));
      listButtons.get('bullet').setAttribute('aria-pressed', String(!!info && info.fmt === 'bullet'));
      listButtons.get('number').setAttribute('aria-pressed', String(!!info && info.fmt !== 'bullet'));
    }
  }
  function showCounts() {
    const text = [...page.querySelectorAll('p[data-p]')].map(block => block.textContent).join(' ');
    const words = text.trim() ? text.trim().split(/\s+/).length : 0;
    const range = selectionRange(), picked = range && !range.collapsed ? range.toString().trim() : '';
    counts.textContent = picked ? `${picked.split(/\s+/).length} of ${words} words` : `${words} word${words === 1 ? '' : 's'} · ${text.replace(/\s/g, '').length} characters`;
  }
  function showOutline() {
    if (outlinePanel.hidden) return;
    outlinePanel.replaceChildren(el('h3', '', 'Outline'));
    const headings = [...page.querySelectorAll('p[role=heading]')];
    if (!headings.length) outlinePanel.append(el('p', 'muted', 'Headings appear here.'));
    for (const heading of headings) {
      const b = el('button', 'docx-outline-item', heading.textContent.trim() || 'Untitled'); b.type = 'button';
      b.style.paddingLeft = `${(Number(heading.getAttribute('aria-level')) - 1) * 12 + 6}px`;
      b.addEventListener('click', () => { heading.scrollIntoView({ block: 'start', behavior: 'instant' }); if (!readonly) { page.focus({ preventScroll: true }); caretAtBlock(heading, 0); } });
      outlinePanel.append(b);
    }
  }
  function printDocument() {
    flush();
    const copy = el('div', 'docx-print');
    copy.append(sheet.cloneNode(true));
    for (const n of copy.querySelectorAll('[contenteditable]')) n.removeAttribute('contenteditable');
    document.body.append(copy); document.body.classList.add('docx-printing');
    const finish = () => { document.body.classList.remove('docx-printing'); copy.remove(); window.removeEventListener('afterprint', finish); };
    window.addEventListener('afterprint', finish);
    window.print();
  }
  async function save() {
    if (readonly || saving || destroyed || !onSave) return;
    flush(); syncDom();
    if (!isDirty()) return;
    saving = true; const snapshot = history[historyIndex]; typingAt = 0;
    page.setAttribute('contenteditable', 'false'); updateTools(); status.textContent = 'Saving…';
    try {
      await onSave(model.export());
      if (!destroyed) { savedXml = snapshot; onChange(isDirty()); status.textContent = 'Saved'; }
    } catch (err) {
      if (!destroyed) status.textContent = `Save failed: ${err.message}`;
    } finally {
      saving = false;
      if (!destroyed) { page.setAttribute('contenteditable', 'true'); updateTools(); }
    }
  }

  // ---- Typing, pasting and shortcuts --------------------------------------------------------
  let composing = false;
  function pasteText(text, html) {
    if (readonly || saving) return;
    flush();
    const blocks = html ? blocksFromHtml(html) : text.replace(/\r\n?/g, '\n').split('\n').map(line => ({ pieces: [{ text: line }] }));
    if (!blocks.length) return;
    if (!selectionRange()?.collapsed) deleteSelected();
    blocks.forEach((block, index) => {
      if (index > 0) { pending = null; splitBlock(); pending = null; }
      const baseKey = currentRunKey();
      for (const piece of block.pieces) {
        if (!piece.text) continue;
        let key = baseKey;
        if (piece.b || piece.i || piece.u || piece.s) {
          const rPr = runFormats.get(baseKey)?.cloneNode(true) ?? make('rPr');
          if (piece.b) setProperty(rPr, 'b', true);
          if (piece.i) setProperty(rPr, 'i', true);
          if (piece.u) setProperty(rPr, 'u', 'single');
          if (piece.s) setProperty(rPr, 'strike', true);
          key = formatKey(rPr);
        }
        insertTextHere(piece.text, key);
        if (piece.link && /^(https?:|mailto:|tel:)/i.test(piece.link)) {
          const span = selection().anchorNode.parentElement, id = model.addRelation(REL.hyperlink, piece.link, true);
          const node = make('hyperlink'); node.setAttributeNS(R, 'r:id', id);
          const link = el('a', 'docx-link'); link.dataset.link = register(sources, node); link.title = piece.link;
          span.before(link); link.append(span);
          const after = document.createRange(); after.setStartAfter(link); after.collapse(true);
          selection().removeAllRanges(); selection().addRange(after);
        }
      }
      const here = blockOf(selectionRange()?.startContainer), node = here && sources.get(here.dataset.p);
      if (node && (block.style || block.list)) {
        if (block.style) { ensureStyle(block.style); paragraphProperty(propertiesOf(node), 'pStyle').setAttributeNS(W, 'w:val', block.style); }
        if (block.list) {
          kid(propertiesOf(node), 'numPr')?.remove();
          const [numPr] = fragment(`<w:numPr><w:ilvl w:val="0"/><w:numId w:val="${listId(block.list)}"/></w:numPr>`);
          const pPr = propertiesOf(node);
          pPr.insertBefore(numPr, [...pPr.children].find(child => PARA_ORDER.indexOf(child.localName) > PARA_ORDER.indexOf('numPr')) ?? null);
        }
        showParagraph(here, node);
      }
    });
    changed();
  }
  /** Reads pasted HTML as plain data: nothing from it reaches the page or the file as markup. */
  function blocksFromHtml(html) {
    const parsed = new DOMParser().parseFromString(html, 'text/html'), out = [];
    let current = null;
    const BLOCK = new Set(['P', 'DIV', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'LI', 'TR', 'BLOCKQUOTE', 'PRE', 'SECTION', 'ARTICLE']);
    const open = extra => { current = { pieces: [], ...extra }; out.push(current); };
    const walk = (node, marks, list) => {
      for (const child of node.childNodes) {
        if (child.nodeType === 3) {
          const value = child.nodeValue.replace(/\s+/g, ' ');
          if (!value.trim() && !current?.pieces.length) continue;
          if (!current) open({});
          current.pieces.push({ text: value, ...marks });
        } else if (child.nodeType === 1) {
          const tag = child.nodeName;
          if (['SCRIPT', 'STYLE', 'META', 'LINK', 'TITLE', 'HEAD'].includes(tag)) continue;
          if (tag === 'BR') { open({}); continue; }
          const next = { ...marks }, css = child.style;
          if (tag === 'B' || tag === 'STRONG' || css?.fontWeight === 'bold' || Number(css?.fontWeight) >= 600) next.b = true;
          if (tag === 'I' || tag === 'EM' || css?.fontStyle === 'italic') next.i = true;
          if (tag === 'U' || css?.textDecorationLine?.includes('underline') || css?.textDecoration?.includes('underline')) next.u = true;
          if (tag === 'S' || tag === 'STRIKE' || tag === 'DEL' || css?.textDecorationLine?.includes('line-through')) next.s = true;
          if (tag === 'A' && child.getAttribute('href')) next.link = child.getAttribute('href');
          const nextList = tag === 'UL' ? 'bullet' : tag === 'OL' ? 'number' : list;
          if (BLOCK.has(tag)) {
            const heading = /^H([1-4])$/.exec(tag);
            open({ ...(heading ? { style: `Heading${heading[1]}` } : {}), ...(tag === 'LI' && nextList ? { list: nextList } : {}), ...(tag === 'BLOCKQUOTE' ? { style: 'Quote' } : {}) });
            walk(child, next, nextList);
            current = null;
          } else walk(child, next, nextList);
        }
      }
    };
    walk(parsed.body, {}, null);
    for (const block of out) {
      if (block.pieces.length) { block.pieces[0].text = block.pieces[0].text.trimStart(); const last = block.pieces[block.pieces.length - 1]; last.text = last.text.trimEnd(); }
    }
    return out.filter((block, index) => block.pieces.some(p => p.text) || (index > 0 && index < out.length - 1));
  }
  const atBlockStart = (range, block) => offsetInBlock(block, range.startContainer, range.startOffset) === 0;
  const atBlockEnd = (range, block) => { const r = document.createRange(); r.selectNodeContents(block); r.setStart(range.endContainer, range.endOffset); return !r.toString().length && !r.cloneContents().querySelector?.('[data-x]:not([hidden])'); };
  function onBeforeInput(e) {
    if (readonly || saving) { e.preventDefault(); return; }
    const type = e.inputType, range = selectionRange();
    if (!range) return;
    const block = blockOf(range.startContainer);
    switch (type) {
      case 'insertText': case 'insertReplacementText':
        if (!range.collapsed && type === 'insertText') {
          const key = (range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentElement).closest('span[data-r]')?.dataset.r ?? pending ?? '0';
          e.preventDefault(); deleteSelected(); insertTextHere(e.data ?? '', key); pending = null; afterTyping();
        } else if (pending || (block && !range.startContainer.parentElement?.closest('span[data-r]') && range.startContainer.nodeType === 1)) {
          const key = currentRunKey();
          e.preventDefault(); insertTextHere(e.data ?? '', key); pending = null; afterTyping();
        }
        return;
      case 'insertParagraph': e.preventDefault(); flush(); splitBlock(); return;
      case 'insertLineBreak': e.preventDefault(); flush(); insertLineBreak(); return;
      case 'deleteContentBackward': case 'deleteWordBackward': case 'deleteSoftLineBackward': case 'deleteHardLineBackward':
        pending = null;
        if (!range.collapsed) { e.preventDefault(); flush(); deleteSelected(); changed(); return; }
        if (block && atBlockStart(range, block)) {
          e.preventDefault(); flush();
          const node = sources.get(block.dataset.p), pPr = kid(node, 'pPr');
          if (kid(pPr, 'numPr')) { kid(pPr, 'numPr').remove(); kid(pPr, 'ind')?.remove(); showParagraph(block, node); changed(); caretAtBlock(block, 0); return; }
          mergeBackward(block);
        }
        return;
      case 'deleteContentForward': case 'deleteWordForward': case 'deleteSoftLineForward': case 'deleteHardLineForward':
        pending = null;
        if (!range.collapsed) { e.preventDefault(); flush(); deleteSelected(); changed(); return; }
        if (block && atBlockEnd(range, block)) {
          e.preventDefault(); flush();
          const next = block.nextElementSibling;
          if (next?.dataset.p) { const length = block.textContent.length; mergeInto(block, next); tidyBlock(block); caretAtBlock(block, length); changed(); }
        }
        return;
      case 'deleteByCut': e.preventDefault(); flush(); deleteSelected(); changed(); return;
      case 'historyUndo': e.preventDefault(); restore(historyIndex - 1); return;
      case 'historyRedo': e.preventDefault(); restore(historyIndex + 1); return;
      case 'insertCompositionText': case 'insertFromComposition': case 'deleteCompositionText': return;
      default: e.preventDefault(); // Bold from the browser's own menus, drops, and so on.
    }
  }
  function onKeydown(e) {
    const key = e.key.toLowerCase(), command = e.ctrlKey || e.metaKey;
    if (command && key === 's') { e.preventDefault(); e.stopPropagation(); save(); return; }
    if (command && key === 'f') { e.preventDefault(); e.stopPropagation(); openFind(!readonly); return; }
    if (e.key === 'Escape' && !findBar.hidden) { findBar.hidden = true; page.focus({ preventScroll: true }); return; }
    if (readonly || e.target === findInput || e.target === replaceInput) return;
    if (!command && e.key === 'Tab' && page.contains(e.target)) {
      e.preventDefault(); e.stopPropagation();
      if (nextCellIfInTable(e.shiftKey)) return;
      const range = selectionRange(), block = blockOf(range?.startContainer), node = block && sources.get(block.dataset.p);
      if (node && kid(kid(node, 'pPr'), 'numPr')) { changeIndent(e.shiftKey ? -1 : 1); return; }
      if (e.shiftKey) { changeIndent(-1); return; }
      deleteSelected(); insertTextHere('\t', currentRunKey()); pending = null; afterTyping();
      return;
    }
    if (!command) return;
    const stop = () => { e.preventDefault(); e.stopPropagation(); };
    if (key === 'z' || key === 'y') { stop(); restore(historyIndex + (key === 'y' || e.shiftKey ? 1 : -1)); return; }
    if (key === 'b') { stop(); toggle('b'); return; }
    if (key === 'i') { stop(); toggle('i'); return; }
    if (key === 'u') { stop(); toggle('u', 'single'); return; }
    if (key === 'k') { stop(); editLink(); return; }
    if (key === 'h') { stop(); openFind(true); return; }
    if (key === 'enter') { stop(); insertPageBreak(); return; }
    if (e.shiftKey && key === '7') { stop(); toggleList('number'); return; }
    if (e.shiftKey && key === '8') { stop(); toggleList('bullet'); }
  }
  function nextCellIfInTable(backward) {
    if (!cellOf(selectionRange()?.startContainer)) return false;
    const cell = cellOf(selectionRange().startContainer), cells = [...cell.closest('table').querySelectorAll('td[data-tc]')];
    if (!backward && cell === cells[cells.length - 1]) {
      addRow(true);
      const here = cellOf(selectionRange()?.startContainer), fresh = here && [...here.closest('table').querySelectorAll('td[data-tc]')], next = fresh && fresh[fresh.indexOf(here) + 1];
      if (next) caretAtBlock(next.querySelector('p[data-p]'), 0);
      return true;
    }
    return nextCell(backward ? -1 : 1);
  }
  function onSelectionChange() {
    if (destroyed || !root.isConnected) return;
    const range = selectionRange();
    if (!range) return;
    if (painter && !range.collapsed) {
      const key = painter; painter = null;
      const spans = isolateSelected(range);
      for (const span of spans) styleSpan(span, key);
      selectSpans(spans); changed();
    }
    if (!range.collapsed || page.contains(document.activeElement)) { updateTools(); showCounts(); }
  }
  let resize = null;
  function onPointerDown(e) {
    const image = e.target.closest?.('img.docx-image');
    if (readonly || !image || !image.dataset.x && !image.closest('[data-x]')) return;
    const holder = image.dataset.x ? image : image.closest('[data-x]');
    page.querySelectorAll('.docx-image-picked').forEach(n => n.classList.remove('docx-image-picked'));
    holder.classList.add('docx-image-picked');
    const box = image.getBoundingClientRect();
    if (e.clientX > box.right - 18 && e.clientY > box.bottom - 18) {
      e.preventDefault();
      resize = { image, holder, startX: e.clientX, startWidth: box.width };
      window.addEventListener('pointermove', onPointerMove); window.addEventListener('pointerup', onPointerUp);
    }
  }
  const onPointerMove = e => { if (resize) resize.image.style.width = `${Math.max(24, resize.startWidth + e.clientX - resize.startX)}px`; };
  function onPointerUp() {
    window.removeEventListener('pointermove', onPointerMove); window.removeEventListener('pointerup', onPointerUp);
    if (!resize) return;
    resizeImage(resize.holder, parseFloat(resize.image.style.width));
    const block = blockOf(resize.holder); if (block) signatures.delete(block.dataset.p);
    resize = null; changed();
  }

  buildToolbar();
  if (!readonly) {
    page.setAttribute('contenteditable', 'true');
    page.addEventListener('beforeinput', onBeforeInput);
    page.addEventListener('input', () => { if (!composing) afterTyping(); });
    page.addEventListener('compositionstart', () => { composing = true; });
    page.addEventListener('compositionend', () => { composing = false; afterTyping(); });
    page.addEventListener('paste', e => { e.preventDefault(); e.stopPropagation(); pasteText(e.clipboardData.getData('text/plain'), e.clipboardData.getData('text/html')); });
    page.addEventListener('drop', e => { e.preventDefault(); e.stopPropagation(); });
    page.addEventListener('pointerdown', onPointerDown);
  } else page.setAttribute('aria-readonly', 'true');
  page.addEventListener('click', e => {
    const link = e.target.closest?.('a[data-link]');
    if (link && (readonly || e.ctrlKey || e.metaKey) && /^(https?:|mailto:|tel:)/i.test(link.title)) { e.preventDefault(); window.open(link.title, '_blank', 'noopener,noreferrer'); }
  });
  root.addEventListener('keydown', onKeydown);
  document.addEventListener('selectionchange', onSelectionChange);
  render();
  return {
    export() { flush(); syncDom(); return model.export(); },
    save,
    undo: () => restore(historyIndex - 1),
    redo: () => restore(historyIndex + 1),
    get dirty() { return isDirty() || !!syncTimer; },
    destroy() {
      destroyed = true; clearTimeout(syncTimer);
      document.removeEventListener('selectionchange', onSelectionChange);
      for (const url of imageUrls.values()) URL.revokeObjectURL(url);
      root.remove();
    },
  };
}
