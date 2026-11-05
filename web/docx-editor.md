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
without the supplied UI.

## What it edits

The whole page is one editing surface, so selections run freely across text and paragraphs.

- **Text:** bold, italic, underline, strikethrough, superscript, subscript, font, size,
  color, highlight, copy formatting, clear formatting. With a selection they apply to exactly
  that text; with none, to what is typed next.
- **Paragraphs:** the document's own styles and built-in ones (Title, Subtitle, Heading 1-4,
  Quote, Code, added to `styles.xml` when first used), alignment, line spacing, indents.
- **Lists:** bulleted and numbered, nested with Tab and Shift+Tab. Existing lists show their
  real markers, worked out from `numbering.xml`.
- **Links, pictures, tables:** insert and change links (http, https, mailto, tel); insert
  pictures and resize them by the corner; insert tables, add and delete rows and columns, color
  cells, Tab between cells. Merged cells display correctly.
- **Document:** page breaks, line breaks, page setup (paper, orientation, margins), find and
  replace, an outline of the headings, word count, print or save as PDF, and accept or reject
  all tracked changes.
- **Pasting:** rich text keeps bold, italic, underline, strikethrough, links, headings and
  lists. Pasted HTML is only read as data: nothing from it reaches the page or the file as markup.

Shortcuts (Ctrl or Cmd): S save, Z undo, Shift+Z or Y redo, B, I, U, K link, F find, H replace,
Enter page break, Shift+7 numbered list, Shift+8 bulleted list. Shift+Enter is a line break.

Undo and redo keep up to 50 snapshots, trimmed to a 20 MB target.

## In OwnNAS

Choose **New file → Word document (.docx)**, or open an existing DOCX. Save writes the same
file. `GET /api/docx` returns document bytes and an `x-docx-hash` version; `POST /api/write-docx`
accepts `{path, data: base64, expected_hash}`. Both require login; writes also require write
access and the existing CSRF header. The version check prevents stale editor saves. External
filesystem or upload changes can still race the final replacement; this is not collaborative
editing. Saves use a synced temporary file followed by rename and do not create a backup.

## What is kept, and the limits

A paragraph you did not touch keeps its XML exactly as it was. In one you edit, what the editor
does not understand (bookmarks, comment marks, fields, tracked changes, content controls,
objects) stays in place. Other parts of the package are only rewritten when an edit needs it:
`styles.xml`, `numbering.xml`, the relationships and the content types gain entries, and nothing
is removed from them. ZIP compression metadata and XML serialization may change. Limits are 25 MB
compressed, 100 MB expanded, and 4096 parts. Digitally signed and enforced-protection documents
are rejected.

This is an editor, not Word's layout engine. There is one continuous page rather than separate
pages, floating pictures show in line, and headers, footers, footnotes and comments are shown but
not edited. Fields and tracked changes are locked until accepted or rejected. A table of contents
is shown but locked, and Word updates it. A field running across paragraphs locks those
paragraphs. Page setup changes the last section only.

`scripts/test-docx-editor.mjs` runs these in headless Chrome.
