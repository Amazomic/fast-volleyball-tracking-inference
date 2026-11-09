#!/usr/bin/env python3
"""Safely remove stale browser-upload copies from the isolated Rust Quick Reel sandbox.

Not used by the application runtime: diagnostic/manual maintenance only.
Requires an exact latest run ID even for --apply. Never touches original files,
prepared-video cache, current request sources, or other project data.
"""
import argparse
import json
import os
import re
import stat
from pathlib import Path

HEX = re.compile(r"^[0-9a-f]{32}$")
DROP = re.compile(r"^drop_[0-9a-f]{32}\.(?:mp4|mov|mkv|avi|webm|m4v|mts)$", re.I)

def regular(path):
    try:
        mode = path.lstat().st_mode
        return stat.S_ISREG(mode)
    except FileNotFoundError:
        return False

def owned_dir(path):
    if path.is_symlink() or not path.is_dir():
        raise ValueError("Unsafe Quick Reel directory: " + str(path))

def read_json(path):
    if not regular(path) or path.stat().st_size > 128 * 1024:
        raise ValueError("Unsafe Quick Reel metadata: " + str(path))
    with path.open("r", encoding="utf8") as stream:
        return json.load(stream)

def plan(root, expected_id):
    root = Path(root).absolute()
    sandbox = Path.home() / ".cache" / "volley-rust-sandbox"
    if root != sandbox / "quick_reels":
        raise ValueError("Cleanup allowed ONLY inside canonical isolated Rust Quick Reel sandbox")
    owned_dir(sandbox)
    owned_dir(root)
    owned_dir(root / "uploads")
    owned_dir(root / "selections")
    if not HEX.fullmatch(expected_id):
        raise ValueError("Expected latest Quick Reel UUID")
    latest = read_json(root / "latest.json")["id"]
    if latest != expected_id:
        raise ValueError("Latest Quick Reel changed; refusing cleanup")
    folder = root / latest
    owned_dir(folder)
    active_request = read_json(folder / "request.json")
    if active_request.get("id") != latest:
        raise ValueError("Current run metadata mismatch")
    active = set()
    for row in active_request.get("videos", []):
        path = row.get("source")
        if not isinstance(path, str):
            raise ValueError("Missing source in current Quick Reel")
        active.add(path)
    cutoff = (folder / "request.json").stat().st_mtime_ns
    uploads, selections = [], []
    for entry in sorted((root / "selections").iterdir()):
        if not entry.name.endswith(".json") or not HEX.fullmatch(entry.stem):
            continue
        if not regular(entry):
            continue
        record = read_json(entry)
        if record.get("id") != entry.stem or record.get("display_name") is None:
            continue  # Linked originals must not be released by this maintenance tool.
        source = record.get("source")
        if not isinstance(source, str):
            continue
        source_path = Path(source)
        if source_path.parent != root / "uploads" or not DROP.fullmatch(source_path.name):
            continue
        if source in active or entry.stat().st_mtime_ns >= cutoff:
            continue
        if not regular(source_path) or source_path.stat().st_mtime_ns >= cutoff:
            continue
        statinfo = source_path.stat()
        if (record.get("mtime_ns") != statinfo.st_mtime_ns or
            record.get("size") != statinfo.st_size or
            record.get("name") != source_path.name):
            continue
        uploads.append((source_path, statinfo.st_size, statinfo.st_mtime_ns))
        selections.append(entry)
    if read_json(root / "latest.json").get("id") != expected_id:
        raise ValueError("Latest run changed while scanning")
    return uploads, selections, active, cutoff

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--expected-latest-id", required=True)
    parser.add_argument("--apply", action="store_true")
    opts = parser.parse_args()
    uploads, selections, active, cutoff = plan(opts.root, opts.expected_latest_id)
    reclaimed = 0
    for (target, size, mtime), selection in zip(uploads, selections):
        if opts.apply:
            # Revalidate at removal time: current run and metadata cannot change silently.
            _, _, current_active, current_cutoff = plan(opts.root, opts.expected_latest_id)
            if str(target) in current_active or current_cutoff != cutoff:
                raise ValueError("Quick Reel sources changed while cleaning")
            if not regular(target):
                raise ValueError("Video no longer exists")
            meta = target.stat()
            if meta.st_size != size or meta.st_mtime_ns != mtime:
                raise ValueError("Stale copy mutated while cleaning")
            target.unlink()
            selection.unlink()
        reclaimed += size
    print(json.dumps({
        "mode": "applied" if opts.apply else "dry-run",
        "latest_id": opts.expected_latest_id,
        "protected_current_sources": len(active),
        "deleted_copies" if opts.apply else "candidate_copies": [x[0].name for x in uploads],
        "bytes_reclaimed" if opts.apply else "candidate_bytes": reclaimed
    }, indent=2))
if __name__ == "__main__":
    main()
