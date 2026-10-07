# Update signing keys

```bash
# from the repo root, after building ownnas:
./target/release/ownnas update keygen --out releases/keys
```

- `update.sk` — private key (hex). **Never upload to the VPS.**
- `update.pk` — public key (hex). Pass to OwnNAS as `--update-pubkey` / `OWNNAS_UPDATE_PUBKEY`.

`publish.sh` signs `latest.json` into `latest.json.sig` when `update.sk` is present.
