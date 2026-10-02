# OrCAD validation

The OrCAD front-end (`src-tauri/crates/eda-parse-orcad`, `src-tauri/crates/extract/src/orcad`)
is checked against three independent sources. None is vendored, none is a build
dependency, and the corpus they run over is public repositories that may not be
redistributed. Point the scripts at a local copy.

## 1. Capture's own netlist

Many published Capture projects ship the netlist Capture itself wrote
(`allegro/pstxnet.dat`, with `pstchip.dat` / `pstxprt.dat` for hidden power pins).

```bash
cargo run --release -p extract --example orcad_netcheck -- <corpus>
```

Compares, per design, every net's pin set and name with Capture's, and flags
netlists older than the design (a pin name Capture's netlist does not know).

## 2. Allegro's own reports

```bash
cargo build --release -p eda-parse-orcad --examples
python scripts/orcad-diff/allegro_reports.py place   --corpus <corpus>
python scripts/orcad-diff/allegro_reports.py classes --corpus <corpus>
cargo run --release -p extract --example orcad_brdcheck -- <corpus>
```

- `place`: every part's reference, position, rotation and side against the
  `place_txt.txt` / fabmaster exports beside the board. Fabmaster files also list
  pseudo-rows (`ETCH`, `BOARD GEOMETRY`, `BOUNDARY`), which never match a part.
- `classes`: matches decoded primitives to the CLASS/SUBCLASS names in Allegro's
  extract reports (`sym.txt`) by coordinates. This is where the subclass table in
  `orcad/pcb.rs` comes from. A code no report confirms keeps a hex name.
- `orcad_brdcheck`: builds every board's geometry (timed) and compares pad nets
  with the Allegro netlist imported into the board.

## 3. KiCad's OrCAD importers (the differential)

KiCad master (late 2026) imports Allegro boards and Capture designs. That is an
independent implementation of the same formats.

```bash
cargo build -p extract --bin pcb-extract --release
KICAD_CLI=<kicad build>/kicad/kicad-cli \
  python scripts/orcad-diff/differential.py --corpus <corpus> [--boards-only|--designs-only]
```

An uninstalled `kicad-cli` needs `KICAD_STOCK_DATA_HOME` set to its build
directory, and its `_pcbnew.kiface` / `_eeschema.kiface` linked beside it.

The comparator normalises only what is a convention rather than a disagreement:

- rotation sense: the IR is Y-down and clockwise-positive. KiCad is
  counter-clockwise, and its bottom-side flip about the X axis adds 180 degrees;
- KiCad's `UNK` prefix on a footprint that has no component (we keep its
  reference text as drawn);
- KiCad's sheet-path prefix on net names, and Capture's `_<block>` suffix on a
  name local to a reused folder instance.

Exit 1 means there is something to triage. The report lands in
`output/orcad-diff/differential.json`, with per-pin examples of every grouping or
naming difference.
