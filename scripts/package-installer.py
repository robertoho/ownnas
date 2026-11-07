#!/usr/bin/env python3
"""Build the self-extracting Linux installer: dist/ownnas-VERSION-linux-ARCH.run.

It is scripts/linux-installer.sh followed by a tar.gz of the release binaries
(ownnas, and ownnas-sync, ownnas-mount, ownnascli when they were built).

  python3 scripts/package-installer.py 0.9.5 x86_64-unknown-linux-gnu
"""

from __future__ import annotations

import argparse
import hashlib
import io
import re
import tarfile
from pathlib import Path

ARCHES = {"x86_64-unknown-linux-gnu": "x86_64", "aarch64-unknown-linux-gnu": "aarch64"}
BINARIES = ["ownnas", "ownnas-sync", "ownnas-mount", "ownnascli"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("version")
    parser.add_argument("target", choices=ARCHES)
    args = parser.parse_args()
    version = args.version.removeprefix("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        parser.error("version must look like 1.2.3")
    arch = ARCHES[args.target]

    release = Path("target") / args.target / "release"
    if not (release / "ownnas").is_file():
        raise SystemExit(f"build the server first: {release / 'ownnas'} is missing")

    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode="w:gz") as bundle:
        for name in BINARIES:
            path = release / name
            if path.is_file():
                info = bundle.gettarinfo(str(path), arcname=name)
                info.mode, info.uid, info.gid, info.uname, info.gname = 0o755, 0, 0, "root", "root"
                with path.open("rb") as f:
                    bundle.addfile(info, f)
                print(f"  + {name}")

    header = Path("scripts/linux-installer.sh").read_text(encoding="utf-8").replace("\r\n", "\n")
    header = header.replace("@VERSION@", version).replace("@ARCH@", arch)
    if not header.endswith("__PAYLOAD_BELOW__\n"):
        raise SystemExit("linux-installer.sh must end with the __PAYLOAD_BELOW__ line")

    out = Path("dist") / f"ownnas-{version}-linux-{arch}.run"
    out.parent.mkdir(exist_ok=True)
    out.write_bytes(header.encode() + payload.getvalue())
    out.chmod(0o755)
    digest = hashlib.sha256(out.read_bytes()).hexdigest()
    (out.parent / f"{out.name}.sha256").write_text(f"{digest}  {out.name}\n", encoding="ascii")
    print(f"Built {out} ({out.stat().st_size / 1e6:.0f} MB, sha256 {digest})")


if __name__ == "__main__":
    main()
