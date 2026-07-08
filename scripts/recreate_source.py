#!/usr/bin/env python3
"""Recreate a KiCad design's raw source files from a SpinZero raw store.

The raw store (`<project>/raw/`) keeps each source file as a zstd-compressed,
content-addressed blob under `objects/<aa>/<blake3>`, and a per-user append-only
revision log (`revisions.<user>.jsonl`) whose `create` events carry the
`{relative path -> blob hash}` map for that revision. This script reads that map
and decompresses each blob back to its original path — the inverse of
`rawstore::materialize` in the app.

Requires the `zstandard` package:  pip install zstandard

Usage:
    python recreate_source.py <project_dir> --list
    python recreate_source.py <project_dir> <out_dir> [--revision r_xxxxxxxxxxxx]

If --revision is omitted, the latest revision (newest timestamp) is used.
"""
import argparse
import glob
import json
import os
import sys


def load_creates(raw_dir):
    """Every `create` event across all per-user logs, oldest-first by timestamp."""
    creates = []
    for log in glob.glob(os.path.join(raw_dir, "revisions.*.jsonl")):
        with open(log, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                ev = json.loads(line)
                if ev.get("action") == "create":
                    creates.append(ev)
    creates.sort(key=lambda e: e["ts"])
    return creates


def main():
    ap = argparse.ArgumentParser(description="Recreate KiCad source from a SpinZero raw store.")
    ap.add_argument("project_dir", help="the SpinZero project folder (contains raw/)")
    ap.add_argument("out_dir", nargs="?", help="where to write the recreated files")
    ap.add_argument("--revision", help="revision id (r_...); default = latest")
    ap.add_argument("--list", action="store_true", help="list available revisions and exit")
    args = ap.parse_args()

    raw_dir = os.path.join(args.project_dir, "raw")
    if not os.path.isdir(raw_dir):
        sys.exit(f"no raw store at {raw_dir}")

    creates = load_creates(raw_dir)
    if not creates:
        sys.exit("no revisions found")

    if args.list:
        for e in reversed(creates):  # newest first
            dirty = "*" if e.get("git_dirty") else ""
            print(f"{e['revision_id']}  {e['ts'][:19]}  git:{e.get('git_hash')}{dirty}  "
                  f"files:{len(e.get('source_hashes', {}))}")
        return

    if not args.out_dir:
        sys.exit("out_dir is required (or pass --list)")

    if args.revision:
        rev = next((e for e in creates if e["revision_id"] == args.revision), None)
        if rev is None:
            sys.exit(f"revision {args.revision} not found (try --list)")
    else:
        rev = creates[-1]  # latest by timestamp

    import zstandard  # imported here so --list works without the package

    dctx = zstandard.ZstdDecompressor()
    objects = os.path.join(raw_dir, "objects")
    written = 0
    print(f"revision {rev['revision_id']} ({rev['ts'][:19]}) -> {args.out_dir}")
    for rel, h in sorted(rev.get("source_hashes", {}).items()):
        blob = os.path.join(objects, h[:2], h)
        if not os.path.isfile(blob):
            print(f"  MISSING blob for {rel} ({h[:12]}) — pruned or not yet synced", file=sys.stderr)
            continue
        with open(blob, "rb") as fh:
            data = dctx.stream_reader(fh).read()  # streaming: blobs carry no content-size header
        dest = os.path.join(args.out_dir, *rel.split("/"))
        os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
        with open(dest, "wb") as out:
            out.write(data)
        written += 1
        print(f"  {rel}")
    print(f"done: {written} file(s)")


if __name__ == "__main__":
    main()
