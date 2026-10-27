# OwnNAS

OwnNAS is a small web server you compile yourself. It shares one folder and its subfolders on your own network or VPN, with a username and password. Your files stay where they are; accounts, tags, and albums live in a SQLite file next to a thumbnail cache.

- **File browser:** upload, download, move, rename, a Trash, and earlier versions of saved files.
- **Previews:** photos (RAW, HEIC, Live Photos), video, music, PDFs, Office documents, e-books, 3D models, maps, and more. Text and spreadsheets can be edited in place.
- **Music player:** play a folder and its subfolders, or a playlist, in a bar that keeps playing while you browse, with a quick panel for the queue and the music played lately.
- **Search:** file names and the words inside documents and pictures, plus tags and comments.
- **Photo gallery:** a timeline, nested albums that never move your files, places on a map, and workspaces (Family, Work, a customer…).
- **Smart photos:** optional search by description, similar photos, and faces grouped into people, all on your own machine.
- **Terminals in any folder:** open a shell in any folder of the NAS from a browser or phone, with status for coding agents like Claude Code.
- **Library tools:** duplicate finder, folder sizes, and disk health and temperatures.
- **Backups:** encrypted, scheduled backups of the library and OwnNAS's data to a USB drive, an S3-compatible bucket, or another computer over SFTP, with restore from any snapshot.
- **Users:** several accounts, administrators, read-only mode, and an activity log.
- **Sync and apps:** a two-way folder sync client, and native Android and iPhone apps with the gallery, search, and terminals.
- **Easy to run:** one binary for Windows, macOS, and Linux, HTTPS, and signed updates.

For a full walkthrough (thumbnails, media playback, tags, Trash, keyboard shortcuts, and more), see [DOCS.md](DOCS.md).

## Build

Install a C compiler as well as Rust. SQLite is compiled into the binary.

- Windows: Visual Studio Build Tools, or Visual Studio with the C++ workload
- macOS: Xcode Command Line Tools (`xcode-select --install`)
- Linux: `build-essential` (Debian/Ubuntu) or the equivalent `gcc` package

```bash
cargo build --release
```

The binary is `target/release/ownnas` (`ownnas.exe` on Windows). Copy that file to the machine that will host the folder. Build on each operating system you want to run it on.

On Linux, `./build.sh` builds the release binary. `./serve.sh` starts it using
`$HOME/Pictures` as the shared folder, `$XDG_DATA_HOME/ownnas` (or
`$HOME/.local/share/ownnas`) for its database, and `0.0.0.0:8787` as the listen
address. Override those defaults with `OWNNAS_ROOT`, `OWNNAS_DATA`, and
`OWNNAS_ADDR`. The server prompts in the terminal to create the first account;
you can also set `OWNNAS_USER` and `OWNNAS_PASSWORD` in the environment.

## Run

```bash
ownnas serve --root /path/to/library --username admin --password "a long passphrase"
```

On Linux, the equivalent script command is `OWNNAS_ROOT=/path/to/library ./serve.sh`.

On Windows PowerShell:

```powershell
.\target\release\ownnas.exe serve --root D:\media --username admin --password "a long passphrase"
```

Then open `http://127.0.0.1:8787`. From another device on the VPN, open `http://<this-computer-vpn-address>:8787`.

The library page can bookmark folders, remember recently opened files, search by name and file contents (text, PDF text, and OCR’d images when `tesseract` is available), measure a folder, download the current folder as a zip, and show an activity log. A file preview can move, duplicate, hash, step through images, and attach shared tags and comments (keyed by path, with size/mtime fingerprints so OwnNAS can warn when a file changed without dropping the notes; metadata follows OwnNAS rename/move/trash/restore).

The database and thumbnail cache default to `./ownnas-data/ownnas.db`. That folder must sit outside `--root`.

```bash
ownnas user add --username alice --password "another passphrase"
ownnas user list
ownnas user password --username alice --password "a replacement passphrase"
ownnas user remove --username alice
ownnas serve --root D:\media --readonly
ownnas serve --root D:\media --addr 0.0.0.0:8787 --open
```

On the first start, when the database has no accounts, OwnNAS asks in the terminal for anything you left out: the shared folder, the username, and the password. Pass `--root`, `--username`, and `--password` to skip those prompts. `--username` is ignored once an account exists.

See `ownnas serve --help` for the full flag list and examples.

## Android

The `android/` project packages the same Rust server as a native Android
library and displays the existing OwnNAS web UI in a WebView. It binds only to
`127.0.0.1`, stores its database and caches in the app data directory, and
supports arm64, 32-bit ARM, and x86_64. Open `android/` in Android Studio or
run `android\\gradlew.bat assembleDebug` on Windows. See [android/README.md](android/README.md)
for the storage model and build details.

## iPhone and iPad

`clients/ios` is a native SwiftUI app on the same Rust client library as the Android app in
`clients/android`. It covers folders, search, uploads, the Gallery (workspaces, places, On this
day), and terminals. See [clients/ios/README.md](clients/ios/README.md) to build it with Xcode.

## Sync a folder with your computer

`ownnas-sync` is a separate command-line client (in `clients/desktop`) that keeps a local
folder and a folder on the server in sync in both directions. It builds on Windows, macOS,
and Linux:

```bash
cargo build --release -p ownnas-sync
ownnas-sync setup ~/OwnNAS --server https://nas.example:8787 --username alice --remote Documents
ownnas-sync run ~/OwnNAS --watch
```

`setup` signs in once and stores a token in `~/OwnNAS/.ownnas-sync/config.json`; the
password is not saved. Pass `--ca-cert ca.crt` when the server uses a certificate from
`ownnas tls generate`. `run` syncs once; `--watch` syncs again every 30 seconds
(`--interval`), and `--dry-run` only lists what would change.

- Files deleted on one side are not destroyed on the other. The server moves them to its
  Trash, and the client moves local files to `.ownnas-sync/deleted` for 30 days.
- When both sides changed the same file, the client keeps both copies. Your local edit is
  saved as `name (conflict <computer> <date>).ext` and uploaded next to the server's version.
- `.DS_Store`, `Thumbs.db`, `desktop.ini`, and Office/LibreOffice lock files are not synced.
- Against a `--readonly` server, the client only downloads.

The client uses the `/api/sync/*` endpoints, which native apps can use as well.
Authenticate with `POST /api/login` and `{"token": true}`, then send
`Authorization: Bearer <token>`. Tokens stay valid while in use.

## Terminals in the browser

Administrators can open a shell in any folder from the right-click menu (**Open terminal
here**), for example to run Claude Code, opencode, or other command-line agents on the
files. It is off by default: turn it on in **Settings → Terminal**.

- Terminals keep running when you close the tab or lose the connection. Reopen them from
  the right-click menu (**Terminals**) on any device; recent output (up to 2 MB) is replayed.
  **Pop out** opens one in its own browser tab, and phones get a row of Esc/Tab/Ctrl/arrow keys.
- Right-click (or long-press) a terminal tab to rename, duplicate, or close it, or to show its
  current folder in the file browser (**Show folder** does the same).
- The panel sits beside the files, with every terminal tiled in a grid; drag the edge between
  them to resize. **⇧⌘L** (Ctrl+Shift+L off a Mac) or **Ctrl+`** shows and hides it, **⇧⌘E**
  opens a terminal in the folder you are browsing, **⇧⌘ + arrows** move the keyboard between
  the files and the terminals, **⇧⌘U** hides a terminal (it keeps running; its tab brings it
  back), **⇧⌘X** closes it, **⇧⌘↵** expands it to fill the space (or the ⤢ on its tile; ✕
  hides it), **⌃1…⌃9** (Alt+1… off a Mac) jump to a numbered terminal, and **⇧⌘K** lists
  every keyboard shortcut. They work from
  inside a terminal too.
- On phones the panel keeps one row of tabs, with everything else under **⋯**, and follows
  the on-screen keyboard. The key row has Esc, Tab, Shift+Tab, sticky Ctrl and Alt (they also
  modify arrows, Home/End, and PgUp/PgDn), Paste (images too), **Select** (the output as text
  you can select and copy), text size, and a button that hides the keyboard. Add OwnNAS to
  your home screen to use it full screen without the browser's address bar.
- Drag files from the file list into a terminal to type their paths (relative to the folder
  the shell is in, when they are inside it).
- Paste or drop files (such as a screenshot) into a terminal to upload them and type their
  paths, as a local terminal would. They are kept in a temporary folder for that terminal and
  deleted when it closes or the server restarts.
- The shell runs as the account that runs OwnNAS, with its sign-ins and API keys, so sign
  in to your agents on that machine once. Consider running OwnNAS under a dedicated account.
- An optional terminal password is asked once per sign-in (for up to 12 hours). Changing
  any terminal setting requires your account password, and turning terminals off ends them all.
- Terminals are refused over plain HTTP unless you connect from this machine, your LAN,
  or a VPN address (such as Tailscale). Use HTTPS otherwise.
- The default shell is `$SHELL` (a login shell) on macOS and Linux and PowerShell on
  Windows; set another one in Settings. Not available with `--readonly` or on Android.

### Coding agents

Each terminal tab shows the agent running in it and what it is doing; **Overview** lists
all of them. The terminals button in the header shows a badge, the page title a count, and
(after **Notify me when an agent needs me**) the browser a notification when an agent needs
permission, asks a question, or finishes. **Edited** lists the files the agent changed and
**Changes** shows `git status` for the folder; both open files in OwnNAS.

- Without setup, OwnNAS recognizes Claude Code, opencode, Codex, Gemini CLI, Aider, and
  other agents from the terminal's foreground process (macOS and Linux), and treats the
  terminal bell and desktop-notification escape codes (OSC 9, 99, 777) as requests for
  attention.
- **Settings → Terminal → Agent integrations** installs a Claude Code hook (in
  `~/.claude/settings.json`, backed up first) or an opencode plugin
  (`~/.config/opencode/plugins/ownnas.js`) for step-by-step status. They report only from
  inside OwnNAS terminals, through a per-terminal token to a listener on `127.0.0.1`, and do
  nothing elsewhere. **Remove** takes them out again.

## Run in the background

OwnNAS stays in the foreground. Use the OS service manager so it survives logout and starts on boot. Pass absolute paths for the binary, `--root`, and `--data`. Create the account once interactively before installing the service.

### macOS (launchd)

Save `~/Library/LaunchAgents/com.ownnas.serve.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.ownnas.serve</string>
  <key>ProgramArguments</key>
  <array>
    <string>/absolute/path/to/ownnas</string>
    <string>serve</string>
    <string>--root</string><string>/absolute/path/to/library</string>
    <string>--data</string><string>/absolute/path/to/ownnas-data</string>
    <string>--addr</string><string>0.0.0.0:8787</string>
    <string>--restart-by-supervisor</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>WorkingDirectory</key><string>/absolute/path/to/ownnas-data/..</string>
  <key>StandardOutPath</key><string>/tmp/ownnas.log</string>
  <key>StandardErrorPath</key><string>/tmp/ownnas.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.ownnas.serve.plist
launchctl unload ~/Library/LaunchAgents/com.ownnas.serve.plist
```

### Linux (systemd)

Save `/etc/systemd/system/ownnas.service` (or `~/.config/systemd/user/ownnas.service` for a user unit):

```ini
[Unit]
Description=OwnNAS folder browser
After=network.target

[Service]
Type=simple
ExecStart=/absolute/path/to/ownnas serve --root /absolute/path/to/library --data /absolute/path/to/ownnas-data --addr 0.0.0.0:8787 --restart-by-supervisor
WorkingDirectory=/absolute/path/to/ownnas-data/..
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now ownnas
sudo systemctl status ownnas
```

### Windows

Easiest options:

1. **Task Scheduler** — create a task that runs at logon:
   `C:\path\to\ownnas.exe serve --root D:\media --data C:\ownnas-data --addr 0.0.0.0:8787 --restart-by-supervisor`
2. **NSSM** — wrap the same command as a Windows service (`nssm install OwnNAS`) and set its exit action to restart.

Point the task or service at the release binary with absolute paths. Prefer machine start only after the account already exists in `ownnas-data`.

Thumbnails are small JPEGs (PNG for pictures with transparency) kept in the data folder; the Gallery makes them in the background after each scan, newest photos first, so scrolling does not wait. Video thumbnails use `ffmpeg` when it is on `PATH`. HEIC and AVIF photos (from iPhones, for example) get thumbnails through `sips` on macOS, or `heif-convert` or `ffmpeg` elsewhere. Content search OCR for images uses `tesseract` when it is on `PATH`. Pictures, SVG, PDF, audio, video playback, text, CSV, JSON, zip, and tar (including `.tar.gz`) preview without either.

Put OwnNAS behind HTTPS and pass `--secure-cookie` if you terminate TLS with a reverse proxy. The login is the application boundary; the VPN is the network boundary.

## Backups

**Settings → Backup** (administrators) backs up the library and OwnNAS's own data with [restic](https://restic.net), which needs to be installed (`brew install restic`, `sudo apt install restic`, or `winget install restic.restic`; 0.17 or newer). The data includes the database with users, albums, tags, and settings, and the file history. Thumbnails and caches are left out, because OwnNAS can make them again.

- **Where:** a folder on another drive, an S3-compatible bucket (AWS, Backblaze B2, Cloudflare R2, Wasabi, MinIO…), or another computer over SFTP, signed in with this computer's SSH key. The page shows the key, or makes one.
- **Encrypted:** OwnNAS makes up a password for the backup and shows it once. Write it down somewhere else: without it the backup cannot be restored, also by you. Settings → Backup shows it again after you enter your account password.
- **When:** every day or every week at a time you choose, or only when you click Back up now. A backup missed while the computer was off runs when it is back. A failed one is tried again an hour later, up to three times. After the first backup, only what changed is sent.
- **How long:** one snapshot a day for 7 days, one a week for 4 weeks, and one a month for 12 months by default. Older ones are removed after each backup.
- **Checked:** once a month OwnNAS reads a sample of the backup back. Check the backup reads 5% on demand.
- **Restore:** pick a snapshot, open the folder you want, and restore it. It is copied into a new folder, `Restored from backup/<date> <name>`, in the library, so nothing is overwritten.
- **Warnings:** if a backup fails, or none has worked for two scheduled times in a row, administrators see it in the warning line above the files.

To bring back OwnNAS's database on a new computer, restore it with restic yourself:

```bash
restic -r <destination> restore latest --include '*/backup-staging/ownnas.db' --target /tmp/restore
```

It lands in `/tmp/restore` under the path it had, ending in `backup-staging/ownnas.db`. Put it into the new data folder before OwnNAS's first start there. The library restores the same way, or from the page once OwnNAS runs.

## Updates

New installations check the signed GitHub release feed once per day. When there is a new version, administrators see a small note in the status bar. Choosing it opens **Settings → Updates**, where they choose when to install it. The ✕ hides the note for a week, or until an even newer version comes out. `--update-url` and `--update-pubkey` still override the default feed for custom installations. Service-managed installs should pass `--restart-by-supervisor` so the service manager starts the replaced binary after a clean shutdown. Existing installations must first install a release that includes the updater. The release workflow requires the `OWNNAS_UPDATE_PRIVATE_KEY` Actions secret; its matching public key is embedded in the application.

OwnNAS also includes embedded ODT and XLSX editors. Open an `.odt` or `.xlsx` file, or create one from **New file**. The ODT editor edits the page directly, with formatting, headings and lists; the spreadsheet editor shows number formats, colours and merged cells, and recalculates common formulas as you type. The ODT module can be reused in other projects; see [web/odt-editor.md](web/odt-editor.md).


The **Gallery** (the picture button at the top, or #gallery) shows every photo and video in the library by the day it was taken, newest first, with albums and tags. Albums are virtual: they list photos wherever they are, without moving, copying, or changing them, and albums can hold other albums, like folders. Add photos from the Gallery (select them, then Add to album…), from the file browser (right-click → Add to album…), or from the preview. Two optional smart features run entirely on the server (Settings → Gallery switches them on and downloads their models once): **search by what photos show** — type words like “dog on the beach” in the Gallery’s search box, or drop a picture on it or choose Find similar photos to see photos that look alike (CLIP ViT-B/32, about 155 MB) — and **faces and people**, which finds faces, groups them into people you can name, merge (Same person as…), hide, or correct (Not this person), in a People tab (YuNet and SFace from the OpenCV model zoo, about 39 MB). Face data stays in OwnNAS’s database and can be deleted from Settings at any time. Both work through the library at the indexing speed you choose; not available in the Android build. Photos you would rather not see in the timeline can be hidden from it (right-click → Hide from Photos); they stay in their folders, albums, and tags, and Hidden in the Photos header lists them to bring back. Albums and tags are shared by everyone on the server, and they keep up when files are moved, renamed, or put in the Trash through OwnNAS. The timeline comes from an index OwnNAS keeps in its own database (dates from the photos' EXIF, or the file date); it is refreshed at start, every 30 minutes, and with Rescan (Settings → Gallery sets how hard this may work the computer: Slow, Medium, the default, or Fast), and it skips hidden folders, the Trash, and Apple Photos libraries.

**Workspaces** keep groups of photos apart in the Gallery — Family, Work, Customer 1, and so on. A workspace is a name and some library folders; the switcher at the top left of the Gallery shows one workspace at a time (or All photos), and Photos, Albums, People, Places, Tags, search, Find similar photos, and On this day then show only the photos and videos in its folders. Each album belongs to one workspace (or to none, shown only under All photos), and can be moved to another with Move to workspace…. Administrators create, edit, and delete workspaces from the switcher's menu; deleting one keeps its albums and never touches photos or folders. The Gallery remembers the workspace last shown, and its address names it (`#gallery/w3/albums`, `#gallery/all`).

SQLite databases (`.db`, `.sqlite`, `.sqlite3`, `.db3`) open in a read-only viewer: pick a table or view to page through its rows (sorted by any column and filtered by text in any column), see its columns, `CREATE` statement, and indexes, or run your own `SELECT` queries (up to 1,000 rows, stopped after 10 seconds). The database is opened read-only, so the viewer never changes or locks it for the program that uses it; long text and blobs can be opened or downloaded whole.
## Native HTTPS with your own certificates

OwnNAS can serve HTTPS directly without a proxy. Generate a private CA and a
server certificate on the machine hosting OwnNAS:

```powershell
.\ownnas.exe tls generate --out ownnas-tls --host localhost --host 127.0.0.1 --host 192.168.1.50 --host nas.example.com
.\ownnas.exe serve --root D:\media --addr 0.0.0.0:8787 --tls-cert ownnas-tls/server.crt --tls-key ownnas-tls/server.key
```

Replace the example IP and domain with the addresses users will actually enter.
Open `https://192.168.1.50:8787` or `https://nas.example.com:8787`. Include
`127.0.0.1` if using `--open`. DNS names must resolve to the server. HTTPS enables
Secure login cookies automatically. Without the TLS options, OwnNAS continues to
serve HTTP. You can also supply an existing PEM certificate chain and matching
unencrypted PEM private key. Both TLS options are required together; invalid
certificates or keys prevent HTTPS startup rather than falling back to HTTP.

At startup `ownnas serve` prints `HTTPS certificate SHA-256: …`. The Android app shows the same
fingerprint the first time it connects, and trusts the server once you confirm it matches, or
when you pick `ca.crt` instead.

Distribute **only `ca.crt` or `ca.cer`** to users, through a trusted channel. Never
include `ca.key` or `server.key` in downloads or FTP uploads. Each installation
should have its own private keys. Keep the certificate directory outside the shared folder. Restrict access to the key files on Windows
using the folder's Security permissions. On Unix, generated keys have mode 0600.

On Windows, double-click `ca.cer`, choose **Install Certificate**, select
**Current User**, and place it in **Trusted Root Certification Authorities**.
Alternatively, from a terminal running as that user:

```powershell
certutil -user -addstore Root ownnas-tls\ca.cer
```

On macOS, import `ca.crt` into Keychain Access and set its SSL trust to Always
Trust. On iPhone/iPad, install the certificate profile, then enable full trust
under Settings → General → About → Certificate Trust Settings. Android allows
installing a CA certificate under security settings; individual apps may have
their own trust policy. Browsers with their own certificate store may require a
separate import. Installing a private CA allows it to authenticate HTTPS servers;
users should install only a CA they recognize and trust.

Generated server certificates last one year; the CA lasts ten years. The
`tls generate` command refuses to overwrite an existing directory. To replace
an expired server certificate, issue a new certificate using the saved CA key
and the same DNS/IP names, or generate a new directory and have users install
its new CA. Restart OwnNAS after replacing its certificate/key files.

The shared folder is saved in the database after setup. Later starts reuse it
when `--root` is omitted. Use `start.bat --root "D:\media"` in the Windows
package to change the saved folder. Older installations without a saved folder
ask for it once on the next interactive start.

In Icons and Masonry views, use the bottom status bar Size slider to resize tiles. The
selected size is remembered in the browser and also applies on mobile.

On macOS, Command+Up opens the parent folder and Command+Down opens the
selected folder or previews the selected file. These shortcuts leave text
inputs, editors, and dialogs alone.

Explorer browsing additions:

- Type while the file browser is focused to move into Search and filter names
  immediately. Text inputs, dialogs, previews, and modifier shortcuts keep
  their usual behavior. Press Enter in Search for recursive/content results.
- Details view has sortable Name, Size, Type, Modified, and Created columns.
  Drag column separators to resize, or focus a separator and use Left/Right.
  Column widths and sorting are remembered. On narrow screens, scroll sideways.
- Group files by Type, Date, or Tags using the Group selector. Click a group
  heading to collapse or expand it. A file with multiple tags appears in each
  tag group; selecting it selects the same file throughout.
- 3D thumbnails render automatically for visible supported models, including
  search results. They use the same loaders as the 3D preview, with a neutral
  background and lighting. Rendering runs one file at a time. A browser cache
  stores up to 256 thumbnails, keyed by library, user, path, modification time,
  file size, and thumbnail version. A different browser generates its own cache.
  The first generation downloads the model. Files over 80 MB or models over
  two million triangles keep their icon. CAD conversion has a 30-second budget.
  Broken/unsupported models keep their icon; hover for the reason. GLTF buffers
  and textures may reference sibling files inside the library. Remote external
  resources are not loaded. WebGL is required for thumbnail rendering.

Column view keeps each folder level visible beside its children. Select a folder to open the next column. Drag files onto folders, columns, or breadcrumb destinations to move them; hold Option/Alt or Ctrl to copy. Desktop file drops still upload to the current folder.

The bottom toolbar includes Undo (Ctrl/Cmd+Z) for rename, move, and sending items to Trash. History lasts for the current signed-in page session, up to 100 individual items; a multi-item operation is undone one item at a time. Undo refuses an occupied original location and remains available for retry. Copy, upload, edits, and permanent deletion are not currently undoable.
