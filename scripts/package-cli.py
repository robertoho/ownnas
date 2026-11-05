#!/usr/bin/env python3
"""Packages ownnascli for a release target, next to the server's archives in dist/, after
`cargo build --release --target <target> -p ownnascli`: ownnascli-<version>-<target>.tar.gz, or a
.zip on Windows, with a .sha256 file.
"""

import argparse
import hashlib
import io
import tarfile
import zipfile
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="release tag, for example v0.9.3")
    parser.add_argument("target", help="Rust target triple")
    parser.add_argument("--built", type=Path, help="where the program was built (default: target/<target>/release)")
    args = parser.parse_args()
    version = args.tag.removeprefix("v")
    windows = "windows" in args.target
    program = "ownnascli.exe" if windows else "ownnascli"
    built = (args.built or Path("target") / args.target / "release") / program
    output_dir = Path("dist")
    output_dir.mkdir(exist_ok=True)
    readme = (
        f"OwnNAS {version}: ownnascli, fast file transfer with an OwnNAS server from the command line.\n\n"
        "  ownnascli login https://nas.example:8787\n"
        "  ownnascli ls ownnas:Photos\n"
        "  ownnascli cp ~/Documents ownnas:Documents\n"
        "  ownnascli sync ~/Documents ownnas:Documents --dry-run\n"
        "  ownnascli photos ~/Pictures --workspace Family\n\n"
        "ownnascli --help tells the rest. It needs a server of the same version or later.\n"
    )
    root = f"ownnascli-{version}-{args.target}"
    if windows:
        archive = output_dir / f"{root}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            bundle.write(built, f"{root}/{program}")
            bundle.writestr(f"{root}/README.txt", readme)
    else:
        archive = output_dir / f"{root}.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(built, arcname=f"{root}/{program}")
            text = readme.encode("utf-8")
            info = tarfile.TarInfo(f"{root}/README.txt")
            info.size = len(text)
            info.mode = 0o644
            bundle.addfile(info, io.BytesIO(text))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output_dir / f"{archive.name}.sha256").write_text(f"{digest}  {archive.name}\n", encoding="ascii")
    print(f"Packaged {archive}")


if __name__ == "__main__":
    main()
