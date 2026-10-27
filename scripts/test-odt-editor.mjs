// NODE_PATH is not used by ESM; override ODT_TEST_DOM to use another jsdom install.
// Browser-only behaviour (the toolbar's formatting commands, Enter and Backspace) is not covered:
// jsdom has no editing commands. These checks cover rendering, writing edits back, and the package.
import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import { Buffer } from 'node:buffer';
const { JSDOM } = await import(pathToFileURL(process.env.ODT_TEST_DOM || '/tmp/ownnas-docx-test/node_modules/jsdom/lib/api.js'));
const dom = new JSDOM('<div id="editor"></div>');
for (const key of ['window', 'document', 'DOMParser', 'XMLSerializer', 'Event', 'Node', 'NodeFilter', 'getComputedStyle']) globalThis[key] = key === 'window' ? dom.window : dom.window[key];
const { zipSync, unzipSync, strToU8, strFromU8 } = await import('../web/vendor/three/addons/libs/fflate.module.js');
const { openOdt, mountOdtEditor } = await import('../web/odt-editor.js');
const office = 'urn:oasis:names:tc:opendocument:xmlns:office:1.0';
const textNs = 'urn:oasis:names:tc:opendocument:xmlns:text:1.0';
const styleNs = 'urn:oasis:names:tc:opendocument:xmlns:style:1.0';
const fo = 'urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0';
const tableNs = 'urn:oasis:names:tc:opendocument:xmlns:table:1.0';
const MIME = 'application/vnd.oasis.opendocument.text';
const content = `<office:document-content xmlns:office="${office}" xmlns:text="${textNs}" xmlns:style="${styleNs}" xmlns:fo="${fo}" xmlns:table="${tableNs}" office:version="1.3"><office:automatic-styles><style:style style:name="P1" style:family="paragraph"><style:paragraph-properties fo:text-align="center"/></style:style></office:automatic-styles><office:body><office:text><text:sequence-decls><text:sequence-decl text:name="Table"/></text:sequence-decls><text:p text:style-name="P1"><text:span text:style-name="Emphasis">Original</text:span> text<text:bookmark text:name="here"/></text:p><text:list><text:list-item><text:p>Item</text:p></text:list-item></text:list><table:table><table:table-row><table:table-cell><text:p>Cell</text:p></table:table-cell></table:table-row></table:table></office:text></office:body></office:document-content>`;
const styles = `<office:document-styles xmlns:office="${office}" xmlns:style="${styleNs}" xmlns:fo="${fo}"><office:styles><style:style style:name="Emphasis" style:family="text"><style:text-properties fo:font-style="italic"/></style:style></office:styles></office:document-styles>`;
const pack = files => zipSync({ mimetype: [strToU8(MIME), { level: 0 }], ...files });
const bytes = pack({ 'content.xml': strToU8(content), 'styles.xml': strToU8(styles), 'Pictures/image.png': new Uint8Array([1, 2, 3]) });

assert.throws(() => openOdt(new Uint8Array([1, 2, 3])));
assert.throws(() => openOdt(pack({ 'content.xml': strToU8(content), 'META-INF/documentsignatures.xml': strToU8('<signature/>') })), /signed/);

const container = document.getElementById('editor');
let saved = null, dirtyFlag = false;
const editor = mountOdtEditor(container, bytes, { onChange: dirty => { dirtyFlag = dirty; }, onSave: async data => { saved = data; } });
const page = container.querySelector('.odt-page');
assert.equal(page.contentEditable, 'true');
assert.equal(container.querySelectorAll('.odt-page table td').length, 1);
assert.equal(container.querySelectorAll('.odt-page li').length, 1);
const span = page.querySelector('[data-style="Emphasis"]');
assert.equal(span.style.fontStyle, 'italic', 'document styles are drawn');
assert.equal(page.querySelector('p').style.textAlign, 'center', 'paragraph styles are drawn');
assert.ok(page.querySelector('.odt-marker[data-atom]'), 'bookmarks are kept as locked pieces');

// Typing: text is written back as text, never markup; runs of spaces become <text:s>.
span.firstChild.data = '<script>safe</script> & changed';
const loose = [...page.querySelector('p').childNodes].find(node => node.nodeType === 3);
loose.data = '  two  spaces';
// Formatting as the browser writes it: a bold element around a word.
const bold = document.createElement('b');
bold.textContent = 'Bold';
page.querySelector('li').append(' ', bold);
// Enter: the browser copies the paragraph, data and all.
const copy = page.querySelector('p').cloneNode(false);
copy.textContent = 'Second paragraph';
page.querySelector('p').after(copy);
page.dispatchEvent(new Event('input', { bubbles: true }));
assert.equal(dirtyFlag, true);
await editor.save();
assert.equal(editor.dirty, false);

const out = unzipSync(saved);
assert.deepEqual(out['Pictures/image.png'], new Uint8Array([1, 2, 3]), 'other parts are kept');
assert.deepEqual(out.mimetype, strToU8(MIME));
const archive = Buffer.from(saved);
assert.equal(archive.toString('latin1', 30, 38), 'mimetype', 'mimetype is the first member');
assert.equal(archive.readUInt16LE(8), 0, 'and stored, not compressed');
const xml = new DOMParser().parseFromString(strFromU8(out['content.xml']), 'application/xml');
const paragraphs = [...xml.getElementsByTagNameNS(textNs, 'p')];
assert.equal(paragraphs[0].getElementsByTagNameNS(textNs, 'span')[0].textContent, '<script>safe</script> & changed');
assert.equal(paragraphs[0].getElementsByTagNameNS(textNs, 'span')[0].getAttributeNS(textNs, 'style-name'), 'Emphasis', 'untouched formatting keeps its style');
assert.equal(paragraphs[0].getElementsByTagNameNS(textNs, 's').length, 2, 'extra spaces are written as text:s');
assert.equal(paragraphs[0].getElementsByTagNameNS(textNs, 'bookmark').length, 1, 'the bookmark survives');
assert.equal(paragraphs[1].textContent, 'Second paragraph');
assert.equal(paragraphs[1].getAttributeNS(textNs, 'style-name'), 'P1', 'a split paragraph keeps its style');
assert.equal(xml.getElementsByTagNameNS(textNs, 'sequence-decls').length, 1, 'declarations are kept');
const item = xml.getElementsByTagNameNS(textNs, 'list-item')[0];
const boldSpan = item.getElementsByTagNameNS(textNs, 'span')[0];
assert.equal(boldSpan.textContent, 'Bold');
const boldStyle = [...xml.getElementsByTagNameNS(styleNs, 'style')].find(style => style.getAttributeNS(styleNs, 'name') === boldSpan.getAttributeNS(textNs, 'style-name'));
assert.equal(boldStyle.getElementsByTagNameNS(styleNs, 'text-properties')[0].getAttributeNS(fo, 'font-weight'), 'bold');
assert.equal(xml.getElementsByTagNameNS(tableNs, 'table-cell')[0].textContent, 'Cell');

editor.undo();
assert.equal(editor.dirty, true);
assert.ok(!container.querySelector('.odt-page b'), 'undo redraws the document');
editor.redo();
assert.equal(editor.dirty, false);
editor.destroy();

const readonly = mountOdtEditor(container, bytes, { readonly: true });
assert.equal(container.querySelectorAll('[contenteditable="true"]').length, 0);
assert.equal(container.querySelectorAll('button').length, 0);
readonly.destroy();

const blank = pack({ 'content.xml': strToU8(`<office:document-content xmlns:office="${office}" xmlns:text="${textNs}"><office:body><office:text/></office:body></office:document-content>`) });
const emptyEditor = mountOdtEditor(container, blank);
assert.equal(container.querySelectorAll('.odt-page p').length, 1, 'an empty document gets a paragraph to type in');
emptyEditor.destroy();
console.log('ODT rendering, write-back, package preservation, read-only, and undo/redo tests passed.');
