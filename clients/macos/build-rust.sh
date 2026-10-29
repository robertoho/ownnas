#!/bin/bash
# Builds the shared Rust client library (crates/ownnas-ffi) for Macs (Apple silicon and Intel, in
# one library) and generates its Swift bindings:
#   Generated/OwnNASCore.xcframework   the static library, with the C header and module map
#   Generated/ownnas_ffi.swift         the Swift API (compiled into the app and its extension)
# Xcode runs this before each build (see project.yml); Cargo skips the work when nothing changed.
# Pass --release (Xcode does for Release builds) for an optimized library.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$here/Generated"
profile=debug
flags=()
if [[ "${1:-}" == "--release" || "${CONFIGURATION:-}" == "Release" ]]; then
  profile=release
  flags=(--release)
fi

# Xcode starts scripts with a bare PATH; find rustup's cargo where Homebrew or rustup put it.
export PATH="/opt/homebrew/opt/rustup/bin:/usr/local/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null || { echo "error: cargo not found; install Rust with rustup" >&2; exit 1; }
# Xcode's own settings would point Cargo's C compiles at the app's SDK; let each target pick its own.
unset SDKROOT IPHONEOS_DEPLOYMENT_TARGET MACOSX_DEPLOYMENT_TARGET
export MACOSX_DEPLOYMENT_TARGET=15.0
# Inside Xcode the C compiler would not find the macOS headers on its own.
SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
export SDKROOT

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}" >/dev/null 2>&1 || true
cd "$repo"
for target in "${targets[@]}"; do
  cargo build -q -p ownnas-ffi --lib --target "$target" ${flags[@]+"${flags[@]}"}
done

# Bindings come from a host build of the same library (UniFFI reads its metadata).
cargo build -q -p ownnas-ffi --lib
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cargo run -q -p ownnas-ffi --features cli --bin uniffi-bindgen -- \
  generate --library "$repo/target/debug/libownnas_ffi.dylib" --language swift --out-dir "$work/bindings" >/dev/null

mkdir -p "$work/headers"
cp "$work/bindings/ownnas_ffiFFI.h" "$work/headers/"
cp "$work/bindings/ownnas_ffiFFI.modulemap" "$work/headers/module.modulemap"
# One library for both kinds of Mac.
lipo -create "$repo/target/aarch64-apple-darwin/$profile/libownnas_ffi.a" "$repo/target/x86_64-apple-darwin/$profile/libownnas_ffi.a" -output "$work/libownnas_ffi.a"

mkdir -p "$out"
rm -rf "$out/OwnNASCore.xcframework"
xcodebuild -create-xcframework -library "$work/libownnas_ffi.a" -headers "$work/headers" -output "$out/OwnNASCore.xcframework" >/dev/null
# Only rewrite the Swift file when it changed, so Xcode does not recompile it every build.
if ! cmp -s "$work/bindings/ownnas_ffi.swift" "$out/ownnas_ffi.swift"; then
  cp "$work/bindings/ownnas_ffi.swift" "$out/ownnas_ffi.swift"
fi
echo "Built OwnNASCore.xcframework for macOS ($profile)"
