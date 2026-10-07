#!/usr/bin/env bash
# Build latest.json (+ optional latest.json.sig) from binaries in releases/vVERSION/
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
VERSION="${1:-}"
BASE_URL="${2:-}"
NOTES="${3:-}"

if [[ -z "$VERSION" || -z "$BASE_URL" ]]; then
  echo "Usage: $0 <version> <base-url> [notes]" >&2
  echo "  Example: $0 0.2.0 https://updates.example.com \"Bug fixes\"" >&2
  exit 1
fi

BASE_URL="${BASE_URL%/}"
DIR="$ROOT/v$VERSION"
if [[ ! -d "$DIR" ]]; then
  echo "Missing folder: $DIR" >&2
  echo "Create it and place named binaries inside, then re-run." >&2
  exit 1
fi

sha_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

ARTIFACTS_JSON=""
FIRST=1
for path in "$DIR"/ownnas-*; do
  [[ -e "$path" ]] || continue
  [[ -f "$path" ]] || continue
  name="$(basename "$path")"
  # Map filename → target triple
  case "$name" in
    ownnas-x86_64-pc-windows-msvc.exe) target="x86_64-pc-windows-msvc" ;;
    ownnas-x86_64-apple-darwin) target="x86_64-apple-darwin" ;;
    ownnas-aarch64-apple-darwin) target="aarch64-apple-darwin" ;;
    ownnas-x86_64-unknown-linux-gnu) target="x86_64-unknown-linux-gnu" ;;
    ownnas-aarch64-unknown-linux-gnu) target="aarch64-unknown-linux-gnu" ;;
    *)
      echo "Skipping unrecognized file: $name" >&2
      continue
      ;;
  esac
  hash="$(sha_of "$path")"
  size="$(wc -c < "$path" | tr -d ' ')"
  url="$BASE_URL/v$VERSION/$name"
  entry=$(printf '{"url":"%s","sha256":"%s","size":%s}' "$url" "$hash" "$size")
  if [[ $FIRST -eq 1 ]]; then
    ARTIFACTS_JSON=$(printf '"%s":%s' "$target" "$entry")
    FIRST=0
  else
    ARTIFACTS_JSON=$(printf '%s,"%s":%s' "$ARTIFACTS_JSON" "$target" "$entry")
  fi
  echo "  $target  $hash  ($size bytes)"
done

if [[ -z "$ARTIFACTS_JSON" ]]; then
  echo "No recognized binaries in $DIR" >&2
  exit 1
fi

PUBLISHED="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
NOTES_ESC=$(printf '%s' "${NOTES:-OwnNAS $VERSION}" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')

cat > "$ROOT/latest.json" <<EOF
{
  "version": "$VERSION",
  "notes": $NOTES_ESC,
  "publishedAt": "$PUBLISHED",
  "minVersion": "0.1.0",
  "artifacts": {
    $ARTIFACTS_JSON
  }
}
EOF

echo "Wrote $ROOT/latest.json"

OWNNAS_BIN=""
for candidate in "$ROOT/../target/release/ownnas" "$ROOT/../target/release/ownnas.exe" "$(command -v ownnas || true)"; do
  if [[ -n "$candidate" && -x "$candidate" ]]; then
    OWNNAS_BIN="$candidate"
    break
  fi
done

if [[ -f "$ROOT/keys/update.sk" ]]; then
  if [[ -z "$OWNNAS_BIN" ]]; then
    echo "Private key present but ownnas binary not found; skip signing." >&2
    echo "Build OwnNAS and run: ownnas update sign --key releases/keys/update.sk --manifest releases/latest.json" >&2
  else
    "$OWNNAS_BIN" update sign --key "$ROOT/keys/update.sk" --manifest "$ROOT/latest.json"
    echo "Wrote $ROOT/latest.json.sig"
  fi
else
  echo "No keys/update.sk — latest.json is unsigned (SHA-256 of binaries still required)."
fi

echo
echo "Upload to your VPS (exclude the private key):"
echo "  rsync -av --exclude keys/update.sk $ROOT/ user@vps:/var/www/ownnas-updates/"
