# MCP: widening what agents can do

`/mcp` (src/mcp.rs) lets AI agents use the library with an agent key (src/agent_keys.rs).
A key is limited to its folders. Hidden files and the Trash are never visible. Every call is
written to the agent log. Until now the endpoint was read-only, with five tools: `list_folder`,
`search`, `read_text`, `get_info` and `get_thumbnail`.

Rules that hold for every item below:

- Everything goes through `agent_keys::check`, so links and moves cannot leave the key's
  folders.
- "Not found" and "not allowed" give the same answer.
- Counts and names (people, albums) only reflect photos inside the key's folders, so a key
  learns nothing about what lies outside them.
- The text inside files is data, never instructions; tool descriptions say so.
- No level ever allows permanent deletion, emptying the Trash, settings, accounts, terminals or
  agent keys.

Legend: `[x]` done · `[~]` partly · `[ ]` to do

## 1. Reading more (level: read)

- [x] `photo_search`: photos matching a description ("beach at sunset", "receipts"), using
  smart search. Needs smart search turned on; says so when it is off.
- [x] `similar_photos`: photos that look like a given photo.
- [x] `list_albums` / `album_photos`: albums that have photos in the key's folders, and their
  photos.
- [x] `list_people` / `person_photos`: people recognised in photos in the key's folders.
- [x] `photos_taken`: photos and videos taken between two dates, newest first.
- [x] `read_table`: a spreadsheet (xlsx, xls, ods, numbers…), CSV/TSV or data file (Parquet,
  Arrow, Avro, JSON Lines) as rows of cells, one sheet and range at a time.
- [x] `list_versions` / `read_version`: a file's earlier versions from the file history, and the
  text of one.
- [x] `get_info` adds photo and video details: date taken, camera, lens, exposure, place
  (GPS), duration and resolution.
- [x] `get_thumbnail` takes `size: "large"` (up to 1600 px) so an agent can read a photographed
  document.
- [x] `search` takes `offset` to page through results.
- [x] `recent_files`: files changed under a folder since a date, from the file index.
- [x] `folder_usage`: a folder's size, its sub-folders and largest files, from the file index.
  Hidden files are not counted.
- [x] `find_duplicates`: duplicate groups under a folder, through `duplicates::find`, giving up
  after a minute (hashes read so far are kept, so asking again goes faster). Copies outside
  the key's folders are left out, and groups left with one copy with them.
- [x] `read_text` for images: returns the text already read by OCR, when there is some.
- [x] Results as `structuredContent` (MCP 2025-06-18) alongside the text, and an
  `outputSchema` for the main reading tools.
- [x] Files as MCP resources: `resources/list` (the files at the top of the key's folders),
  `resources/templates/list` (`ownnas:///{+path}`) and `resources/read` (text, or a large
  picture for photos). Reads are in the agent log.
- [x] Prompts: `summarise_folder`, `find_documents`, `photo_story`, and `tidy_folder` for
  organising keys.

## 2. Organising (level: organise)

Changes that are easy to undo. A key at this level can do everything a read key can, plus:

- [x] `add_tags` / `remove_tags` on files and folders.
- [x] `create_folder`.
- [x] `rename` and `move` within the key's folders (both ends are checked).
- [x] `move_to_trash`: only to the Trash. Protected files are refused.
- [x] `add_to_album` / `remove_from_album`; `create_album`.
- [x] Picking the level in Settings → Agents when making a key.
- [x] **Undo**: every change is stored in `agent_changes` with how to reverse it, and only what
  really changed is undone (a tag that was already there stays). Settings → Agents → Changes
  has an Undo button per change, and "Undo today's changes" (newest first; ones that can no
  longer be undone are skipped and named). Undo never overwrites: if the original place is
  taken, or a folder made by the agent has files in it now, it stops and says so.
- [x] **Approval mode** per key ("Ask me before each change", on by default for new organising
  keys, and can be switched on or off later): the paths are checked, then the change waits as
  pending. The agent gets a request id and can follow it with `pending_changes`. Approving
  carries it out with every check made again; a key revoked or expired in the meantime
  changes nothing. There's Approve all, and a note at the top of Settings → Agents when changes
  are waiting.
- [x] A note in the status bar for administrators when changes are waiting ("2 agent changes
  waiting"), which opens Settings → Agents. Checked every minute.
- [x] A daily limit: after 500 changes in a day (UTC), a key that doesn't ask first has the
  rest wait for approval, and the agent is told why. The number can be set per key.

## 3. Writing (level: write)

A key at this level can do everything an organising key can, plus the tools below. The same
approval mode, daily limit and undo apply. Before a file changes, what it held is kept in its
history (`versions::keep_for_agent`, once per change, not once per editing session), and Undo
restores that version. A file or copy the agent made goes to the Trash on Undo.

- [x] `write_file`: makes a text file (up to 5 MB), or replaces one with `replace: true`. Not
  hidden names, and only in a folder that is there.
- [x] `append_rows` to CSV and TSV (keeping the file's separator, semicolons included, and its
  line endings).
- [x] `edit_text`: replaces an exact passage that must appear exactly once, in UTF-8 text files.
- [x] `append_rows` for spreadsheets (xlsx, any sheet: rows after the last one, numbers as
  numbers) and `add_paragraphs` for Word (docx), OpenDocument (odt), text and Markdown, done on
  the server (`office_edit.rs`) by rewriting only the one XML part that changes. `edit_text`
  also replaces passages in docx (across formatting runs, within a paragraph) and odt (within
  one stretch of text).
- [x] `upload_file` from base64, up to 25 MB a call; a name taken gets a number, or
  `replace: true`.
- [x] `sort_into_folders` (organise level): a folder's files into sub-folders by a pattern
  (`{YYYY}/{MM}`, `{type}`, `{ext}`…), with `dry_run`, undone as one change.
- [x] `copy`.

## Notes

- [x] `list_notes`, `read_note` (with the notes linking to it) for every key whose folders hold
  the notes folder; `create_note`, `append_to_note`, `tick_item` at the write level, with
  approval and undo like other changes. See todo/notes.md.

## 4. Controls on keys

- [x] Tools chosen one by one per key, in Settings → Agents → **Limits & tools…**: unchosen
  tools are not listed to the agent and refuse if called anyway.
- [x] A key's folders (or single files) can be changed after it is made, keeping its secret:
  Settings → Agents → **Folders & files…** (`POST /api/agent-keys/folders`).
- [x] File extensions per key (Limits & tools): only files ending in them can be seen, read,
  changed or made; folders stay browsable; folders holding other kinds are not moved, copied
  or put in the Trash. Enforced in `agent_keys::check` (the key's `Scope`), so every tool
  follows it.
- [x] Read-only folders inside an organise or write key. The most specific of the key's
  folders decides, so a folder of its own inside a read-only one stays writable. This is
  checked when a change is asked for and again when it is made (approved changes too).
- [x] Rate limit: 300 requests a minute per key; past it, HTTP 429 with `Retry-After`.
- [x] Setting the rate and daily limits per key (same dialog; empty means the usual 300 a minute
  and 500 a day).
- [x] Usage per key in Settings → Agents → **Usage**: calls a day over 30 days (days with
  refusals in red), calls by tool (allowed, refused, failed) and changes by state.
- [x] Keys refused 20 times or more in the last hour are flagged: in the status bar ("Agent key
  “x” refused 25 times") and on the key's card, with a pointer to its Activity and Revoke.
- [x] OAuth sign-in (MCP authorization spec): metadata (RFC 9728 and 8414), registration
  (RFC 7591), an approval screen in the web app (folders, level, ask first), codes with PKCE
  (S256 only) traded once within 10 minutes, and refreshing, which turns the key's secret over.
  Each connection is an ordinary agent key ("Claude (connected)"), so the log, limits, undo and
  revoking apply. Redirects: https, http on localhost, or an app's own address.

## Also done

- [x] `rename_many` (organise level): rename by pattern, find and replace, or case, with
  `dry_run` and undo as one change.
- [x] Dates from file names (`IMG-20260928-WA0006`) for `sort_into_folders` and `rename_many`
  (`date: "name"`).
