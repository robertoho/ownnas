#!/usr/bin/env bash
# End-to-end check of ownnas-mount (clients/linux): a server on this computer, its folder
# mounted with FUSE, and files going both ways. Needs fusermount3 and getfattr/setfattr
# (Debian and Ubuntu: fuse3 and attr). Run from the repository after
# `cargo build -p ownnas -p ownnas-mount` (BIN=target/release for release builds).
set -euo pipefail
bin=$(realpath "${BIN:-target/debug}")
work=$(mktemp -d)
export HOME=$work/home
unset XDG_CONFIG_HOME XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME
library=$work/library
data=$work/data
place=$work/OwnNAS
mkdir -p "$library/Docs" "$data" "$HOME"
printf 'hello from the server' > "$library/Docs/readme.txt"

server=
mount=
cleanup() {
    [ -n "$mount" ] && kill "$mount" 2>/dev/null
    sleep 1
    fusermount3 -uz "$place" 2>/dev/null || true
    [ -n "$server" ] && kill "$server" 2>/dev/null
    echo '--- log'
    tail -40 "$HOME/.local/state/ownnas/mount.log" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT
fail() {
    echo "FAILED: $1"
    exit 1
}
wait_until() {
    local what=$1
    shift
    for _ in $(seq 60); do
        if "$@" >/dev/null 2>&1; then
            echo "ok: $what"
            return
        fi
        sleep 0.5
    done
    fail "$what"
}
state() { getfattr --only-values -n user.ownnas.state "$1" 2>/dev/null; }
has() { [ "$(cat "$1" 2>/dev/null)" = "$2" ]; }
is_state() { [ "$(state "$1")" = "$2" ]; }

OWNNAS_USER=admin OWNNAS_PASSWORD='test passphrase 123' "$bin/ownnas" user add --data "$data"
"$bin/ownnas" serve --root "$library" --data "$data" --addr 127.0.0.1:8899 > "$work/server.log" 2>&1 &
server=$!
wait_until 'the server is up' curl -sf http://127.0.0.1:8899/api/health

OWNNAS_PASSWORD='test passphrase 123' "$bin/ownnas-mount" setup --server http://127.0.0.1:8899 --username admin --folder "$place" --no-service
"$bin/ownnas-mount" run &
mount=$!

# Folders list from the server; files take no space until read.
wait_until "the server's folders show" test -f "$place/Docs/readme.txt"
[ "$(stat -c %s "$place/Docs/readme.txt")" = 21 ] || fail "readme.txt shows $(stat -c %s "$place/Docs/readme.txt") bytes"
is_state "$place/Docs/readme.txt" online || fail "readme.txt is $(state "$place/Docs/readme.txt") before it was read"
echo 'ok: files are online-only until read'
has "$place/Docs/readme.txt" 'hello from the server' || fail "reading gave '$(cat "$place/Docs/readme.txt")'"
is_state "$place/Docs/readme.txt" downloaded || fail "readme.txt is $(state "$place/Docs/readme.txt") after it was read"
echo 'ok: reading downloads the file'

printf 'written on Linux' > "$place/Docs/from-linux.txt"
wait_until 'a file made here reaches the server' has "$library/Docs/from-linux.txt" 'written on Linux'

printf 'made on the server' > "$library/Docs/from-server.txt"
wait_until 'a file made on the server shows here' has "$place/Docs/from-server.txt" 'made on the server'

mv "$place/Docs/from-linux.txt" "$place/Docs/renamed.txt"
wait_until 'a rename here happens on the server' test -f "$library/Docs/renamed.txt"
[ ! -e "$library/Docs/from-linux.txt" ] || fail 'the old name is still on the server'
mkdir "$place/Work"
wait_until 'a folder made here is made on the server' test -d "$library/Work"
mv "$place/Docs/renamed.txt" "$place/Work/"
wait_until 'a move here happens on the server' test -f "$library/Work/renamed.txt"

# Saving as editors do: a temporary file renamed over the real one, which is sent alone.
printf 'second version' > "$place/Docs/.goutputstream-ABC123"
mv "$place/Docs/.goutputstream-ABC123" "$place/Docs/readme.txt"
wait_until "an editor's save reaches the server" has "$library/Docs/readme.txt" 'second version'
ls -a "$library/Docs" | grep -q goutputstream && fail "the editor's temporary file reached the server"
printf ', and more' >> "$place/Docs/readme.txt"
wait_until 'an edit in place reaches the server' has "$library/Docs/readme.txt" 'second version, and more'

# Changed on both sides: this computer's version goes up as a conflict copy.
cat "$place/Docs/from-server.txt" > /dev/null
printf 'edited on the server' > "$library/Docs/from-server.txt"
printf 'edited on Linux' > "$place/Docs/from-server.txt"
conflict_copy() { grep -l 'edited on Linux' "$library/Docs/"*conflict* 2> /dev/null | grep -q .; }
wait_until "both sides' edits are kept" conflict_copy
has "$library/Docs/from-server.txt" 'edited on the server' || fail "the server's version was replaced"
wait_until "the file shows the server's version" has "$place/Docs/from-server.txt" 'edited on the server'

# Always keep, and free up space.
head -c 300000 /dev/urandom > "$library/Docs/photo.jpg"
wait_until 'a new server file shows' test -f "$place/Docs/photo.jpg"
"$bin/ownnas-mount" keep "$place/Docs"
wait_until 'keeping a folder downloads what is in it' is_state "$place/Docs/photo.jpg" kept
"$bin/ownnas-mount" free "$place/Docs"
is_state "$place/Docs/photo.jpg" online || fail "photo.jpg is $(state "$place/Docs/photo.jpg") after freeing its folder"
echo 'ok: freeing a folder empties what is in it'
cmp -s "$place/Docs/photo.jpg" "$library/Docs/photo.jpg" || fail 'photo.jpg changed on the way'
echo 'ok: it downloads again when read'

rm "$place/Work/renamed.txt"
wait_until 'a deletion here happens on the server' test ! -e "$library/Work/renamed.txt"
rm "$library/Docs/from-server.txt"
wait_until 'a deletion on the server shows here' test ! -e "$place/Docs/from-server.txt"

"$bin/ownnas-mount" status
kill -TERM "$mount"
wait "$mount" || fail 'ownnas-mount run did not stop cleanly'
mount=
grep -q " $place " /proc/self/mounts && fail 'the folder is still mounted after stopping'
echo 'ok: stopping unmounts'
echo 'Linux mount round trip passed.'
