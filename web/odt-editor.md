# Reusable ODT editor

`odt-editor.js` is a framework-independent browser ES module. It uses the
project's existing fflate module for OpenDocument ZIP package handling. Copy
the module and its ZIP dependency into another project, then include
`docx-editor.css` for the shared document editor styles.

```js
import { mountOdtEditor, openOdt } from './odt-editor.js';
const editor = mountOdtEditor(container, odtBytes, {
  readonly: false,
  onChange(dirty) { /* update navigation guards */ },
  async onSave(bytes) { /* persist Uint8Array; reject on failure */ },
});
await editor.save();
editor.destroy();
```

`openOdt(Uint8Array)` validates and opens an ODT package and exposes `xml`,
`body`, and `export()`. The editor shows the document as one editable page:
the browser handles typing, selection, Enter and paste (plain text), and each
change is written back into `content.xml`. Every paragraph, list and table keeps
the XML node it came from, so their attributes and styles survive edits.

The toolbar has undo/redo, paragraph style (normal text, headings 1-4), font
size, bold, italic, underline, strikethrough, text colour, highlight,
alignment, bulleted and numbered lists, and clear formatting. Ctrl/Cmd+B, I, U,
Z, Y (or Shift+Z) and S work too; Tab and Shift+Tab indent list items.
Formatting applies to the selected text: the editor adds automatic text styles
(`OwnNAS_T…`) based on the text's existing style, and removes the ones it no
longer uses on save. Backspace and Delete across paragraphs are done by the
editor itself, so joined text keeps its own styles.

Images show in place. Fields, notes, comments, bookmarks, tables of contents
and other content the editor can't edit show as locked pieces and are saved
unchanged. Table cells are editable, but the table's shape is not.

The page's Content-Security-Policy in OwnNAS (`style-src 'self'`) blocks the
style attributes the browser's editing commands write; the editor applies what
its commands write through the CSSOM instead, and drops style text the browser
adds on its own. The editor does not use the `data-css` attribute, which OwnNAS
reserves.

In OwnNAS, choose **New file → OpenDocument text (.odt)** or open an existing
ODT. `GET /api/odt` returns bytes with an `x-odt-hash`; `POST /api/write-odt`
accepts `{path, data: base64, expected_hash}`. Both require login; writes also
require write access and the existing CSRF header. Saves reject stale versions,
invalid packages, signed documents, and files in Trash.

The editor is not a full LibreOffice layout engine: page size, margins, headers,
footers, columns and pagination are not shown. Other ZIP members, including
embedded images, remain in the package, and the required `mimetype` member stays
first and uncompressed. Encrypted documents cannot be opened. Limits are 25 MB
compressed, 100 MB expanded, and 4096 ZIP members.

Run the browser-module regression checks with:

```sh
npm install --prefix /tmp/ownnas-docx-test jsdom
node scripts/test-odt-editor.mjs
```

The jsdom checks cover rendering, writing edits back and the package; the
toolbar's commands need a real browser.

Set `ODT_TEST_DOM` to an alternate absolute `jsdom/lib/api.js` path if needed.
