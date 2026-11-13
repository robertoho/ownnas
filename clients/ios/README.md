# OwnNAS for iPhone and iPad

A native iOS app (SwiftUI) for OwnNAS servers. Like the Android app, it runs the shared Rust
client library: `crates/ownnas-core` does the networking, the terminal emulator, and sync, and
`crates/ownnas-ffi` exposes it to Swift through [UniFFI](https://mozilla.github.io/uniffi-rs/).

What works now:

- **Servers:** several servers, each signed in with a token kept in the Keychain (this device
  only). HTTPS with a private certificate works too. The app shows the certificate's SHA-256
  fingerprint to compare with the one `ownnas serve` prints, or takes the `ca.crt` from
  `ownnas tls generate`.
- **Folders:**
  - Folders with thumbnails, pull to refresh, and badges on folders with open terminals.
  - Layouts, as in the web app (the view button in the toolbar): List, Icons (square tiles, small
    to large), Masonry (photos keep their shape), and Columns (the folders above as columns
    beside this one). Columns need a window at least 600 points wide (any iPhone on its side, an
    iPad, or a wider or folding iPhone), measured from the window rather than the model. The
    layout is remembered separately for narrow and wide windows, so turning the phone switches by
    itself. Selecting, menus, and actions work in each layout.
  - Open a file in Quick Look, or share it, with download progress.
- **Search:** names, tags, and the words inside documents, PDFs, and pictures, in a folder and
  below.
- **Uploads:**
  - Upload photos and videos (the photo picker) or any files (the Files picker). This uses the
    web app's endpoint.
  - When a name is taken, the app asks with the same choices: replace, keep both, keep the
    newer one, archive the existing one, or skip.
  - **Scan documents:** the system document camera finds the page's edges, takes the photo when
    the page is steady and in focus, straightens it, and offers more pages, retakes, cropping,
    and filters (colour, greyscale, black and white, photo). The scan is then named and
    uploaded as one PDF or a JPEG per page. PDFs carry the text Vision reads on each page as an
    invisible layer, so they can be searched (on the server too) and their text selected.
    Debug builds on the Simulator, which has no camera, scan two sample pages instead.
- **Changing files:** new folder, rename, and move to the Trash. Servers started with
  `--readonly` show no write actions.
- **Selecting several:** "Select" in the ⋯ menu (or in a row's long-press menu), then tap rows
  or Select All. The bottom bar has Share, Move to…, Copy to…, and Trash (Restore and Delete in
  the Trash), and More has Rename, Duplicate, Tag…, Compress to ZIP, Extract (an archive), and
  New folder with the selection. Move and copy pick the destination in a folder browser that
  can make new folders; when a name is taken there the server keeps both. A row's long-press
  menu has the same actions for that item alone.
- **More actions, as in the web app:** Cut or Copy, then "Paste here" in another folder;
  Protect from deletion and Allow deletion; Compress to ZIP, TAR.GZ, TAR.XZ, or TAR, and Extract
  here; photos: Convert to (JPEG, PNG, WebP, GIF, BMP, TIFF) and Resize (Small to 4K, half,
  quarter; as copies or in place); PDFs: Join, Extract pages, Split into pages, Rotate pages;
  Versions (restore or delete an earlier version); Add to album. Inside the Trash, the ⋯ menu
  empties it.
- **Videos and folders:** Convert video (format, size, speed, sound), followed on the server with
  progress and Stop. The ⋯ menu adds New file (text, Markdown, Word, spreadsheet, and more),
  Folder size, Download as ZIP (to share), Bookmark this folder and Bookmarks…, and Show hidden
  files.
- **Folder tools:** Find duplicates (keeps the oldest copy of each by default; untick to keep
  more, then move the rest to the Trash), Sort into folders (by date, kind, extension, first
  letter, or your own pattern, with a preview and Undo), and Activity.
- **Gallery:**
  - Photos by day, with On this day above them.
  - Albums inside albums, people (when the server has faces on), places on a map, and tags.
  - Search by description and "Find similar" (when smart search is on).
  - The title chooses a workspace (Family, Work…), and the app remembers it for each server.
  - Select photos to add them to an album, tag them, hide them from Photos, or delete them from
    the server.
  - Photos open full screen: swipe between them, pinch or double-tap to zoom, see their
    details, and share them. Videos play.
- **Photo backup:** backs up the camera roll (originals, newest first, iCloud photos too) to the
  server's media upload folder, in `<folder>/<this phone>/<year>/<month>/`, like the Android app.
  Wi-Fi only, videos or not, pause and resume, statistics, the photos sent lately, and the photos
  deleted on the server (which are not sent again unless allowed). It runs when the app opens,
  when new photos appear while it is open, with Back up now, and when iOS gives the app
  background time, which it decides (usually at night while charging).
- **Files app:** each signed-in server is a location in the iOS Files app (and in other apps'
  Open and Save dialogs). The first time, Files asks to turn it on. Folders are listed from the
  server when opened, files download when opened, and new files and folders, edits, renames,
  moves, and deletions go to the server. "Delete Now" in Files moves to the server's Trash.
  Thumbnails come from the server. The app and its extension share the server list through an
  App Group (`OWNNAS_APP_GROUP` in `project.yml`) and the sign-in tokens through a shared
  Keychain group, so signing in to your own build needs those two capabilities on your team,
  which automatic signing adds.
- **Terminals:**
  - For administrators when the server has them on. The list shows what each coding agent is
    doing.
  - A terminal opens in a native view with the web app's extra keys (Esc, Tab, sticky Ctrl and
    Alt, arrows, Home/End, PgUp/PgDn, ^C…). A hardware keyboard works too.
  - Drag to scroll back. The menu pastes and sets the text size.
  - "Insert a file from the server…" in the menu picks files (or a folder) on the server and
    types their paths as the shell sees them (relative to the folder it is in now), to point an
    agent at them.
  - "Open terminal here" and "Open Claude Code here" (and other agents an administrator allowed
    in the web app's Settings → Terminal → Agents) in a folder's menus start the agent in a new
    terminal in that folder.

Not yet, compared with Android:

- two-way folder sync;
- terminal shortcuts.

## Build

You need:

- Xcode 16 or newer;
- [XcodeGen](https://github.com/yonaskolb/XcodeGen) (`brew install xcodegen`);
- Rust from rustup. The build script adds the `aarch64-apple-ios` and `aarch64-apple-ios-sim`
  targets.

From this folder:

```bash
./build-rust.sh      # the Rust library as Generated/OwnNASCore.xcframework, and its Swift bindings
xcodegen             # OwnNAS.xcodeproj from project.yml
open OwnNAS.xcodeproj
```

Choose your team under Signing & Capabilities, then press Run.

- **Rebuilding the Rust library:** Xcode runs `build-rust.sh` before every build, so Rust
  changes are picked up. Cargo skips the work when nothing changed. Release builds use
  `--release`.
- **Where Xcode finds cargo:** it starts scripts with a bare PATH, so the script looks for
  cargo in Homebrew's rustup, `/usr/local/opt/rustup`, and `~/.cargo/bin`.
- **Simulators:** only Apple silicon simulators are built.
- **Editing the project:** change `project.yml`, not the `.xcodeproj`, and run `xcodegen`
  again.

## Trying it against a local server

The simulator shares your Mac's network, so a server started with
`--addr 127.0.0.1:8787` is `127.0.0.1:8787` in the app. On an iPhone, use the Mac's LAN or VPN
address. iOS asks once for permission to use the local network.

`OwnNASUITests` walks through the app against a running server and saves a screenshot of each
screen. The library needs a `Docs` folder with a file that contains "hello", and some
geotagged photos.

```bash
TEST_RUNNER_OWNNAS_SERVER=127.0.0.1:8787 TEST_RUNNER_OWNNAS_USER=admin \
TEST_RUNNER_OWNNAS_PASSWORD='…' TEST_RUNNER_OWNNAS_SHOTS=/tmp/shots \
xcodebuild test -project OwnNAS.xcodeproj -scheme OwnNAS \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' CODE_SIGN_IDENTITY=-
```

Without `TEST_RUNNER_OWNNAS_SERVER`, the test is skipped.
