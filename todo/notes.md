# Notes (Google Keep's quick capture, Obsidian's linked Markdown)

A **Notes** view, next to Files and the Gallery, for quick notes and checklists, kept as plain
Markdown files in a library folder (`Notes/` by default). Because they are only `.md` files,
they open in Obsidian or any editor, sync with `ownnas-sync`, are backed up and versioned like
everything else, and agents can read and write them over MCP. There is no database of notes to
lock them in: the files are the notes.

## Why it fits OwnNAS

- Most of the parts exist already: the Markdown editor and preview, tags, content search,
  file versions, the Trash, protection from deletion, sync, and the MCP write tools.
- It answers "where do I put this quickly?", which a file browser doesn't.
- Obsidian users can point a vault at the synced folder and keep using it.

## Keep-like (capture and glance)

- [x] A grid of cards (masonry, like Keep): title, the first lines, checklist items, colour.
  Pinned notes first.
- [x] A "Take a note…" box at the top: type and press Enter to save, with no file name to
  choose. The file is named from the first line (`Groceries.md`), or by date and time when
  there is no title.
- [x] Checklists: `- [ ]` lines show as checkboxes you can tick on the card itself, which
  rewrites that line in the file.
- [x] Colour, pinned and archived are kept in the front matter (`color: yellow`,
  `pinned: true`), which Obsidian shows as properties and otherwise ignores. Archive moves the
  file to `Notes/Archive/`.
- [x] Labels are the existing OwnNAS tags, and `#hashtags` written in the text count too.
- [x] Pictures pasted or dropped into a note are saved in `Notes/attachments/` and linked as
  `![](attachments/…)`.
- [x] On the phone: the web app is a share target (text and links shared from other apps open
  as a new note), and long-pressing its icon offers New note. Still to do: sharing pictures
  and files (needs a service worker for POST), and the native Android app.
- [x] Reminders (`remind:` in the front matter): set from the menu, due notes on top, and a
  notification while OwnNAS is open. Earlier versions from the menu.

## Obsidian-like (write and connect)

- [x] Open a card in a full editor with live preview, using the existing Markdown editor.
- [x] `[[Wiki links]]` between notes, completed as you type, with **Backlinks** ("linked from")
  under each note. Renaming a note updates the links to it (it's in our hands, since moves go
  through OwnNAS).
- [x] Daily notes: one click opens `Notes/Daily/2026-11-07.md`, made from a template if there
  is one.
- [x] Templates in `Notes/Templates/`.
- [x] Sub-folders work as notebooks (a row of filters above the cards); everything else is search and tags.
- [ ] Later: a graph of links, and `![[embeds]]`.

## Under the hood

- [ ] A small index of notes in SQLite (`notes`: path, title, first lines, tags, links,
  checklist counts, colour, pinned, updated), kept up to date by the file watcher, so the grid
  and backlinks are fast and edits made outside (Obsidian, sync) show up.
- [ ] Saving goes through the existing write path, so each save keeps a version in the history.
  Two people editing the same note: a hash check and "it changed meanwhile" (as the DOCX editor
  does now).
- [x] Settings: which folder holds the notes. Per-user notes could later live in
  `Notes/<user>/`.

## Agents (MCP)

- [x] `list_notes`, `read_note` (with its backlinks), `create_note`, `append_to_note`,
  `tick_item`: specialised versions of the write tools, so "add milk to my shopping list" or
  "summarise this into a note" work naturally. They go through the same approval and undo.

## A first version worth shipping

The grid with quick capture, checklists you can tick, colours and pins in the front matter,
tags, search, and opening a note in the existing editor. Wiki links, backlinks and daily notes
come second; the phone share sheet third.

## Status (first version done)

- Done: the Notes view (`#notes`, the notes icon in the header) with the card grid, quick
  capture (text or checklist, colour), ticking items on cards, colour and pin in the front
  matter, labels from tags and #hashtags, search, the archive, opening notes in the Markdown
  editor, daily notes with a template, choosing the notes folder (admins, the folder button),
  and backlinks ("Linked from …") from `[[links]]`. Tested in `scripts/test-notes.mjs` (headless
  Chrome) and `notes.rs` unit tests. Agents: see todo/mcp.md.
- Second round, done: the Markdown preview hides front matter, shows checklists as boxes,
  pictures from the library and clickable `[[links]]` (a link to a note that is not there yet
  offers to make it); typing `[[` in the editor suggests notes; pictures pasted or dropped into
  the editor are saved in `attachments/` next to the note and linked; renaming a note (in Files
  or by an agent, and undoing that) rewrites the links to it in the other notes, keeping their
  versions; notebooks (sub-folders) filter the cards, and new notes go into the one shown.
- Next: the phone share sheet, an index in SQLite if folders with thousands of notes get slow
  (today the folder is read on each visit), `![[embeds]]`, and a graph of links.
