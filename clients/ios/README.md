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
  - Open a file in Quick Look, or share it, with download progress.
- **Search:** names, tags, and the words inside documents, PDFs, and pictures, in a folder and
  below.
- **Uploads:**
  - Upload photos and videos (the photo picker) or any files (the Files picker). This uses the
    web app's endpoint.
  - When a name is taken, the app asks with the same choices: replace, keep both, keep the
    newer one, archive the existing one, or skip.
- **Changing files:** new folder, rename, and move to the Trash. Servers started with
  `--readonly` show no write actions.
- **Gallery:**
  - Photos by day, with On this day above them.
  - Albums inside albums, people (when the server has faces on), places on a map, and tags.
  - Search by description and "Find similar" (when smart search is on).
  - The title chooses a workspace (Family, Work…), and the app remembers it for each server.
  - Select photos to add them to an album, tag them, hide them from Photos, or delete them from
    the server.
  - Photos open full screen: swipe between them, pinch or double-tap to zoom, see their
    details, and share them. Videos play.
- **Terminals:**
  - For administrators when the server has them on. The list shows what each coding agent is
    doing.
  - A terminal opens in a native view with the web app's extra keys (Esc, Tab, sticky Ctrl and
    Alt, arrows, Home/End, PgUp/PgDn, ^C…). A hardware keyboard works too.
  - Drag to scroll back. The menu pastes and sets the text size.

Not yet, compared with Android:

- the Files app integration (a File Provider extension);
- phone backups;
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
