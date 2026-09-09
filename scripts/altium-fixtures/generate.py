#!/usr/bin/env python3
"""Generate the Altium fixtures the reference corpus does not contain.

`reference_designs` holds five real boards and not one harness, substack,
embedded board or library file, which is why those four readers went unbuilt
(docs/altium-extraction-notes.md, D9.4). The reference parser
(`altium-monkey`, the §8.3 oracle) can *author* all four, so the corpus stops
being an accident of what five designers happened to draw.

The outputs are Altium's own binary formats written by a third-party tool, so
they prove framing and field decoding — not that Altium itself would write the
same bytes. Where the two could differ, the reader is written to accept both.

Usage (from the repository root):

    python scripts/altium-fixtures/generate.py [--monkey DIR]
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

DEST = Path("src-tauri/crates/eda-parse-altium/tests/fixtures/generated")

# (example id, produced file, fixture name) — one example may write several
# files; only the ones named here are checked in.
FIXTURES = [
    (
        "pcbdoc_create_rigid_flex_impedance_backdrill",
        "pcbdoc_create_rigid_flex_impedance_backdrill.PcbDoc",
        "rigid_flex.PcbDoc",
    ),
    ("schdoc_add_harness_connector", "harness_example.SchDoc", "harness.SchDoc"),
    ("hello_schlib", "hello_schlib.SchLib", "symbols.SchLib"),
    ("hello_pcblib", "hello_pcblib.PcbLib", "footprints.PcbLib"),
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--monkey", default=r"D:\git_repo\altium_monkey")
    args = ap.parse_args()

    monkey = Path(args.monkey)
    if not (monkey / "examples" / "manifest.toml").is_file():
        print(f"no altium-monkey checkout at {monkey}", file=sys.stderr)
        return 1

    DEST.mkdir(parents=True, exist_ok=True)
    for example, produced, fixture in FIXTURES:
        script = Path("examples") / example / f"{example}.py"
        print(f"--- {example}")
        run = subprocess.run(
            ["uv", "run", "python", str(script)], cwd=monkey, capture_output=True, text=True
        )
        if run.returncode != 0:
            print(run.stdout + run.stderr, file=sys.stderr)
            return run.returncode
        src = monkey / "examples" / example / "output" / produced
        if not src.is_file():
            print(f"{example} did not write {produced}", file=sys.stderr)
            return 1
        shutil.copyfile(src, DEST / fixture)
        print(f"    {fixture}  {src.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
