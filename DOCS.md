# OwnNAS documentation

OwnNAS is a self-hosted web file browser. You compile a single binary, point it at one shared folder, and open it in a browser on your machine or VPN. Accounts and sessions live in SQLite beside a thumbnail cache; the files themselves never leave the folder you choose.

This document covers how the app works in detail. For a short quick-start, see [README.md](README.md).

---

## Concepts

| Piece | Role |
| --- | --- |
| `--root` | The shared library folder (and its subfolders). |
| `--data` | SQLite DB + thumbnail cache. **Must sit outside `--root`.** Default: `./ownnas-data`. |
| Accounts | Stored in `ownnas-data/ownnas.db` (argon2 password hashes + sessions). |
| Web UI | Embedded HTML/CSS/JS served from the binary (`/` and `/assets/*`). |

OwnNAS is meant to sit on a private network or VPN. The login is the application boundary; the VPN is the network boundary. Put it behind HTTPS and use `--secure-cookie` when TLS is terminated by a reverse proxy.

---

## Running

```bash
cargo build --release
./target/release/ownnas serve --root /path/to/library --username admin --password "a long passphrase"
```

Default listen address: `0.0.0.0:8787`. Open `http://127.0.0.1:8787` locally, or `http://<host>:8787` from another device on the VPN.

### Useful flags

| Flag | Meaning |
| --- | --- |
| `--root DIR` | Shared folder (`OWNNAS_ROOT`). |
| `--data DIR` | DB + thumbs (`OWNNAS_DATA`, default `ownnas-data`). |
| `--addr HOST:PORT` | Listen address (`OWNNAS_ADDR`). |
| `--username` / `--password` | Seed the first account only (`OWNNAS_USER` / `OWNNAS_PASSWORD`). Prefer the env var for the password. |
| `--readonly` | Browse, preview, download — no uploads, renames, deletes, tags edits, etc. |
| `--secure-cookie` | Mark the session cookie `Secure` (use behind HTTPS). |
| `--open` | Open a browser on the host after start. |

Account management:

```bash
ownnas user add --username alice --password "…"
ownnas user list
ownnas user password --username alice --password "…"
ownnas user remove --username alice
```

On first start with an empty DB, OwnNAS can prompt in the terminal for missing `--root` / username / password.

Helper scripts in the repo (`build.sh` / `build.bat`, `serve.sh` / `serve.bat`) wrap release builds and a local serve command. Adjust paths and passwords before use.

---

## Browsing the library

After sign-in you get a file manager over `--root`:

- **Breadcrumbs** for the current path; click a segment to jump.
- **OwnNAS** in the header jumps to the library root.
- **Filter** by file name or tag (current folder only).
- **Sort** by name, date, or size (ascending / descending).
- **Views**: Icons (square thumbs), Grid (equal-sized cards, 4:3 thumbs), List.
- **Hidden** toggle to show/hide dotfiles (special OwnNAS folders always appear).

### Selection (Explorer-style)

- Click selects one item; **Ctrl/Cmd+click** toggles; **Shift+click** selects a range.
- Drag a **rubber-band rectangle** on empty space to select; hold Ctrl/Cmd while dragging to add.
- **Ctrl/Cmd+A** selects all visible items; **Escape** clears the selection (and closes menus/preview).
- Arrow keys move the selection; **Enter** opens the selected item.
- **Ctrl/Cmd+C / X** copy/cut; **Ctrl/Cmd+V** pastes OwnNAS clipboard or uploads OS clipboard files.
- **Delete** moves selection to Trash (or deletes forever if already in Trash).

### Context menus

- **Right-click empty space**: New folder, Paste (when clipboard has items).
- **Right-click selection / ··· menu**: Open, Download, Copy/Cut, Paste, Duplicate, Move, Rename, Trash/Delete, Restore (in Trash).
- **New folder with selection** (2+ items): creates a folder in the current directory and moves the selected items into it.

### Clipboard strip

Cut/copy keeps a floating clipboard panel with Paste / Clear until you finish a cut or clear it.

---

## Special folders

OwnNAS creates/manages two reserved folders inside `--root`:

| Folder | Purpose |
| --- | --- |
| `.ownnas-trash` | Shared Trash. Soft-delete moves here; Restore returns items; Empty Trash removes forever. |
| `.ownnas-archive` | Upload conflict archive (older / replaced files). |

Legacy `Trash/` and `Archive/` names are migrated when safe. These folders stay visible even when Hidden is off. Deleting something already in Trash removes it permanently.

---

## Thumbnails

Thumbnails are generated **on demand** when the UI requests `/api/thumb?path=…`, then stored under `ownnas-data/thumbs/`.

### How caching works

1. The cache key is a SHA-256 of the absolute source path + modified time + file size.
2. Files land in `thumbs/<aa>/<hash>.png`.
3. If the file changes (size or mtime), the key changes and a new thumb is built.
4. Concurrent builds write a temp PNG then rename into place.

### Images

- Decoded with the Rust `image` crate (EXIF orientation applied).
- Resized to fit within **480×480**, saved as PNG.
- Limits: max dimension ~25k px, max decode budget ~256 MB.

**Thumbnailed:** jpeg, png, gif, webp, bmp, tiff, ico, jfif (and similar).

**Not thumbnailed as raster:** svg/svgz (shown as live SVG in the grid instead), heic/heif, avif, jxl.

### Videos

- Requires **`ffmpeg` on `PATH`**. At startup OwnNAS prints whether video thumbs are enabled.
- Seeks to ~1s (falls back to 0s), extracts one frame, scales to fit 480×480, writes PNG.
- Timeout ~25s per video. Without ffmpeg, videos still list and can still play in preview, but show a “VID” glyph instead of a poster.

### Grid badges

Tags appear on the thumbnail as small stacked **color badges** (1–2 letter initials). Hover for the full tag; click a badge to filter the folder by that tag.

---

## Previews and media playback

Opening a file opens the preview panel. Content streams from `/api/raw?path=…` (download uses `download=1`).

| Kind | Behavior |
| --- | --- |
| **Images / SVG** | Inline image; arrow buttons step through other images in the folder. |
| **Video** | HTML5 `<video controls playsinline>` with optional poster from `/api/thumb`. |
| **Audio** | HTML5 `<audio controls>`. |
| **PDF** | Inline iframe. |
| **Text / code / CSV / JSON / Markdown** | Fetched via `/api/meta` and rendered (tables, markdown, highlighted text as appropriate). |
| **Archives (zip/tar/…)** | Listing / meta preview when supported. |
| **Other** | Meta panel + download. |

### Can you play videos and sound online?

**Yes.** OwnNAS plays them in the browser preview:

- **Video** extensions treated as video: `mp4`, `m4v`, `webm`, `mkv`, `mov`, `avi`, `ogv`, `mpeg`, `mpg`, `wmv`.
- **Audio** extensions treated as audio: `mp3`, `wav`, `flac`, `ogg`, `opus`, `m4a`, `aac`, `wma`.

Playback uses the **browser’s native codecs**, not a server transcoder. In practice:

- **Usually fine:** MP4/H.264 + AAC, WebM, MP3, WAV, many M4A/AAC, Opus/Ogg (browser-dependent).
- **Often unreliable:** MKV, AVI, WMV, some MOV codecs, FLAC (Safari), exotic containers.

If a format will not play, download it and open it in a desktop player. Range requests for seeking depend on the browser and how the raw endpoint is served; basic play/pause and timeline work for typical progressive media.

---

## Tags and comments

Each file/folder path can have:

- **Tags** — shared labels; searchable/filterable in the current folder and via search.
- **Comments** — threaded notes with author + time.

Metadata is stored in SQLite keyed primarily by **path**, with a **size + mtime fingerprint**:

- Notes follow OwnNAS rename / move / trash / restore / copy when paths are rewritten.
- If a file changes on disk outside OwnNAS, opening it can show a “file changed” warning so you can acknowledge and keep notes attached to the new fingerprint.

Tags and comments are edited in the preview panel (hidden in `--readonly`).

---

## Uploads and conflicts

- Toolbar: upload files or an entire folder.
- Drag-and-drop onto the file area.
- Paste files from the OS clipboard (when not editing a sensitive field).

Conflict policy (per file or “apply to rest”):

| Choice | Effect |
| --- | --- |
| Overwrite | Replace the existing file. |
| Keep both | Save under a unique name. |
| Archive older | Keep the newer of the two in place; archive the older into `.ownnas-archive`. |
| Archive existing | Move the on-disk file to Archive; keep the upload. |
| Ignore | Skip this upload. |
| Cancel | Abort the batch. |

Uploads show a status panel with progress and cancel.

---

## Downloads

- Single file: preview Download, or selection Download.
- Multiple items / folders: zipped via `/api/download` or folder zip via `/api/zip`.
- Progress + cancel in the status panel.

---

## Library tools

| Tool | What it does |
| --- | --- |
| **Bookmarks** | Pin folders; toggle from the toolbar. |
| **Recent** | Files you opened (not from Trash). |
| **Search** | Name (and tag) search under the current path. |
| **Folder size** | Recursive usage for the current folder. |
| **Activity** | Recent server-side actions (login, mkdir, move, upload, …). |
| **SHA-256** | Hash the current preview file (size-capped). |

---

## File operations (when not readonly)

| Action | Notes |
| --- | --- |
| New folder | Toolbar or empty-space menu. |
| Rename / Move / Copy / Duplicate | Context menu or preview actions. |
| Cut / Paste | Same-directory cut is a no-op paste (“already here”). |
| Trash / Restore / Empty Trash | Soft delete into `.ownnas-trash`. |
| New folder with selection | Mkdir + move selection into it. |

Paths never escape `--root` (traversal, absolute paths, and drive letters are rejected).

---

## Security notes

- Session cookie after login; mutating requests require the `X-OwnNAS: 1` header (CSRF-style guard).
- Passwords hashed with argon2.
- Active content (SVG/HTML/JS, …) is served carefully for preview (e.g. text/plain where needed) to reduce script execution risk.
- Prefer VPN + HTTPS in any exposure beyond localhost.
- Do not commit real passwords in `serve.sh` / `serve.bat` to a public remote.

---

## Data layout

```
ownnas-data/
  ownnas.db          # users, sessions, bookmarks, recents, events, tags, comments, fingerprints
  thumbs/
    <aa>/
      <sha256>.png   # cached thumbnails
```

Inside `--root` (managed by OwnNAS):

```
.ownnas-trash/       # trash + restore metadata
.ownnas-archive/     # archived upload conflict files
```

---

## Settings

Click **Settings** in the header.

### Appearance

Pick a theme inspired by [Omarchy](https://omarchy.org) palettes (Tokyo Night, Catppuccin, Everforest, Gruvbox, Kanagawa, Osaka Jade, Matte Black, Rose Pine, plus the OwnNAS default). The choice is stored in this browser (`localStorage`).

### Users (administrators only)

The first account created on a server is an **administrator**. Admins can:

- List accounts
- Create users (optionally as admin)
- Reset passwords
- Grant / revoke admin
- Remove accounts (not your own; not the last admin)

Existing databases are upgraded automatically: if no admin exists, the oldest account is promoted.

---

## Keyboard shortcuts (browser)

| Shortcut | Action |
| --- | --- |
| Ctrl/Cmd+A | Select all in folder |
| Ctrl/Cmd+C / X / V | Copy / cut / paste (or OS file paste) |
| Delete | Trash / delete selection |
| Enter | Open selection |
| Arrows (+ Shift) | Move / extend selection |
| Escape | Close overlays; clear selection |

---

## Dependencies worth knowing

- **Rust toolchain + C compiler** to build (SQLite is compiled in).
- **ffmpeg** optional, for video thumbnails only — not required for video/audio *playback*.
- Modern browser with HTML5 media support for in-page play.

---

## Related

- [README.md](README.md) — build, first run, background service samples (launchd / systemd / Windows).
