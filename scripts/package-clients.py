#!/usr/bin/env python3
"""Packages the desktop sync clients for a release target, next to the server's archives in dist/:

- Linux: ownnas-mount-<version>-<target>.tar.gz (clients/linux, after
  `cargo build --release --target <target> -p ownnas-mount`)
- Windows: ownnas-cloud-<version>-<target>.zip, with ownnas-cloud.exe and ownnas-cloud-agent.exe
  (clients/windows, after `cargo build --release --target <target> -p ownnas-cloud`)

Each with a .sha256 file, as scripts/package-release.py writes for the server. The Mac app is a
notarized disk image made on a Mac (clients/macos/release.sh), not here.
"""

import argparse
import hashlib
import tarfile
import zipfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="release tag, for example v0.8.0")
    parser.add_argument("target", help="Rust target triple")
    args = parser.parse_args()
    version = args.tag.removeprefix("v")
    built = Path("target") / args.target / "release"
    output_dir = Path("dist")
    output_dir.mkdir(exist_ok=True)

    if "linux" in args.target:
        name, files = "ownnas-mount", ["ownnas-mount"]
        readme = (
            f"OwnNAS {version}: ownnas-mount, the servers as folders with online-only files (FUSE).\n\n"
            "  ./ownnas-mount setup --server https://nas.example:8787 --username <you>\n\n"
            "Needs fusermount3 (Debian and Ubuntu: the fuse3 package). See\n"
            "https://github.com/robertoho/ownnas/blob/main/clients/linux/README.md\n"
        )
    elif "windows" in args.target:
        name, files = "ownnas-cloud", ["ownnas-cloud.exe", "ownnas-cloud-agent.exe"]
        readme = (
            f"OwnNAS {version}: ownnas-cloud, the servers in Explorer with online-only files.\n\n"
            "Keep both programs in the same folder, then:\n\n"
            "  ownnas-cloud setup --server https://nas.example:8787 --username <you>\n\n"
            "ownnas-cloud-agent starts at each sign-in to Windows and keeps them in sync.\n"
        )
    else:
        print(f"No desktop sync client is packaged for {args.target}")
        return

    root = f"{name}-{version}-{args.target}"
    if "windows" in args.target:
        archive = output_dir / f"{root}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for file in files:
                bundle.write(built / file, f"{root}/{file}")
            bundle.writestr(f"{root}/README.txt", readme)
    else:
        archive = output_dir / f"{root}.tar.gz"
        readme_path = output_dir / f"{root}-README.txt"
        readme_path.write_text(readme, encoding="utf-8")
        try:
            with tarfile.open(archive, "w:gz") as bundle:
                for file in files:
                    bundle.add(built / file, arcname=f"{root}/{file}")
                bundle.add(readme_path, arcname=f"{root}/README.txt")
        finally:
            readme_path.unlink(missing_ok=True)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output_dir / f"{archive.name}.sha256").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    print(f"Packaged {archive} ({digest})")


if __name__ == "__main__":
    main()
