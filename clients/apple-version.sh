#!/bin/bash
# The Mac and iPhone apps' version, from the release version in the workspace Cargo.toml
# ([workspace.package] version), so it never lags behind the server and the other clients.
#
#   apple-version.sh            prints the version (release.sh names the disk image with it)
#   apple-version.sh --stamp    (an Xcode build phase) writes it into the built Info.plist:
#     CFBundleShortVersionString  the release version, e.g. 0.9.9
#     CFBundleVersion             the build: the number of commits, which only grows
#     OwnNASCommit                the commit built, with "+" when it had changes not committed
# project.yml runs it after each target's build, before Xcode signs the bundle.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
version=$(awk '
    /^\[/ { section = $0; next }
    section == "[workspace.package]" && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
' "$repo/Cargo.toml")
[ -n "$version" ] || { echo "error: no [workspace.package] version in $repo/Cargo.toml" >&2; exit 1; }

if [ "${1:-}" != "--stamp" ]; then
    echo "$version"
    exit 0
fi

build=1
commit=""
if git -C "$repo" rev-parse --git-dir >/dev/null 2>&1; then
    build=$(git -C "$repo" rev-list --count HEAD)
    commit=$(git -C "$repo" rev-parse --short HEAD)
    git -C "$repo" diff --quiet HEAD -- 2>/dev/null || commit="$commit+"
fi

plist="$TARGET_BUILD_DIR/$INFOPLIST_PATH"
buddy=/usr/libexec/PlistBuddy
"$buddy" -c "Set :CFBundleShortVersionString $version" "$plist"
"$buddy" -c "Set :CFBundleVersion $build" "$plist"
"$buddy" -c "Delete :OwnNASCommit" "$plist" 2>/dev/null || true
[ -z "$commit" ] || "$buddy" -c "Add :OwnNASCommit string $commit" "$plist"
echo "$PRODUCT_NAME $version (build $build${commit:+, $commit})"
