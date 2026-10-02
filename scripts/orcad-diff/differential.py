#!/usr/bin/env python3
"""The OrCAD differential: our extraction against KiCad's own OrCAD importers.

KiCad (master, late 2026) imports both halves of an OrCAD project: Allegro
boards (`kicad-cli pcb import`) and Capture designs (`kicad-cli sch import`).
It is an independent implementation of the same formats, so where the two
readings agree we are not merely asserting our own beliefs twice.

Like the Altium differential, the reference is a DEVELOPMENT-TIME ORACLE:
available locally, never vendored, never a build dependency. Point
`--kicad-cli` (or `KICAD_CLI`) at a build that has the importers.

Boards compare, per board:
  * placements  - every reference designator, position (mm), rotation, side;
  * pad nets    - two pads share a net in ours iff they do in KiCad's;
  * counts      - footprints, track segments+arcs, vias.
Designs compare, per design:
  * pin grouping - two pins share a net in ours iff they do in KiCad's netlist;
  * net names    - the name each group carries (sheet paths stripped).

Run:
  cargo build -p extract --bin pcb-extract --release
  python scripts/orcad-diff/differential.py --corpus <dir> [--boards-only|--designs-only] [--limit N]

Exit 1 means there is something to triage, not that the build is broken. The
full report goes to output/orcad-diff/differential.json.
"""

import argparse
import json
import math
import os
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXTRACT = ROOT / "src-tauri" / "target" / "release" / "pcb-extract"


# ---------------------------------------------------------------- s-expressions

def sexpr(text):
    """Parse a KiCad s-expression file into nested lists of strings."""
    tok = re.compile(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+')
    stack, cur = [], []
    for m in tok.finditer(text):
        t = m.group(0)
        if t == "(":
            stack.append(cur)
            cur = []
        elif t == ")":
            done = cur
            cur = stack.pop()
            cur.append(done)
        else:
            cur.append(t[1:-1].replace('\\"', '"') if t.startswith('"') else t)
    return cur[0] if cur else []


def kids(node, name):
    return [c for c in node if isinstance(c, list) and c and c[0] == name]


def kid(node, name):
    k = kids(node, name)
    return k[0] if k else None


def kicad_board(path):
    """Footprints (ref -> x, y, angle, side), pad nets and counts."""
    b = sexpr(Path(path).read_text(errors="replace"))
    fps, pads = {}, {}
    for fp in kids(b, "footprint"):
        ref = next((p[2] for p in kids(fp, "property") if len(p) > 2 and p[1] == "Reference"), "")
        # Older files place a footprint with (at x y a); newer ones with
        # (transform (translate x y) (rotate a) ...).
        tr = kid(fp, "transform")
        if tr:
            t = kid(tr, "translate") or ["translate", "0", "0"]
            r = kid(tr, "rotate") or ["rotate", "0"]
            x, y, ang = float(t[1]), float(t[2]), float(r[1])
        else:
            at = kid(fp, "at") or ["at", "0", "0"]
            x, y, ang = float(at[1]), float(at[2]), float(at[3]) if len(at) > 3 else 0.0
        side = "bottom" if (kid(fp, "layer") or ["", "F.Cu"])[1].startswith("B.") else "top"
        fps[ref] = (x, y, ang, side)
        for pad in kids(fp, "pad"):
            net = kid(pad, "net")
            name = net[-1] if net else ""
            if name and name != "0":
                pads[(ref.upper(), str(pad[1]).upper())] = name
    counts = {
        "footprints": len(fps),
        "tracks": len(kids(b, "segment")) + len(kids(b, "arc")),
        "vias": len(kids(b, "via")),
    }
    return fps, pads, counts


# ---------------------------------------------------------------- our side

def ours_extract(src, out):
    r = subprocess.run([str(EXTRACT), "design", str(src), "-o", str(out)], capture_output=True, text=True, timeout=1800)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip()[:300])
    m = json.loads((out / "design_review_manifest.json").read_text())
    return m, json.loads((out / m["design_json"]).read_text())


def ours_board(out):
    g = json.loads((out / "pcb" / "geometry.json").read_text())
    side = {i: ("bottom" if l.get("side") == "back" else "top") for i, l in enumerate(g["layers"])}
    fps = {c["ref"]: (c["x"], c["y"], c["angle"], side.get(c["layer"], "top")) for c in g["components"]}
    nets = g["nets"]
    pads = {}
    for p in g["pads"]:
        if p["comp"] >= 0 and p["net"]:
            pads[(g["components"][p["comp"]]["ref"].upper(), p["num"].upper())] = nets[p["net"]]
    counts = {
        "footprints": len(g["components"]),
        "tracks": len(g["tracks"]["seg"]["w"]) + len(g["tracks"]["arc"]["w"]),
        "vias": len(g["vias"]),
    }
    return fps, pads, counts


# ---------------------------------------------------------------- comparison

def grouping(a, b):
    """Pins two sides both know, and how many sit in the group the other side
    puts them in (each of our groups mapped to its majority partner)."""
    common = [k for k in a if k in b]
    votes = defaultdict(Counter)
    for k in common:
        votes[a[k]][b[k]] += 1
    alike = sum(1 for k in common if votes[a[k]].most_common(1)[0][0] == b[k])
    return len(common), alike


def angle_eq(ours, theirs, side):
    # Ours is the IR's (Y-down, clockwise-positive); KiCad's is counter-clockwise
    # on the top. KiCad flips a bottom footprint about the X axis, which turns
    # it by a further 180 degrees: its angle there is 180 minus ours.
    d = ((ours + theirs - 180.0) if side == "bottom" else (ours + theirs)) % 360.0
    return min(d, 360.0 - d) < 0.01


def diff_board(brd, kicad, tmp):
    ref_pcb = tmp / "ref.kicad_pcb"
    r = subprocess.run([kicad, "pcb", "import", "-o", str(ref_pcb), str(brd)], capture_output=True, text=True, timeout=1800)
    if r.returncode != 0 or not ref_pcb.exists():
        return {"board": str(brd), "reference_failed": (r.stderr or r.stdout).strip()[-300:]}
    kf, kp, kc = kicad_board(ref_pcb)
    try:
        _, _ = ours_extract(brd, tmp / "ours")
    except RuntimeError as e:
        return {"board": str(brd), "ours_failed": str(e)}
    of, op, oc = ours_board(tmp / "ours")
    placed, misplaced = 0, []
    for ref, (x, y, a, s) in kf.items():
        # A footprint with no reference designator cannot be paired by name.
        if not ref:
            continue
        # KiCad names a footprint placed without a component after its
        # reference text, prefixed `UNK`; ours keeps the text as drawn.
        o = of.get(ref) or (of.get(ref[3:]) if ref.startswith("UNK") else None)
        if o and abs(o[0] - x) < 0.01 and abs(o[1] - y) < 0.01 and angle_eq(o[2], a, s) and o[3] == s:
            placed += 1
        elif len(misplaced) < 10:
            misplaced.append({"ref": ref, "kicad": [x, y, a, s], "ours": list(o) if o else None})
    pins, alike = grouping(op, kp)
    return {
        "board": str(brd),
        "placements": {"kicad": sum(1 for r in kf if r), "agree": placed, "examples": misplaced},
        "pad_nets": {"pins_both": pins, "grouped_alike": alike, "kicad_pins": len(kp), "our_pins": len(op)},
        "counts": {"ours": oc, "kicad": kc},
    }


def kicad_netlist(xml_path):
    root = ET.parse(xml_path).getroot()
    out = {}
    for net in root.iter("net"):
        # KiCad prefixes a sheet path (`/sheet/NAME`); Capture names are bare.
        # A Capture name can itself hold '/', so the full name is kept and
        # compared by its tail.
        name = net.get("name", "")
        for node in net.iter("node"):
            out[(node.get("ref", "").upper(), node.get("pin", "").upper())] = name
    return out


def diff_design(dsn, kicad, tmp):
    sch = tmp / "ref.kicad_sch"
    r = subprocess.run([kicad, "sch", "import", "--format", "orcad", "-o", str(sch), str(dsn)], capture_output=True, text=True, timeout=1800)
    if r.returncode != 0 or not sch.exists():
        return {"design": str(dsn), "reference_failed": (r.stderr or r.stdout).strip()[-300:]}
    xml = tmp / "ref.xml"
    r = subprocess.run([kicad, "sch", "export", "netlist", "--format", "kicadxml", "-o", str(xml), str(sch)], capture_output=True, text=True, timeout=1800)
    if r.returncode != 0 or not xml.exists():
        return {"design": str(dsn), "reference_failed": "netlist export: " + (r.stderr or r.stdout).strip()[-300:]}
    theirs = kicad_netlist(xml)
    try:
        _, d = ours_extract(dsn, tmp / "ours")
    except RuntimeError as e:
        return {"design": str(dsn), "ours_failed": str(e)}
    ours = {}
    for n in d["nets"]:
        for t in n.get("terminals", []):
            ours[(t["designator"].upper(), t["pin"].upper())] = n["name"]
    pins, alike = grouping(ours, theirs)
    votes = defaultdict(Counter)
    for k in ours:
        if k in theirs:
            votes[ours[k]][theirs[k]] += 1
    split = [
        {"pin": "%s.%s" % k, "ours": ours[k], "kicad": theirs[k]}
        for k in ours
        if k in theirs and votes[ours[k]].most_common(1)[0][0] != theirs[k]
    ][:15]
    # A generated name (N01234) is each tool's own invention: only explicit
    # names are compared.
    named = [k for k in ours if k in theirs and not re.fullmatch(r"N\d+.*", ours[k])]
    # Capture's netlister suffixes a name local to a reused folder instance
    # with `_<block>` where the bare name collides; KiCad puts the instance in
    # a sheet path instead (stripped above). Same base name, two conventions.
    def same_name(o, t):
        o, t = o.upper(), t.upper()
        tails = {t, t.rsplit("/", 1)[-1]} | ({t[1:]} if t.startswith("/") else set())
        return any(o == x or o.startswith(x + "_") or t.endswith("/" + o) for x in tails)
    same = sum(1 for k in named if same_name(ours[k], theirs[k]))
    renamed = sorted({(ours[k], theirs[k]) for k in named if not same_name(ours[k], theirs[k])})[:15]
    return {
        "grouping_examples": split,
        "naming_examples": renamed,
        "design": str(dsn),
        "pins": {"both": pins, "grouped_alike": alike, "kicad": len(theirs), "ours": len(ours)},
        "names": {"explicitly_named_pins": len(named), "same": same},
    }


def find(corpus, exts):
    out = []
    for p in sorted(Path(corpus).rglob("*")):
        if p.suffix.lower() in exts and p.is_file() and ".git" not in p.parts:
            out.append(p)
    return out


def is_cfb(p):
    with open(p, "rb") as f:
        return f.read(8) == bytes.fromhex("d0cf11e0a1b11ae1")


def is_allegro(p):
    with open(p, "rb") as f:
        b = f.read(4)
    return len(b) == 4 and 0x000F <= (int.from_bytes(b, "little") >> 16) <= 0x0016


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--kicad-cli", default=os.environ.get("KICAD_CLI", "kicad-cli"))
    ap.add_argument("--boards-only", action="store_true")
    ap.add_argument("--designs-only", action="store_true")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--out", default=str(ROOT / "output" / "orcad-diff"))
    a = ap.parse_args()
    if not EXTRACT.exists():
        sys.exit(f"build first: {EXTRACT} not found")
    report = {"boards": [], "designs": []}
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    if not a.designs_only:
        boards = [p for p in find(a.corpus, {".brd"}) if is_allegro(p)]
        for brd in boards[: a.limit or None]:
            with tempfile.TemporaryDirectory() as t:
                r = diff_board(brd, a.kicad_cli, Path(t))
            report["boards"].append(r)
            print(summary_board(r), flush=True)
    if not a.boards_only:
        designs = [p for p in find(a.corpus, {".dsn"}) if is_cfb(p)]
        for dsn in designs[: a.limit or None]:
            with tempfile.TemporaryDirectory() as t:
                r = diff_design(dsn, a.kicad_cli, Path(t))
            report["designs"].append(r)
            print(summary_design(r), flush=True)
    totals = total(report)
    report["totals"] = totals
    (out / "differential.json").write_text(json.dumps(report, indent=1))
    print(json.dumps(totals, indent=1))
    clean = totals["boards"]["placements_agree"] == totals["boards"]["placements"] and \
        totals["boards"]["pins_alike"] == totals["boards"]["pins"] and \
        totals["designs"]["pins_alike"] == totals["designs"]["pins"]
    sys.exit(0 if clean else 1)


def summary_board(r):
    name = Path(r["board"]).name
    if "placements" not in r:
        return f"BOARD {name}: {r.get('reference_failed') or r.get('ours_failed')}"
    p, n, c = r["placements"], r["pad_nets"], r["counts"]
    return (f"BOARD {name}: placements {p['agree']}/{p['kicad']}, pad nets {n['grouped_alike']}/{n['pins_both']}"
            f" (kicad {n['kicad_pins']}, ours {n['our_pins']}), counts ours {c['ours']} kicad {c['kicad']}")


def summary_design(r):
    name = Path(r["design"]).name
    if "pins" not in r:
        return f"DESIGN {name}: {r.get('reference_failed') or r.get('ours_failed')}"
    p, n = r["pins"], r["names"]
    return (f"DESIGN {name}: pins grouped alike {p['grouped_alike']}/{p['both']} (kicad {p['kicad']}, ours {p['ours']}),"
            f" explicit names same {n['same']}/{n['explicitly_named_pins']}")


def total(report):
    t = {"boards": Counter(), "designs": Counter()}
    for r in report["boards"]:
        if "placements" in r:
            t["boards"]["compared"] += 1
            t["boards"]["placements"] += r["placements"]["kicad"]
            t["boards"]["placements_agree"] += r["placements"]["agree"]
            t["boards"]["pins"] += r["pad_nets"]["pins_both"]
            t["boards"]["pins_alike"] += r["pad_nets"]["grouped_alike"]
        else:
            t["boards"]["not_compared"] += 1
    for r in report["designs"]:
        if "pins" in r:
            t["designs"]["compared"] += 1
            t["designs"]["pins"] += r["pins"]["both"]
            t["designs"]["pins_alike"] += r["pins"]["grouped_alike"]
            t["designs"]["named"] += r["names"]["explicitly_named_pins"]
            t["designs"]["named_same"] += r["names"]["same"]
        else:
            t["designs"]["not_compared"] += 1
    return {k: dict(v) for k, v in t.items()}


if __name__ == "__main__":
    main()
