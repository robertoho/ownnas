#!/bin/zsh
set -euo pipefail
cd "$(dirname "$0")"

root="${HOME}/fotos"

if [[ ! -x target/release/ownnas ]]; then
    echo "target/release/ownnas was not found. Run ./build.sh first."
    exit 1
fi

exec ./target/release/ownnas serve --root "$root" --username admin
