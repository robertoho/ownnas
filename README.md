# OwnNAS

OwnNAS is a small web server you compile yourself. It shares one folder and its subfolders on your own network or VPN, with a username and password, file previews, and thumbnails. Accounts and sessions live in a SQLite file next to a thumbnail cache. The files themselves stay in the folder you choose.

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

The library page can bookmark folders, remember recently opened files, search by name, measure a folder, download the current folder as a zip, and show an activity log. A file preview can move, duplicate, hash, and step through images.

The database and thumbnail cache default to `./ownnas-data/ownnas.db`. That folder must sit outside `--root`.

```bash
ownnas user add --username alice --password "another passphrase"
ownnas user list
ownnas user password --username alice --password "a replacement passphrase"
ownnas user remove --username alice
ownnas serve --root D:\media --readonly
ownnas serve --root D:\media --addr 0.0.0.0:8787 --open
```

`--username` on `serve` is used only when the database has no accounts yet.

Video thumbnails use `ffmpeg` when it is on `PATH`. Pictures, SVG, PDF, audio, video playback, text, CSV, JSON, zip, and tar (including `.tar.gz`) preview without it.

Put OwnNAS behind HTTPS and pass `--secure-cookie` if you terminate TLS with a reverse proxy. The login is the application boundary; the VPN is the network boundary.
