#!/usr/bin/env python3
"""Combine per-target release fragments into the signed update feed."""
import json
import sys
from datetime import datetime, timezone
from pathlib import Path


tag, notes = sys.argv[1], sys.argv[2]
version = tag.removeprefix("v")
artifacts = {}
for fragment in Path("dist").glob("update-*.json"):
    item = json.loads(fragment.read_text(encoding="utf-8"))
    artifacts[item["target"]] = {key: item[key] for key in ("url", "sha256", "size")}
if not artifacts:
    raise SystemExit("No update target fragments were downloaded")
manifest = {
    "version": version,
    "notes": notes.strip(),
    "publishedAt": datetime.now(timezone.utc).isoformat(),
    "artifacts": artifacts,
}
Path("dist/latest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
