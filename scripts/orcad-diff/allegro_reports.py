#!/usr/bin/env python3
"""Check the Allegro board reader against Allegro's own report files.

Many published Allegro projects ship files Allegro itself wrote from the same
board. Two kinds are used here, both read-only evidence:

  place   `place_txt.txt` / fabmaster pick-and-place exports: every placed
          part's reference, position, rotation and side. Compared with what
          `brd_place` decodes from the `.brd` beside them.
  classes extract reports (`sym.txt`): every primitive with its CLASS and
          SUBCLASS names and coordinates. Matching our decoded primitives to
          them by coordinates learns what each numeric (class, subclass) code
          means. This is where the subclass table in `orcad/pcb.rs` comes from.

Run (after `cargo build --release -p eda-parse-orcad --examples`):
  python scripts/orcad-diff/allegro_reports.py place   --corpus <dir>
  python scripts/orcad-diff/allegro_reports.py classes --corpus <dir>
"""

import argparse
import collections
import glob
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "src-tauri" / "target" / "release" / "examples"


def read_place(path):
    """Reference -> (x, y, rotation, 'm' when mirrored), in the file's units
    converted to mm."""
    txt = open(path, errors="replace").read()
    scale = 0.0254 if "UUNITS = MILS" in txt.upper() else 1.0
    out = {}
    for line in txt.splitlines():
        if line.startswith("S!"):
            # Fabmaster: S!REFDES!CLASS!...!SYM_MIRROR!SYM_ROTATE!SYM_X!SYM_Y!...
            f = line.split("!")
            try:
                out[f[1]] = (float(f[11]), float(f[12]), float(f[10]) % 360, "m" if f[9] == "YES" else "")
            except (ValueError, IndexError):
                pass
        elif "!" in line and not line.startswith(("#", "A!", "J!")):
            f = [x.strip() for x in line.split("!")]
            try:
                out[f[0]] = (float(f[1]) * scale, float(f[2]) * scale, float(f[3]) % 360, f[4])
            except (ValueError, IndexError):
                pass
        elif line.strip() and not line.startswith(("#", "UUNITS", "VERSION", "A!", "J!")):
            f = line.split()
            try:
                side = "m" if len(f) > 5 and f[4] == "m" else ""
                out[f[0]] = (float(f[1]) * scale, float(f[2]) * scale, float(f[3]) % 360, side)
            except (ValueError, IndexError):
                pass
    return out


def ours_place(brd):
    r = subprocess.run([str(EXAMPLES / "brd_place"), str(brd)], capture_output=True, text=True)
    out = {}
    for line in r.stdout.splitlines():
        p = line.split()
        if len(p) >= 5:
            out[p[0]] = (float(p[1]), float(p[2]), float(p[3]) % 360, "m" if len(p) == 6 and p[4] == "m" else "")
    return out


def cmd_place(corpus):
    refs = [p for pat in ("place_txt*.txt", "*pickAndPlace*.txt") for p in glob.glob(f"{corpus}/**/{pat}", recursive=True)]
    for ref in sorted(set(refs)):
        d = Path(ref).parent
        boards = sorted({os.path.realpath(b) for pat in ("*.brd", "../*.brd", "../allegro/*.brd", "../Cadence/Allegro/*.brd") for b in glob.glob(f"{d}/{pat}")})
        if not boards:
            continue
        theirs = read_place(ref)
        # Several boards in reach: the one sharing most designators is the
        # one the report was written from.
        scored = [(len(set(theirs) & set(o)), b, o) for b in boards for o in [ours_place(b)]]
        _, board, ours = max(scored, key=lambda x: x[0])
        boards = [board]
        ok = sum(
            1
            for k, v in theirs.items()
            if k in ours and abs(ours[k][0] - v[0]) < 0.01 and abs(ours[k][1] - v[1]) < 0.01
            and abs(ours[k][2] - v[2]) < 0.01 and ours[k][3] == v[3]
        )
        print(f"{ok:5d}/{len(theirs):<5d} {os.path.relpath(boards[0], corpus)}  vs  {os.path.relpath(ref, corpus)}")


def cmd_classes(corpus):
    votes = collections.defaultdict(collections.Counter)
    for asy in sorted({os.path.dirname(p) for p in glob.glob(f"{corpus}/**/sym.txt", recursive=True)}):
        design = Path(asy).parent.parent
        boards = glob.glob(f"{design}/**/*.brd", recursive=True) or glob.glob(f"{design.parent}/**/*.brd", recursive=True)
        if len(boards) != 1:
            continue
        prims = subprocess.run([str(EXAMPLES / "brd_prims"), boards[0]], capture_output=True, text=True).stdout
        idx = collections.defaultdict(set)
        for line in prims.splitlines():
            p = line.split()
            if p[0] == "L":
                a = (round(float(p[3]), 1), round(float(p[4]), 1))
                b = (round(float(p[5]), 1), round(float(p[6]), 1))
                idx[("L",) + tuple(sorted([a, b]))].add((int(p[1]), int(p[2])))
            else:
                idx[("T", round(float(p[3]), 1), round(float(p[4]), 1))].add((int(p[1]), int(p[2])))
        for line in open(f"{asy}/sym.txt", errors="replace"):
            f = line.rstrip("\n").split("!")
            if f[0] != "S" or len(f) < 18:
                continue
            cls, sub, kind = f[9], f[10], f[12]
            try:
                if kind in ("LINE", "ARC"):
                    a = (round(float(f[14]), 1), round(float(f[15]), 1))
                    b = (round(float(f[16]), 1), round(float(f[17]), 1))
                    key = ("L",) + tuple(sorted([a, b]))
                elif kind == "TEXT":
                    key = ("T", round(float(f[14]), 1), round(float(f[15]), 1))
                else:
                    continue
            except ValueError:
                continue
            for cs in idx.get(key, ()):
                votes[cs][(cls, sub)] += 1
    for cs in sorted(votes):
        best = votes[cs].most_common(3)
        print(f"{cs[0]:#04x}/{cs[1]:#04x} n={sum(votes[cs].values()):6d}  " + "; ".join(f"{c}/{s}:{n}" for (c, s), n in best))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["place", "classes"])
    ap.add_argument("--corpus", required=True)
    a = ap.parse_args()
    if not (EXAMPLES / "brd_place").exists():
        sys.exit("build first: cargo build --release -p eda-parse-orcad --examples")
    {"place": cmd_place, "classes": cmd_classes}[a.what](a.corpus)


if __name__ == "__main__":
    main()
