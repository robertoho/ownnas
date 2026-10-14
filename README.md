# OwnNAS

OwnNAS is a small web server you compile yourself. It shares one folder and its subfolders on your own network or VPN, with a username and password, file previews, and thumbnails. Accounts and sessions live in a SQLite file next to a thumbnail cache. The files themselves stay in the folder you choose.

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

Video thumbnails use `ffmpeg` when it is on `PATH`. Content search OCR for images uses `tesseract` when it is on `PATH`. Pictures, SVG, PDF, audio, video playback, text, CSV, JSON, zip, and tar (including `.tar.gz`) preview without either.

Put OwnNAS behind HTTPS and pass `--secure-cookie` if you terminate TLS with a reverse proxy. The login is the application boundary; the VPN is the network boundary.

## Updates

New installations check the signed GitHub release feed once per day. Administrators see an available version in **Settings → Updates** and choose when to install it. `--update-url` and `--update-pubkey` still override the default feed for custom installations. Service-managed installs should pass `--restart-by-supervisor` so the service manager starts the replaced binary after a clean shutdown. Existing installations must first install a release that includes the updater. The release workflow requires the `OWNNAS_UPDATE_PRIVATE_KEY` Actions secret; its matching public key is embedded in the application.

OwnNAS also includes a basic embedded ODT editor. Open an `.odt` file or create one from **New file → OpenDocument text**. Its browser module can be reused in other projects; see [web/odt-editor.md](web/odt-editor.md).

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
