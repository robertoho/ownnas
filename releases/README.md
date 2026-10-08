# OwnNAS update host (static)

## Automated GitHub releases

The GitHub Actions workflow in `.github/workflows/release.yml` builds Linux x86_64,
Windows x86_64, and macOS x86_64 plus Apple Silicon binaries. To publish a release,
set the package version in `Cargo.toml`, commit the change, then push a matching tag:

```bash
cargo check  # refresh Cargo.lock after changing Cargo.toml
git add Cargo.toml Cargo.lock
git commit -m "Release v0.2.0"
git tag v0.2.0
git push origin main v0.2.0
```

The tag must match the Cargo package version. Actions creates a draft release, builds
and packages each platform, uploads the archives and SHA-256 files, then publishes
the release after all builds succeed. To build assets for an existing tag, run
**Build and publish release** from the Actions tab and enter that tag. This GitHub
Release workflow is separate from the static updater files described below.

Upload this whole `releases/` folder to your VPS (nginx, Caddy, Apache, or any static file host). OwnNAS instances fetch `latest.json`, then download the matching binary.

## Layout

```
releases/
  latest.json              # current release pointer (required)
  latest.json.sig          # optional ed25519 signature (hex) of latest.json
  v0.1.0/                  # one folder per version
    ownnas-x86_64-pc-windows-msvc.exe
    ownnas-x86_64-apple-darwin
    ownnas-aarch64-apple-darwin
    ownnas-x86_64-unknown-linux-gnu
    ownnas-aarch64-unknown-linux-gnu
  keys/                    # keep PRIVATE key off the VPS
  publish.sh               # build latest.json (+ optional signature)
  nginx.conf.example
```

**Do not upload `keys/update.sk`.** Only the public key goes into OwnNAS (`--update-pubkey`) or into `keys/update.pk` for your records.

## Quick start on the VPS

1. Copy built binaries into `vX.Y.Z/` with the exact names above.
2. On your build machine (with the private key):

```bash
./releases/publish.sh 0.2.0 https://updates.example.com
```

3. Sync to the VPS (example):

```bash
rsync -av --exclude keys/update.sk releases/ user@vps:/var/www/ownnas-updates/
```

4. Point OwnNAS at the manifest:

```bash
ownnas serve --root /data/library \
  --update-url https://updates.example.com/latest.json \
  --update-pubkey "$(cat releases/keys/update.pk)"
```

## Generate a signing key (once)

```bash
ownnas update keygen --out releases/keys
```

This writes:

- `releases/keys/update.sk` — **secret**, never publish
- `releases/keys/update.pk` — hex public key for `--update-pubkey`

## Binary names OwnNAS looks for

| Target triple | File name |
| --- | --- |
| `x86_64-pc-windows-msvc` | `ownnas-x86_64-pc-windows-msvc.exe` |
| `x86_64-apple-darwin` | `ownnas-x86_64-apple-darwin` |
| `aarch64-apple-darwin` | `ownnas-aarch64-apple-darwin` |
| `x86_64-unknown-linux-gnu` | `ownnas-x86_64-unknown-linux-gnu` |
| `aarch64-unknown-linux-gnu` | `ownnas-aarch64-unknown-linux-gnu` |

Build examples:

```bash
cargo build --release
cp target/release/ownnas releases/v0.2.0/ownnas-$(rustc -vV | sed -n 's/^host: //p')

# cross-compile as you prefer, then rename into the table above
```

## Nginx sketch

See `nginx.conf.example`. Serve the directory over **HTTPS**. OwnNAS refuses non-HTTPS update URLs unless the host is `localhost` / `127.0.0.1`.
