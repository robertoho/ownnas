#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/ownnas"

if [[ ! -x "$BINARY" ]]; then
    echo "OwnNAS has not been built yet. Run $SCRIPT_DIR/build.sh first." >&2
    exit 1
fi

export OWNNAS_ROOT="${OWNNAS_ROOT:-$HOME/Pictures}"
export OWNNAS_DATA="${OWNNAS_DATA:-${XDG_DATA_HOME:-$HOME/.local/share}/ownnas}"
export OWNNAS_ADDR="${OWNNAS_ADDR:-0.0.0.0:8787}"

exec "$BINARY" serve "$@"
