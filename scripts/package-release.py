#!/usr/bin/env python3
"""Package a native OwnNAS release build and write a SHA-256 sidecar."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import tarfile
import zipfile
from pathlib import Path


TARGETS = {
    "x86_64-unknown-linux-gnu": ("tar.gz", "ownnas"),
    "x86_64-pc-windows-msvc": ("zip", "ownnas.exe"),
    "x86_64-apple-darwin": ("tar.gz", "ownnas"),
    "aarch64-apple-darwin": ("tar.gz", "ownnas"),
    "aarch64-unknown-linux-gnu": ("tar.gz", "ownnas"),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("version", help="release version, with or without a leading v")
    parser.add_argument("target", choices=TARGETS)
    args = parser.parse_args()

    version = args.version.removeprefix("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        parser.error("version must use the vMAJOR.MINOR.PATCH format")

    extension, executable = TARGETS[args.target]
    binary = Path("target") / args.target / "release" / executable
    if not binary.is_file():
        raise SystemExit(f"release binary does not exist: {binary}")

    output_dir = Path("dist")
    output_dir.mkdir(exist_ok=True)
    base = f"ownnas-{version}-{args.target}"
    raw_name = f"{base}-binary" + (".exe" if executable.endswith(".exe") else "")
    raw_path = output_dir / raw_name
    raw_path.write_bytes(binary.read_bytes())
    raw_digest = hashlib.sha256(raw_path.read_bytes()).hexdigest()
    (output_dir / f"update-{args.target}.json").write_text(
        json.dumps({"target": args.target, "url": f"https://github.com/robertoho/ownnas/releases/download/v{version}/{raw_name}", "sha256": raw_digest, "size": raw_path.stat().st_size}, separators=(",", ":")),
        encoding="utf-8",
    )
    archive = output_dir / f"{base}.{extension}"
    root = f"ownnas-{version}"
    readme = (
        f"OwnNAS {version} ({args.target})\n\n"
        "Run `ownnas --help` to see available commands. To start the server, run:\n\n"
        "  ownnas serve --root <folder>\n"
    )

    if extension == "zip":
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            bundle.write(binary, f"{root}/{executable}")
            bundle.writestr(f"{root}/README.txt", readme)
    else:
        readme_path = output_dir / "README.txt"
        readme_path.write_text(readme, encoding="utf-8")
        try:
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(binary, arcname=f"{root}/{executable}")
                bundle.add(readme_path, arcname=f"{root}/README.txt")
        finally:
            readme_path.unlink(missing_ok=True)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    sidecar = output_dir / f"{archive.name}.sha256"
    sidecar.write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    print(f"Packaged {archive} ({digest})")


if __name__ == "__main__":
    main()
