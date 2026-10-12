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
  Uploads stream from the picked file, show progress, can be cancelled, and never replace
  anything: a taken name becomes `name (2).ext`. The file keeps its modification time.
- New folder, rename, and move to the server's Trash (restorable from the web app).
  Servers started with `--readonly` show no write actions.

Uploads run while the app is open; background transfers come with sync. A file picker
provider and terminals come after that.

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
