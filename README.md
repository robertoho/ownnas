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

## Run

```bash
ownnas serve --root /path/to/library --username admin --password "a long passphrase"
```

On Windows PowerShell:

```powershell
.\target\release\ownnas.exe serve --root D:\media --username admin --password "a long passphrase"
```

Then open `http://127.0.0.1:8787`. From another device on the VPN, open `http://<this-computer-vpn-address>:8787`.

The library page can bookmark folders, remember recently opened files, search by name, measure a folder, download the current folder as a zip, and show an activity log. A file preview can move, duplicate, hash, step through images, and attach shared tags and comments (keyed by path, with size/mtime fingerprints so OwnNAS can warn when a file changed without dropping the notes; metadata follows OwnNAS rename/move/trash/restore).

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
ExecStart=/absolute/path/to/ownnas serve --root /absolute/path/to/library --data /absolute/path/to/ownnas-data --addr 0.0.0.0:8787
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
   `C:\path\to\ownnas.exe serve --root D:\media --data C:\ownnas-data --addr 0.0.0.0:8787`
2. **NSSM** — wrap the same command as a Windows service (`nssm install OwnNAS`).

Point the task or service at the release binary with absolute paths. Prefer machine start only after the account already exists in `ownnas-data`.

Video thumbnails use `ffmpeg` when it is on `PATH`. Pictures, SVG, PDF, audio, video playback, text, CSV, JSON, zip, and tar (including `.tar.gz`) preview without it.

Put OwnNAS behind HTTPS and pass `--secure-cookie` if you terminate TLS with a reverse proxy. The login is the application boundary; the VPN is the network boundary.
