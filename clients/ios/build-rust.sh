#!/bin/bash
# Builds the shared Rust client library (crates/ownnas-ffi) for iPhones and the simulator, and
# generates its Swift bindings:
#   Generated/OwnNASCore.xcframework   the static libraries, with the C header and module map
#   Generated/ownnas_ffi.swift         the Swift API (compiled into the app)
# Xcode runs this before each build (see project.yml); Cargo skips the work when nothing changed.
# Pass --release (Xcode does for Release builds) for optimized libraries.
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
export IPHONEOS_DEPLOYMENT_TARGET=17.0

targets=(aarch64-apple-ios aarch64-apple-ios-sim)
rustup target add "${targets[@]}" >/dev/null 2>&1 || true
cd "$repo"
for target in "${targets[@]}"; do
  cargo build -q -p ownnas-ffi --lib --target "$target" ${flags[@]+"${flags[@]}"}
done

# Bindings come from a host build of the same library (UniFFI reads its metadata), with the
# macOS headers (inside Xcode the C compiler would not find them on its own).
SDKROOT="$(xcrun --sdk macosx --show-sdk-path)" cargo build -q -p ownnas-ffi --lib
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cargo run -q -p ownnas-ffi --features cli --bin uniffi-bindgen -- \
  generate --library "$repo/target/debug/libownnas_ffi.dylib" --language swift --out-dir "$work/bindings" >/dev/null

mkdir -p "$work/headers"
cp "$work/bindings/ownnas_ffiFFI.h" "$work/headers/"
cp "$work/bindings/ownnas_ffiFFI.modulemap" "$work/headers/module.modulemap"

mkdir -p "$out"
rm -rf "$out/OwnNASCore.xcframework"
args=()
for target in "${targets[@]}"; do
  args+=(-library "$repo/target/$target/$profile/libownnas_ffi.a" -headers "$work/headers")
done
xcodebuild -create-xcframework "${args[@]}" -output "$out/OwnNASCore.xcframework" >/dev/null
# Only rewrite the Swift file when it changed, so Xcode does not recompile it every build.
if ! cmp -s "$work/bindings/ownnas_ffi.swift" "$out/ownnas_ffi.swift"; then
  cp "$work/bindings/ownnas_ffi.swift" "$out/ownnas_ffi.swift"
fi
echo "Built OwnNASCore.xcframework ($profile)"
