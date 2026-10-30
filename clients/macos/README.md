# OwnNAS for the Mac

A menu bar app that puts each OwnNAS server you sign in to in Finder's sidebar, as Dropbox and
OneDrive do. Files show without being downloaded ("online-only") and download when you open
them. Finder's **Remove Download** frees the space again, and what you add, change, rename or
delete there goes back to the server (deleting moves to the server's Trash).

The Finder part is a File Provider extension, the same code as the iPhone app's Files location
(`../ios/OwnNASFiles`). Changes made on the server, whether through OwnNAS or on its disk
directly, reach Finder within seconds: the app follows the server's change feed
(`/api/sync/changes`) and tells Finder, and the extension reads what changed.

## Building

Needs Xcode, XcodeGen (`brew install xcodegen`) and Rust (rustup).

```bash
cd clients/macos
xcodegen
xcodebuild -project OwnNAS.xcodeproj -scheme OwnNAS -configuration Release -derivedDataPath build/dd build
```

Xcode runs `build-rust.sh`, which builds the shared Rust library for Apple silicon and Intel
Macs and its Swift bindings. The app needs macOS 15 or later.

It is signed with the Developer ID of team K27HXR2Q62 (see `project.yml`). Its App Group starts
with the team ID, so no provisioning profile is needed. To build under another team, change
`DEVELOPMENT_TEAM` and `OWNNAS_APP_GROUP`.

### A release

`./release.sh` builds `build/dist/OwnNAS-<version>-mac.dmg`, a disk image for the releases page.
It holds the app for Apple silicon and Intel, notarized and stapled, so it opens on any Mac
without warnings. The version comes from `MARKETING_VERSION` in `project.yml`.

Notarizing needs the team's Apple credentials in this Mac's keychain. Store them once:

```bash
xcrun notarytool store-credentials ownnas-notary --apple-id <Apple ID> --team-id K27HXR2Q62
```

This asks for an app-specific password, made at account.apple.com.

The icon is drawn in `icon/AppIcon.svg`. `icon/AppIcon-small.svg` is the same with heavier
lines, for the 16 and 32 pixel sizes. To change the icon, render the PNGs in
`OwnNAS/Assets.xcassets/AppIcon.appiconset` from them, for example with `rsvg-convert -w 512`.

## The first time

1. Open OwnNAS: its icon is in the menu bar, and a window asks for the server, username and
   password.
2. macOS keeps a new app's place in Finder off until you allow it. If the menu says so, choose
   **Allow in System Settings…** and turn OwnNAS on under File Providers. If you cannot find it
   there, run `pluginkit -e use -i app.ownnas.mac.files` in Terminal.
3. **Open at login** in the menu keeps it running, so changes on the server keep coming.

## Notes

- The sign-in token stays in your login Keychain. The extension gets it from the app with the
  place in Finder, because Developer ID apps cannot share Keychain items without a provisioning
  profile.
- Debug builds can sign in without the window, for tests: launch with `OWNNAS_TEST_ADDRESS`,
  `OWNNAS_TEST_USER` and `OWNNAS_TEST_PASSWORD` set (`open --env …`).
