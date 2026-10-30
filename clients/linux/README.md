# OwnNAS on Linux (`ownnas-mount`)

Each server you sign in to becomes a folder, `~/OwnNAS/<name>`. It lists everything the server
has, but files take no space until you open them. Opened files stay on this computer, in
`~/.cache/ownnas`, up to a size limit (10 GB by default). Past that, the files used least
recently are dropped and download again when opened. Changes go both ways within seconds, as
with the Mac app and the Windows client.

It is a FUSE filesystem, served by `ownnas-mount run` as a systemd user service. It needs
`fusermount3`, which most desktops already have (Debian and Ubuntu: the `fuse3` package).

## Set up

```sh
cargo build --release -p ownnas-mount
target/release/ownnas-mount setup --server https://nas.example:8787 --username ana
```

`setup` does the following:

- Signs in.
- Saves the server in `~/.config/ownnas/mount.json`, which only you can read.
- Adds the folder to the sidebar of GTK file managers (Files, Nemo, Caja, Thunar).
- Installs and starts `ownnas-mount.service`, so the folder is mounted now and at each login.

`setup` options:

- `--folder`: mount somewhere else.
- `--cache-gb`: change the cache size limit.
- `--ca-cert` / `--pin`: trust a server with its own certificate, as with `ownnas-sync`.
- `--no-service`: run `ownnas-mount run` yourself instead of installing the service.

Other commands:

- `ownnas-mount status`: the servers, how much each takes on this computer, and changes not sent yet.
- `ownnas-mount remove <server or name>`: unmount a server, sign out and empty its cache. It
  refuses while changes made here haven't reached the server.
- The log is in `~/.local/state/ownnas/mount.log`, and also in `journalctl --user -u ownnas-mount`.

## Always keep, and free up space

- `ownnas-mount keep <files or folders>`: always keep them on this computer. They download now,
  and new versions download as they arrive.
- `ownnas-mount free <files or folders>`: drop them from this computer. They stay on the server
  and download again when opened.

`setup` adds the same two items to file managers' menus:

- **Dolphin:** an "OwnNAS" submenu (a service menu).
- **GNOME Files:** shown only on files in OwnNAS folders. It needs nautilus-python:
  `python3-nautilus` on Debian and Ubuntu, `nautilus-python` on Fedora and Arch.
- **Nemo:** two actions.

Both commands set an extended attribute on the file, `user.ownnas.keep` (1 or 0). Reading
`user.ownnas.state` tells what this computer has:

- `online`: on the server only.
- `downloaded`: in the cache.
- `kept`: always kept.
- `uploading`: changed here and not sent yet.

```sh
getfattr -n user.ownnas.state ~/OwnNAS/library/report.pdf
```

## How changes go

- **Saving a file:** it is saved in the cache and sent a couple of seconds after it is closed.
  The upload only goes through over the version it started from. If the server's copy changed
  in the meantime, this computer's version goes up next to it as
  `name (conflict <computer> <date>).ext`, and the file shows the server's version. Nothing is
  overwritten.
- **Editors' saves:** a temporary file written and renamed over the real one (GNOME's
  `.goutputstream-…`, swap files) never reaches the server. Only the real file is sent.
- **Folders, renames, moves and deletions:** these happen on the server at once. Deleting moves
  files to the server's Trash. File managers can't make their own trash folder in OwnNAS
  folders, so they offer to delete instead, and those deletions still go to the server's Trash.
- **Changes on the server:** they show within seconds through the server's change feed. With
  servers from before the feed, folders are looked at again after 30 seconds.
- **Without a connection:** files on this computer still open, folders opened before still list,
  and changes are sent once the server is back.

## Test

`scripts/test-linux-mount.sh` starts a server and checks the whole round trip: listing,
downloading, sending, renames, editors' saves, conflicts, keep and free, deletions both ways, and
unmounting on stop. CI runs it on every push. It needs `fuse3` and `attr`:

```sh
cargo build -p ownnas -p ownnas-mount
scripts/test-linux-mount.sh
```
