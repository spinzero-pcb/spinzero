#!/usr/bin/env python3
"""The plan §8.3 differential: our Altium front-end against the reference parser.

`docs/altium-extraction-plan.md` §8.3 ends every phase with a differential run
against a mature independent implementation of the same formats, because the §10
corner cases are exactly the class of bug our own tests cannot find — we would be
asserting the same wrong belief in the code and in the test.

The reference stays a **development-time oracle**: available locally, never
vendored, never a build dependency, never shipped.  This harness drives its CLI
(and, for M0 only, `ref_probe.py` inside its interpreter) and diffs a normalised
comparison document against ours.

Both sides are normalised the same way, and normalisation absorbs only the
differences we *intend* (plan §7): units, our neutral vocabularies, our field
names, sorted collections, `%UTF8%` key folding.  What survives is a real
disagreement, and every one of them resolves to exactly one of the plan's three
triage outcomes — our bug, an intended divergence, or a difference in the
reference.  Nothing here decides that; it only reports.

Configuration, all optional, all environment (the reference's location is
configured locally rather than written into the repo):

    SPINZERO_ALTIUM_REFERENCE         reference CLI            (default: altium-cruncher on PATH)
    SPINZERO_ALTIUM_REFERENCE_PYTHON  interpreter that can import the reference
                                      (default: the CLI's own venv, then this one)
    SPINZERO_ALTIUM_CORPUS            corpus root              (default: D:\\git_repo\\reference_designs)
    SPINZERO_PCB_EXTRACT              our CLI                  (default: the release build)

Usage:

    python scripts/altium-diff/differential.py                 # M0 and M1, whole corpus
    python scripts/altium-diff/differential.py --phase m0
    python scripts/altium-diff/differential.py --design EVAL_FFXMR12MM1H
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterable

REPO = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = Path(r"D:\git_repo\reference_designs")
DEFAULT_EXTRACT = REPO / "src-tauri" / "target" / "release" / "pcb-extract.exe"
if not DEFAULT_EXTRACT.exists():  # non-Windows dev box
    DEFAULT_EXTRACT = REPO / "src-tauri" / "target" / "release" / "pcb-extract"

# How many examples of one difference class are printed before the rest are
# counted.  A differential that prints 4000 lines does not get triaged.
EXAMPLES = 6


# --------------------------------------------------------------------------- #
# configuration
# --------------------------------------------------------------------------- #


class Config:
    def __init__(self, args: argparse.Namespace) -> None:
        self.corpus = Path(
            args.corpus or os.environ.get("SPINZERO_ALTIUM_CORPUS") or DEFAULT_CORPUS
        )
        self.extract = Path(
            args.extract or os.environ.get("SPINZERO_PCB_EXTRACT") or DEFAULT_EXTRACT
        )
        self.reference = (
            args.reference
            or os.environ.get("SPINZERO_ALTIUM_REFERENCE")
            or shutil.which("altium-cruncher")
            or "altium-cruncher"
        )
        self.reference_python = (
            os.environ.get("SPINZERO_ALTIUM_REFERENCE_PYTHON") or self._guess_python()
        )
        self.out = Path(args.out) if args.out else REPO / "output" / "altium-diff"
        self.oracle = Path(
            args.oracle or os.environ.get("SPINZERO_ALTIUM_ORACLE") or self.out / "oracle"
        )
        self.oracle_mode = (
            "build" if args.build_oracle else "offline" if args.offline else "use"
        )
        self.reference_version = "unknown"

    def _guess_python(self) -> str:
        """The reference CLI's own interpreter, so `import altium_monkey` works.

        A uv tool install puts the executable in `<venv>/Scripts` (or `bin`)
        beside the interpreter; a shim on PATH is followed to its real home.
        """
        exe = Path(self.reference)
        for candidate in (exe.parent / "python.exe", exe.parent / "python"):
            if candidate.exists():
                return str(candidate)
        for root in (
            Path.home() / "AppData/Roaming/uv/tools/altium-cruncher",
            Path.home() / ".local/share/uv/tools/altium-cruncher",
        ):
            for candidate in (root / "Scripts/python.exe", root / "bin/python"):
                if candidate.exists():
                    return str(candidate)
        return sys.executable

    def check(self) -> list[str]:
        problems = []
        if self.oracle_mode == "offline" and not self.oracle.is_dir():
            problems.append(
                f"no recorded oracle corpus at {self.oracle}"
                " — run once with --build-oracle"
            )
        if not self.corpus.is_dir():
            problems.append(f"corpus not found: {self.corpus} (set SPINZERO_ALTIUM_CORPUS)")
        if not Path(self.extract).exists():
            problems.append(
                f"pcb-extract not found: {self.extract}"
                " (cargo build -p extract --bin pcb-extract --release)"
            )
        try:
            proc = subprocess.run(
                [self.reference, "version"],
                capture_output=True,
                timeout=120,
                check=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            # Every recorded answer is stamped with this, so an upgraded
            # reference makes the corpus stale instead of silently disagreeing.
            self.reference_version = " ".join(
                line.strip() for line in (proc.stdout or "").splitlines() if line.strip()
            ) or "unknown"
        except Exception as exc:
            problems.append(
                f"reference CLI not runnable: {self.reference} ({exc})"
                " — set SPINZERO_ALTIUM_REFERENCE"
            )
        probe = subprocess.run(
            [self.reference_python, "-c", "import altium_monkey"],
            capture_output=True,
            timeout=120,
        )
        if probe.returncode != 0:
            problems.append(
                f"reference library not importable by {self.reference_python}"
                " — set SPINZERO_ALTIUM_REFERENCE_PYTHON (M0 needs it; M1 does not)"
            )
        return problems


# --------------------------------------------------------------------------- #
# corpus discovery — mirrors crates/extract/tests/altium_corpus.rs
# --------------------------------------------------------------------------- #


class Design:
    def __init__(self, entry: Path, board: Path | None) -> None:
        self.entry = entry
        self.board = board
        self.name = entry.stem
        # The `.PrjPcb`, or None for a directory that has none. A project-less
        # design is the one place the two sides disagree about what "the design"
        # even is; see `compare_m1_design`.
        self.project = entry if entry.suffix.lower() == ".prjpcb" else None

    @property
    def documents(self) -> list[Path]:
        """Every Altium document in the design's directory, sorted."""
        parent = self.entry.parent
        docs = [
            p
            for p in sorted(parent.iterdir())
            if p.suffix.lower() in (".schdoc", ".pcbdoc")
        ]
        return docs


def discover(root: Path) -> list[Design]:
    projects: list[Path] = []
    loose_dirs: dict[Path, list[Path]] = {}
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        suffix = path.suffix.lower()
        if suffix == ".prjpcb":
            projects.append(path)
    project_dirs = {p.parent for p in projects}
    for path in root.rglob("*.SchDoc"):
        if path.parent in project_dirs:
            continue
        loose_dirs.setdefault(path.parent, []).append(path)

    designs: list[Design] = []
    for project in sorted(projects):
        designs.append(Design(project, sibling(project, ".pcbdoc")))
    # A project-less directory is ONE design however it is entered: enter it
    # through the schematic whose name is closest to the board's, so the bundle
    # is named after the design rather than after its disclaimer sheet.
    for directory, schematics in sorted(loose_dirs.items()):
        board = sibling(schematics[0], ".pcbdoc")
        if board is None:
            continue
        target = board.stem.lower()
        schematics.sort(key=lambda s: (-shared_prefix(s.stem.lower(), target), s.name))
        designs.append(Design(schematics[0], board))
    return designs


def sibling(path: Path, suffix: str) -> Path | None:
    for candidate in sorted(path.parent.iterdir()):
        if candidate.suffix.lower() == suffix:
            return candidate
    return None


def shared_prefix(a: str, b: str) -> int:
    n = 0
    for x, y in zip(a, b):
        if x != y:
            break
        n += 1
    return n


# --------------------------------------------------------------------------- #
# difference reporting
# --------------------------------------------------------------------------- #


class Report:
    """A flat list of difference classes, each with a count and examples."""

    def __init__(self) -> None:
        self.classes: dict[str, list[str]] = {}
        self.counts: dict[str, int] = {}
        self.notes: list[str] = []

    def add(self, kind: str, example: str) -> None:
        self.counts[kind] = self.counts.get(kind, 0) + 1
        bucket = self.classes.setdefault(kind, [])
        if len(bucket) < EXAMPLES:
            bucket.append(example)

    def note(self, line: str) -> None:
        self.notes.append(line)

    @property
    def total(self) -> int:
        return sum(self.counts.values())

    def render(self, indent: str = "  ") -> list[str]:
        out = []
        for line in self.notes:
            out.append(f"{indent}. {line}")
        for kind in sorted(self.counts, key=lambda k: -self.counts[k]):
            count = self.counts[kind]
            out.append(f"{indent}! {kind}: {count}")
            for example in self.classes[kind]:
                out.append(f"{indent}    {example}")
            if count > len(self.classes[kind]):
                out.append(f"{indent}    … and {count - len(self.classes[kind])} more")
        if not self.counts:
            out.append(f"{indent}= agrees")
        return out

    def as_json(self) -> dict[str, Any]:
        return {
            "notes": self.notes,
            "differences": {
                kind: {"count": self.counts[kind], "examples": self.classes[kind]}
                for kind in sorted(self.counts)
            },
        }


def short(value: Any, width: int = 60) -> str:
    text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
    text = text.replace("\n", "\\n")
    return text if len(text) <= width else text[: width - 1] + "…"


# --------------------------------------------------------------------------- #
# running the two sides
# --------------------------------------------------------------------------- #


def run(cmd: list[str], cwd: Path | None = None, timeout: int = 1800) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )


def our_dump(cfg: Config, document: Path) -> dict[str, Any] | None:
    proc = run([str(cfg.extract), "dump", str(document), "--full"])
    if proc.returncode != 0:
        return None
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None


def ref_dump(cfg: Config, document: Path) -> dict[str, Any] | None:
    probe = Path(__file__).with_name("ref_probe.py")
    proc = run([cfg.reference_python, str(probe), str(document)])
    if proc.returncode != 0:
        return None
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None


def our_design(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    proc = run([str(cfg.extract), "design", str(design.entry), "-o", str(out)])
    if proc.returncode != 0:
        return None
    return load_one(out, "_design.json")


def ref_design(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    proc = run(
        [cfg.reference, "design", design.entry.name, "-o", str(out)],
        cwd=design.entry.parent,
    )
    if proc.returncode != 0:
        return None
    return load_one(out, "_design.json")


def our_bom(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """Our GROUPED BOM, which is the CSV.

    `pcb-extract bom --format grouped-json` is the *flat* document the app
    ingests — one row per component, because grouping folds onto whichever
    fields the active preset groups by and the extractor cannot know them. The
    fab CSV is the one that coalesces identical parts, so that is what the
    reference's grouped JSON is comparable to.
    """
    proc = run(
        [str(cfg.extract), "bom", str(design.entry), "--format", "grouped-csv", "-o", str(out)]
    )
    if proc.returncode != 0:
        return None
    for path in sorted(out.rglob("*_bom.csv")):
        return read_grouped_csv(path)
    return None


def our_geometry(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """Our board geometry document, from a full design run."""
    proc = run([str(cfg.extract), "design", str(design.entry), "-o", str(out)])
    if proc.returncode != 0:
        return None
    path = out / "pcb" / "geometry.json"
    if not path.exists():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def our_render(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """Our rendered sheets, reduced to per-sheet object-class counts."""
    proc = run([str(cfg.extract), "design", str(design.entry), "-o", str(out)])
    if proc.returncode != 0:
        return None
    manifest = out / "design_review_manifest.json"
    if not manifest.exists():
        return None
    doc = json.loads(manifest.read_text(encoding="utf-8"))
    sheets: dict[str, dict[str, int]] = {}
    for entry in doc.get("schematic_svgs", []):
        path = out / entry["file"]
        if not path.exists():
            continue
        sheets[stem_key(entry.get("sheet_name", entry["file"]))] = index_our_svg(
            path.read_text(encoding="utf-8")
        )
    return {"sheets": sheets}


def stem_key(name: str) -> str:
    """A sheet's identity across the two sides: the document name without its
    extension, lower-cased.

    Only a trailing `.SchDoc` is stripped, and only that. `Path.stem` drops
    whatever follows the LAST dot, which turns `REF_5BR3995BZ_16W1 V1.0` into
    `… V1` on one side and leaves it whole on the other, so the two sides stop
    joining and every sheet reads as missing.
    """
    name = Path(name).name
    if name.lower().endswith(".schdoc"):
        name = name[: -len(".schdoc")]
    return name.lower()


# Our `data-primitive` (plus `data-kind` where it splits a class) mapped onto the
# comparison's own vocabulary. Only classes BOTH sides express are listed; the
# rest are reported as not compared rather than silently dropped.
OUR_CLASS = {
    ("symbol", ""): "component",
    ("pin", ""): "pin",
    ("wire", ""): "wire",
    ("bus", ""): "bus",
    ("label", ""): "netlabel",
    ("port", "global"): "port",
    ("port", "hier"): "sheetentry",
    ("power-symbol", ""): "power",
    ("sheet-symbol", ""): "sheet",
    ("netclass-flag", ""): "parameterset",
    ("image", ""): "image",
    ("graphic", "compile-mask"): "blanket",
    # A `RECORD=211` region masks a sheet too, but it is a different record and
    # the reference has no kind for it, so it is not compared.
    ("image", "linked"): "image",
    ("text", "designator"): "designator",
    ("text", "field"): "parameter",
}

PRIMITIVE_RE = re.compile(r'<g ([^>]*?)data-primitive="([a-z-]+)"([^>]*)>')
KIND_RE = re.compile(r'data-kind="([a-z-]+)"')
UUID_RE = re.compile(r'data-uuid="([^"]*)"')


def index_our_svg(svg: str) -> dict[str, list[str]]:
    """Our drawn objects, by class, each named by the `UniqueID` the file gave it.

    Identity rather than a count: a difference then says WHICH object, and a
    class whose totals happen to match for two opposite reasons stops looking
    like agreement.

    Our SVG also carries a `~`-prefixed handle for an object the file left
    UNNAMED — Altium names no junction, and drops the id on a few percent of pins
    and graphics — so that the viewer can frame it and the diff can pair it. The
    reference has no id for those at all, so the handle is dropped back to "" here
    and the class falls to the count comparison the empty case already takes.
    """
    out: dict[str, list[str]] = {}
    for before, primitive, after in PRIMITIVE_RE.findall(svg):
        attrs = before + after
        kind = KIND_RE.search(attrs)
        key = OUR_CLASS.get((primitive, kind.group(1) if kind else ""))
        if key is None and primitive == "power-symbol":
            # A power port's `data-kind` is its Altium style number, not a class.
            key = "power"
        if key is None:
            continue
        uuid = UUID_RE.search(attrs)
        found = uuid.group(1) if uuid else ""
        out.setdefault(key, []).append("" if found.startswith("~") else found)
    return out


# The reference's on-screen record kinds, in the same vocabulary. `parameter`
# and `designator` records exist for every parameter a part carries, drawn or
# not, so they are counted only when the record actually draws a string — the
# same rule M2 applies to board text with nothing to draw.
REF_CLASS = {
    "component": "component",
    "pin": "pin",
    "wire": "wire",
    "bus": "bus",
    "netlabel": "netlabel",
    "port": "port",
    "sheetentry": "sheetentry",
    "power": "power",
    "sheet_symbol": "sheet",
    "sheetsymbol": "sheet",
    "parameterset": "parameterset",
    "image": "image",
    "blanket": "blanket",
    "designator": "designator",
    "parameter": "parameter",
}
TEXT_CLASSES = {"designator", "parameter"}


def ref_render(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """The reference's on-screen geometry for every sheet, reduced to the same
    per-sheet class counts.

    `sch-ir` is Altium's own drawing-operation oracle rather than a second SVG,
    which is why it can be compared at all: an SVG's element count depends on
    how each side chose to draw a symbol, and the operation record does not.
    """
    proc = run([cfg.reference, "sch-ir", str(design.entry), "-o", str(out)])
    if proc.returncode != 0:
        return None
    sheets: dict[str, dict[str, int]] = {}
    for path in sorted(out.rglob("*.gotir.json")):
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        counts: dict[str, list[str]] = {}
        for record in doc.get("records", []):
            key = REF_CLASS.get(str(record.get("kind", "")).lower())
            if key is None:
                continue
            # The reference emits an operation record for every primitive the
            # placement OWNS, then marks the ones it does not put on the page.
            # A multi-part symbol is where that shows: the MCU144E1 ADC sheet
            # places part 1 of `U1` and the reference lists all 334 of the
            # symbol's pins, 186 of them flagged. Altium draws only the placed
            # part's pins (plan corner case 32), and so do we, so the flag is
            # the reference's own statement of what is on the page.
            if record.get("skip_svg"):
                continue
            if key in TEXT_CLASSES and not any(
                op.get("type") == "gotString" for op in record.get("operations", [])
            ):
                continue
            # `handle` is `<document>\\<UniqueID>`, which is the same id our
            # `data-uuid` carries — so the two sides can be joined object by
            # object instead of only counted.
            counts.setdefault(key, []).append(str(record.get("handle", "")).rsplit("\\", 1)[-1])
        sheets[stem_key(Path(doc.get("source_path", path.name)).name)] = counts
    return {"sheets": sheets} if sheets else None


def ref_board(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """The reference's parse of the board, reduced to what M2 compares."""
    if design.board is None:
        return None
    proc = run([cfg.reference, "json-dump", str(design.board), "-o", str(out)])
    if proc.returncode != 0:
        return None
    for path in sorted(out.rglob("*.json")):
        if path.name == "manifest.json":
            continue
        return reduce_board(json.loads(path.read_text(encoding="utf-8")))
    return None


def ref_pnp(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    """Altium's own pick-and-place semantics, which is what a component centroid
    has to agree with (corner case 23). It needs a project file; a loose
    document has none, and that comparison is skipped for it."""
    if design.project is None:
        return None
    proc = run(
        [
            cfg.reference, "pnp", design.project.name, "--format", "json",
            "--units", "mm", "--position-mode", "altium-pick-place", "-o", str(out),
        ],
        cwd=design.project.parent,
    )
    if proc.returncode != 0:
        return None
    # A project with variants writes one file per variant; the base one is the
    # file with no variant in its name.
    files = sorted(out.glob("*_pnp.json"), key=lambda p: len(p.name))
    if not files:
        return None
    doc = json.loads(files[0].read_text(encoding="utf-8"))
    return {
        "placements": {
            p["designator"]: [p["center_x"], p["center_y"], p.get("rotation", 0.0)]
            for p in doc.get("placements", [])
        }
    }


def read_grouped_csv(path: Path) -> dict[str, Any]:
    import csv

    lines = []
    with path.open(encoding="utf-8", newline="") as handle:
        for index, row in enumerate(csv.DictReader(handle), start=1):
            designators = [d.strip() for d in (row.get("designators") or "").split(",") if d.strip()]
            lines.append(
                {
                    "item": index,
                    "quantity": int(row.get("quantity") or 0),
                    "designators": designators,
                }
            )
    return {
        "lines": lines,
        "line_count": len(lines),
        "component_count": sum(len(line["designators"]) for line in lines),
    }


def ref_bom(cfg: Config, design: Design, out: Path) -> dict[str, Any] | None:
    proc = run(
        [cfg.reference, "bom", design.entry.name, "--format", "grouped-json", "-o", str(out)],
        cwd=design.entry.parent,
    )
    if proc.returncode != 0:
        return None
    return load_one(out, "_bom.json")


def load_one(root: Path, ending: str) -> dict[str, Any] | None:
    for path in sorted(root.rglob("*.json")):
        if path.name.endswith(ending):
            return json.loads(path.read_text(encoding="utf-8"))
    return None




# --------------------------------------------------------------------------- #
# timing
# --------------------------------------------------------------------------- #
#
# What each side costs to extract one design, which is the other reason the
# oracle corpus exists. Our side is measured on every run; the reference's time
# is measured when it is ASKED and recorded beside its answer, so a later run
# served from the corpus still reports what that answer cost to produce.


class Clock:
    """Wall-clock seconds per named span, for one design."""

    def __init__(self) -> None:
        self.spans: dict[str, float] = {}
        self.recorded: set[str] = set()
        self.unknown: set[str] = set()

    @contextmanager
    def measure(self, key: str):
        start = time.perf_counter()
        try:
            yield
        finally:
            self.add(key, time.perf_counter() - start)

    def add(self, key: str, seconds: float | None, from_corpus: bool = False) -> None:
        # A recorded answer whose cost was never measured — one carried over from
        # a reference version that is no longer installed — is counted as
        # UNKNOWN rather than as zero, so the totals stay honest.
        if seconds is None:
            self.unknown.add(key)
        else:
            self.spans[key] = self.spans.get(key, 0.0) + seconds
        if from_corpus:
            self.recorded.add(key)

    def total(self, prefix: str) -> float:
        return sum(v for k, v in self.spans.items() if k.startswith(prefix))

    def line(self, side: str, prefix: str) -> str:
        parts = sorted(
            (k[len(prefix) :], v) for k, v in self.spans.items() if k.startswith(prefix)
        )
        detail = ", ".join(f"{name} {value:.1f}s" for name, value in parts if value >= 0.05)
        stamp = " [from the corpus]" if any(k.startswith(prefix) for k in self.recorded) else ""
        missing = sorted(k[len(prefix) :] for k in self.unknown if k.startswith(prefix))
        if missing:
            stamp += f" + {', '.join(missing)} not measured"
        return f"{side} {self.total(prefix):.1f}s ({detail}){stamp}"

    def as_json(self) -> dict[str, Any]:
        return {
            "seconds": {k: round(v, 3) for k, v in sorted(self.spans.items())},
            "ours_total": round(self.total("ours:"), 3),
            "reference_total": round(self.total("ref:"), 3),
            "reference_from_corpus": sorted(self.recorded),
            "not_measured": sorted(self.unknown),
        }


# --------------------------------------------------------------------------- #
# the oracle corpus
# --------------------------------------------------------------------------- #
#
# Running the reference over the corpus takes minutes per design — it renders
# every sheet and every board layer on its way to the JSON we actually read. So
# its answers are recorded ONCE, per document, and kept: that recording is the
# corpus this differential runs against from then on, and a design added to
# `reference_designs` joins it by being recorded the same way.
#
# What is stored is the REDUCED reference document — the fields the comparison
# reads — not the bundle. The bundle is tens of megabytes of SVG per design and
# none of it is compared.
#
# Each entry carries the reference's version and the digest of the source file
# it was read from, so a design that changes, or a reference that is upgraded,
# makes the entry stale and says so rather than quietly comparing against last
# year's answer.


def digest(path: Path) -> str:
    """SHA-256 of a source document, so an edited design invalidates its entry."""
    h = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def reduce_design(design: dict[str, Any]) -> dict[str, Any]:
    """The parts of the reference's design document the comparison reads."""
    hierarchy = design.get("schematic_hierarchy") or {}
    return {
        "components": [
            {
                "designator": c.get("designator"),
                "value": c.get("value"),
                "footprint": c.get("footprint"),
                "library_ref": c.get("library_ref"),
                "description": c.get("description"),
                "classification": c.get("classification"),
                "parameters": c.get("parameters"),
                "hierarchy": c.get("hierarchy"),
            }
            for c in design.get("components", [])
        ],
        "nets": [
            {
                "name": n.get("name"),
                "terminals": [
                    {"designator": t.get("designator"), "pin": t.get("pin")}
                    for t in n.get("terminals", [])
                ],
            }
            for n in design.get("nets", [])
        ],
        "schematic_hierarchy": {
            "documents": hierarchy.get("documents"),
            "hierarchy_paths": hierarchy.get("hierarchy_paths"),
        },
    }


# Kinds of primitive M2 counts per layer. The reference names them the way its
# own document does; our side derives the same partition from the geometry IR.
BOARD_KINDS = ("tracks", "arcs", "pads", "vias", "texts", "regions", "fills")


def drawn_text(fields: dict[str, Any], document: dict[str, Any]) -> bool:
    """Whether a board text resolves to something visible."""
    text = (fields.get("text_content") or "").strip()
    if not text:
        return False
    key = text.lower()
    if key not in (".designator", ".comment"):
        return True
    index = fields.get("component_index")
    components = document.get("components") or []
    if index is None or not (0 <= index < len(components)):
        return False
    owner = components[index]
    raw = owner.get("raw_record") or {}
    if key == ".designator":
        # A component whose `SOURCEDESIGNATOR` is blank still has a designator
        # when the board marks one of its texts as such — that is corner case 7
        # again, and the placeholder resolves to it.
        return bool(
            (owner.get("designator") or raw.get("SOURCEDESIGNATOR") or "").strip()
            or index in designated(document)
        )
    return bool((owner.get("comment") or raw.get("COMMENT") or "").strip())


def designated(document: dict[str, Any]) -> set[int]:
    """Components the board marks a designator text for."""
    cached = document.get("_designated")
    if cached is None:
        cached = {
            t["fields"]["component_index"]
            for t in document.get("texts") or []
            if t["fields"].get("is_designator") and (t["fields"].get("text_content") or "").strip()
            and t["fields"].get("component_index") is not None
        }
        document["_designated"] = cached
    return cached


def reduce_board(doc: dict[str, Any]) -> dict[str, Any]:
    """The parts of the reference's board document the M2 comparison reads.

    Everything is kept in the reference's own units (Altium's 1/10000 mil, and
    mils for the board origin); the normaliser converts once, on our side.
    """
    document = doc.get("document") or {}
    board = document.get("board") or {}
    counts: dict[str, int] = {}
    for kind in BOARD_KINDS:
        for item in document.get(kind) or []:
            fields = item.get("fields") or item
            # A text with nothing to draw is not a primitive we emit (plan §7),
            # so the reference's count is taken over the texts that resolve to
            # something. The rule is reproduced here, not our implementation of
            # it: a blank string, and a `.Designator` / `.Comment` whose owner
            # has neither, draw nothing in Altium either.
            if kind == "texts" and not drawn_text(fields, document):
                continue
            key = f"{kind}/{fields.get('layer')}"
            counts[key] = counts.get(key, 0) + 1

    def pad(item: dict[str, Any]) -> list[Any]:
        f, props = item["fields"], item.get("properties") or {}
        return [
            f["x"], f["y"], f["top_width"], f["top_height"],
            props.get("effective_top_shape", f["top_shape"]),
            props.get("corner_radius_percentage") or 0,
            f["hole_size"], f["rotation"], bool(f["is_plated"]),
            f["designator"], f["net_index"], f["component_index"], f["layer"],
        ]

    def via(item: dict[str, Any]) -> list[Any]:
        f = item["fields"]
        return [f["x"], f["y"], f["diameter"], f["hole_size"], f["net_index"]]

    stack, at = [], 1
    by_id = {l.get("layer_id"): l for l in board.get("layer_stackup") or []}
    while at and at not in stack and at in by_id:
        stack.append(at)
        at = by_id[at].get("layer_next") or 0
    # The stackup covers only the 32 copper layers; every layer a primitive can
    # name is in the board record's flat table, which is the join key the
    # per-layer counts need.
    raw = board.get("raw_record") or {}
    names = {str(i): raw.get(f"LAYER{i}NAME") for i in range(1, 83)}
    names.update({str(i): l["name"] for i, l in by_id.items() if l.get("name")})
    return {
        "origin": [board.get("origin_x"), board.get("origin_y")],
        "stack": stack,
        "layer_names": {i: n for i, n in names.items() if n},
        "counts": counts,
        "pads": [pad(p) for p in document.get("pads") or []],
        "vias": [via(v) for v in document.get("vias") or []],
        "nets": [n.get("name") for n in document.get("nets") or []],
    }


def reduce_bom(bom: dict[str, Any]) -> dict[str, Any]:
    return {
        "line_count": bom.get("line_count"),
        "component_count": bom.get("component_count"),
        "lines": [
            {
                "item": line.get("item"),
                "quantity": line.get("quantity"),
                "designators": line.get("designators"),
            }
            for line in bom.get("lines", [])
        ],
    }


class Oracle:
    """The recorded reference answers, one file per design."""

    def __init__(self, cfg: "Config") -> None:
        self.root = cfg.oracle
        self.version = cfg.reference_version
        self.mode = cfg.oracle_mode  # "use" | "build" | "offline"
        self.stale: list[str] = []
        self.kept: list[str] = []
        self.recorded = 0

    def path(self, design: "Design") -> Path:
        # Designs can share a stem across directories, so the file is named for
        # the design and disambiguated by a digest of its path.
        tag = hashlib.sha256(str(design.entry).encode("utf-8")).hexdigest()[:8]
        return self.root / f"{safe(design.name)}.{tag}.json"

    def load(self, design: "Design") -> dict[str, Any]:
        # Even a rebuild loads what is there: `fresh` is what decides whether an
        # answer is used, and keeping the file lets a failed re-ask fall back.
        path = self.path(design)
        if not path.exists():
            return {}
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            return {}

    def save(self, design: "Design", entry: dict[str, Any]) -> None:
        self.root.mkdir(parents=True, exist_ok=True)
        self.path(design).write_text(
            json.dumps(entry, indent=1, ensure_ascii=False), encoding="utf-8"
        )

    def held(self, entry: dict[str, Any], key: str) -> dict[str, Any] | None:
        """Whatever is recorded for `key`, fresh or not."""
        return (entry.get("answers") or {}).get(key)

    def fresh(self, entry: dict[str, Any], key: str, source: Path) -> dict[str, Any] | None:
        """The recorded answer for `key`, or None when it is missing or stale."""
        if self.mode == "build":
            return None
        held = self.held(entry, key)
        if not held:
            return None
        # The version is stamped per ANSWER, not per design, so an entry can
        # honestly hold answers from two references — which is what happens when
        # a new reference stops being able to read a design the old one could.
        recorded_by = held.get("reference_version") or entry.get("reference_version")
        if recorded_by != self.version:
            self.stale.append(f"{key}: recorded by {recorded_by}")
            return None
        if held.get("digest") != digest(source):
            self.stale.append(f"{key}: {source.name} changed since it was recorded")
            return None
        return held

    def record(
        self, entry: dict[str, Any], key: str, source: Path, value: Any, seconds: float
    ) -> None:
        entry.setdefault("answers", {})[key] = {
            "reference_version": self.version,
            "digest": digest(source),
            # What the reference took to produce this answer, kept so a run
            # served from the corpus can still report the reference's cost.
            "seconds": round(seconds, 3),
            "value": value,
        }
        self.recorded += 1


def safe(name: str) -> str:
    return "".join(c if c.isalnum() or c in "-_." else "_" for c in name)


# --------------------------------------------------------------------------- #
# M0 — framing and decoding
# --------------------------------------------------------------------------- #


def normalise_our_m0(dump: dict[str, Any]) -> dict[str, Any]:
    streams: dict[str, Any] = {}
    fields: dict[str, dict[str, str]] = {}
    framings: dict[str, str] = {}
    for stream in dump.get("streams", []):
        name = stream["stream"].lower()
        framings[name] = stream.get("framing", "?")
        streams[name] = {
            "bytes": stream.get("bytes"),
            "records": stream.get("records"),
            "text": stream.get("text_records"),
            "binary": stream.get("binary_records"),
            "types": stream.get("types", {}),
        }
        for index, record in enumerate(stream.get("decoded", [])):
            if record.get("mode") != "text":
                continue
            raw = {k: v for k, v in record.get("fields", [])}
            if qualifying(raw):
                fields[f"{name}#{index}"] = fold_utf8(raw)
    return {"streams": streams, "text_fields": fields, "framings": framings}


def qualifying(fields: dict[str, str]) -> bool:
    for key, value in fields.items():
        if key.upper().startswith("%UTF8%"):
            return True
        if "|" in value or any(ord(ch) > 127 for ch in value):
            return True
    return False


def fold_utf8(fields: dict[str, str]) -> dict[str, str]:
    plain: dict[str, str] = {}
    utf8: dict[str, str] = {}
    for key, value in fields.items():
        upper = key.upper()
        if upper.startswith("UNHANDLED"):
            # A `|` chunk with no `=`. The reference keeps it under a synthetic
            # key so it can write the record back; we drop it. Representation.
            continue
        if upper.startswith("%UTF8%"):
            utf8.setdefault(upper[len("%UTF8%") :], value)
        else:
            plain.setdefault(upper, value)
    plain.update(utf8)
    return plain


def reference_mojibake(ours: Any, theirs: Any) -> bool:
    """True when the two values are the same text and the reference mis-decoded it.

    A `%UTF8%` pair spells the escape character as its UTF-8 encoding, so
    un-escaping the BYTES splits the sequence and the decode falls back to a
    Latin-1 reading of UTF-8 bytes: `0Â°` for `0°`. Our reader un-escapes in the
    character domain instead (notes D8.1), and the reference still does not, so
    the two disagree on 339 values that are one string. Recognising the exact
    double encoding is the test — anything else stays a difference.
    """
    if not isinstance(ours, str) or not isinstance(theirs, str) or ours == theirs:
        return False
    try:
        return ours.encode("utf-8").decode("latin-1") == theirs
    except (UnicodeDecodeError, UnicodeEncodeError):
        return False


def pipe_escape_kept(ours: Any, theirs: Any) -> bool:
    """True when the only difference is Altium's `0xA6` escape for a pipe.

    A value that itself contains a record — `Record=PageOptions|Center…` inside
    one field — writes its separators as `0xA6` so they do not break the outer
    record's framing. Plan corner case 2 makes that an escape and we un-escape
    it; the reference keeps the broken-bar character. Both read the same bytes,
    and 253 of the corpus's decoded values differ only here.
    """
    if not isinstance(ours, str) or not isinstance(theirs, str) or ours == theirs:
        return False
    return "¦" in theirs and theirs.replace("¦", "|") == ours


def unevaluated_expression(ours: Any, theirs: Any, their_params: dict[str, Any]) -> bool:
    """True when the reference kept `=Name` where we resolved it to the same value.

    Plan corner case 5: a parameter value beginning `=` is a reference to another
    parameter, and reading it literally puts `=Value` in the BOM's value column.
    We evaluate it; the reference does not. The two agree whenever resolving the
    reference's own expression against the reference's own parameters yields what
    we wrote — so that is the test, and a value the reference cannot resolve
    stays a difference.
    """
    if not isinstance(theirs, str) or not theirs.startswith("="):
        return False
    name = theirs[1:].strip()
    for key, value in their_params.items():
        if key.casefold() == name.casefold():
            return value == ours
    # `PRJ_*` is Altium's own prefix for a PROJECT parameter, which lives in the
    # `.PrjPcb` rather than on the part. The reference publishes neither the
    # value nor the project's parameter table, so the expression itself is the
    # only evidence — and it is unambiguous.
    return name.upper().startswith("PRJ_")


def compare_m0(ours: dict[str, Any], theirs: dict[str, Any], report: Report) -> None:
    our_streams = ours["streams"]
    their_streams = theirs["streams"]
    framings = ours["framings"]

    for name in sorted(set(our_streams) | set(their_streams)):
        if name not in our_streams:
            report.add("stream only the reference sees", name)
            continue
        if name not in their_streams:
            report.add("stream only we see", name)
            continue
        mine, yours = our_streams[name], their_streams[name]
        if "error" in yours:
            report.note(f"reference could not read {name}: {short(yours['error'])}")
            continue
        if mine["bytes"] != yours["bytes"]:
            report.add(
                "stream length", f"{name}: ours {mine['bytes']} vs ref {yours['bytes']}"
            )
        framing = framings.get(name, "?")
        if framing.startswith("Blocks"):
            # A block-framed stream against the reference's prefixed walk is a
            # representation difference, not a disagreement about content; the
            # typed primitive counts below are the comparable number.
            continue
        if framing == "Flat":
            # Flat means "not record-framed", so we read nothing from it. That is
            # right for a raw payload and wrong if the stream really does hold
            # records — which is exactly what the reference's count says.
            if yours.get("nonempty") or yours["binary"]:
                report.add(
                    "stream we read as Flat that the reference reads as records",
                    f"{name}: ref {yours['records']} records {short(yours['types'])}",
                )
            continue
        if mine["records"] != yours["records"]:
            report.add(
                "record count", f"{name}: ours {mine['records']} vs ref {yours['records']}"
            )
        for key in sorted(set(mine["types"]) | set(yours["types"])):
            a, b = mine["types"].get(key, 0), yours["types"].get(key, 0)
            if a != b:
                report.add("record type histogram", f"{name} {key}: ours {a} vs ref {b}")

    # Primitive streams: our Blocks(n) walk against the reference's typed scanner.
    for stream, count in sorted(theirs.get("primitives", {}).items()):
        if stream == "__error__":
            report.note(f"reference primitive parse failed: {short(count)}")
            continue
        name = f"{stream}/data"
        mine = our_streams.get(name)
        if mine is None:
            report.add("primitive stream missing on our side", name)
            continue
        if mine["records"] != count:
            report.add(
                "primitive count", f"{name}: ours {mine['records']} vs ref {count}"
            )

    compare_field_maps(ours["text_fields"], theirs["text_fields"], report)


def compare_field_maps(
    ours: dict[str, dict[str, str]], theirs: dict[str, dict[str, str]], report: Report
) -> None:
    mojibake = 0
    only_ours = set(ours) - set(theirs)
    only_theirs = set(theirs) - set(ours)
    for key in sorted(only_ours)[:EXAMPLES]:
        report.add("decoded record only we flag", key)
    for key in sorted(only_theirs)[:EXAMPLES]:
        report.add("decoded record only the reference flags", key)
    for key in sorted(set(ours) & set(theirs)):
        mine, yours = ours[key], theirs[key]
        for field in sorted(set(mine) | set(yours)):
            a, b = mine.get(field), yours.get(field)
            if a == b:
                continue
            if a is None:
                report.add("decoded field only the reference has", f"{key} {field}")
            elif b is None:
                report.add("decoded field only we have", f"{key} {field}")
            elif reference_mojibake(a, b) or pipe_escape_kept(a, b):
                mojibake += 1
            else:
                report.add(
                    "decoded value", f"{key} {field}: ours {short(a)!r} vs ref {short(b)!r}"
                )
    if mojibake:
        report.note(
            f"{mojibake} decoded values where the reference kept a byte we un-escape "
            "(notes D8.1, plan corner case 2); the text is the same"
        )


# --------------------------------------------------------------------------- #
# M1 — the design model and the BOM
# --------------------------------------------------------------------------- #

# Component fields both sides name identically.  `value` is Altium's Comment and
# `footprint` the current PCB implementation; both sides resolve `=Expression`
# forms before emitting, so a literal `=Value` here is a disagreement, not noise.
COMPONENT_FIELDS = ("value", "footprint", "library_ref", "description")

# Parameters we synthesise so an Altium bundle satisfies the shared bundle
# contract (`bom.rs` reads `kicad_in_bom` whatever the source was). They have no
# counterpart in the file or in the reference — an intended divergence, §7.
SYNTHESISED_PARAMETERS = {
    "ALTIUM_COMPONENT_KIND",
    "ALTIUM_ALTERNATE_IN",
    "ALTIUM_NOT_FITTED_IN",
    "ALTIUM_SOURCE_LIBRARY",
    "KICAD_DNP",
    "KICAD_IN_BOM",
    "KICAD_ON_BOARD",
}


def normalise_components(design: dict[str, Any]) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for component in design.get("components", []):
        designator = component.get("designator") or ""
        entry = {field: component.get(field) for field in COMPONENT_FIELDS}
        classification = component.get("classification") or {}
        entry["kind"] = classification.get("type")
        entry["pin_count"] = classification.get("pin_count")
        parameters = {
            k.upper(): v
            for k, v in (component.get("parameters") or {}).items()
            if k.upper() not in SYNTHESISED_PARAMETERS
        }
        entry["parameter_keys"] = sorted(parameters)
        entry["parameters"] = parameters
        out[designator] = entry
    return out


def duplicate_designators(design: dict[str, Any]) -> dict[str, int]:
    """Designators emitted more than once.

    Plan §7 says a multi-part symbol is ONE component with its placements kept
    for rendering, so a designator appearing twice is a contract violation — and,
    like a duplicate net name, it hides the disagreement from a comparison keyed
    by designator, where the last row silently wins.
    """
    seen: dict[str, int] = {}
    for component in design.get("components", []):
        designator = component.get("designator") or ""
        seen[designator] = seen.get(designator, 0) + 1
    return {d: n for d, n in seen.items() if n > 1}


def duplicate_net_names(design: dict[str, Any]) -> list[str]:
    """Net names emitted more than once.

    A net name is the handle every downstream reader uses — the board
    cross-check, the net classes, a finding that quotes a net. It is not by
    itself a defect: Altium compiles two nets called `P5V` on the eval design,
    and both tools report them. It IS a difference when one side repeats a name
    the other does not, so only that is reported.
    """
    seen: dict[str, int] = {}
    for net in design.get("nets", []):
        if len(net.get("terminals", [])) < 2:
            continue
        name = net.get("name") or ""
        seen[name] = seen.get(name, 0) + 1
    return sorted(name for name, count in seen.items() if count > 1)


def normalise_nets(design: dict[str, Any]) -> dict[str, list[str]]:
    """Net name -> sorted `DESIGNATOR.PIN` terminals.

    Single-terminal nets are dropped on both sides: the reference does not emit
    them at all unless `NetlistSinglePinNets` is set, and we emit them under an
    `unconnected-(…)` name the review skills already read.  An intended
    divergence (plan §7); the count of what was dropped is reported.
    """
    parts = part_suffixes(design)
    out: dict[str, list[str]] = {}
    for net in design.get("nets", []):
        terminals = sorted(
            f"{parts.get(t.get('designator'), t.get('designator'))}.{t.get('pin')}"
            for t in net.get("terminals", [])
        )
        if len(terminals) < 2:
            continue
        # A NAME can belong to more than one net, on both sides: the eval design
        # labels `P5V` on one sheet and ports it on others, and Altium compiles
        # two nets called `P5V`. Keying by name alone dropped one of them from
        # the comparison entirely and made the survivor look short of terminals.
        name = net.get("name") or ""
        key = name
        n = 1
        while key in out:
            n += 1
            key = f"{name}#{n}"
        out[key] = terminals
    return out


def part_suffixes(design: dict[str, Any]) -> dict[str, str]:
    """`Q1A` -> `Q1` for a multi-part symbol's terminals.

    Altium shows the parts of one component as `Q1A` and `Q1B`, and the
    reference names its TERMINALS that way while its component table still says
    `Q1` — so its own netlist names a designator its own BOM does not have. We
    name both `Q1`, because a terminal has to resolve to the component and to the
    footprint pad it lands on, and pad 2 belongs to `Q1`. The mapping is built
    from the reference's own component list: a terminal designator that is not a
    component, but becomes one when a trailing letter is removed, is a part.
    """
    known = {c.get("designator") for c in design.get("components") or []}
    out: dict[str, str] = {}
    for net in design.get("nets", []):
        for terminal in net.get("terminals", []):
            designator = terminal.get("designator") or ""
            if designator in known or designator in out or len(designator) < 2:
                continue
            if designator[-1].isalpha() and designator[:-1] in known:
                out[designator] = designator[:-1]
    return out


def dropped_single_pin(design: dict[str, Any]) -> int:
    return sum(1 for net in design.get("nets", []) if len(net.get("terminals", [])) < 2)


def compare_m1_design(
    ours: dict[str, Any], theirs: dict[str, Any], report: Report, design: "Design | None" = None
) -> None:
    mine, yours = normalise_components(ours), normalise_components(theirs)
    # Two ways the reference states the same fact differently. Both are counted
    # and named rather than reported one row at a time.
    mojibake = 0
    unevaluated = 0

    # A project-less directory is ONE design to us — the loader walks every
    # schematic beside the entry file — and ONE SHEET to the reference, whose
    # `design` command takes a single document. The board's component table says
    # ours is the design; comparing per designator would report every component
    # on the other sheets as a difference, so the reference's smaller read is
    # reported once and the comparison continues over what it did read.
    partial = design is not None and design.project is None and set(yours) < set(mine)
    if partial:
        report.note(
            f"the reference read {len(yours)} of {len(mine)} components: entered through"
            f" {design.entry.name}, it reads that sheet alone where we read the"
            " whole project-less directory"
        )
    report.note(
        f"components: ours {len(mine)}, ref {len(yours)}"
        f"   nets: ours {len(ours.get('nets', []))}, ref {len(theirs.get('nets', []))}"
        f"   single-pin nets dropped: ours {dropped_single_pin(ours)},"
        f" ref {dropped_single_pin(theirs)}"
    )

    if not partial:
        for designator in sorted(set(mine) - set(yours)):
            report.add("component only we have", designator)
    for side, label in ((ours, "we emit"), (theirs, "the reference emits")):
        for designator, count in sorted(duplicate_designators(side).items()):
            report.add(f"designator {label} more than once", f"{designator} ({count} rows)")

    for designator in sorted(set(yours) - set(mine)):
        report.add("component only the reference has", designator)
    for designator in sorted(set(mine) & set(yours)):
        a, b = mine[designator], yours[designator]
        for field in COMPONENT_FIELDS + ("pin_count",):
            if a[field] == b[field]:
                continue
            if unevaluated_expression(a[field], b[field], b["parameters"]):
                unevaluated += 1
                continue
            if reference_mojibake(a[field], b[field]) or pipe_escape_kept(a[field], b[field]):
                mojibake += 1
                continue
            report.add(
                f"component {field}",
                f"{designator}: ours {short(a[field])!r} vs ref {short(b[field])!r}",
            )
        if a["kind"] != b["kind"]:
            # Vocabularies are ours by design (plan §7); reported separately so
            # a genuine classification difference is not buried in field noise.
            report.add(
                "component classification (our vocabulary)",
                f"{designator}: ours {a['kind']} vs ref {b['kind']}",
            )
        missing = set(b["parameter_keys"]) - set(a["parameter_keys"])
        extra = set(a["parameter_keys"]) - set(b["parameter_keys"])
        if missing:
            report.add("parameter keys we lack", f"{designator}: {short(sorted(missing))}")
        if extra:
            report.add("parameter keys only we have", f"{designator}: {short(sorted(extra))}")
        for key in sorted(set(a["parameters"]) & set(b["parameters"])):
            mine_v, their_v = a["parameters"][key], b["parameters"][key]
            if mine_v == their_v:
                continue
            if unevaluated_expression(mine_v, their_v, b["parameters"]):
                unevaluated += 1
                continue
            if reference_mojibake(mine_v, their_v) or pipe_escape_kept(mine_v, their_v):
                mojibake += 1
                continue
            report.add(
                "parameter value",
                f"{designator} {key}: ours {short(mine_v)!r} vs ref {short(their_v)!r}",
            )

    if unevaluated:
        report.note(
            f"{unevaluated} values the reference kept as `=Name` and we resolved to "
            "the same text (plan corner case 5)"
        )
    if mojibake:
        report.note(
            f"{mojibake} values the reference decoded as Latin-1 over UTF-8 bytes "
            "(notes D8.1); the text is the same"
        )

    # A repeated net name is only a difference when the two sides disagree about
    # it. Both tools emit `P5V` twice on the eval design, because the design
    # says so.
    mine_dup, their_dup = duplicate_net_names(ours), duplicate_net_names(theirs)
    for name in sorted(set(mine_dup) - set(their_dup)):
        report.add("net name we emit more than once", name)
    for name in sorted(set(their_dup) - set(mine_dup)):
        report.add("net name the reference emits more than once", name)

    our_nets, their_nets = normalise_nets(ours), normalise_nets(theirs)
    if not partial:
        for name in sorted(set(our_nets) - set(their_nets)):
            report.add("net name only we have", f"{name} ({len(our_nets[name])} terminals)")
    for name in sorted(set(their_nets) - set(our_nets)):
        report.add(
            "net name only the reference has", f"{name} ({len(their_nets[name])} terminals)"
        )
    for name in sorted(set(our_nets) & set(their_nets)):
        a, b = our_nets[name], their_nets[name]
        if a != b and not (partial and set(b) < set(a)):
            missing = sorted(set(b) - set(a))
            extra = sorted(set(a) - set(b))
            report.add(
                "net terminal membership",
                f"{name}: missing {short(missing, 40)} extra {short(extra, 40)}",
            )

    compare_hierarchy(ours, theirs, report)


def compare_hierarchy(ours: dict[str, Any], theirs: dict[str, Any], report: Report) -> None:
    """The sheet path each component sits on.

    We carry the path on the component (`hierarchy.sheet_path`). The reference
    puts the sheet's FILENAME there and publishes the path separately, in
    `schematic_hierarchy.hierarchy_paths` — so the comparable reference value is
    assembled from that document, and `hierarchy.sheet` is only a fallback for a
    flat design that has no hierarchy document at all.
    """
    mine = {
        (c.get("designator") or ""): normalise_path(
            (c.get("hierarchy") or {}).get("sheet_path") or ""
        )
        for c in ours.get("components", [])
    }
    yours = reference_paths(theirs)
    if yours is None:
        return

    shared = set(mine) & set(yours)
    for designator in sorted(shared):
        if mine[designator] != yours[designator]:
            report.add(
                "hierarchy path",
                f"{designator}: ours {short(mine[designator])} vs ref {short(yours[designator])}",
            )


def reference_paths(design: dict[str, Any]) -> dict[str, str] | None:
    """Designator -> sheet path, assembled from the reference's own documents.

    The reference puts a component's SHEET FILENAME on the component and its
    path in `schematic_hierarchy`: `hierarchy_paths[].levels[]` are the sheet
    symbols from the root down, and `source_sheet_index` says which document the
    path leads to. A sheet placed once has one path, so the filename identifies
    it; a sheet placed twice (a channel) has two and cannot be resolved this way,
    so those components are left out of the comparison rather than guessed at.
    """
    hierarchy = design.get("schematic_hierarchy") or {}
    paths = hierarchy.get("hierarchy_paths")
    documents = hierarchy.get("documents")
    if not paths or not documents:
        return None
    filename_of = {
        doc.get("sheet_index"): str(doc.get("filename") or "") for doc in documents
    }
    by_filename: dict[str, set[str]] = {}
    for path in paths:
        filename = filename_of.get(path.get("source_sheet_index"), "")
        spelled = "/" + "/".join(
            str(level.get("designator") or "").upper() for level in path.get("levels", [])
        )
        by_filename.setdefault(filename.lower(), set()).add(spelled)

    out: dict[str, str] = {}
    for component in design.get("components", []):
        sheet = str((component.get("hierarchy") or {}).get("sheet") or "").lower()
        candidates = by_filename.get(sheet, set())
        if len(candidates) == 1:
            out[component.get("designator") or ""] = next(iter(candidates))
    return out


def normalise_path(path: str) -> str:
    parts = [p for p in path.replace("\\", "/").split("/") if p]
    return "/" + "/".join(Path(p).stem.upper() for p in parts)


def compare_m1_bom(
    ours: dict[str, Any], theirs: dict[str, Any], report: Report, partial: bool = False
) -> None:
    """The BOM row set, keyed by the designators a row covers.

    Grouping policy is ours (plan §7), so rows are compared as a *partition of
    designators*: a row set that partitions the same designators the same way is
    agreement, whatever the row order or the field spellings.
    """
    def rows(bom: dict[str, Any]) -> dict[str, tuple[int, frozenset[str]]]:
        out = {}
        for row in bom.get("lines", []):
            designators = frozenset(row.get("designators") or [])
            if not designators:
                continue
            out[min(designators)] = (row.get("quantity"), designators)
        return out

    mine, yours = rows(ours), rows(theirs)
    report.note(
        f"BOM lines: ours {ours.get('line_count')}, ref {theirs.get('line_count')}"
        f"   components: ours {ours.get('component_count')}, ref {theirs.get('component_count')}"
    )

    our_designators = {d for _, group in mine.values() for d in group}
    their_designators = {d for _, group in yours.values() for d in group}
    for designator in sorted(their_designators - our_designators):
        report.add("BOM designator only the reference has", designator)
    if not partial:
        for designator in sorted(our_designators - their_designators):
            report.add("BOM designator only we have", designator)

    for key in sorted(set(mine) & set(yours)):
        _, a = mine[key]
        _, b = yours[key]
        if a != b and not (partial and b < a):
            report.add(
                "BOM grouping",
                f"row {key}: ours {short(sorted(a), 40)} vs ref {short(sorted(b), 40)}",
            )


# --------------------------------------------------------------------------- #
# driver
# --------------------------------------------------------------------------- #


# Our board geometry is Altium's own space with Y flipped about the workspace
# height; the normaliser undoes exactly that and nothing else, so a real
# disagreement about a coordinate still shows up as one.
WORKSPACE_MM = 2540.0
UNIT_MM = 0.0254 / 10000.0
MIL_MM = 0.0254
# 0.5 um: below what either side rounds to, above the last binary digit of a
# millimetre round-trip.
COORD_TOL = 0.0005

# The reference's pad-shape codes against the bundle's. Round is a circle only
# when its sides are equal, which is the one case the code alone cannot settle.
REF_SHAPE = {2: 1, 3: 2, 4: 2, 9: 2}


def bundle_shape(ref_shape: int, w: float, h: float) -> int:
    if ref_shape == 1:
        return 0 if abs(w - h) < 1e-6 else 3
    return REF_SHAPE.get(ref_shape, 5)


def net_name(nets: list[str], index: int) -> str:
    return nets[index] if 0 <= index < len(nets) else ""


def net_index_name(theirs: dict[str, Any], index: Any) -> str:
    """The reference names a primitive's net by its index into `Nets6`, which is
    the same table our index 0 reserves a sentinel in front of."""
    names = theirs.get("nets") or []
    if index is None or not (0 <= index < len(names)):
        return ""
    return names[index] or ""


def compare_m2_board(ours: dict[str, Any], theirs: dict[str, Any], report: Report) -> None:
    """Layer table, per-layer primitive counts, pad and via geometry, and the
    net a primitive carries (plan section 8.3, M2 row)."""
    layers = ours.get("layers") or []
    names = [l.get("name") for l in layers]
    nets = ours.get("nets") or []
    ref_names = theirs.get("layer_names") or {}

    # --- the layer table. Only the layers the reference also names are
    # compared: it describes the copper stack, we describe every layer drawn on.
    for legacy in theirs.get("stack") or []:
        name = ref_names.get(str(legacy))
        if name and name not in names:
            report.add("stack layer missing from ours", f"{legacy} {name}")
    stack = [names[i] for i, l in enumerate(layers) if l.get("role") == "copper"]
    want = [ref_names.get(str(i)) for i in theirs.get("stack") or []]
    if want and stack[: len(want)] != want:
        report.add("copper stack order", f"ours {stack[:len(want)]} vs ref {want}")

    # --- per-layer primitive counts, on the kinds both sides name the same way.
    mine: dict[str, int] = {}

    def bump(kind: str, layer_index: int) -> None:
        key = f"{kind}/{names[layer_index]}"
        mine[key] = mine.get(key, 0) + 1

    for index in ours["tracks"]["seg"]["layer"]:
        bump("tracks", index)
    for index in ours["tracks"]["arc"]["layer"]:
        bump("arcs", index)
    for g in ours["graphics"]:
        bump({"seg": "tracks", "arc": "arcs", "circle": "arcs"}.get(g["kind"], "poly"), g["layer"])
    for t in ours["texts"]:
        bump("texts", t["layer"])
    named: dict[str, int] = {}
    for key, count in (theirs.get("counts") or {}).items():
        kind, _, legacy = key.partition("/")
        name = ref_names.get(legacy)
        if kind in ("tracks", "arcs", "texts") and name:
            named[f"{kind}/{name}"] = count
    comparable = {k for k in mine if k.split("/", 1)[0] in ("tracks", "arcs", "texts")}
    for key in sorted(set(named) | comparable):
        if named.get(key, 0) != mine.get(key, 0):
            report.add(
                "primitive count per layer",
                f"{key}: ours {mine.get(key, 0)} vs ref {named.get(key, 0)}",
            )

    # --- pads and vias, index by index: both sides keep the stream's own order.
    theirs_pads = theirs.get("pads") or []
    ours_pads = ours.get("pads") or []
    if len(theirs_pads) != len(ours_pads):
        report.add("pad count", f"ours {len(ours_pads)} vs ref {len(theirs_pads)}")
    for i, (t, o) in enumerate(zip(theirs_pads, ours_pads)):
        x, y, w, h, shape, corner, hole, rot, plated, designator, net = t[:11]
        want_fields = {
            "x": x * UNIT_MM,
            "y": WORKSPACE_MM - y * UNIT_MM,
            "w": w * UNIT_MM,
            "h": h * UNIT_MM,
            "drill": hole * UNIT_MM,
            "angle": -rot,
        }
        got = {
            "x": o["x"], "y": o["y"], "w": o["w"], "h": o["h"],
            "drill": o.get("drill", 0.0), "angle": o["angle"],
        }
        for field, value in want_fields.items():
            tol = 0.01 if field == "angle" else COORD_TOL
            if abs(value - got[field]) > tol:
                report.add(f"pad {field}", f"[{i}] {designator}: ours {got[field]} vs ref {value}")
        if o["num"] != designator:
            report.add("pad designator", f"[{i}] ours {o['num']} vs ref {designator}")
        if o["shape"] != bundle_shape(shape, w, h):
            report.add("pad shape", f"[{i}] {designator}: ours {o['shape']} vs ref {shape}")
        if o["shape"] == 2 and abs(o.get("rratio", 0.0) - corner / 200.0) > 1e-6:
            report.add("pad corner radius", f"[{i}] ours {o.get('rratio')} vs ref {corner}%")
        if bool(o.get("npth")) != bool(hole > 0 and not plated):
            report.add("pad plating", f"[{i}] {designator}: ours npth={o.get('npth', False)}")
        if net_name(nets, o["net"]) != net_index_name(theirs, net):
            report.add(
                "pad net",
                f"[{i}] {designator}: ours {net_name(nets, o['net'])!r}"
                f" vs ref {net_index_name(theirs, net)!r}",
            )

    theirs_vias = theirs.get("vias") or []
    ours_vias = ours.get("vias") or []
    if len(theirs_vias) != len(ours_vias):
        report.add("via count", f"ours {len(ours_vias)} vs ref {len(theirs_vias)}")
    for i, (t, o) in enumerate(zip(theirs_vias, ours_vias)):
        x, y, diameter, hole, net = t
        want_fields = {
            "x": x * UNIT_MM,
            "y": WORKSPACE_MM - y * UNIT_MM,
            "size": diameter * UNIT_MM,
            "drill": hole * UNIT_MM,
        }
        for field, value in want_fields.items():
            if abs(value - o[field]) > COORD_TOL:
                report.add(f"via {field}", f"[{i}] ours {o[field]} vs ref {value}")
        if net_name(nets, o["net"]) != net_index_name(theirs, net):
            report.add("via net", f"[{i}] ours {net_name(nets, o['net'])!r}")


def compare_m2_centroids(
    ours: dict[str, Any], theirs: dict[str, Any], origin: list[Any], report: Report
) -> None:
    """A component sits at the centre of its own pads' bounding box, which is
    what Altium's pick-and-place exports and what corner case 23 is about. The
    reference reports it relative to the board origin; ours is absolute, so the
    origin is added back before comparing."""
    if not origin or origin[0] is None:
        report.note("no board origin: centroids not compared")
        return
    ox, oy = origin[0] * MIL_MM, origin[1] * MIL_MM
    placed = {c["ref"]: c for c in ours.get("components") or []}
    for designator, (x, y, rotation) in sorted(theirs.get("placements", {}).items()):
        c = placed.get(designator)
        if c is None:
            report.add("component the reference places and we do not", designator)
            continue
        want = (x + ox, WORKSPACE_MM - (y + oy))
        if abs(c["x"] - want[0]) > 0.06 or abs(c["y"] - want[1]) > 0.06:
            report.add(
                "component centroid",
                f"{designator}: ours ({c['x']:.3f}, {c['y']:.3f})"
                f" vs ref ({want[0]:.3f}, {want[1]:.3f})",
            )
        if abs(((-rotation) - c["angle"] + 180) % 360 - 180) > 0.01:
            report.add("component rotation", f"{designator}: ours {c['angle']} vs ref {rotation}")


def phase_m2(
    cfg: Config, design: Design, report: Report, oracle: Oracle, entry: dict, clock: Clock
) -> None:
    if design.board is None:
        report.note("no board in this design")
        return
    with tempfile.TemporaryDirectory(prefix="altium-diff-m2-") as tmp:
        root = Path(tmp)
        with clock.measure("ours:geometry"):
            ours = our_geometry(cfg, design, root / "ours-geom")
        theirs = ask(
            oracle, entry, "m2:board", design.board,
            lambda: ref_board(cfg, design, root / "ref-board"),
            clock, "ref:board",
        )
        if ours is None:
            report.add("board geometry we cannot build", design.name)
            return
        if not theirs or not theirs.get("pads"):
            report.add("board the reference cannot read", design.name)
        else:
            compare_m2_board(ours, theirs, report)

        pnp = ask(
            oracle, entry, "m2:pnp", design.board,
            lambda: ref_pnp(cfg, design, root / "ref-pnp"),
            clock, "ref:pnp",
        )
        if not pnp or not pnp.get("placements"):
            report.note("the reference produced no pick-and-place for this design")
        elif theirs:
            compare_m2_centroids(ours, pnp, theirs.get("origin") or [], report)


def compare_m3_render(
    ours: dict[str, Any], theirs: dict[str, Any], report: Report, loose: bool = False
) -> None:
    """Per-sheet, per-class object counts, both sides in the same vocabulary.

    A renderer's failure mode is silence: a symbol that never reaches the page,
    a class of object nobody looks for, an owner rule that quietly drops half a
    sheet's parameters. A count per class per sheet is the smallest thing that
    catches all three.
    """
    our_sheets = ours.get("sheets", {})
    their_sheets = theirs.get("sheets", {})
    for name in sorted(set(their_sheets) - set(our_sheets)):
        report.add("sheet the reference renders and we do not", name)
    extra = sorted(set(our_sheets) - set(their_sheets))
    if extra and loose:
        # A design with no project file has no document list, so we walk every
        # `.SchDoc` beside the named one — a project-less design is still a
        # design (plan §7) — while the reference renders only the file it was
        # handed. That is an intended divergence, not a difference.
        report.note(
            f"loose document: we render {len(extra)} further sheet(s) beside it "
            f"({', '.join(extra[:3])}{'…' if len(extra) > 3 else ''})"
        )
    else:
        for name in extra:
            report.add("sheet we render and the reference does not", name)

    for name in sorted(set(our_sheets) & set(their_sheets)):
        mine, yours = our_sheets[name], their_sheets[name]
        for cls in sorted(set(mine) | set(yours)):
            ours_ids, their_ids = mine.get(cls, []), yours.get(cls, [])
            a, b = set(ours_ids), set(their_ids)
            # An id neither side kept (an object the file left unnamed) can only
            # be compared by count.
            if "" in a or "" in b or not a or not b:
                if len(ours_ids) != len(their_ids):
                    report.add(
                        f"{cls} count on a sheet",
                        f"{name}: ours {len(ours_ids)}, reference {len(their_ids)}",
                    )
                continue
            for uuid in sorted(a - b):
                report.add(f"{cls} we draw and the reference does not", f"{name}: {uuid}")
            for uuid in sorted(b - a):
                report.add(f"{cls} the reference draws and we do not", f"{name}: {uuid}")
            dup = len(ours_ids) - len(a)
            if dup and len(ours_ids) != len(their_ids):
                report.add(
                    f"{cls} drawn more than once",
                    f"{name}: ours {len(ours_ids)} for {len(a)} objects",
                )


def phase_m3(
    cfg: Config, design: Design, report: Report, oracle: Oracle, entry: dict, clock: Clock
) -> None:
    with tempfile.TemporaryDirectory(prefix="altium-diff-m3-") as tmp:
        root = Path(tmp)
        with clock.measure("ours:render"):
            ours = our_render(cfg, design, root / "ours-render")
        theirs = ask(
            oracle, entry, "m3:render", design.entry,
            lambda: ref_render(cfg, design, root / "ref-render"),
            clock, "ref:render",
        )
        if ours is None:
            report.add("design we cannot render", design.name)
            return
        if not theirs:
            report.add("design the reference cannot render", design.name)
            return
        compare_m3_render(ours, theirs, report, loose=design.project is None)
        report.note(
            "board layers are compared by M2's geometry, which the layer SVGs are "
            "rendered from; the plan's raster half is not built (see the notes)"
        )


def compare_m4_pairing(
    ours: dict[str, Any], theirs: dict[str, Any], report: Report, loose: bool = False
) -> None:
    """Schematic-to-board instance pairing (plan section 8.3, M4 row).

    The diff engine pairs a footprint across two revisions by the SCHEMATIC
    instance it came from — Altium's `SOURCEUNIQUEID`, which is the chain of
    sheet-symbol ids down to the placed part. Two revisions of one design are not
    in the corpus, but the claim that pairing rests on is, and it is checkable on
    a single revision: every footprint carries an identity, no two footprints
    share one, and the instance each identity names is the one the reference
    places on that sheet. A pairing key that is missing, duplicated or pointing
    at the wrong sheet cannot pair correctly across a revision either.

    The board writes the chain relative to the project's own root while the
    reference writes it from the top-level sheet symbol down, so the board's
    prefix is compared as a SUFFIX of the reference's path rather than for
    equality — that difference is framing, not disagreement.

    A key whose chain the SCHEMATIC does not corroborate — absent entirely, or
    naming a sheet symbol no document places — is counted and named, not reported
    as a difference. The corpus has both: the eval design writes Q7 with no chain
    at all and its fiducials with a sheet-symbol id that was re-drawn out of the
    schematic. That is the board/schematic mismatch the plan already records as a
    divergence (section 7, board authority), and it costs the pairing nothing:
    the key is still stable and still unique, which is all pairing asks of it.
    """
    comps = ours.get("components") or []
    if not comps:
        report.add("board with no placed components", "nothing to pair")
        return

    unnamed = [c["ref"] for c in comps if not c.get("uuid")]
    for designator in sorted(unnamed)[:EXAMPLES]:
        report.add("footprint with no pairing key", designator)
    if len(unnamed) > EXAMPLES:
        report.counts["footprint with no pairing key"] = len(unnamed)

    seen: dict[str, str] = {}
    for c in comps:
        uid = c.get("uuid") or ""
        if not uid:
            continue
        if uid in seen:
            report.add(
                "pairing key shared by two footprints",
                f"{seen[uid]} and {c['ref']} both claim {uid}",
            )
        seen[uid] = c["ref"]

    hierarchy = theirs.get("schematic_hierarchy") or {}
    paths = hierarchy.get("hierarchy_paths") or []
    sheet_of = {
        c.get("designator"): str((c.get("hierarchy") or {}).get("sheet") or "")
        for c in theirs.get("components") or []
    }
    if not paths:
        # A flat design has one sheet and no chain; there is nothing to resolve,
        # so identity and uniqueness above are the whole claim.
        report.note("flat design: no hierarchy chain to resolve a pairing key against")
        return

    chainless: list[str] = []
    stale: list[str] = []
    for c in sorted(comps, key=lambda c: c["ref"]):
        uid = c.get("uuid") or ""
        segments = [s for s in uid.replace("/", "\\").split("\\") if s]
        if len(segments) < 2:
            chainless.append(c["ref"])
            continue
        prefix = segments[:-1]
        matches = [
            p
            for p in paths
            if [x for x in str(p.get("unique_id_path") or "").split("\\") if x][-len(prefix):]
            == prefix
        ]
        if not matches:
            stale.append(f"{c['ref']} ({'/'.join(prefix)})")
            continue
        want = sheet_of.get(c["ref"])
        if want is None:
            report.add("footprint the reference has no schematic part for", c["ref"])
            continue
        leaves = {
            str((p.get("levels") or [{}])[-1].get("child_filename") or "").lower()
            for p in matches
        }
        if want.lower() not in leaves:
            report.add(
                "pairing key resolving to the wrong sheet",
                f"{c['ref']}: ours {sorted(leaves)} vs ref {want!r}",
            )

    if chainless:
        report.note(
            f"{len(chainless)} footprints carry a pairing key with no sheet chain "
            f"({', '.join(sorted(chainless)[:4])}) - the board's own omission"
        )
    if stale:
        report.note(
            f"{len(stale)} footprints name a sheet symbol the schematic no longer "
            f"places ({', '.join(sorted(stale)[:4])}) - board/schematic mismatch"
        )


def phase_m4(
    cfg: Config, design: Design, report: Report, oracle: Oracle, entry: dict, clock: Clock
) -> None:
    if design.board is None:
        report.note("no board in this design")
        return
    with tempfile.TemporaryDirectory(prefix="altium-diff-m4-") as tmp:
        root = Path(tmp)
        with clock.measure("ours:geometry"):
            ours = our_geometry(cfg, design, root / "ours-geom")
        # M4 asks the reference nothing new: the hierarchy document M1 already
        # records is what a pairing key has to resolve against.
        theirs = ask(
            oracle,
            entry,
            "m1:design",
            design.entry,
            lambda: (lambda d: reduce_design(d) if d else None)(
                ref_design(cfg, design, root / "ref-design")
            ),
            clock,
            "ref:design",
        )
        if ours is None:
            report.add("board geometry we cannot build", design.name)
            return
        if not theirs or not theirs.get("components"):
            report.add("design the reference cannot build", design.name)
            return
        compare_m4_pairing(ours, theirs, report, loose=design.project is None)


def phase_m0(
    cfg: Config, design: Design, report: Report, oracle: Oracle, entry: dict, clock: Clock
) -> None:
    for document in design.documents:
        with clock.measure("ours:m0"):
            ours = our_dump(cfg, document)
        theirs = ask(
            oracle,
            entry,
            f"m0:{document.name}",
            document,
            lambda: ref_dump(cfg, document),
            clock,
            "ref:m0",
        )
        if ours is None:
            report.add("document we cannot read", document.name)
            continue
        if theirs is None:
            report.add("document the reference cannot read", document.name)
            continue
        sub = Report()
        compare_m0(normalise_our_m0(ours), theirs, sub)
        for kind, count in sub.counts.items():
            for example in sub.classes[kind]:
                report.add(f"{kind}", f"[{document.name}] {example}")
            if count > len(sub.classes[kind]):
                report.counts[kind] = report.counts.get(kind, 0) + count - len(sub.classes[kind])
        for note in sub.notes:
            report.note(f"[{document.name}] {note}")


def phase_m1(
    cfg: Config, design: Design, report: Report, oracle: Oracle, entry: dict, clock: Clock
) -> None:
    with tempfile.TemporaryDirectory(prefix="altium-diff-") as tmp:
        root = Path(tmp)
        with clock.measure("ours:design"):
            ours = our_design(cfg, design, root / "ours-design")
        theirs = ask(
            oracle,
            entry,
            "m1:design",
            design.entry,
            lambda: (lambda d: reduce_design(d) if d else None)(
                ref_design(cfg, design, root / "ref-design")
            ),
            clock,
            "ref:design",
        )
        if ours is None:
            report.add("design we cannot build", design.name)
        elif not theirs or not theirs.get("components"):
            report.add("design the reference cannot build", design.name)
        else:
            compare_m1_design(ours, theirs, report, design)

        with clock.measure("ours:bom"):
            our_rows = our_bom(cfg, design, root / "ours-bom")
        their_rows = ask(
            oracle,
            entry,
            "m1:bom",
            design.entry,
            lambda: (lambda b: reduce_bom(b) if b else None)(
                ref_bom(cfg, design, root / "ref-bom")
            ),
            clock,
            "ref:bom",
        )
        if our_rows is None:
            report.add("BOM we cannot build", design.name)
        elif not their_rows or not their_rows.get("lines"):
            report.add("BOM the reference cannot build", design.name)
        else:
            partial = design.project is None and their_rows.get(
                "component_count", 0
            ) < our_rows.get("component_count", 0)
            compare_m1_bom(our_rows, their_rows, report, partial)


def ask(
    oracle: Oracle,
    entry: dict,
    key: str,
    source: Path,
    run_reference,
    clock: Clock,
    span: str,
) -> Any:
    """One reference answer: from the recorded corpus when it is fresh, else by
    running the reference and recording what it said, and what it cost.

    `--offline` refuses to run it, which is how a run proves it used the corpus
    and nothing else.
    """
    held = oracle.fresh(entry, key, source)
    if held is not None:
        clock.add(span, seconds_of(held), from_corpus=True)
        return held.get("value")
    if oracle.mode == "offline":
        held = oracle.held(entry, key)
        if held is None:
            return None
        clock.add(span, seconds_of(held), from_corpus=True)
        return held.get("value")
    start = time.perf_counter()
    value = run_reference()
    seconds = time.perf_counter() - start
    if value is not None:
        clock.add(span, seconds)
        oracle.record(entry, key, source, value, seconds)
        return value
    # The current reference cannot answer. An older one could, and its answer is
    # still the best oracle available — coverage should not fall away because the
    # reference regressed, so the recorded answer is kept and SAID to be old.
    held = oracle.held(entry, key)
    if held is None:
        return None
    oracle.kept.append(
        f"{key}: kept the answer recorded by"
        f" {held.get('reference_version') or 'an earlier reference'};"
        " the current reference fails on this design"
    )
    clock.add(span, seconds_of(held), from_corpus=True)
    return held.get("value")


def seconds_of(held: dict[str, Any]) -> float | None:
    value = held.get("seconds")
    return None if value is None else float(value)


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8")  # the corpus carries non-ASCII
        except AttributeError:
            pass

    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--phase", choices=("m0", "m1", "m2", "m3", "m4", "all"), default="all")
    parser.add_argument("--design", help="substring of the design name to run alone")
    parser.add_argument("--corpus")
    parser.add_argument("--reference", help="reference CLI")
    parser.add_argument("--extract", help="our pcb-extract binary")
    parser.add_argument("--out", help="where the JSON report is written")
    parser.add_argument(
        "--examples", type=int, help=f"examples printed per difference class (default {EXAMPLES})"
    )
    parser.add_argument("--oracle", help="where the recorded reference answers live")
    parser.add_argument(
        "--build-oracle",
        action="store_true",
        help="re-ask the reference for every design and record the answers as the corpus",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="compare against the recorded corpus only; never run the reference",
    )
    args = parser.parse_args(argv[1:])

    if args.examples:
        globals()["EXAMPLES"] = args.examples

    cfg = Config(args)
    problems = cfg.check()
    if cfg.oracle_mode == "offline":
        problems = [p for p in problems if "reference" not in p or "oracle" in p]
    fatal = [p for p in problems if "M1 does not" not in p]
    for problem in problems:
        print(f"config: {problem}", file=sys.stderr)
    if fatal:
        return 2
    if problems and args.phase in ("m0", "all") and cfg.oracle_mode != "offline":
        print("config: M0 needs the reference library; run --phase m1", file=sys.stderr)
        return 2

    designs = discover(cfg.corpus)
    if args.design:
        designs = [d for d in designs if args.design.lower() in d.name.lower()]
    if not designs:
        print("no designs found", file=sys.stderr)
        return 2

    oracle = Oracle(cfg)
    print(f"reference : {cfg.reference}  ({cfg.reference_version})")
    print(f"ours      : {cfg.extract}")
    print(f"corpus    : {cfg.corpus}  ({len(designs)} designs)")
    print(f"oracle    : {cfg.oracle}  [{cfg.oracle_mode}]")
    print()

    results: dict[str, Any] = {}
    clocks: dict[str, Clock] = {}
    total = 0
    for design in designs:
        print(f"=== {design.name}  ({design.entry.name})")
        entry = oracle.load(design)
        before = oracle.recorded
        clock = Clock()
        clocks[design.name] = clock
        design_result: dict[str, Any] = {}
        for phase, runner in (
            ("m0", phase_m0), ("m1", phase_m1), ("m2", phase_m2), ("m3", phase_m3),
            ("m4", phase_m4),
        ):
            if args.phase not in (phase, "all"):
                continue
            report = Report()
            runner(cfg, design, report, oracle, entry, clock)
            print(f"  {phase.upper()}")
            for line in report.render("    "):
                print(line)
            design_result[phase] = report.as_json()
            total += report.total
        design_result["timing"] = clock.as_json()
        results[design.name] = design_result
        if oracle.recorded > before:
            oracle.save(design, entry)
            print(f"    + recorded {oracle.recorded - before} reference answers")
        print(f"  TIME  {clock.line('ours', 'ours:')}")
        print(f"        {clock.line('reference', 'ref:')}")
        print()

    cfg.out.mkdir(parents=True, exist_ok=True)
    path = cfg.out / "differential.json"
    path.write_text(json.dumps(results, indent=2, ensure_ascii=False), encoding="utf-8")
    if oracle.kept:
        print()
        print(f"{len(oracle.kept)} answers the current reference could not re-produce:")
        for line in oracle.kept:
            print(f"  {line}")
    if oracle.stale:
        print()
        what = "used anyway (offline)" if cfg.oracle_mode == "offline" else "re-asked"
        print(f"{len(oracle.stale)} recorded answers were stale and {what}:")
        for line in oracle.stale[:10]:
            print(f"  {line}")
        if len(oracle.stale) > 10:
            print(f"  … and {len(oracle.stale) - 10} more")
    print()
    print("extraction time per design (wall clock, this machine)")
    print(f"  {'design':38s} {'documents':>9s} {'ours':>9s} {'reference':>10s} {'ratio':>7s}")
    ours_all = ref_all = 0.0
    for design in designs:
        clock = clocks.get(design.name)
        if clock is None:
            continue
        mine, theirs = clock.total("ours:"), clock.total("ref:")
        ours_all += mine
        ref_all += theirs
        ratio = f"{theirs / mine:.0f}x" if mine > 0.001 else "-"
        mark = "+" if any(k.startswith("ref:") for k in clock.unknown) else " "
        print(
            f"  {design.name[:38]:38s} {len(design.documents):9d}"
            f" {mine:8.1f}s {theirs:8.1f}s{mark} {ratio:>7s}"
        )
    ratio = f"{ref_all / ours_all:.0f}x" if ours_all > 0.001 else "-"
    print(
        f"  {'TOTAL':38s} {sum(len(d.documents) for d in designs):9d}"
        f" {ours_all:8.1f}s {ref_all:8.1f}s  {ratio:>7s}"
    )
    if any(k.startswith("ref:") for c in clocks.values() for k in c.unknown):
        print("  + the reference's time is unmeasured for part of this design:")
        print("    its answer was carried over from a version that is no longer installed")
    print()
    print(f"{total} differences; report written to {path}")
    print(f"oracle corpus: {cfg.oracle}")
    # A difference is never auto-adopted (plan §8.3 triage rule), so a non-zero
    # exit means "there is something to triage", not "the build is broken".
    return 1 if total else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
