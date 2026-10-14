# OwnNAS for Android

A native Android app (Kotlin, Jetpack Compose, Material 3) for OwnNAS servers. The networking
and, later, sync run in Rust: `crates/ownnas-core` is the client library shared with the
`ownnas-sync` CLI, and `crates/ownnas-ffi` exposes it to Kotlin through
[UniFFI](https://mozilla.github.io/uniffi-rs/).

What works now:

- Several servers, each signed in with a token encrypted by an Android Keystore key.
- Folders with thumbnails and pull to refresh.
- Open a file in the app that handles its type, or share it, with download progress.
- Upload photos and videos (the system photo picker) or any files (the system file picker).
  Uploads use the web app's endpoint (`POST /api/upload`): when a name is taken the app asks
  with the same choices (overwrite, keep both, archive older, archive existing, ignore), and
  the activity log records them the same way. Files stream from the picker, show progress,
  and can be cancelled.
- New folder, rename, and move to the server's Trash (restorable from the web app).
  Servers started with `--readonly` show no write actions.

- Each signed-in server appears in the system file picker and the Files app, so any app can
  open, attach, save into, rename, and delete files there (`data/OwnNasDocuments.kt`).

- Terminals, for administrators when the server has them turned on: the list shows each
  terminal's coding-agent status, and a terminal opens in a native view with the web app's
  extra keys (Esc, Tab, Ctrl, Alt, arrows, …). Drag to scroll back, pinch to resize the text.
  Pasting a copied image or file, inserting one from the keyboard, or attaching one uploads
  it to the terminal's temporary folder and pastes its path, as the web terminal does. The
  terminal button shows how many terminals want attention, and folders with open terminals
  carry a badge in their agent's status color.
  The emulator and the WebSocket run in Rust (`crates/ownnas-core/src/terminal.rs`, using
  the `vt100` crate), so the iOS app can share them.

- Shortcuts tab: terminal shortcuts open a terminal in a server folder and run a command
  there (such as `tmux new -A -s main`), going back to the same terminal while it runs; and
  snippets, saved commands to run from the ⚡ button in any terminal.
- HTTPS with a private certificate: when a server's certificate is not publicly trusted, the
  app shows its SHA-256 fingerprint to compare with the one `ownnas serve` prints, and pins it
  once you confirm, or trusts the `ca.crt` from `ownnas tls generate` (which survives
  certificate renewals). Trust is stored per server and also covers terminals and the
  file picker.

- Two-way folder sync: from a server folder's menu, “Sync with this phone…” pairs it with a
  folder on the phone (DCIM, Documents, an SD card…). It runs the same Rust engine as
  `ownnas-sync`: deletions go to the server's Trash or to `.ownnas-sync/deleted` on the phone
  for 30 days, and edits on both sides keep both copies. Syncs run every 15 minutes with
  WorkManager while there is a network, and on demand from the Sync screen. Sync needs
  “All files access”, which the app asks for when you set one up.

Uploads from the + button run while the app is open.

## Build

Needs:

- Rust from rustup with the Android targets, and cargo-ndk:
  ```bash
  rustup target add aarch64-linux-android x86_64-linux-android
  cargo install cargo-ndk
  ```
- The Android SDK with NDK `29.0.14206865` (Android Studio → SDK Manager → SDK Tools).
- JDK 17 or 21 for Gradle. Android Studio's bundled JDK works:
  `export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"` on a Mac.

Then, from this folder:

```bash
./gradlew assembleDebug     # app/build/outputs/apk/debug/app-debug.apk
./gradlew installDebug      # onto the connected phone or emulator
```

Or open this folder in Android Studio and press Run.

Each build runs two tasks before compiling Kotlin:

- `cargoNdk<Variant>` builds `libownnas_ffi.so` for `arm64-v8a` and `x86_64` with cargo-ndk
  (`--release` for release builds). Cargo skips the work when nothing changed.
- `uniffiBindgen<Variant>` generates the Kotlin bindings (package `app.ownnas.core`) into
  `app/build/generated/uniffi/`.

Gradle finds `cargo` in `rust.bin` from `local.properties` if set, then in
`/opt/homebrew/opt/rustup/bin`, `/usr/local/opt/rustup/bin`, `~/.cargo/bin`, and `PATH`.
Android Studio started from the Dock does not see your shell's PATH, so set `rust.bin` if
cargo lives somewhere else.

## Trying it against a local server

The emulator reaches your computer at `10.0.2.2`, so a server started with
`--addr 127.0.0.1:8787` is `10.0.2.2:8787` in the app. For a phone on USB, forward the port
and use `localhost:8787`:

```bash
adb reverse tcp:8787 tcp:8787
```

On your own network, use the server's LAN or VPN address. Use `https://` for anything
reachable from outside it.

## Gallery

Each server has a Gallery (the picture button in a folder, or Gallery in a server's menu):
photos and videos by the day they were taken with a year scrubber on the right, nested albums,
people (when the server has faces switched on), tags, and search in words or "Find similar"
(when smart search is on). Long-press photos to add them to an album, tag them, hide them from
Photos, or (in a person) say they are not that person. Photos open full screen: swipe between
them, pinch or double-tap to zoom; videos open in the phone's player.

## Phone backups

"Back up this phone" (the cloud button on the main screen, or in the Gallery's menu) copies new
photos and videos to a server's media upload folder, which an administrator sets in the web app
(Settings → Gallery → Phone backups). Each phone gets its own folder, by year and month, and
photos keep their original dates. It runs hourly and soon after new photos appear (on Wi-Fi only,
unless switched off), never deletes anything, and skips what is already on the server.
