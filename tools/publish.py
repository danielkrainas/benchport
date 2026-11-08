#!/usr/bin/env python3
"""Publish a new build into a Benchport catalog.

    python3 tools/publish.py catalog.json jump-test 0.3.2 path/to/JumpTest.zip \
        [--url https://cdn.example.com/builds/jump-test-0.3.2.zip] \
        [--platform windows|macos|linux] [--executable JumpTest.exe] [--notes "What changed"]

Computes the zip's sha256 + size, updates (or creates) the game entry, and
bumps the catalog's "version" to today's date. Then upload the zip and the
catalog.json and every launcher picks the update up on its next check.

If --url is omitted, the zip's path relative to catalog.json is used, which is
right when you upload the whole folder as-is. Standard library only.
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
    ap.add_argument("catalog")
    ap.add_argument("id")
    ap.add_argument("version")
    ap.add_argument("zip")
    ap.add_argument("--url", help="download URL (default: path relative to the catalog)")
    ap.add_argument("--platform", choices=["windows", "macos", "linux"], help="publish as a platform-specific build")
    ap.add_argument("--executable", help="executable path inside the zip")
    ap.add_argument("--name", help="display name (required for a new game)")
    ap.add_argument("--notes", help="release notes")
    a = ap.parse_args()

    cat_path = Path(a.catalog)
    catalog = json.loads(cat_path.read_text()) if cat_path.exists() else {"version": "", "games": []}
    zip_path = Path(a.zip)
    if not zip_path.is_file():
        ap.error(f"{zip_path} does not exist")

    url = a.url or os.path.relpath(zip_path.resolve(), cat_path.resolve().parent).replace(os.sep, "/")
    game = next((g for g in catalog["games"] if g["id"] == a.id), None)
    if game is None:
        if not a.name or not a.executable:
            ap.error("new game: --name and --executable are required")
        game = {"id": a.id, "name": a.name}
        catalog["games"].append(game)
        print(f"Adding new game {a.id}")

    build = {"downloadUrl": url, "sha256": sha256(zip_path), "size": zip_path.stat().st_size}
    if a.executable:
        build["executable"] = a.executable
    if a.platform:
        game.setdefault("platforms", {}).setdefault(a.platform, {}).update(build)
    else:
        game.update(build)

    game["version"] = a.version
    game["released"] = date.today().isoformat()
    if a.notes is not None:
        game["releaseNotes"] = a.notes
    catalog["version"] = date.today().isoformat()

    tmp = cat_path.with_suffix(".tmp")
    tmp.write_text(json.dumps(catalog, indent=2, ensure_ascii=False) + "\n")
    tmp.replace(cat_path)
    print(f"{a.id} -> {a.version}  sha256={build['sha256'][:16]}…  size={build['size']:,} bytes  url={url}")
    print(f"Now upload {zip_path.name} and {cat_path.name}.")


if __name__ == "__main__":
    main()
