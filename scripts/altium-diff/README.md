# The §8.3 differential

`docs/altium-extraction-plan.md` §8.3 ends every phase of the Altium work with a
differential run against a mature independent implementation of the same file
formats. The §10 corner cases are silent wrong answers — extraction succeeds and
the review is confidently incorrect — which is exactly the class of bug our own
tests cannot find, because we would be asserting the same wrong belief in the
code and in the test.

The reference is a **development-time oracle**. It is available locally to the
team, never vendored, never a build dependency, never shipped, and its location
is configured on the machine rather than written into this repo.

## Running it

```bash
cargo build -p extract --bin pcb-extract --release
python scripts/altium-diff/differential.py                 # M0 and M1, whole corpus
python scripts/altium-diff/differential.py --phase m0
python scripts/altium-diff/differential.py --design EVAL --examples 30
python scripts/altium-diff/differential.py --offline        # recorded answers only
```

Exit code 1 means "there is something to triage", not "the build is broken": a
difference is never auto-adopted. The full report is written to
`output/altium-diff/differential.json`.

## The oracle corpus

Asking the reference takes minutes per design — it renders every sheet and every
board layer on the way to the JSON we read — so its answers are **recorded
once** and kept. That recording is the corpus every later run compares against,
and a design dropped into `reference_designs` joins it the first time it is run.

```bash
python scripts/altium-diff/differential.py --build-oracle   # re-ask and re-record
```

What is stored, under `output/altium-diff/oracle/` (one file per design), is the
*reduced* reference document — the fields the comparison reads — not the bundle,
which is tens of megabytes of SVG per design and none of it compared. Each
recorded answer carries the reference's version string and a SHA-256 of the
source document it came from, so an edited design or an upgraded reference makes
the entry stale, says so, and is re-asked; it never quietly compares against last
year's answer.

`--offline` refuses to run the reference at all, which is how a run proves it
used the corpus and nothing else. That is the mode for a machine with the corpus
but no reference installed.

Each answer is stamped with the reference version that produced it, per answer
rather than per design, because the two can differ: when the current reference
**cannot** read a design an older one could, the recorded answer is kept, used,
and reported as carried over rather than dropped. Coverage does not fall away
because the reference regressed.

## Timing

Every run prints what each side took to extract each design. Ours is measured
live; the reference's cost is measured when it is asked and recorded beside its
answer, so a run served from the corpus still reports what that answer cost. An
answer carried over from a reference that is no longer installed is marked `+`
and left out of the totals rather than counted as free.

## Configuration

Everything is optional and read from the environment.

| Variable | What | Default |
|---|---|---|
| `SPINZERO_ALTIUM_REFERENCE` | the reference CLI | `altium-cruncher` on `PATH` |
| `SPINZERO_ALTIUM_REFERENCE_PYTHON` | an interpreter that can import the reference library | the CLI's own virtualenv |
| `SPINZERO_ALTIUM_CORPUS` | corpus root | `D:\git_repo\reference_designs` |
| `SPINZERO_PCB_EXTRACT` | our CLI | `src-tauri/target/release/pcb-extract` |
| `SPINZERO_ALTIUM_ORACLE` | the recorded reference answers | `output/altium-diff/oracle` |

M1 needs only the CLI. M0 additionally needs the library, because a record-level
comparison has no CLI surface; `ref_probe.py` is the one file here that touches
the reference's API, and it runs inside the reference's own interpreter.

## What is compared

| Phase | Comparison document |
|---|---|
| M0 | per stream: byte length, record count, record counts by type; the typed primitive counts for the board's block-framed streams; and the decoded key/value map of every text record that carries a `%UTF8%` key, an escaped pipe, or a non-ASCII byte |
| M1 | component set keyed by designator (value, footprint, library ref, description, pin count, parameter key set **and** parameter values); net set keyed by name with sorted `DESIGNATOR.PIN` membership; hierarchy depth; the BOM row set as a partition of designators |
| M2 | layer table and copper stack order; primitive counts per layer; pad and via geometry index by index (position, size, shape, corner radius, drill, rotation, designator, plating, net); component centroids and rotations against Altium's own pick-and-place |

M2 asks the reference two questions: `json-dump` of the `.PcbDoc`, and
`pnp --position-mode altium-pick-place`, which is the export a component centroid
has to agree with (plan corner case 23). A design with no `.PrjPcb` gets no
pick-and-place and says so.

M3–M4 rows of the plan's table are not implemented yet; add them beside
`compare_m2_board` as those milestones land.

## Normalisation, and what it is allowed to absorb

Both sides emit the same comparison document, so the normaliser only absorbs
differences we *intend* (plan §7). It currently folds:

- `%UTF8%KEY` over `KEY`, and upper-cases keys — the two sides pick between the
  spellings at different moments, and the decoded **value** is what M0 compares;
- the reference's synthetic `UNHANDLED<n>` keys for `|` chunks with no `=`;
- our `Blocks(n)` framing of the board's primitive streams, which is compared
  against the reference's typed primitive counts instead of its record walk;
- single-terminal nets, which the reference does not emit at all unless
  `NetlistSinglePinNets` is set and we emit under an `unconnected-(…)` name;
- BOM grouping policy, by comparing rows as a partition of designators;
- board text with nothing to draw: we do not emit a text whose resolved string is
  empty, so the same *rule* (not our implementation of it) is applied to the
  reference's per-layer counts;
- hierarchy path spelling, by comparing depth rather than the sheet's name.

Teaching the normaliser anything new is the **second** triage outcome below, and
it is a decision, not a convenience. Do not silence a difference to make the run
quiet.

## Triage rule (plan §8.3)

A difference is never auto-adopted. Each one resolves to exactly one of:

1. **our bug** — fix it, and add the case to plan §10 with a named test;
2. **an intended divergence** — record it in plan §7, and teach the normaliser;
3. **a difference in the reference** — record it, keep ours, and note why.

The standing triage is in
[`docs/altium-extraction-notes.md`](../../docs/altium-extraction-notes.md).
