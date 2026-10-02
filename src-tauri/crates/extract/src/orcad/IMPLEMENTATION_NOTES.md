# OrCAD extraction — implementation notes

Branch `claude/orcad-extraction-2ukybn`, based on `altium-extraction`. Validation
tooling and how to rerun it: `scripts/orcad-diff/README.md`.

## What landed

| Milestone | What it does |
|---|---|
| C0 | `ole-cfb` crate split out of the Altium parser; `eda-parse-orcad` reads Capture's Library, Cache, Packages, pages, Views directory, page order and the occurrence tree. Framing depth is detected (no per-type table). |
| C1 | Capture → `design::Component` / `netlist::Frag` / `Design`: placements through `Orient::place`, occurrence designators and units, hidden power pins, aliases, power, off-page, ports and blocks, Capture's naming rank and `_<block>` suffixes. |
| C1b | Legacy grammar (Library 2.x, Capture 9/10): short-prefix framing, u16 property indices, primitives without length envelopes; both display-property widths; legacy-framed Packages inside modern files. |
| A0 | Allegro board walker, 16.0–19.0: header, string table, every object type sized per format generation. |
| A2 | Allegro → `ir::Geometry`: footprints, pads (padstacks, mask/paste), vias, tracks, copper shapes with voids, zone boundaries, keepouts, graphics, rectangles, drill figures, text, outline. Net classes from constraint sets and match groups. Board-only design model and BOM. |
| C3 | Capture sheet SVGs (cache artwork, pins, displayed properties, wires, aliases, junctions, power/port/off-page bodies, blocks, border, title block, notes wrapped, overbars) and `schematics/geometry.json`. Board layer SVGs reuse the IR-based renderer. |
| M4 | App: detection (`.opj` > `.DSN` > `.brd`, content-sniffed), watcher, frontend `ProjectKind`, cache epoch 44. |
| M5 | CIS variants; Allegro constraint sets; 3D model references (`models/models.json`). |
| Tools | `pcb-extract dump` for `.DSN/.OLB/.brd`; `scripts/orcad-diff/` (KiCad differential, Allegro report checks). |

No code, comments or identifiers were taken from KiCad or any other tool. KiCad's
`FORMAT.md`, `ORCAD_V2_FORMAT.md` and `orcad_dsn.ksy` were read as format evidence;
every decoding was confirmed on corpus bytes and named in this codebase's terms.

## Validation

Corpus: 75 public repositories from OpenOrCadParser's list, plus two found
online (a Capture 9.2 design set and a Capture + Allegro amplifier): 1,704
Capture files and 99 boards. Also KiCad's Allegro QA boards.

| Check | Result |
|---|---|
| Capture files decoded | 1,700 / 1,704 with no notes (the other 4 are a Specctra file and non-compound files), including 13 legacy designs and 542 legacy libraries |
| Nets vs Capture's own `pstxnet.dat` | 1,537 / 1,559 nets with identical pins (98.6%), 97.9% same name, 5,470 / 5,485 pins; the remainder traced to netlists older than their designs |
| Designs vs KiCad master's Capture importer | 143 designs: 10,300 / 10,305 pins grouped alike (the five: three where Capture's own netlist agrees with us, one duplicate designator in the design, one alias choice), 98.2% same explicit name |
| Every sheet handle | 144 designs: every component, net and geometry id resolves to a `data-uuid` on a sheet; no duplicate net names |
| Board placements vs Allegro's `place_txt` / fabmaster | exact on every current export (5/5, 24/24, 55/55, 247/247, 12/12 parts) |
| Boards vs KiCad master's Allegro importer | 93 boards: 33,821 / 33,821 pad nets alike; 6,130 / 6,135 placements (the five are names KiCad invents); track and via counts equal |
| Pad nets vs the Allegro netlist beside each board | grouping identical wherever the netlist is current |
| Board walk | 96 boards 16.4–17.5 to end of stream, 0 misaligned blocks; VCU118 (58 MB, 1.07 M objects) in 2.4 s |
| Subclass meanings | learned from 16 boards' extract reports (~400k primitives matched by coordinates) |
| KiCad QA registry expectations | net classes (DP/MG membership counts, PWR, DEFAULT), via sizes, pad position, drill-figure circle all reproduced; KiCad's class naming (`Allegro_`, `DP_`, `MG_`, `W20mil`) is KiCad's own and not copied |
| CIS variant | TI LAUNCHXL-CC1310 "Standard": not-fitted set equals TI's released BOM DNM list, 25/25 |

Bugs the differential found and fixed: pin numbers on parts placing a non-default
symbol revision (pins 1/2 swapped), storage-name escapes (`:` and `/` as 0x03/0x02)
leaving hierarchical folders orphaned, off-page connectors not meeting same-named
aliases, orphan-folder name collisions.

## Deviations

1. **Allegro before 16.0** (v13–15) is rejected with a clear message ("re-save in 16 or later"); two corpus boards.
2. **Silkscreen role** only for subclasses an extract report confirmed (BOARD/PACKAGE GEOMETRY silkscreen, REF DES silkscreen). VALUE/DEVICE TYPE/TOLERANCE/PART NUMBER silkscreen subclasses keep their own named `user` layers: whether a film prints them is film configuration, and by convention it does not. Unconfirmed fixed subclasses keep a hex name and the `user` role (counted in `source.board.stats.unnamed_layers`).
3. **Pad shapes** the IR cannot draw: octagon and chamfered rectangle become rounded rectangles, shape-symbol pads become code 5 with their box, other rare shapes rectangles; counted (`pads_approximated`). A through pad's shape is its first copper layer's.
4. **Mask/paste**: 17.2+ padstack slots 14/15 (mask) and 16/17 (paste); earlier formats name only the top mask (slot 1) and paste (slot 6), so a pre-17.2 plated hole's bottom mask follows its top.
5. **Zones**: arcs flattened to ~5° steps; voids folded into the outline with zero-width bridges (the IR carries one ring). Dynamic copper (teardrops, fillets) is emitted as copper like any other shape.
6. **Footprint-library templates** (objects owned by a footprint definition) are skipped and counted; a board saved as a library therefore shows only placed instances (KiCad imports the definitions).
7. **Footprints without a component** keep the reference text they draw as their designator (KiCad prefixes `UNK`). Allegro nets with an empty name are named `UNNAMED_<key>` and kept out of net classes.
8. **Board-only BOM** lists placed footprints; components netlisted but never placed are not on it.
9. **Schematic look**: Capture's factory colours (user preferences are not in the design); per-object colours ignored. Pin name/number positions and the name beside a power/port/off-page symbol are computed (Capture stores none). Embedded pictures draw as a dashed frame.
10. **CIS**: a variant leaves off the members of its selected groups whose name says do-not-fit (DNM/DNP/DNS/DNI/…). Group memberships and their stored 0/1 states are reported as data; `orcad_not_fitted_in` names the variants, the base design stays fully fitted (as on the Altium path).
11. **Constraint sets** report the first copper layer's record (width, spacing; clearance and pair gap on 17.2+). Per-layer variation is not reported.
12. **3D models**: Allegro stores only a file name. A file beside the board (or in `step/`, `3d/`, `models/`) is copied; otherwise the reference is counted unresolved. Rotation follows the KiCad writer's sign convention.
13. **Board pairing** looks for the OPJ's board name, a same-stem `.brd`, then a single `.brd` in the project folder or `allegro/`, `pcb/`, `layout/`, `board/`. Deeper layouts (e.g. CutiePi's tape-out folder) are not found.

## Open points

- `atamega.brd` (16.6) has a constraint cross-reference block (0x27) with no stated end and non-zero bytes after it; the walk keeps what precedes it (one corpus board).
- Unconfirmed fixed subclasses: BOARD GEOMETRY 0xED–0xEF, 0xFA, 0xFB; MANUFACTURING 0xF3/0xF4 (likely autosilk) and 0xFD; PACKAGE GEOMETRY 0xF1; DRAWING FORMAT 0xF8/0xFC.
- Some 16.x boards key a constraint set's name by a table index (0x0300xxxx) this reader does not resolve; such sets are named `UNNAMED_<n>`.
- CIS group-state semantics outside do-not-fit groups (the Kinoma design's CIS data is also stale: most ids name no part).
- Names: 1.8% of explicitly named pins carry a different name in KiCad's import, all an alias choice on multi-named nets; against Capture's own netlists our names agree 97.9%, the rest stale netlists.
- Legacy: no hierarchical legacy design and no Library 1.x (Capture 7) sample in the corpus; the legacy hierarchy and short display-property paths are exercised only by flat designs and library packages.
- Allegro components carry no stable instance id, so `CompDef.uuid` is empty and revision pairing is by designator.
- The KiCad oracle is a development-time build of master (`kicad-cli pcb|sch import`); the 10.0.6 release and the nightly image do not read `.brd`.
