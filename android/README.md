# OwnNAS Android

This is a thin Android host for the existing Rust OwnNAS server. The app loads
the same embedded HTML/CSS/JavaScript UI in a WebView; the Rust `cdylib` owns
the HTTP server, SQLite database, authentication, thumbnails, previews, and
file operations.

## Build

Open this `android/` folder in Android Studio, or run:

```powershell
.\gradlew.bat assembleDebug
```

For command-line builds, configure the Android SDK first with `ANDROID_HOME`
and `ANDROID_SDK_ROOT` (or let Android Studio create `local.properties`).

The Gradle build compiles the Rust library for `armeabi-v7a`, `arm64-v8a`, and
`x86_64` using the Android NDK, then packages each `libownnas.so` into the APK.
The first build may install the three Rust Android targets with `rustup`.

The app uses an app-private default folder under `getExternalFilesDir()` and a
private data directory for `ownnas.db` and caches. The Rust server binds only
to `127.0.0.1:8787`; it is not exposed to the LAN by the Android build.

## Limitations

Android’s Storage Access Framework returns document URIs rather than normal
filesystem paths. This first Android host therefore uses a real filesystem
path supplied in the setup screen; the default folder is the app-private
OwnNAS folder. File uploads from the WebView use Android’s document picker.
