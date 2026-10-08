#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust and Cargo are required. Install Rust from https://rustup.rs/ and try again." >&2
    exit 1
fi

cargo build --locked --release
echo "Built $SCRIPT_DIR/target/release/ownnas"
