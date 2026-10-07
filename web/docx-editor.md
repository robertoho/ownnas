# Reusable DOCX editor

`docx-editor.js` is a browser ES module with no OwnNAS API or framework dependency.
It uses the existing MIT-licensed fflate module for ZIP handling (see
https://github.com/101arrowz/fflate). Copy the module and that dependency to another
project, update its import path, and include `docx-editor.css`.

```js
import { mountDocxEditor, openDocx } from './docx-editor.js';
const editor = mountDocxEditor(container, docxBytes, {
  readonly: false,
  onChange(dirty) { /* update navigation guards */ },
  async onSave(bytes) { /* persist Uint8Array; reject on failure */ },
});
await editor.save();
editor.destroy();
```

`openDocx(Uint8Array)` exposes the parsed `xml`, `body`, and `export()` for use
without the supplied UI. Mount handles text editing, bold/italic/underline on the
active run, font size and text color, paragraph alignment, heading outlines,
inserting/splitting paragraphs, basic tables, plain-text paste, and Ctrl/Cmd+S.
Undo/redo covers text, formatting, and structure changes with up to 50 snapshots
trimmed to a 20 MB target (at least two snapshots are retained). Ctrl/Cmd+Z,
Ctrl/Cmd+Shift+Z, Ctrl+Y, and Ctrl/Cmd+B/I/U are supported.
The host owns persistence and conflict handling. A failed save retains edits.

In OwnNAS, choose **New file → Word document (.docx)**, or open an existing DOCX.
Save writes the same file. Download uses the app's normal download action.
`GET /api/docx` returns document bytes and an `x-docx-hash` version;
`POST /api/write-docx` accepts `{path, data: base64, expected_hash}`. Both require
login; writes also require write access and the existing CSRF header. The version
check prevents stale editor saves. External filesystem/upload changes can still
race the final replacement; this is not collaborative editing. Saves use a synced
temporary file followed by rename and do not create a backup.

The editor preserves other ZIP parts byte-for-byte after decompression, including
media, relationships, styles, and headers. It modifies the main document's XML
through DOM nodes, never by importing HTML. ZIP compression metadata and XML
serialization may change. Limits are 25 MB compressed, 100 MB expanded, and 4096
parts. Digitally signed and enforced-protection documents are rejected.

This is a basic editor, not a full Word layout engine. Existing style inheritance,
numbering, pagination, merged-cell layout, drawings, footnotes, and headers are not
faithfully displayed. Fields, tracked changes, and content controls are locked;
unsupported parts remain in the saved package. Formatting applies to an entire
run, not an arbitrary selected range. Enter splits simple text-only paragraphs at
the caret; paragraphs containing unsupported structures receive a new paragraph
after the current one. Shift+Enter line breaks are disabled. Heading choices use
direct font and outline formatting without assuming source styles exist.
It does not provide collaboration or autosave.

Validation:

```sh
cargo test --lib
npm install --prefix /tmp/ownnas-docx-test jsdom
node scripts/test-docx-editor.mjs
```

Set `DOCX_TEST_DOM` to an alternate absolute `jsdom/lib/api.js` path if needed.
