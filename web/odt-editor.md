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
`body`, and `export()`. The editor supports text editing, bold, italic,
underline, font size, color, paragraph alignment, headings, tables and lists
for display, read-only mode, undo/redo, and Ctrl/Cmd+S. The host owns persistence
and conflict handling. Other ZIP members, including embedded images, remain in
the package. The required `mimetype` member stays first and uncompressed.

In OwnNAS, choose **New file → OpenDocument text (.odt)** or open an existing
ODT. `GET /api/odt` returns bytes with an `x-odt-hash`; `POST /api/write-odt`
accepts `{path, data: base64, expected_hash}`. Both require login; writes also
require write access and the existing CSRF header. Saves reject stale versions,
invalid packages, signed documents, and files in Trash.

The editor is not a full LibreOffice layout engine. It preserves package parts
and XML structures it does not edit, but previews only common paragraphs,
headings, lists, and tables. Page layout, fields, charts, drawings, comments,
footnotes, headers, and complex style inheritance are not fully represented.
Formatting applies to the active text node. Encrypted documents cannot be
opened. Limits are 25 MB compressed, 100 MB expanded, and 4096 ZIP members.

Run the browser-module regression checks with:

```sh
npm install --prefix /tmp/ownnas-docx-test jsdom
node scripts/test-odt-editor.mjs
```

Set `ODT_TEST_DOM` to an alternate absolute `jsdom/lib/api.js` path if needed.
