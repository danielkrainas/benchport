#!/usr/bin/env python3
"""Publish a new launcher build into a Benchport version.json.

    python3 tools/publish-launcher.py version.json 0.2.0 path/to/Benchport_0.2.0_amd64.AppImage \
        --platform linux [--url https://cdn.example.com/launcher/Benchport_0.2.0_amd64.AppImage] \
        [--executable benchport.exe] [--notes "What changed"] [--minimum 0.1.5] [--page https://…]

Run it once per platform build. It computes the artifact's sha256 + size,
records it under "platforms", sets "version"/"released", and leaves other
platforms' entries alone (stale ones for an older version are dropped, so a
platform never gets offered a build that doesn't match "version").

Artifacts:
  * linux   — the .AppImage itself (or a zip containing it)
  * windows — the .exe, or a zip containing it (use --executable if renamed)
  * macos   — a zip containing the .app bundle
Platform can be "linux", "windows", "macos", or "<os>-<arch>" such as
"macos-aarch64" when you ship per-architecture builds.

If --url is omitted, the artifact's path relative to version.json is used.
Then point the launcher at it with "updateUrl" in launcher.json and upload
both files. Standard library only.
"""

import argparse
import hashlib
import json
import os
from datetime import date
from pathlib import Path


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("manifest", help="path to version.json (created if missing)")
    ap.add_argument("version")
    ap.add_argument("artifact")
    ap.add_argument("--platform", required=True, help='e.g. linux, windows, macos, macos-aarch64')
    ap.add_argument("--url", help="download URL (default: path relative to version.json)")
    ap.add_argument("--executable", help="launcher executable inside a zip artifact")
    ap.add_argument("--notes", help="release notes shown in the launcher")
    ap.add_argument("--minimum", help="minimumVersion: older launchers must update before playing")
    ap.add_argument("--page", help="web page where users can download the launcher manually")
    a = ap.parse_args()

    path = Path(a.manifest)
    manifest = json.loads(path.read_text()) if path.exists() else {}
    art = Path(a.artifact)
    if not art.is_file():
        ap.error(f"{art} does not exist")

    if manifest.get("version") != a.version:
        # New release: entries for other platforms belong to the old version.
        stale = [k for k, v in manifest.get("platforms", {}).items() if v.get("version") != a.version]
        for k in stale:
            del manifest["platforms"][k]
        if stale:
            print(f"Dropped builds from the previous release: {', '.join(stale)} (re-publish them for {a.version})")

    url = a.url or os.path.relpath(art.resolve(), path.resolve().parent).replace(os.sep, "/")
    entry = {"url": url, "sha256": sha256(art), "size": art.stat().st_size, "version": a.version}
    if a.executable:
        entry["executable"] = a.executable

    manifest["version"] = a.version
    manifest["released"] = date.today().isoformat()
    for key, val in (("notes", a.notes), ("minimumVersion", a.minimum), ("page", a.page)):
        if val is not None:
            manifest[key] = val
    manifest.setdefault("platforms", {})[a.platform] = entry

    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n")
    tmp.replace(path)
    print(f"launcher {a.version} [{a.platform}]  sha256={entry['sha256'][:16]}…  size={entry['size']:,} bytes  url={url}")
    print(f"Now upload {art.name} and {path.name}.")


if __name__ == "__main__":
    main()
