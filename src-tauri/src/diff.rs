//! The semantic diff engine — `diff.a0`.
//!
//! A pure function over two already-materialized design bundles (the `design.json`
//! indexes + the PCB geometry IR of each side) that emits one `DiffDoc`: a list of
//! human-readable, canvas-anchored change statements ("C14 value 100n → 1µ",
//! "/VBUS rerouted on In2.Cu: +12 −3", "R7 moved 3.2 mm, rotated 90°"). See
//! `docs/visual-diff-plan.md` §2.
//!
//! Design constraints that shape the code:
//! - **Deterministic.** Output must be byte-identical for identical inputs — the
//!   codebase treats determinism as load-bearing (cache/dedupe rely on it). Every
//!   collection is sorted with a total order before it becomes a change; change ids
//!   are assigned by ordinal after the final sort.
//! - **Pure.** No filesystem, no locks, no clock. The command layer (`lib.rs`) loads
//!   the bundles and writes the cache; this module only computes.
//! - **Semantic, not pixel.** We diff extracted artifacts, so serialization churn
//!   (uuid/tstamp/reordering) never reaches the changeset by construction.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::design::DesignIndexes;

// ============================================================ the diff document

/// One `(revA, revB)` changeset. Field names match the `diff.a0` schema in
/// `docs/visual-diff-plan.md` §2 exactly (they cross the IPC boundary to
/// `src/lib/diff.ts`), so rename here only in lockstep with the TS mirror.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DiffDoc {
    #[serde(default = "schema_id")]
    pub schema: String,
    /// Older / base side.
    pub a: DiffSide,
    /// Newer / target side.
    pub b: DiffSide,
    pub changes: Vec<Change>,
    /// Count of changes by impact class.
    pub stats: Stats,
    /// Source-hash-identical schematic sheets that were skipped (sheet numbers).
    #[serde(rename = "sheetsPruned")]
    pub sheets_pruned: Vec<i64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct DiffSide {
    pub rev: String,
    pub label: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Stats {
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub electrical: u32,
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub placement: u32,
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub cosmetic: u32,
    #[serde(default, skip_serializing_if = "is_zero_u")]
    pub doc: u32,
}

fn is_zero_u(v: &u32) -> bool {
    *v == 0
}

/// A single semantic change. `id` is assigned last (ordinal, stable within the doc).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Change {
    pub id: String,
    pub group: Group,
    pub kind: Kind,
    pub impact: Impact,
    /// The one-line semantic statement.
    pub title: String,
    /// Longer secondary explanation; empty when there is nothing to add.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub anchors: Anchors,
    /// Which canvas(es) can show this change.
    pub side: Side,
    /// Text to emphasize inside the A-side tint (e.g. the OLD value string of a
    /// field modification) — the renderer colours the matching text red.
    #[serde(rename = "emphA", default, skip_serializing_if = "Option::is_none")]
    pub emph_a: Option<String>,
    /// Text to emphasize inside the B-side tint (the NEW value string) — green.
    #[serde(rename = "emphB", default, skip_serializing_if = "Option::is_none")]
    pub emph_b: Option<String>,
}

impl Default for Change {
    /// The defaulted shape every construction site shares: no id yet (assigned last,
    /// by ordinal), no detail, no emphasis, both-sided, empty anchors. Sites fill in
    /// the signal fields (group/kind/impact/title/anchors) and `..Default::default()`.
    fn default() -> Self {
        Change {
            id: String::new(),
            group: Group::Component,
            kind: Kind::Modified,
            impact: Impact::Cosmetic,
            title: String::new(),
            detail: String::new(),
            anchors: Anchors::default(),
            side: Side::Both,
            emph_a: None,
            emph_b: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Component,
    Net,
    Placement,
    Routing,
    Zone,
    Silk,
    Text,
    Outline,
    Sheet,
    Doc,
    /// BOM-terms restatement of component changes (plan §8): line added/removed,
    /// qty change, line-identity migration, DNP flip, designator move. Derived
    /// from the component maps — never recomputed from a BOM artifact.
    Bom,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Added,
    Removed,
    Modified,
    Renamed,
    Moved,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Impact {
    Electrical,
    Placement,
    Cosmetic,
    Doc,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    A,
    B,
    Both,
}

/// Canvas anchors: where to zoom when this change is stepped to. Both may be present
/// (a net rewire lands on schematic *and* PCB); absent ones are omitted.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Anchors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schematic: Option<SchematicAnchor>,
    /// A-side schematic anchor, set only when the changed object's uuids DIFFER
    /// between the two revisions (a re-annotated symbol, a renamed net's old wires).
    /// The A island paints `schematic_a` when present, else `schematic`.
    #[serde(rename = "schematicA", default, skip_serializing_if = "Option::is_none")]
    pub schematic_a: Option<SchematicAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcb: Option<PcbAnchor>,
    /// BOM-table anchor (plan §8): identifies the table row the change lands on
    /// (scroll + flash) and carries the structured line data the table/CSV render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bom: Option<BomAnchor>,
}

/// The schema id stamped into every diff doc.
fn schema_id() -> String {
    "diff.a0".to_string()
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SchematicAnchor {
    pub sheet: i64,
    /// Element uuids on that sheet (drives the highlight tint).
    pub uuids: Vec<String>,
}

/// One BOM line's identity + the structured delta a change carries. `key` is the
/// grouping key `(value, short footprint, mpn)` joined with `\u{1f}` — the frontend
/// computes the same key over its `BomLine`s to find the row (with a designator-
/// overlap fallback for lines whose BOM-artifact fields differ from the schematic's).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct BomAnchor {
    pub key: String,
    pub value: String,
    /// Short footprint (library prefix stripped), matching `BomLine.footprint`.
    pub footprint: String,
    pub mpn: String,
    /// Line quantity on the A (older) side; 0 when the line is new.
    #[serde(rename = "qtyA")]
    pub qty_a: i64,
    /// Line quantity on the B (newer) side; 0 when the line was removed.
    #[serde(rename = "qtyB")]
    pub qty_b: i64,
    /// The row's designators (B side; A side for a removed line), sorted.
    pub designators: Vec<String>,
    /// Designators responsible for a qty increase ("+2: R33, R34").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Designators responsible for a qty decrease.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Symbol-property edits every member of the line agrees on (MSL, Automotive Grade,
    /// …), so the table can render "old → new" in whatever preset column shows them.
    /// The same edits are spelled out in the change's `detail` for the panel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<BomFieldEdit>,
}

/// One `field: old → new` edit carried on a BOM anchor.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct BomFieldEdit {
    pub field: String,
    pub old: String,
    pub new: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PcbAnchor {
    /// `[x, y, w, h]` in board mm (camera-landing rect). Absent when only a net/comp
    /// id is known (the renderer can land via net/comp bbox instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// True for a routing row that owns the net's changed VIAS (not tracks). A via
    /// spans several copper layers, so it can't belong to any single per-layer track
    /// row — the overlay matches via primitives to this row by net instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vias: bool,
}

// ================================================= geometry IR (deserialize side)
//
// `crate::extract::ir` is Serialize-only, so the diff engine carries its own
// Deserialize mirror of the fields it reads from `pcb/geometry.json`. Kept minimal:
// only what placement/routing/zone/silk/outline diffing needs. Unknown fields are
// ignored (no `deny_unknown_fields`), so extractor additions don't break parsing.

#[derive(Deserialize, Default, Clone)]
pub struct Geometry {
    #[serde(default)]
    pub layers: Vec<GeomLayer>,
    #[serde(default)]
    pub nets: Vec<String>,
    #[serde(default)]
    pub components: Vec<GeomComp>,
    #[serde(default)]
    pub tracks: GeomTracks,
    #[serde(default)]
    pub vias: Vec<GeomVia>,
    #[serde(default)]
    pub zones: Vec<GeomZone>,
    #[serde(default)]
    pub graphics: Vec<GeomGraphic>,
    #[serde(default)]
    pub texts: Vec<GeomText>,
    #[serde(default)]
    pub frame: Option<GeomFrame>,
}

#[derive(Deserialize, Default, Clone)]
pub struct GeomFrame {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub rev: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub paper: String,
}

#[derive(Deserialize, Clone)]
pub struct GeomLayer {
    pub name: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Deserialize, Clone)]
pub struct GeomComp {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub layer: i32,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    /// Stable per-instance identity (the KiCad footprint uuid; EPOCH ≥ 19).
    /// Empty for legacy caches / Altium — those fall back to designator pairing.
    #[serde(default)]
    pub uuid: String,
}

#[derive(Deserialize, Default, Clone)]
pub struct GeomTracks {
    /// Straight segments — `xy` packed 4 per element `[x1,y1,x2,y2]`.
    #[serde(default)]
    pub seg: GeomTrackCol,
    /// Arcs — `xy` packed 6 per element `[sx,sy,mx,my,ex,ey]`.
    #[serde(default)]
    pub arc: GeomTrackCol,
}

/// A struct-of-arrays column of track primitives (segments or arcs — identical shape,
/// only the `xy` stride differs, so one struct serves both `seg` and `arc`). `xy` holds
/// the packed coordinates; `w`/`layer`/`net` are one entry per primitive.
#[derive(Deserialize, Default, Clone)]
pub struct GeomTrackCol {
    #[serde(default)]
    pub xy: Vec<f64>,
    #[serde(default)]
    pub w: Vec<f64>,
    #[serde(default)]
    pub layer: Vec<u16>,
    #[serde(default)]
    pub net: Vec<u32>,
}

#[derive(Deserialize, Clone)]
pub struct GeomVia {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub size: f64,
    #[serde(default)]
    pub net: u32,
    #[serde(default)]
    pub layers: Vec<u16>,
}

#[derive(Deserialize, Clone)]
pub struct GeomZone {
    pub layer: u16,
    #[serde(default)]
    pub net: u32,
    #[serde(default)]
    pub filled: bool,
    #[serde(default)]
    pub pts: Vec<f64>,
}

#[derive(Deserialize, Clone)]
pub struct GeomGraphic {
    pub layer: u16,
    #[serde(default)]
    pub width: f64,
    pub kind: String,
    #[serde(default)]
    pub data: Vec<f64>,
    /// Owning component index (footprint art placed to board space); None = loose
    /// board graphic. Lets the graphics diff skip art a component-level row explains.
    #[serde(default)]
    pub comp: Option<i64>,
}

#[derive(Deserialize, Default, Clone)]
pub struct GeomText {
    pub layer: u16,
    pub text: String,
    pub x: f64,
    pub y: f64,
    // Style fields (mirror of `extract::ir::TextDef`) — absent in the JSON means the
    // extractor's defaults. Compared so a restyle (resize, pen change, font swap)
    // surfaces as a change instead of masking as "unchanged".
    #[serde(default)]
    pub size: Option<f64>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub thickness: Option<f64>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub font: Option<String>,
    /// Owning component index (footprint reference/value/user text); None = loose
    /// board text. Lets the text diff skip text a component-level row explains.
    #[serde(default)]
    pub comp: Option<i64>,
}

// ======================================= schematic geometry (deserialize side)
//
// Deserialize mirror of `crate::extract::sch_geom` — per-element schematic geometry
// keyed by uuid, so the diff can split + anchor graphical edits (a nudged power
// symbol, a redrawn wire) that carry no semantic row. Absent for older caches, which
// keep the one-row-per-sheet `graphical_edit_fallback`.

#[derive(Deserialize, Default, Clone)]
pub struct SchGeometry {
    #[serde(default)]
    pub sheets: Vec<SchSheetGeom>,
}

#[derive(Deserialize, Default, Clone)]
pub struct SchSheetGeom {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub elements: Vec<SchElem>,
}

#[derive(Deserialize, Clone)]
pub struct SchElem {
    pub uuid: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub bbox: [f64; 4],
    #[serde(default)]
    pub sig: String,
}

// ================================================================= input bundle

/// One side of the comparison, already materialized. The command layer builds this
/// from a cache dir; the engine never touches the filesystem.
pub struct Bundle {
    pub rev: String,
    pub label: String,
    pub indexes: DesignIndexes,
    /// Schematic sheet number -> the `.kicad_sch` source file it came from, for
    /// source-hash pruning (`SheetLite` doesn't carry the filename).
    pub sheet_files: HashMap<i64, String>,
    /// The parsed PCB geometry IR, when the bundle has a board. `None` for
    /// schematic-only bundles.
    pub geometry: Option<Geometry>,
    /// Per-element schematic geometry (uuid → position + signature), when the
    /// extraction emitted it. `None` for older caches → the graphical-edit fallback.
    pub sch_geometry: Option<SchGeometry>,
    /// The `.kicad_pcb` source file name, for PCB-pass pruning.
    pub pcb_file: Option<String>,
    /// Per-component (refdes → full property map). Lets the component pass flag edits
    /// to arbitrary symbol fields (Package, Tolerance, Automotive Grade, …) that aren't
    /// first-class on `CompLite`. Empty for older caches.
    pub comp_params: HashMap<String, std::collections::BTreeMap<String, String>>,
}

// ============================================================ tuning constants

/// Position tolerance (mm) below which two component placements count as "same
/// position" — used by re-annotation rename folding.
const POS_EPS_MM: f64 = 0.001;

/// A component move under this distance (mm) isn't worth a placement change row.
const MOVE_EPS_MM: f64 = 0.05;

/// A schematic symbol whose placed bbox centre moved at least this far (mm, sheet
/// units) gets a cosmetic "moved on schematic" row. Half the KiCad 1.27 mm grid, so
/// any real one-step nudge registers while extractor rounding (1 µm) never does.
const SCH_MOVE_EPS_MM: f64 = 0.6;

/// An angle delta under this (deg) isn't a rotation.
const ANGLE_EPS_DEG: f64 = 0.01;

/// Net-rename fold threshold: terminal-set Jaccard similarity at/above this folds a
/// remove+add of two differently-named nets into one rename (plan §2, "~0.7").
const NET_RENAME_JACCARD: f64 = 0.7;

/// Zone area deltas below this (mm²) are refill noise, not a change (plan §2, "~1 mm²").
/// The same floor gates the shape (symmetric-difference) signal — copper that moved by
/// less than a square millimetre isn't worth a row either.
const ZONE_AREA_EPS_MM2: f64 = 1.0;

/// Scanline spacing (mm) for the zone shape (symmetric-difference) estimate. Fine enough
/// to resolve a track-width copper notch; the result only feeds the ~1 mm² threshold, so
/// sub-notch precision isn't needed.
const ZONE_SCAN_STEP_MM: f64 = 0.05;

// ================================================================= entry point

/// An empty changeset for two identical (byte-equal cache-key) revisions.
pub fn empty_doc(rev_a: &str, label_a: &str, rev_b: &str, label_b: &str) -> DiffDoc {
    DiffDoc {
        schema: schema_id(),
        a: DiffSide { rev: rev_a.to_string(), label: label_a.to_string() },
        b: DiffSide { rev: rev_b.to_string(), label: label_b.to_string() },
        changes: Vec::new(),
        stats: Stats::default(),
        sheets_pruned: Vec::new(),
    }
}

/// Compute the changeset for `a` (base) → `b` (target). `source_diff` is the cheap
/// per-file source-hash delta (`rawstore::diff_source_hashes`) used to prune sheets
/// and the whole PCB pass. Pure and deterministic.
pub fn diff_bundles(
    a: &Bundle,
    b: &Bundle,
    source_diff: &crate::rawstore::RevisionDiff,
) -> DiffDoc {
    let mut raw: Vec<Change> = Vec::new();

    // --- schematic-source pruning: a sheet whose .kicad_sch blob is unchanged
    // between A and B cannot contain a schematic change. Union both sides' sheet
    // maps so a renamed/added/removed sheet file isn't spuriously "pruned".
    let changed_sources: HashSet<&str> = source_diff
        .added
        .iter()
        .chain(&source_diff.removed)
        .chain(&source_diff.changed)
        .map(|s| s.as_str())
        .collect();
    let sheets_pruned = pruned_sheets(a, b, &changed_sources);

    // --- semantic groups over the design.json indexes ---
    let comp_delta = diff_components(a, b, &mut raw);
    diff_nets(a, b, &comp_delta, &mut raw);
    diff_pins(a, b, &comp_delta, &mut raw);
    diff_bom(a, b, &comp_delta, &mut raw);
    diff_sheets(a, b, &mut raw);
    diff_docs(a, b, &mut raw);

    // --- PCB geometry groups, unless the whole board is source-identical ---
    if pcb_pass_needed(a, b, &changed_sources) {
        if let (Some(ga), Some(gb)) = (a.geometry.as_ref(), b.geometry.as_ref()) {
            diff_placement(ga, gb, &mut raw);
            diff_routing(ga, gb, &mut raw);
            diff_zones(ga, gb, &mut raw);
            diff_graphics_and_text(ga, gb, &comp_delta, &mut raw);
        }
    }

    // --- schematic graphical edits, split + anchored (needs the per-element geometry
    // artifact). Emits one row per distinct edit — a moved power symbol, a redrawn
    // wire — each anchored to its uuid so clicking it lands the camera. Runs before the
    // fallback so its anchored sheets suppress the clubbed one-row-per-sheet row.
    diff_sch_graphics(a, b, &changed_sources, &mut raw);

    // --- graphical-edit fallback: a sheet whose .kicad_sch source changed but whose
    // edit produced no semantic row above (a nudged power symbol, redrawn wires,
    // moved text) still deserves one line — otherwise the edit is invisible. Covers
    // legacy caches (no geometry artifact) and edits the geometry model doesn't carry.
    graphical_edit_fallback(a, b, &changed_sources, &mut raw);

    finalize(a, b, raw, sheets_pruned)
}

/// Sort the raw changes into a stable total order, assign ordinal ids, and tally stats.
fn finalize(a: &Bundle, b: &Bundle, mut raw: Vec<Change>, mut sheets_pruned: Vec<i64>) -> DiffDoc {
    raw.sort_by(|x, y| change_sort_key(x).cmp(&change_sort_key(y)));
    sheets_pruned.sort_unstable();
    sheets_pruned.dedup();

    let mut stats = Stats::default();
    let changes: Vec<Change> = raw
        .into_iter()
        .enumerate()
        .map(|(i, mut c)| {
            // BOM rows are derived restatements of component changes (plan §8) —
            // counting them would double-report the same edit in the stats.
            if c.group != Group::Bom {
                match c.impact {
                    Impact::Electrical => stats.electrical += 1,
                    Impact::Placement => stats.placement += 1,
                    Impact::Cosmetic => stats.cosmetic += 1,
                    Impact::Doc => stats.doc += 1,
                }
            }
            c.id = format!("ch_{i:04}");
            c
        })
        .collect();

    DiffDoc {
        schema: schema_id(),
        a: DiffSide { rev: a.rev.clone(), label: a.label.clone() },
        b: DiffSide { rev: b.rev.clone(), label: b.label.clone() },
        changes,
        stats,
        sheets_pruned,
    }
}

/// Total order for the stable change sort: by group, then title, then detail. The
/// title carries the identifying string (refdes/net/layer), so lexical order on it
/// is a deterministic, human-sensible grouping.
fn change_sort_key(c: &Change) -> (u8, &str, &str) {
    let g = match c.group {
        Group::Component => 0,
        Group::Net => 1,
        Group::Sheet => 2,
        Group::Doc => 3,
        Group::Placement => 4,
        Group::Routing => 5,
        Group::Zone => 6,
        Group::Silk => 7,
        Group::Text => 8,
        Group::Outline => 9,
        Group::Bom => 10,
    };
    (g, c.title.as_str(), c.detail.as_str())
}

// ============================================================ pruning helpers

/// Sheets whose `.kicad_sch` source file is byte-identical between A and B — reported
/// as `sheetsPruned` in the doc. Informational only: components/nets are diffed from
/// the whole-design indexes, so this does not gate any schematic pass (only the PCB
/// pass is source-gated, via `pcb_pass_needed`). A sheet on only one side is never
/// pruned.
fn pruned_sheets(a: &Bundle, b: &Bundle, changed_sources: &HashSet<&str>) -> Vec<i64> {
    let mut pruned = Vec::new();
    for (num, file) in &a.sheet_files {
        // Only prune sheets present on BOTH sides referencing the SAME file whose
        // blob didn't change.
        let same_on_b = b.sheet_files.get(num).map(|f| f == file).unwrap_or(false);
        if same_on_b && !source_changed(changed_sources, file) {
            pruned.push(*num);
        }
    }
    pruned
}

/// Is `file` (a design.json sheet filename, design-folder-relative) among the changed
/// source paths? The source-hash delta keys are prefixed with the design folder
/// ("MC-02/MC-02.kicad_sch") while design.json carries the bare name — match by
/// exact equality OR path suffix, never by containment.
fn source_changed(changed_sources: &HashSet<&str>, file: &str) -> bool {
    changed_sources.contains(file)
        || changed_sources
            .iter()
            .any(|c| c.ends_with(file) && c[..c.len() - file.len()].ends_with('/'))
}

/// The PCB pass runs unless we can prove the board is source-identical. For KiCad the
/// proof is: both sides name the same `.kicad_pcb` and its blob is unchanged. If we
/// can't identify the pcb file on either side (Altium, or a schematic-only bundle
/// that unexpectedly has geometry), we conservatively diff.
fn pcb_pass_needed(a: &Bundle, b: &Bundle, changed_sources: &HashSet<&str>) -> bool {
    match (a.pcb_file.as_deref(), b.pcb_file.as_deref()) {
        (Some(pa), Some(pb)) if pa == pb => changed_sources.contains(pa),
        _ => true,
    }
}

/// Two schematic bboxes count as "moved" when their centres part by more than half a
/// grid step (the component-move threshold) or their size changes by as much — sub-grid
/// jitter and extractor rounding never trip it.
fn sch_bbox_moved(a: &[f64; 4], b: &[f64; 4]) -> bool {
    let ca = (a[0] + a[2] / 2.0, a[1] + a[3] / 2.0);
    let cb = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
    (ca.0 - cb.0).hypot(ca.1 - cb.1) > SCH_MOVE_EPS_MM
        || (a[2] - b[2]).abs() > SCH_MOVE_EPS_MM
        || (a[3] - b[3]).abs() > SCH_MOVE_EPS_MM
}

/// True when box `a`, inflated by `eps` on every side, overlaps box `b` — the
/// clustering proximity test (`[x, y, w, h]`).
fn sch_boxes_close(a: &[f64; 4], b: &[f64; 4], eps: f64) -> bool {
    let (ax0, ay0, ax1, ay1) = (a[0] - eps, a[1] - eps, a[0] + a[2] + eps, a[1] + a[3] + eps);
    let (bx0, by0, bx1, by1) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax0 <= bx1 && bx0 <= ax1 && ay0 <= by1 && by0 <= ay1
}

fn sch_box_union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let x0 = a[0].min(b[0]);
    let y0 = a[1].min(b[1]);
    let x1 = (a[0] + a[2]).max(b[0] + b[2]);
    let y1 = (a[1] + a[3]).max(b[1] + b[3]);
    [x0, y0, x1 - x0, y1 - y0]
}

#[derive(Clone, Copy, PartialEq)]
enum ElemChange {
    Added,
    Removed,
    Moved,
    Edited,
}

/// One changed schematic element in a cluster: which side(s) carry its uuid, the kind
/// tag (for the row's noun), the nature of the change, and its extent (for clustering).
struct ChangedElem {
    uuid: String,
    kind: String,
    change: ElemChange,
    /// The signature changed — a presentation/content edit (field restyle, redrawn wire).
    /// Orthogonal to any semantic value/footprint row, so it surfaces even for a component
    /// that already has one.
    edited: bool,
    /// The bbox moved past the grid threshold. Can be true *alongside* `edited` (an edited
    /// note that was also dragged), which the cluster verb reports as "edited & moved".
    moved: bool,
    on_a: bool,
    on_b: bool,
    bbox: [f64; 4],
}

/// A human noun for a schematic element kind (for the change title).
fn sch_kind_noun(kind: &str) -> &'static str {
    match kind {
        "power" => "power symbol",
        "symbol" => "symbol",
        "label" | "global_label" | "hier_label" => "label",
        "text" => "note",
        "graphic" => "graphic",
        "netclass_flag" => "net-class flag",
        "wire" => "wire",
        "bus" => "bus",
        "bus_entry" => "bus entry",
        "junction" => "junction",
        _ => "element",
    }
}

/// The dominant element in a cluster (highest priority present), whose noun titles the
/// row — a symbol drag that also stretched its wires reads as "power symbol", not "wire".
fn sch_cluster_noun(members: &[ChangedElem]) -> &'static str {
    const PRIORITY: [&str; 11] = [
        "power", "symbol", "hier_label", "global_label", "label", "text", "graphic",
        "netclass_flag", "bus", "wire", "junction",
    ];
    for k in PRIORITY {
        if members.iter().any(|m| m.kind == k) {
            return sch_kind_noun(k);
        }
    }
    "element"
}

/// One grid step (mm): edits closer than this on a sheet cluster into a single row
/// (a dragged symbol + its stretched wires), while edits further apart stay separate.
const SCH_CLUSTER_EPS_MM: f64 = 2.54;

/// Split each sheet's graphical edits into one anchored change per distinct edit, using
/// the per-element schematic geometry (a nudged power symbol, a redrawn wire — none of
/// which carry a semantic row). Requires the geometry on BOTH sides; a no-op otherwise,
/// so older caches keep the clubbed `graphical_edit_fallback` row. Elements already
/// carried by a semantic anchor (a component move, a net rewire) are skipped so nothing
/// double-reports. Each row anchors to its element uuids, so clicking it lands the camera.
fn diff_sch_graphics(
    a: &Bundle,
    b: &Bundle,
    changed_sources: &HashSet<&str>,
    raw: &mut Vec<Change>,
) {
    let (Some(ga), Some(gb)) = (a.sch_geometry.as_ref(), b.sch_geometry.as_ref()) else {
        return;
    };

    // uuids already explained by a semantic row (component/net anchor) — skip them so a
    // real component's move stays its single row. Owned (not borrowing `raw`) so this
    // pass can push its own rows afterwards.
    let suppressed: HashSet<String> = raw
        .iter()
        .flat_map(|c| c.anchors.schematic.iter().chain(c.anchors.schematic_a.iter()))
        .flat_map(|s| s.uuids.iter().cloned())
        .collect();

    // file → sheet numbers (present on both sides, same file, source changed) — same
    // gating as the fallback; the row lands on the lowest number.
    let mut by_file: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (num, file) in &a.sheet_files {
        let same_on_b = b.sheet_files.get(num).map(|f| f == file).unwrap_or(false);
        if same_on_b && source_changed(changed_sources, file) {
            by_file.entry(file.as_str()).or_default().push(*num);
        }
    }

    for (file, mut nums) in by_file {
        nums.sort_unstable();
        let sheet_num = nums[0];
        let (Some(sa), Some(sb)) = (
            ga.sheets.iter().find(|s| s.file == file),
            gb.sheets.iter().find(|s| s.file == file),
        ) else {
            continue; // no geometry for this file on a side → leave it to the fallback
        };
        let ia: HashMap<&str, &SchElem> =
            sa.elements.iter().map(|e| (e.uuid.as_str(), e)).collect();
        let ib: HashMap<&str, &SchElem> =
            sb.elements.iter().map(|e| (e.uuid.as_str(), e)).collect();

        // Collect changed elements. A-order first (removed / moved / edited), then the
        // B-only additions — both source lists are already sorted, so this is deterministic.
        let mut changed: Vec<ChangedElem> = Vec::new();
        for e in &sa.elements {
            let is_suppressed = suppressed.contains(e.uuid.as_str());
            match ib.get(e.uuid.as_str()) {
                None => {
                    // A removal already carried by a semantic row (a removed component)
                    // needs no second graphical row.
                    if is_suppressed {
                        continue;
                    }
                    changed.push(ChangedElem {
                        uuid: e.uuid.clone(),
                        kind: e.kind.clone(),
                        change: ElemChange::Removed,
                        edited: false,
                        moved: false,
                        on_a: true,
                        on_b: false,
                        bbox: e.bbox,
                    });
                }
                Some(be) => {
                    let edited = e.sig != be.sig;
                    let moved = sch_bbox_moved(&e.bbox, &be.bbox);
                    // A suppressed symbol's MOVE is already its semantic row (e.g. "C68
                    // moved on schematic"), but a presentation EDIT — a restyled or
                    // repositioned field — is never described by a value/footprint/MPN row,
                    // so surface it even when the symbol carries a semantic change.
                    if is_suppressed && !edited {
                        continue;
                    }
                    if edited || moved {
                        changed.push(ChangedElem {
                            uuid: e.uuid.clone(),
                            kind: e.kind.clone(),
                            change: if edited { ElemChange::Edited } else { ElemChange::Moved },
                            edited,
                            moved,
                            on_a: true,
                            on_b: true,
                            bbox: be.bbox, // frame the new position
                        });
                    }
                }
            }
        }
        for e in &sb.elements {
            if suppressed.contains(e.uuid.as_str()) || ia.contains_key(e.uuid.as_str()) {
                continue;
            }
            changed.push(ChangedElem {
                uuid: e.uuid.clone(),
                kind: e.kind.clone(),
                change: ElemChange::Added,
                edited: false,
                moved: false,
                on_a: false,
                on_b: true,
                bbox: e.bbox,
            });
        }
        if changed.is_empty() {
            continue;
        }

        // Deterministic order for clustering: top-left first, uuid to break ties. bboxes
        // are µm-rounded, so integer keys give a total order (f64 has none).
        let key = |v: f64| (v * 1e4) as i64;
        changed.sort_by(|x, y| {
            (key(x.bbox[1]), key(x.bbox[0]), &x.uuid).cmp(&(key(y.bbox[1]), key(y.bbox[0]), &y.uuid))
        });

        // Greedy single-pass clustering: an edit joins the first cluster its box (inflated
        // by a grid step) overlaps, else opens a new one.
        let mut clusters: Vec<([f64; 4], Vec<ChangedElem>)> = Vec::new();
        for el in changed {
            match clusters.iter_mut().find(|(bx, _)| sch_boxes_close(bx, &el.bbox, SCH_CLUSTER_EPS_MM)) {
                Some((bx, members)) => {
                    *bx = sch_box_union(*bx, el.bbox);
                    members.push(el);
                }
                None => clusters.push((el.bbox, vec![el])),
            }
        }

        let name = b
            .indexes
            .sheets
            .iter()
            .find(|s| s.num == sheet_num)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| sheet_num.to_string());

        for (_bbox, members) in clusters {
            let b_uuids: Vec<String> =
                members.iter().filter(|m| m.on_b).map(|m| m.uuid.clone()).collect();
            let a_uuids: Vec<String> =
                members.iter().filter(|m| m.on_a).map(|m| m.uuid.clone()).collect();
            let side = if b_uuids.is_empty() {
                Side::A
            } else if a_uuids.is_empty() {
                Side::B
            } else {
                Side::Both
            };
            // Cluster verb: added/removed when uniform; else "edited" if any signature
            // changed, widened to "edited & moved" when an edit also shifted position
            // (a note that was reworded *and* dragged); else a plain "moved".
            let all = |c: ElemChange| members.iter().all(|m| m.change == c);
            let any_edited = members.iter().any(|m| m.edited);
            let any_moved = members.iter().any(|m| m.moved);
            let (kind, verb) = if all(ElemChange::Added) {
                (Kind::Added, "added")
            } else if all(ElemChange::Removed) {
                (Kind::Removed, "removed")
            } else if any_edited && any_moved {
                (Kind::Modified, "edited & moved")
            } else if any_edited {
                (Kind::Modified, "edited")
            } else {
                (Kind::Moved, "moved")
            };
            let noun = sch_cluster_noun(&members);
            let more = members.len().saturating_sub(1);
            let detail = if more > 0 {
                format!("graphical edit on sheet '{name}' ({} elements); no electrical difference detected", members.len())
            } else {
                format!("graphical edit on sheet '{name}'; no electrical difference detected")
            };
            // Anchor on the side that carries the uuids (B when present, else the A-only
            // removed set); add the A anchor for a two-sided cluster whose A uuids differ.
            let primary = if b_uuids.is_empty() { a_uuids.clone() } else { b_uuids.clone() };
            let mut anchors = Anchors {
                schematic: Some(SchematicAnchor { sheet: sheet_num, uuids: primary }),
                schematic_a: None,
                pcb: None,
                bom: None,
            };
            if side == Side::Both && a_uuids != b_uuids {
                anchors.schematic_a = Some(SchematicAnchor { sheet: sheet_num, uuids: a_uuids });
            }
            raw.push(Change {
                group: Group::Sheet,
                kind,
                impact: Impact::Cosmetic,
                title: format!("Sheet '{name}': {noun} {verb}"),
                detail,
                anchors,
                side,
                ..Default::default()
            });
        }
    }
}

/// One cosmetic row per changed-source sheet (present on both sides, same file) that
/// no schematic-anchored change explains. Multi-instance sheets share one file — the
/// row lands on the lowest sheet number so a hierarchy doesn't repeat itself.
fn graphical_edit_fallback(
    a: &Bundle,
    b: &Bundle,
    changed_sources: &HashSet<&str>,
    raw: &mut Vec<Change>,
) {
    let anchored: HashSet<i64> = raw
        .iter()
        .flat_map(|c| {
            c.anchors
                .schematic
                .iter()
                .chain(c.anchors.schematic_a.iter())
                .map(|s| s.sheet)
        })
        .collect();
    let mut by_file: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (num, file) in &a.sheet_files {
        let same_on_b = b.sheet_files.get(num).map(|f| f == file).unwrap_or(false);
        if same_on_b && source_changed(changed_sources, file) {
            by_file.entry(file.as_str()).or_default().push(*num);
        }
    }
    for (_file, mut nums) in by_file {
        nums.sort_unstable();
        if nums.iter().any(|n| anchored.contains(n)) {
            continue; // a semantic change already lands on this file's sheet(s)
        }
        let num = nums[0];
        let name = b
            .indexes
            .sheets
            .iter()
            .find(|s| s.num == num)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| num.to_string());
        raw.push(Change {
            group: Group::Sheet,
            kind: Kind::Modified,
            impact: Impact::Cosmetic,
            title: format!("Sheet '{name}' has graphical edits"),
            detail: "drawing changed (moved symbols, wires or text); no electrical difference detected".into(),
            anchors: Anchors {
                schematic: Some(SchematicAnchor { sheet: num, uuids: Vec::new() }),
                schematic_a: None,
                pcb: None,
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

// ============================================================ component diff

/// What the component pass learned, fed to the net pass so a component-level event
/// (add/remove/re-annotate) doesn't ALSO surface as net-membership noise: one user
/// action should read as one change row.
#[derive(Default)]
pub struct CompDelta {
    /// Re-annotation pairs, old (A) refdes → new (B) refdes.
    pub renamed: Vec<(String, String)>,
    /// Genuinely added refdes (present only on B).
    pub added: HashSet<String>,
    /// Genuinely removed refdes (present only on A).
    pub removed: HashSet<String>,
}

/// True for symbol properties the component pass already reports elsewhere (as a
/// dedicated field row or the DNP flip), plus KiCad-internal `ki_*` library metadata
/// and pure-documentation fields — none of which should surface again through the
/// generic property diff.
fn is_first_class_param(key: &str) -> bool {
    matches!(
        key,
        // Reported as their own rows above.
        "Reference" | "Value" | "Footprint" | "MPN" | "Manufacturer"
        // DNP is a dedicated flip.
        | "kicad_dnp"
        // Documentation, not an electrical attribute — kept out to avoid noise.
        | "Description" | "Datasheet"
    ) || key.starts_with("ki_")
}

/// Component field comparison + re-annotation rename folding (plan §2).
fn diff_components(a: &Bundle, b: &Bundle, out: &mut Vec<Change>) -> CompDelta {
    let mut delta = CompDelta::default();
    let ca = &a.indexes.components;
    let cb = &b.indexes.components;

    // refdes present on exactly one side.
    let mut only_a: Vec<&String> = ca.keys().filter(|k| !cb.contains_key(*k)).collect();
    let mut only_b: Vec<&String> = cb.keys().filter(|k| !ca.contains_key(*k)).collect();
    only_a.sort();
    only_b.sort();

    // Fold re-annotations: a removed R12 + added R15 with equal (value, footprint) and
    // ~equal placement is ONE rename, not add+remove. Match greedily in sorted order so
    // the pairing is deterministic; each side's candidate is consumed once.
    // Positions are read O(1) from a per-bundle reference→(x,y) index built once here,
    // not by re-scanning geometry.components for every (ra, rb) candidate pair.
    let pos_a = comp_pos_index(a);
    let pos_b = comp_pos_index(b);
    let mut consumed_b: HashSet<&String> = HashSet::new();
    let mut folded_a: HashSet<&String> = HashSet::new();
    for ra in &only_a {
        let comp_a = &ca[*ra];
        let mut best: Option<&String> = None;
        for rb in &only_b {
            if consumed_b.contains(rb) {
                continue;
            }
            let comp_b = &cb[*rb];
            if comp_a.value == comp_b.value
                && comp_a.fp == comp_b.fp
                && placements_match(
                    pos_a.get(ra.as_str()).copied(),
                    pos_b.get(rb.as_str()).copied(),
                )
            {
                best = Some(*rb);
                break;
            }
        }
        if let Some(rb) = best {
            consumed_b.insert(rb);
            folded_a.insert(*ra);
            delta.renamed.push((ra.to_string(), rb.clone()));
            // A re-annotated symbol usually keeps its uuid, but a delete-and-replace
            // does not — carry the A-side anchor so the older canvas can tint too.
            let mut anchors = comp_anchors(b, rb);
            set_schematic_a(&mut anchors, comp_anchors(a, ra));
            out.push(Change {
                group: Group::Component,
                kind: Kind::Renamed,
                impact: Impact::Cosmetic,
                title: format!("{ra} re-annotated → {rb}"),
                detail: format!("same value {} / footprint {}", disp(&comp_a.value), disp(&comp_a.fp)),
                anchors,
                side: Side::Both,
                ..Default::default()
            });
        }
    }

    // Genuine adds/removes (those not folded into a rename).
    for rb in &only_b {
        if consumed_b.contains(rb) {
            continue;
        }
        delta.added.insert(rb.to_string());
        let c = &cb[*rb];
        out.push(Change {
            group: Group::Component,
            kind: Kind::Added,
            impact: Impact::Electrical,
            title: format!("{rb} added"),
            detail: comp_summary(c),
            anchors: comp_anchors(b, rb),
            side: Side::B,
            ..Default::default()
        });
    }
    for ra in &only_a {
        // A removed refdes whose removal was folded into a rename above is skipped —
        // by membership, not by re-deriving the fold predicate: two identical parts
        // can both satisfy the predicate against one consumed B candidate, and only
        // the one actually paired must be suppressed.
        if folded_a.contains(*ra) {
            continue;
        }
        delta.removed.insert(ra.to_string());
        let c = &ca[*ra];
        out.push(Change {
            group: Group::Component,
            kind: Kind::Removed,
            impact: Impact::Electrical,
            title: format!("{ra} removed"),
            detail: comp_summary(c),
            anchors: comp_anchors(a, ra),
            side: Side::A,
            ..Default::default()
        });
    }

    // Field-level modifications for refdes present on both sides.
    let mut common: Vec<&String> = ca.keys().filter(|k| cb.contains_key(*k)).collect();
    common.sort();
    for r in common {
        let (x, y) = (&ca[r], &cb[r]);
        let mut fields: Vec<(&str, &str, &str)> = Vec::new(); // (label, old, new)
        if x.value != y.value {
            fields.push(("value", &x.value, &y.value));
        }
        if x.fp != y.fp {
            fields.push(("footprint", &x.fp, &y.fp));
        }
        if x.mpn != y.mpn {
            fields.push(("MPN", &x.mpn, &y.mpn));
        }
        if x.mfr != y.mfr {
            fields.push(("Manufacturer", &x.mfr, &y.mfr));
        }
        // Edits to arbitrary symbol properties (Package, Tolerance, Automotive Grade,
        // Voltage, …) that `CompLite` doesn't surface. Compared over the union of both
        // sides' keys so an added/removed property registers too. Keys already reported
        // as first-class rows above — and KiCad-internal `ki_*` metadata — are skipped
        // so nothing is double-counted.
        let empty = std::collections::BTreeMap::new();
        let pa = a.comp_params.get(r.as_str()).unwrap_or(&empty);
        let pb = b.comp_params.get(r.as_str()).unwrap_or(&empty);
        let mut param_keys: Vec<&String> = pa.keys().chain(pb.keys()).collect();
        param_keys.sort_unstable();
        param_keys.dedup();
        for key in param_keys {
            if is_first_class_param(key) {
                continue;
            }
            let old = pa.get(key).map(String::as_str).unwrap_or("");
            let new = pb.get(key).map(String::as_str).unwrap_or("");
            if old != new {
                fields.push((key.as_str(), old, new));
            }
        }
        let dnp_flip = x.dnp != y.dnp;

        // Schematic symbol move (bbox-centre delta on the sheet) — its own cosmetic
        // row, independent of field edits and of PCB placement (diff_placement).
        if let (Some(ba), Some(bb)) = (x.bbox, y.bbox) {
            let dx = (bb[0] + bb[2] / 2.0) - (ba[0] + ba[2] / 2.0);
            let dy = (bb[1] + bb[3] / 2.0) - (ba[1] + ba[3] / 2.0);
            let dist = dx.hypot(dy);
            let same_sheet = x.sheet == y.sheet;
            if dist >= SCH_MOVE_EPS_MM || !same_sheet {
                let title = if same_sheet {
                    format!("{r} moved on schematic ({dist:.1} mm)")
                } else {
                    format!("{r} moved to another sheet")
                };
                let mut anchors = comp_anchors(b, r);
                set_schematic_a(&mut anchors, comp_anchors(a, r));
                out.push(Change {
                    group: Group::Component,
                    kind: Kind::Moved,
                    impact: Impact::Cosmetic,
                    title,
                    detail: String::new(),
                    anchors,
                    side: Side::Both,
                    ..Default::default()
                });
            }
        }

        if fields.is_empty() && !dnp_flip {
            continue;
        }
        // The headline uses the first (most electrical) field; the rest go to detail.
        // The first field's old/new strings ride along as per-side emphasis so the
        // canvases can colour exactly the changed text (old red on A, new green on B).
        let (title, detail, impact);
        let mut emph: (Option<String>, Option<String>) = (None, None);
        if let Some((label, old, new)) = fields.first().copied() {
            title = format!("{r} {label} {} → {}", disp(old), disp(new));
            if !old.is_empty() && !new.is_empty() {
                emph = (Some(old.to_string()), Some(new.to_string()));
            }
            let mut extra: Vec<String> = fields
                .iter()
                .skip(1)
                .map(|(l, o, n)| format!("{l} {} → {}", disp(o), disp(n)))
                .collect();
            if dnp_flip {
                extra.push(if y.dnp { "marked DNP".into() } else { "un-marked DNP".into() });
            }
            detail = extra.join("; ");
            // value/footprint/mpn/manufacturer and the part-attribute properties
            // (Package, Tolerance, Automotive Grade, …) are all electrical-ish; footprint
            // is placement-relevant but treated electrical here (land pattern / part).
            impact = Impact::Electrical;
        } else {
            // DNP-only flip.
            title = format!("{r} {}", if y.dnp { "marked DNP" } else { "un-marked DNP" });
            detail = String::new();
            impact = Impact::Electrical;
        }
        let mut anchors = comp_anchors(b, r);
        set_schematic_a(&mut anchors, comp_anchors(a, r));
        out.push(Change {
            emph_a: emph.0,
            emph_b: emph.1,
            group: Group::Component,
            kind: Kind::Modified,
            impact,
            title,
            detail,
            anchors,
            side: Side::Both,
            ..Default::default()
        });
    }
    delta
}

/// Attach `a_side`'s schematic anchor as the change's A-side anchor when it differs
/// from the (B-side) `schematic` — identical anchors need no duplicate.
fn set_schematic_a(anchors: &mut Anchors, a_side: Anchors) {
    if let Some(sa) = a_side.schematic {
        if anchors.schematic.as_ref() != Some(&sa) {
            anchors.schematic_a = Some(sa);
        }
    }
}

/// Two refdes occupy ~the same board position (for re-annotation folding). If neither
/// side has PCB geometry, position is unknown → treat as matching (value+fp equality
/// alone then carries the fold, which is the schematic-only case).
fn placements_match(pa: Option<(f64, f64)>, pb: Option<(f64, f64)>) -> bool {
    match (pa, pb) {
        (Some((xa, ya)), Some((xb, yb))) => (xa - xb).abs() < POS_EPS_MM && (ya - yb).abs() < POS_EPS_MM,
        _ => true,
    }
}

/// Build reference → (x, y) once per bundle so re-annotation matching is an O(1) lookup
/// per candidate pair instead of a linear scan of geometry.components. First occurrence
/// wins, matching the old `find()`'s first-hit behaviour on a duplicated refdes.
fn comp_pos_index(bundle: &Bundle) -> HashMap<&str, (f64, f64)> {
    let mut m: HashMap<&str, (f64, f64)> = HashMap::new();
    if let Some(g) = bundle.geometry.as_ref() {
        for c in &g.components {
            m.entry(c.reference.as_str()).or_insert((c.x, c.y));
        }
    }
    m
}

fn comp_summary(c: &crate::design::CompLite) -> String {
    let mut parts = Vec::new();
    if !c.value.is_empty() {
        parts.push(format!("value {}", c.value));
    }
    if !c.fp.is_empty() {
        parts.push(format!("footprint {}", c.fp));
    }
    if c.dnp {
        parts.push("DNP".into());
    }
    parts.join(", ")
}

/// Schematic + PCB anchors for a component in `bundle` (by refdes).
fn comp_anchors(bundle: &Bundle, refdes: &str) -> Anchors {
    let mut anchors = Anchors::default();
    if let Some(c) = bundle.indexes.components.get(refdes) {
        if let (Some(sheet), false) = (c.sheet, c.svg_id.is_empty()) {
            anchors.schematic = Some(SchematicAnchor { sheet, uuids: vec![c.svg_id.clone()] });
        }
    }
    if let Some(g) = bundle.geometry.as_ref() {
        if let Some(gc) = g.components.iter().find(|c| c.reference == refdes) {
            let layers = (gc.layer >= 0)
                .then(|| g.layers.get(gc.layer as usize).map(|l| l.name.clone()))
                .flatten()
                .into_iter()
                .collect();
            anchors.pcb = Some(PcbAnchor {
                bbox: gc.bbox,
                layers,
                comp: Some(refdes.to_string()),
                net: None,
                vias: false,
            });
        }
    }
    anchors
}

// ================================================================= pin diff

/// `(refdes, pin number)` → `(electrical type, pin name)`, gathered from every net's
/// terminal list — design.json's only per-pin record. A-side designators are
/// canonicalized through `rename` so a re-annotated part keeps its pins. A pin listed on
/// several nets with conflicting types is dropped as ambiguous rather than guessed at.
///
/// Unconnected pins have no terminal anywhere, so they are invisible to this index; the
/// library-body signature (`sch_geom::lib_body_sig`) still surfaces such an edit as a
/// graphical row on the sheet.
fn pin_type_index<'a>(
    bundle: &'a Bundle,
    rename: &HashMap<&str, &str>,
) -> BTreeMap<(String, String), (&'a str, &'a str)> {
    let mut out: BTreeMap<(String, String), (&str, &str)> = BTreeMap::new();
    let mut ambiguous: HashSet<(String, String)> = HashSet::new();
    for net in bundle.indexes.nets.values() {
        for t in &net.terminals {
            let d = rename.get(t.d.as_str()).copied().unwrap_or(t.d.as_str());
            let key = (d.to_string(), t.p.clone());
            match out.get(&key) {
                Some((etype, _)) if *etype != t.pt.as_str() => {
                    ambiguous.insert(key);
                }
                Some(_) => {}
                None => {
                    out.insert(key, (t.pt.as_str(), t.pn.as_str()));
                }
            }
        }
    }
    for key in ambiguous {
        out.remove(&key);
    }
    out
}

/// Pin electrical-type edits (`input` → `output`, `passive` → `power_in`, …).
///
/// A pin's electrical type lives in the *library symbol*, not on the placed instance, so
/// none of the component/net passes above see it: the netlist connectivity is unchanged,
/// every field is unchanged, and nothing moved. It is a genuine electrical statement
/// though — it changes what the part drives and what ERC will accept — so it gets its own
/// `Electrical` row, anchored to the owning component on both canvases.
fn diff_pins(a: &Bundle, b: &Bundle, comps: &CompDelta, out: &mut Vec<Change>) {
    let rename: HashMap<&str, &str> =
        comps.renamed.iter().map(|(x, y)| (x.as_str(), y.as_str())).collect();
    let ia = pin_type_index(a, &rename);
    let ib = pin_type_index(b, &HashMap::new());

    for ((refdes, pin), (old, name_a)) in &ia {
        // A pin that vanished (or whose component did) is already told by the net
        // membership / component removal row — only a *retyped* surviving pin lands here.
        let Some((new, name_b)) = ib.get(&(refdes.clone(), pin.clone())) else { continue };
        if old == new {
            continue;
        }
        let name = if name_b.is_empty() { *name_a } else { *name_b };
        let detail = if name.is_empty() || name == "~" {
            String::new()
        } else {
            format!("pin name '{name}'")
        };
        let mut anchors = comp_anchors(b, refdes);
        set_schematic_a(&mut anchors, comp_anchors(a, refdes));
        out.push(Change {
            group: Group::Component,
            kind: Kind::Modified,
            impact: Impact::Electrical,
            title: format!("{refdes} pin {pin} electrical type {} → {}", disp(old), disp(new)),
            detail,
            anchors,
            side: Side::Both,
            // No emphasis: the electrical type is not drawn on either canvas, so there is
            // no text for the renderer to tint.
            ..Default::default()
        });
    }
}

// ================================================================= BOM diff
//
// Plan §8: `BomLine` is a grouping of the component table, so the BOM changeset is
// DERIVED from the component maps (canonicalized through the component pass's
// re-annotation rename map), never recomputed from a BOM artifact. Groups both
// sides' components by (value, short footprint, mpn) and expresses the deltas in
// build terms: line added/removed, qty change with responsible designators,
// line-identity migration (one "changed" row when designator overlap is high),
// DNP flips, and designator moves between surviving lines.

/// Line-identity fold threshold: designator-set Jaccard at/above this folds an
/// A-only + B-only line pair into one "changed" row (the line's value/fp/mpn edited
/// in place) instead of remove+add.
const BOM_LINE_JACCARD: f64 = 0.5;

/// The `(value, short-footprint, mpn)` grouping key. `\u{1f}` (unit separator) can't
/// appear in schematic fields; the TS side computes the identical key over BomLines.
fn bom_key(value: &str, fp: &str, mpn: &str) -> String {
    format!("{}\u{1f}{}\u{1f}{}", value, fp_short(fp), mpn)
}

/// Short footprint name — library prefix stripped, matching `design::bom_lines`.
fn fp_short(fp: &str) -> &str {
    fp.rsplit(':').next().unwrap_or("")
}

/// Human label for a BOM line: "10k R_0402_1005Metric" (whichever parts exist).
fn bom_label(value: &str, fp: &str, mpn: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !value.is_empty() {
        parts.push(value);
    }
    let short = fp_short(fp);
    if !short.is_empty() {
        parts.push(short);
    }
    if parts.is_empty() && !mpn.is_empty() {
        parts.push(mpn);
    }
    if parts.is_empty() {
        "(unnamed line)".to_string()
    } else {
        parts.join(" ")
    }
}

/// One BOM line during derivation: display fields + the sorted designator set.
struct BomGroup {
    value: String,
    footprint: String,
    mpn: String,
    dsg: BTreeSet<String>,
}

/// One side's BOM grouping: key → line. Designators are canonicalized through
/// `rename` (A-side old name → B-side new name) so a re-annotation produces zero
/// BOM churn.
fn bom_lines_of(
    comps: &HashMap<String, crate::design::CompLite>,
    rename: &HashMap<&str, &str>,
) -> BTreeMap<String, BomGroup> {
    let mut lines: BTreeMap<String, BomGroup> = BTreeMap::new();
    for (refdes, c) in comps {
        let key = bom_key(&c.value, &c.fp, &c.mpn);
        let d = rename.get(refdes.as_str()).copied().unwrap_or(refdes.as_str());
        lines
            .entry(key)
            .or_insert_with(|| BomGroup {
                value: c.value.clone(),
                footprint: fp_short(&c.fp).to_string(),
                mpn: c.mpn.clone(),
                dsg: BTreeSet::new(),
            })
            .dsg
            .insert(d.to_string());
    }
    lines
}

fn bom_anchor(
    key: &str,
    line: &BomGroup,
    qty_a: i64,
    qty_b: i64,
    added: Vec<String>,
    removed: Vec<String>,
) -> Anchors {
    Anchors {
        schematic: None,
        schematic_a: None,
        pcb: None,
        bom: Some(BomAnchor {
            key: key.to_string(),
            value: line.value.clone(),
            footprint: line.footprint.clone(),
            mpn: line.mpn.clone(),
            qty_a,
            qty_b,
            designators: line.dsg.iter().cloned().collect(),
            added,
            removed,
            fields: Vec::new(),
        }),
    }
}

fn bom_change(kind: Kind, title: String, detail: String, side: Side, anchors: Anchors) -> Change {
    Change {
        id: String::new(),
        emph_a: None,
        emph_b: None,
        group: Group::Bom,
        kind,
        impact: Impact::Electrical,
        title,
        detail,
        anchors,
        side,
    }
}

/// The symbol-property edits (MSL, Automotive Grade, Tolerance, …) shared by every
/// member of a BOM line. They ride structured on the anchor — a free-form field name and
/// a free-form value can't be split back out of the `detail` string — so a preset column
/// showing one of them can render "old → new" like the built-in columns do.
///
/// Only edits every common member agrees on are reported: a BOM line is one purchasing
/// decision, and attributing one member's edit to the whole line would put a wrong "old"
/// value in the cell. Members that exist on only one side are ignored (their line already
/// has an add/remove/qty row).
/// Symbol properties `bom_param_edits` must not report: the ones the BOM row already
/// compares itself, and KiCad's internal bookkeeping (`ki_*`/`kicad_*`, which the BOM
/// extractor drops too). Unlike the component pass this keeps `Manufacturer` — it is a
/// BOM column and a purchasing-relevant change — but still drops `Description`, whose
/// blurb churns without the part changing.
fn is_bom_line_field(key: &str) -> bool {
    matches!(key, "Reference" | "Value" | "Footprint" | "MPN" | "Description" | "Datasheet")
        || key.starts_with("ki_")
        || key.starts_with("kicad_")
}

fn bom_param_edits(
    a: &Bundle,
    b: &Bundle,
    rename: &HashMap<&str, &str>,
    dsgs_b: &BTreeSet<String>,
) -> Vec<BomFieldEdit> {
    // B-side designator → its A-side name (the rename map runs A → B).
    let back: HashMap<&str, &str> = rename.iter().map(|(x, y)| (*y, *x)).collect();
    let empty = std::collections::BTreeMap::new();
    let mut agreed: BTreeMap<String, (String, String)> = BTreeMap::new(); // field → (old, new)
    let mut rejected: HashSet<String> = HashSet::new();
    let mut members = 0usize;
    for d in dsgs_b {
        let ra = back.get(d.as_str()).copied().unwrap_or(d.as_str());
        if !a.indexes.components.contains_key(ra) {
            continue;
        }
        members += 1;
        let pa = a.comp_params.get(ra).unwrap_or(&empty);
        let pb = b.comp_params.get(d.as_str()).unwrap_or(&empty);
        let mut keys: Vec<&String> = pa.keys().chain(pb.keys()).collect();
        keys.sort_unstable();
        keys.dedup();
        let mut seen: HashSet<&str> = HashSet::new();
        for key in keys {
            if is_bom_line_field(key) {
                continue;
            }
            let old = pa.get(key).map(String::as_str).unwrap_or("");
            let new = pb.get(key).map(String::as_str).unwrap_or("");
            if old == new {
                continue;
            }
            seen.insert(key.as_str());
            match agreed.get(key) {
                Some((o, n)) if o == old && n == new => {}
                Some(_) => {
                    rejected.insert(key.clone());
                }
                None if members == 1 => {
                    agreed.insert(key.clone(), (old.to_string(), new.to_string()));
                }
                // A later member introducing an edit the earlier ones didn't have means
                // the members disagree.
                None => {
                    rejected.insert(key.clone());
                }
            }
        }
        // An edit an earlier member had but this one doesn't is a disagreement too.
        for key in agreed.keys() {
            if !seen.contains(key.as_str()) {
                rejected.insert(key.clone());
            }
        }
    }
    agreed
        .into_iter()
        .filter(|(k, _)| !rejected.contains(k))
        .map(|(field, (old, new))| BomFieldEdit { field, old, new })
        .collect()
}

/// The panel-facing `"<field> old → new"` lines for a set of edits.
fn edit_bits(edits: &[BomFieldEdit]) -> impl Iterator<Item = String> + '_ {
    edits.iter().map(|e| format!("{} {} → {}", e.field, disp(&e.old), disp(&e.new)))
}

/// Attach the line's field edits to a BOM anchor built by `bom_anchor`.
fn with_field_edits(mut anchors: Anchors, edits: Vec<BomFieldEdit>) -> Anchors {
    if let Some(bom) = anchors.bom.as_mut() {
        bom.fields = edits;
    }
    anchors
}

/// Derive the BOM changeset (plan §8). `comps` is the component pass's verdict —
/// its rename map canonicalizes A-side designators so re-annotations are invisible
/// in BOM terms.
fn diff_bom(a: &Bundle, b: &Bundle, comps: &CompDelta, out: &mut Vec<Change>) {
    let rename: HashMap<&str, &str> =
        comps.renamed.iter().map(|(x, y)| (x.as_str(), y.as_str())).collect();
    let no_rename: HashMap<&str, &str> = HashMap::new();
    let la = bom_lines_of(&a.indexes.components, &rename);
    let lb = bom_lines_of(&b.indexes.components, &no_rename);

    // --- line-identity folds: an A-only key + a B-only key whose designator sets
    // overlap highly are ONE changed line (its value/fp/mpn migrated), not
    // remove+add. Best-Jaccard-first greedy pairing (same idiom as net renames).
    let only_a: Vec<&String> = la.keys().filter(|k| !lb.contains_key(*k)).collect();
    let only_b: Vec<&String> = lb.keys().filter(|k| !la.contains_key(*k)).collect();
    let mut candidates: Vec<(String, &String, &String)> = Vec::new();
    for ka in &only_a {
        for kb in &only_b {
            let j = jaccard(&la[*ka].dsg, &lb[*kb].dsg);
            if j >= BOM_LINE_JACCARD {
                candidates.push((format!("{:08.5}", 1.0 - j), *ka, *kb));
            }
        }
    }
    candidates.sort();
    let mut consumed_a: HashSet<&String> = HashSet::new();
    let mut consumed_b: HashSet<&String> = HashSet::new();
    let mut fold_pair: HashMap<String, String> = HashMap::new(); // A-key → its folded B-key
    for (_, ka, kb) in &candidates {
        if consumed_a.contains(ka) || consumed_b.contains(kb) {
            continue;
        }
        consumed_a.insert(ka);
        consumed_b.insert(kb);
        fold_pair.insert((*ka).clone(), (*kb).clone());
        let ga = &la[*ka];
        let gb = &lb[*kb];
        let mut bits: Vec<String> = Vec::new();
        if ga.value != gb.value {
            bits.push(format!("value {} → {}", disp(&ga.value), disp(&gb.value)));
        }
        if ga.footprint != gb.footprint {
            bits.push(format!("footprint {} → {}", disp(&ga.footprint), disp(&gb.footprint)));
        }
        if ga.mpn != gb.mpn {
            bits.push(format!("MPN {} → {}", disp(&ga.mpn), disp(&gb.mpn)));
        }
        if ga.dsg.len() != gb.dsg.len() {
            bits.push(format!("qty {} → {}", ga.dsg.len(), gb.dsg.len()));
        }
        let edits = bom_param_edits(a, b, &rename, &gb.dsg);
        bits.extend(edit_bits(&edits));
        let added: Vec<String> = gb.dsg.difference(&ga.dsg).cloned().collect();
        let removed: Vec<String> = ga.dsg.difference(&gb.dsg).cloned().collect();
        let anchors = with_field_edits(
            bom_anchor(kb, gb, ga.dsg.len() as i64, gb.dsg.len() as i64, added, removed),
            edits,
        );
        out.push(bom_change(
            Kind::Modified,
            format!(
                "BOM line {} → {}",
                bom_label(&ga.value, &ga.footprint, &ga.mpn),
                bom_label(&gb.value, &gb.footprint, &gb.mpn)
            ),
            bits.join("; "),
            Side::Both,
            anchors,
        ));
    }

    // --- designator moves between lines: a common part (post-rename identity) whose
    // grouping key changed. Unless the line-identity fold above already explains it
    // (the WHOLE line migrated: its A-key folded onto exactly this B-key), the part
    // hopped lines and gets its own row — including when an endpoint line doesn't
    // survive (a move onto a brand-new line, or out of a line that empties). The
    // line add/remove and qty passes below then suppress what these rows explain.
    let mut movers: HashSet<String> = HashSet::new(); // refs explained by a move row
    // (ref, from, to, toKey, bits) — `bits` is the same "value A → B; MPN A → B"
    // string the fold pass builds, so the title's from/to labels are never ambiguous
    // (a pure MPN swap renders identical labels) and the frontend's inline old→new
    // parsing (bomOldValues) lights up for the moved part too.
    let mut move_rows: Vec<(String, String, String, String, String)> = Vec::new();
    // Kept beside `move_rows` rather than in it: the tuple is sorted, and the edits are
    // looked up by designator when the row's anchor is built below.
    let mut move_edits: HashMap<String, Vec<BomFieldEdit>> = HashMap::new();
    for (ra, ca) in &a.indexes.components {
        let d = rename.get(ra.as_str()).copied().unwrap_or(ra.as_str());
        let Some(cb) = b.indexes.components.get(d) else { continue };
        let ka = bom_key(&ca.value, &ca.fp, &ca.mpn);
        let kb = bom_key(&cb.value, &cb.fp, &cb.mpn);
        if ka == kb || fold_pair.get(&ka) == Some(&kb) {
            continue;
        }
        movers.insert(d.to_string());
        let mut bits: Vec<String> = Vec::new();
        if ca.value != cb.value {
            bits.push(format!("value {} → {}", disp(&ca.value), disp(&cb.value)));
        }
        if ca.fp != cb.fp {
            bits.push(format!("footprint {} → {}", disp(&ca.fp), disp(&cb.fp)));
        }
        if ca.mpn != cb.mpn {
            bits.push(format!("MPN {} → {}", disp(&ca.mpn), disp(&cb.mpn)));
        }
        let edits = bom_param_edits(a, b, &rename, &BTreeSet::from([d.to_string()]));
        bits.extend(edit_bits(&edits));
        move_edits.insert(d.to_string(), edits);
        move_rows.push((
            d.to_string(),
            bom_label(&ca.value, &ca.fp, &ca.mpn),
            bom_label(&cb.value, &cb.fp, &cb.mpn),
            kb,
            bits.join("; "),
        ));
    }
    move_rows.sort();
    for (d, from, to, to_key, bits) in &move_rows {
        let line = &lb[to_key];
        let qty_a = la.get(to_key).map(|l| l.dsg.len() as i64).unwrap_or(0);
        let anchors = with_field_edits(
            bom_anchor(to_key, line, qty_a, line.dsg.len() as i64, vec![d.clone()], Vec::new()),
            move_edits.remove(d).unwrap_or_default(),
        );
        out.push(bom_change(
            Kind::Modified,
            format!("BOM: {d} moved {from} → {to} line"),
            bits.clone(),
            Side::Both,
            anchors,
        ));
    }

    // --- genuine line adds/removes (not folded, and not fully explained by moves:
    // a line whose every designator moved in/out is already told by the move rows).
    for kb in &only_b {
        if consumed_b.contains(kb) {
            continue;
        }
        let line = &lb[*kb];
        if line.dsg.iter().all(|d| movers.contains(d)) {
            continue;
        }
        let names: Vec<String> = line.dsg.iter().cloned().collect();
        let anchors = bom_anchor(kb, line, 0, names.len() as i64, names.clone(), Vec::new());
        out.push(bom_change(
            Kind::Added,
            format!("BOM line added: {}", bom_label(&line.value, &line.footprint, &line.mpn)),
            format!("×{}: {}", names.len(), names.join(", ")),
            Side::B,
            anchors,
        ));
    }
    for ka in &only_a {
        if consumed_a.contains(ka) {
            continue;
        }
        let line = &la[*ka];
        if line.dsg.iter().all(|d| movers.contains(d)) {
            continue;
        }
        let names: Vec<String> = line.dsg.iter().cloned().collect();
        let anchors = bom_anchor(ka, line, names.len() as i64, 0, Vec::new(), names.clone());
        out.push(bom_change(
            Kind::Removed,
            format!("BOM line removed: {}", bom_label(&line.value, &line.footprint, &line.mpn)),
            format!("×{}: {}", names.len(), names.join(", ")),
            Side::A,
            anchors,
        ));
    }

    // --- qty changes on lines present on both sides, naming the responsible
    // designators ("+2: R33, R34"). Deltas fully explained by move rows are
    // suppressed (one user action = one row); mixed deltas keep the row with the
    // movers marked.
    for (k, gb) in &lb {
        let Some(ga) = la.get(k) else { continue };
        if ga.dsg == gb.dsg {
            continue;
        }
        let added: Vec<&String> = gb.dsg.difference(&ga.dsg).collect();
        let removed: Vec<&String> = ga.dsg.difference(&gb.dsg).collect();
        if added.iter().chain(&removed).all(|d| movers.contains(*d)) {
            continue;
        }
        let mark = |d: &&String| {
            if movers.contains(*d) {
                format!("{d} (moved)")
            } else {
                (*d).clone()
            }
        };
        let mut bits: Vec<String> = Vec::new();
        if !added.is_empty() {
            bits.push(format!(
                "+{}: {}",
                added.len(),
                added.iter().map(mark).collect::<Vec<_>>().join(", ")
            ));
        }
        if !removed.is_empty() {
            bits.push(format!(
                "−{}: {}",
                removed.len(),
                removed.iter().map(mark).collect::<Vec<_>>().join(", ")
            ));
        }
        let anchors = bom_anchor(
            k,
            gb,
            ga.dsg.len() as i64,
            gb.dsg.len() as i64,
            added.iter().map(|d| (*d).clone()).collect(),
            removed.iter().map(|d| (*d).clone()).collect(),
        );
        out.push(bom_change(
            Kind::Modified,
            format!(
                "BOM {}: qty {} → {}",
                bom_label(&gb.value, &gb.footprint, &gb.mpn),
                ga.dsg.len(),
                gb.dsg.len()
            ),
            bits.join("; "),
            Side::Both,
            anchors,
        ));
    }

    // --- symbol-property edits on a line that is otherwise untouched: same grouping key,
    // same designators, but a field the purchaser cares about moved (MSL, Automotive
    // Grade, Tolerance, …). Without this the BOM tab shows nothing for such a revision,
    // even though the component pass reports it.
    for (k, gb) in &lb {
        let Some(ga) = la.get(k) else { continue };
        if ga.dsg != gb.dsg {
            continue; // the qty/fold passes above already own this line
        }
        let edits = bom_param_edits(a, b, &rename, &gb.dsg);
        if edits.is_empty() {
            continue;
        }
        let detail = edit_bits(&edits).collect::<Vec<_>>().join("; ");
        let qty = gb.dsg.len() as i64;
        let anchors =
            with_field_edits(bom_anchor(k, gb, qty, qty, Vec::new(), Vec::new()), edits);
        out.push(bom_change(
            Kind::Modified,
            format!("BOM {}: fields edited", bom_label(&gb.value, &gb.footprint, &gb.mpn)),
            detail,
            Side::Both,
            anchors,
        ));
    }

    // --- DNP flips, called out separately (fit/no-fit is a build change reviewers
    // miss). One row per flipped designator, anchored to its B-side line.
    let mut flips: Vec<(String, bool, String)> = Vec::new(); // (ref, nowDnp, keyB)
    for (ra, ca) in &a.indexes.components {
        let d = rename.get(ra.as_str()).copied().unwrap_or(ra.as_str());
        let Some(cb) = b.indexes.components.get(d) else { continue };
        if ca.dnp != cb.dnp {
            flips.push((d.to_string(), cb.dnp, bom_key(&cb.value, &cb.fp, &cb.mpn)));
        }
    }
    flips.sort();
    for (d, now_dnp, kb) in &flips {
        let Some(line) = lb.get(kb) else { continue };
        let qty = line.dsg.len() as i64;
        let anchors = bom_anchor(kb, line, qty, qty, Vec::new(), Vec::new());
        out.push(bom_change(
            Kind::Modified,
            format!(
                "BOM: {d} {}",
                if *now_dnp { "marked DNP (do not fit)" } else { "un-marked DNP (now fitted)" }
            ),
            format!("line {}", bom_label(&line.value, &line.footprint, &line.mpn)),
            Side::Both,
            anchors,
        ));
    }
}

// ================================================================= net diff

/// Net membership diff with name-rename folding (Jaccard on the terminal set) and
/// per-pin membership-move changes (plan §2).
///
/// `comps` is the component pass's verdict: A-side designators are canonicalized
/// through its rename map (so a re-annotated C69→C169 is the SAME terminal, not
/// churn on every net it touches), and pins that appeared/vanished with an added/
/// removed component are excluded from membership rows — the component row already
/// tells that story.
fn diff_nets(a: &Bundle, b: &Bundle, comps: &CompDelta, out: &mut Vec<Change>) {
    let na = &a.indexes.nets;
    let nb = &b.indexes.nets;

    // A-side designator → its B-side name (identity when not re-annotated).
    let rename: HashMap<&str, &str> =
        comps.renamed.iter().map(|(x, y)| (x.as_str(), y.as_str())).collect();

    let mut only_a: Vec<&String> = na.keys().filter(|k| !nb.contains_key(*k)).collect();
    let mut only_b: Vec<&String> = nb.keys().filter(|k| !na.contains_key(*k)).collect();
    only_a.sort();
    only_b.sort();

    // --- rename folding: an unmatched name on A + one on B with terminal-set Jaccard
    // >= threshold is one rename. Score every candidate pair; take the highest, then
    // the next, greedily but deterministically (sorted candidate order breaks ties).
    let mut candidates: Vec<(String, &String, &String)> = Vec::new(); // (jac-key, a, b)
    // tb depends only on rb — build each B-side terminal set once, not once per (ra, rb)
    // pair (the inner loop otherwise rebuilds a fresh BTreeSet of formatted strings for
    // every A candidate).
    let tb_sets: Vec<BTreeSet<String>> = only_b.iter().map(|rb| terminal_set(&nb[*rb])).collect();
    for ra in &only_a {
        let ta = terminal_set_mapped(&na[*ra], &rename);
        for (rb, tb) in only_b.iter().zip(tb_sets.iter()) {
            let j = jaccard(&ta, tb);
            if j >= NET_RENAME_JACCARD {
                // Store 1-j zero-padded so ascending sort = best-first, then names.
                candidates.push((format!("{:08.5}", 1.0 - j), *ra, *rb));
            }
        }
    }
    candidates.sort();
    let mut consumed_a: HashSet<&String> = HashSet::new();
    let mut consumed_b: HashSet<&String> = HashSet::new();
    for (_, ra, rb) in &candidates {
        if consumed_a.contains(ra) || consumed_b.contains(rb) {
            continue;
        }
        consumed_a.insert(ra);
        consumed_b.insert(rb);
        let n_pins = terminal_set_mapped(&na[*ra], &rename).len();
        let mut anchors = net_anchors(b, rb);
        set_schematic_a(&mut anchors, net_anchors(a, ra));
        out.push(Change {
            group: Group::Net,
            kind: Kind::Renamed,
            impact: Impact::Electrical,
            title: format!("net {ra} → {rb}"),
            detail: format!("same {n_pins} pin{}", plural(n_pins)),
            anchors,
            side: Side::Both,
            ..Default::default()
        });
    }

    // --- genuine net add/remove (not folded).
    for rb in &only_b {
        if consumed_b.contains(rb) {
            continue;
        }
        let n = &nb[*rb];
        let cnt = n.terminals.len();
        out.push(Change {
            group: Group::Net,
            kind: Kind::Added,
            impact: Impact::Electrical,
            title: format!("net {rb} added"),
            detail: format!("{cnt} pin{}", plural(cnt)),
            anchors: net_anchors(b, rb),
            side: Side::B,
            ..Default::default()
        });
    }
    for ra in &only_a {
        if consumed_a.contains(ra) {
            continue;
        }
        let n = &na[*ra];
        let cnt = n.terminals.len();
        out.push(Change {
            group: Group::Net,
            kind: Kind::Removed,
            impact: Impact::Electrical,
            title: format!("net {ra} removed"),
            detail: format!("{cnt} pin{}", plural(cnt)),
            anchors: net_anchors(a, ra),
            side: Side::A,
            ..Default::default()
        });
    }

    // --- per-pin membership moves + membership changes for nets present on both sides.
    // A terminal that left net X and joined net Y (both present on both sides) is the
    // highest-value electrical statement — surface it once. We compute, per terminal,
    // its net on A vs on B and report the ones that moved between two extant nets.
    let mut common: Vec<&String> = na.keys().filter(|k| nb.contains_key(*k)).collect();
    common.sort();

    // Build terminal -> net maps once (only for nets present on both sides, so a pin
    // that appears/disappears with an add/remove net isn't double-reported).
    let common_set: HashSet<&String> = common.iter().copied().collect();
    let no_rename: HashMap<&str, &str> = HashMap::new();
    let a_of = terminal_owner_map(na, &common_set, &rename);
    let b_of = terminal_owner_map(nb, &common_set, &no_rename);

    let mut moved: Vec<(String, String, String)> = Vec::new(); // (terminal, from, to)
    let mut moved_terms: HashSet<String> = HashSet::new();
    for (term, from) in &a_of {
        if let Some(to) = b_of.get(term) {
            if from != to {
                moved.push((term.clone(), from.clone(), to.clone()));
                moved_terms.insert(term.clone());
            }
        }
    }
    moved.sort();
    // Reverse rename (B refdes → A refdes) so a moved pin's A-side anchor lands on the
    // same part under its OLD designator (a re-annotated C69 → C169).
    let rev_rename: HashMap<&str, &str> =
        comps.renamed.iter().map(|(x, y)| (y.as_str(), x.as_str())).collect();
    for (term, from, to) in moved {
        let (d, p) = split_terminal(&term);
        // Anchor to the COMPONENT whose pin moved — clicking should focus the pin on its
        // own symbol (its home sheet), not jump to the from/to net geometry, which lands
        // on whatever sheet those nets bottom out on (often the root page) (batch 3).
        let a_d = rev_rename.get(d).copied().unwrap_or(d);
        let mut anchors = comp_anchors(b, d);
        set_schematic_a(&mut anchors, comp_anchors(a, a_d));
        // Fall back to the net landing if the part carries no schematic anchor (unplaced
        // in design.json), so the change never becomes unclickable.
        if anchors.schematic.is_none() {
            anchors = net_anchors(b, &to);
            set_schematic_a(&mut anchors, net_anchors(a, &from));
        }
        out.push(Change {
            group: Group::Net,
            kind: Kind::Modified,
            impact: Impact::Electrical,
            title: format!("{d}.{p} moved from {from} to {to}"),
            detail: String::new(),
            anchors,
            side: Side::Both,
            ..Default::default()
        });
    }

    // --- pin-count changes for a net whose membership changed but wasn't FULLY
    // explained by the per-pin moves above (pins gained/lost to/from add/remove nets,
    // or ground rework). The move statement is strictly better, so we only emit this
    // when a net has added/removed pins that are NOT already covered by a move — else
    // a single U1.4 hop would spam both its from- and to-nets with a redundant row.
    for r in common {
        let ta = terminal_set_mapped(&na[r.as_str()], &rename);
        let tb = terminal_set(&nb[r.as_str()]);
        if ta == tb {
            continue;
        }
        // Count only pins whose change isn't already told elsewhere: a reported
        // per-pin move, or a pin that exists on one side only because its component
        // was added/removed (the component row carries that change).
        let comp_of = |t: &String| split_terminal(t).0.to_string();
        let added = tb
            .difference(&ta)
            .filter(|t| !moved_terms.contains(*t) && !comps.added.contains(&comp_of(t)))
            .count();
        let removed = ta
            .difference(&tb)
            .filter(|t| !moved_terms.contains(*t) && !comps.removed.contains(&comp_of(t)))
            .count();
        if added == 0 && removed == 0 {
            continue; // every delta pin is already reported by a move/component row
        }
        let mut bits = Vec::new();
        if added > 0 {
            bits.push(format!("+{added}"));
        }
        if removed > 0 {
            bits.push(format!("−{removed}"));
        }
        out.push(Change {
            group: Group::Net,
            kind: Kind::Modified,
            impact: Impact::Electrical,
            title: format!("net {r} membership changed ({})", bits.join(" ")),
            detail: format!("{} pin{} now", tb.len(), plural(tb.len())),
            anchors: {
                let mut anchors = net_anchors(b, r);
                set_schematic_a(&mut anchors, net_anchors(a, r));
                anchors
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

/// The terminal set of a net as sorted `designator.pin` strings.
fn terminal_set(n: &crate::design::NetLite) -> BTreeSet<String> {
    n.terminals.iter().map(|t| format!("{}\u{0}{}", t.d, t.p)).collect()
}

/// Like [`terminal_set`], with each designator canonicalized through `rename`
/// (A-side names → B-side names), so a re-annotated part keeps its identity.
fn terminal_set_mapped(
    n: &crate::design::NetLite,
    rename: &HashMap<&str, &str>,
) -> BTreeSet<String> {
    n.terminals
        .iter()
        .map(|t| {
            let d = rename.get(t.d.as_str()).copied().unwrap_or(t.d.as_str());
            format!("{}\u{0}{}", d, t.p)
        })
        .collect()
}

fn split_terminal(t: &str) -> (&str, &str) {
    t.split_once('\u{0}').unwrap_or((t, ""))
}

/// terminal ("D\0P") -> the (single) net owning it, restricted to `restrict` nets. A
/// terminal on several nets in this set is dropped (ambiguous — the membership-change
/// path reports it via pin-count instead).
fn terminal_owner_map<'a>(
    nets: &'a HashMap<String, crate::design::NetLite>,
    restrict: &HashSet<&String>,
    rename: &HashMap<&str, &str>,
) -> BTreeMap<String, String> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, net) in nets {
        if !restrict.contains(name) {
            continue;
        }
        for t in &net.terminals {
            let d = rename.get(t.d.as_str()).copied().unwrap_or(t.d.as_str());
            owners
                .entry(format!("{}\u{0}{}", d, t.p))
                .or_default()
                .push(name.clone());
        }
    }
    owners
        .into_iter()
        .filter_map(|(t, mut ns)| {
            ns.sort();
            ns.dedup();
            (ns.len() == 1).then(|| (t, ns.into_iter().next().unwrap()))
        })
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Schematic anchor for a net (from `by_sheet`): pick the lowest sheet number for a
/// deterministic landing sheet, carrying that sheet's uuids.
fn net_anchors(bundle: &Bundle, name: &str) -> Anchors {
    let mut anchors = Anchors::default();
    if let Some(n) = bundle.indexes.nets.get(name) {
        // by_sheet keys are sheet numbers as strings; sort numerically.
        let mut sheets: Vec<(i64, &Vec<String>)> = n
            .by_sheet
            .iter()
            .filter_map(|(k, v)| k.parse::<i64>().ok().map(|num| (num, v)))
            .collect();
        sheets.sort_by_key(|(num, _)| *num);
        if let Some((num, uuids)) = sheets.first() {
            let mut u = (*uuids).clone();
            u.sort();
            u.dedup();
            anchors.schematic = Some(SchematicAnchor { sheet: *num, uuids: u });
        }
    }
    // PCB anchor: the net name carries the landing (the renderer lands via netBBox).
    if let Some(g) = bundle.geometry.as_ref() {
        if g.nets.iter().any(|nn| nn == name) {
            anchors.pcb = Some(PcbAnchor { bbox: None, layers: Vec::new(), comp: None, net: Some(name.to_string()), vias: false });
        }
    }
    anchors
}

// ================================================================ sheet diff

fn diff_sheets(a: &Bundle, b: &Bundle, out: &mut Vec<Change>) {
    // Sheet identity is by name (the plan's schematic pairing reuses `sheetMatches`,
    // which is name-based). A sheet number is not stable across an insertion.
    let names_a: BTreeMap<&str, i64> = a.indexes.sheets.iter().map(|s| (s.name.as_str(), s.num)).collect();
    let names_b: BTreeMap<&str, i64> = b.indexes.sheets.iter().map(|s| (s.name.as_str(), s.num)).collect();

    for (name, num) in &names_b {
        if !names_a.contains_key(name) {
            out.push(Change {
                group: Group::Sheet,
                kind: Kind::Added,
                impact: Impact::Doc,
                title: format!("Sheet '{name}' added"),
                detail: String::new(),
                anchors: Anchors {
                    schematic: Some(SchematicAnchor { sheet: *num, uuids: Vec::new() }),
                    schematic_a: None,
                    pcb: None,
                    bom: None,
                },
                side: Side::B,
                ..Default::default()
            });
        }
    }
    for (name, num) in &names_a {
        if !names_b.contains_key(name) {
            out.push(Change {
                group: Group::Sheet,
                kind: Kind::Removed,
                impact: Impact::Doc,
                title: format!("Sheet '{name}' removed"),
                detail: String::new(),
                anchors: Anchors {
                    schematic: Some(SchematicAnchor { sheet: *num, uuids: Vec::new() }),
                    schematic_a: None,
                    pcb: None,
                    bom: None,
                },
                side: Side::A,
                ..Default::default()
            });
        }
    }
}

// ================================================================= doc diff

/// Title-block / frame field changes, read from the PCB geometry's `frame` (the board
/// title block is the doc source the geometry IR already carries).
fn diff_docs(a: &Bundle, b: &Bundle, out: &mut Vec<Change>) {
    let fa = a.geometry.as_ref().and_then(|g| g.frame.as_ref());
    let fb = b.geometry.as_ref().and_then(|g| g.frame.as_ref());
    let (Some(fa), Some(fb)) = (fa, fb) else {
        return;
    };
    let fields: [(&str, &str, &str); 5] = [
        ("Rev", fa.rev.as_str(), fb.rev.as_str()),
        ("Title", fa.title.as_str(), fb.title.as_str()),
        ("Company", fa.company.as_str(), fb.company.as_str()),
        ("Date", fa.date.as_str(), fb.date.as_str()),
        ("Paper", fa.paper.as_str(), fb.paper.as_str()),
    ];
    for (label, old, new) in fields {
        if old != new {
            out.push(Change {
                group: Group::Doc,
                kind: Kind::Modified,
                impact: Impact::Doc,
                title: format!("{label} field {} → {}", disp(old), disp(new)),
                detail: String::new(),
                anchors: Anchors::default(),
                side: Side::Both,
                ..Default::default()
            });
        }
    }
}

// =========================================================== placement diff

/// Component placement changes from the geometry IR (move / rotate / side flip).
///
/// Instances are paired by **footprint uuid** (the stable per-instance identity,
/// EPOCH ≥ 19): a shifted STITCH1 among 136 same-designator stitching footprints is
/// one accurate row, unchanged siblings none. Instances without a uuid on either
/// side (legacy caches extracted before the field existed, Altium) fall back to
/// designator pairing that is duplicate-safe: within a repeated designator, exact
/// positions consume each other first, then leftovers pair in sorted-position order
/// — never the old first-instance collapse that flooded one row per sibling.
fn diff_placement(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    // --- 1) uuid pairing (exact identity) ---
    let by_uuid_a: HashMap<&str, &GeomComp> = a
        .components
        .iter()
        .filter(|c| !c.uuid.is_empty())
        .map(|c| (c.uuid.as_str(), c))
        .collect();
    let mut paired_a: HashSet<&str> = HashSet::new(); // uuids consumed on A
    let mut pairs: Vec<(&GeomComp, &GeomComp)> = Vec::new();
    let mut rest_b: Vec<&GeomComp> = Vec::new(); // B instances with no uuid match
    for cb in &b.components {
        match (!cb.uuid.is_empty()).then(|| by_uuid_a.get(cb.uuid.as_str())).flatten() {
            Some(ca) => {
                paired_a.insert(ca.uuid.as_str());
                pairs.push((ca, cb));
            }
            None => rest_b.push(cb),
        }
    }
    let rest_a: Vec<&GeomComp> = a
        .components
        .iter()
        .filter(|c| c.uuid.is_empty() || !paired_a.contains(c.uuid.as_str()))
        .collect();

    // --- 2) designator fallback for the unpaired remainder (legacy / Altium) ---
    // Group each side's leftovers by designator; within a group, exact-position
    // matches consume first (an unmoved duplicate never pairs with a moved one),
    // then the leftovers zip in sorted order. Surplus instances on one side are
    // add/remove territory (the component diff's job), not placement rows.
    let mut ga: BTreeMap<&str, Vec<&GeomComp>> = BTreeMap::new();
    for c in rest_a {
        ga.entry(c.reference.as_str()).or_default().push(c);
    }
    let mut gb: BTreeMap<&str, Vec<&GeomComp>> = BTreeMap::new();
    for c in rest_b {
        gb.entry(c.reference.as_str()).or_default().push(c);
    }
    let pos_key = |c: &GeomComp| ((c.x * 1e4).round() as i64, (c.y * 1e4).round() as i64);
    for (refdes, mut va) in ga {
        let Some(vb) = gb.remove(refdes) else { continue };
        let mut vb: Vec<&GeomComp> = vb;
        va.sort_by(|x, y| pos_key(x).cmp(&pos_key(y)));
        vb.sort_by(|x, y| pos_key(x).cmp(&pos_key(y)));
        // Exact-position pairs first (still compared: angle/side may have changed).
        let mut used_b = vec![false; vb.len()];
        let mut left_a: Vec<&GeomComp> = Vec::new();
        for ca in va {
            let hit = (0..vb.len()).find(|&j| !used_b[j] && pos_key(vb[j]) == pos_key(ca));
            match hit {
                Some(j) => {
                    used_b[j] = true;
                    pairs.push((ca, vb[j]));
                }
                None => left_a.push(ca),
            }
        }
        // Zip the moved leftovers in sorted-position order.
        let left_b: Vec<&GeomComp> =
            vb.iter().zip(&used_b).filter(|(_, u)| !**u).map(|(c, _)| *c).collect();
        for (ca, cb) in left_a.into_iter().zip(left_b) {
            pairs.push((ca, cb));
        }
    }

    // --- 3) compare each pair and emit rows for real deltas ---
    pairs.sort_by(|(_, x), (_, y)| {
        (x.reference.as_str(), pos_key(x)).cmp(&(y.reference.as_str(), pos_key(y)))
    });
    for (ca, cb) in pairs {
        let dx = cb.x - ca.x;
        let dy = cb.y - ca.y;
        let dist = dx.hypot(dy);
        let dangle = angle_delta(ca.angle, cb.angle);
        let side_a = layer_name(a, ca.layer);
        let side_b = layer_name(b, cb.layer);
        let flipped = side_a != side_b;

        if dist < MOVE_EPS_MM && dangle.abs() < ANGLE_EPS_DEG && !flipped {
            continue;
        }
        let mut bits: Vec<String> = Vec::new();
        if dist >= MOVE_EPS_MM {
            bits.push(format!("moved {:.2} mm", dist));
        }
        if dangle.abs() >= ANGLE_EPS_DEG {
            bits.push(format!("rotated {:.0}°", dangle));
        }
        if flipped {
            bits.push(format!("to {}", side_short(&side_b)));
        }
        let layers = side_b.into_iter().collect();
        out.push(Change {
            group: Group::Placement,
            kind: Kind::Moved,
            impact: Impact::Placement,
            title: format!("{} {}", cb.reference, bits.join(", ")),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                schematic_a: None,
                pcb: Some(PcbAnchor {
                    bbox: cb.bbox,
                    layers,
                    comp: Some(cb.reference.clone()),
                    net: None,
                    vias: false,
                }),
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

/// Smallest signed angle delta in degrees, normalized to (-180, 180].
fn angle_delta(a: f64, b: f64) -> f64 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d <= -180.0 {
        d += 360.0;
    }
    d
}

fn layer_name(g: &Geometry, idx: i32) -> Option<String> {
    (idx >= 0).then(|| g.layers.get(idx as usize).map(|l| l.name.clone())).flatten()
}

fn side_short(layer: &Option<String>) -> String {
    match layer.as_deref() {
        Some("F.Cu") => "front".into(),
        Some("B.Cu") => "back".into(),
        Some(other) => other.to_string(),
        None => "?".into(),
    }
}

// ============================================================ routing diff

/// Copper track/via set-diff: identity = a content hash of the primitive tuple, so a
/// moved track reads as removed+added — correct for review — grouped so one user
/// action stays one row (plan §2). Tracks group per (layer, net) ("rerouted: +N −M
/// segments"); vias group per net across the WHOLE stack, because one via spans
/// several copper layers — the old per-layer keying turned one moved via into a
/// phantom "+1 −1 segments" row on every layer it crossed, and the overlay could hand
/// the via primitive to only one of them (the rest owned nothing and lit no copper).
fn diff_routing(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    // Per (layer-name, net-name) buckets of track hashes on each side. Layer/net
    // are resolved to names here so the two boards' index tables (which can differ)
    // compare by identity, not by raw index.
    let hashes_a = routing_hashes(a);
    let hashes_b = routing_hashes(b);

    let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
    keys.extend(hashes_a.keys().cloned());
    keys.extend(hashes_b.keys().cloned());

    for (layer, net) in keys {
        let empty = HashSet::new();
        let ha = hashes_a.get(&(layer.clone(), net.clone())).unwrap_or(&empty);
        let hb = hashes_b.get(&(layer.clone(), net.clone())).unwrap_or(&empty);
        let added = hb.difference(ha).count();
        let removed = ha.difference(hb).count();
        if added == 0 && removed == 0 {
            continue;
        }
        // Landing net (skip the empty/no-net sentinel for the anchor).
        let net_anchor = (!net.is_empty()).then(|| net.clone());
        let net_disp = if net.is_empty() { "(no net)".to_string() } else { net.clone() };
        let (kind, verb) = if added > 0 && removed > 0 {
            (Kind::Modified, "rerouted")
        } else if added > 0 {
            (Kind::Added, "routing added")
        } else {
            (Kind::Removed, "routing removed")
        };
        let mut bits = Vec::new();
        if added > 0 {
            bits.push(format!("+{added}"));
        }
        if removed > 0 {
            bits.push(format!("−{removed}"));
        }
        out.push(Change {
            group: Group::Routing,
            kind,
            impact: Impact::Electrical,
            title: format!("{net_disp} {verb} on {layer} ({} segments)", bits.join(" ")),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                schematic_a: None,
                pcb: Some(PcbAnchor {
                    bbox: None,
                    layers: vec![layer],
                    comp: None,
                    net: net_anchor,
                    vias: false,
                }),
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }

    diff_vias(a, b, out);
}

/// Via set-diff per net: one row per net whose via set changed, anchored to the union
/// of copper layers the changed vias span (so focusing the row reveals every layer the
/// via stitches). See `diff_routing` for why vias can't live in the per-layer rows.
fn diff_vias(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    let vias_a = via_hashes(a);
    let vias_b = via_hashes(b);

    let mut nets: BTreeSet<String> = BTreeSet::new();
    nets.extend(vias_a.keys().cloned());
    nets.extend(vias_b.keys().cloned());

    for net in nets {
        let empty = Vec::new();
        let va = vias_a.get(&net).unwrap_or(&empty);
        let vb = vias_b.get(&net).unwrap_or(&empty);
        let ha: HashSet<u64> = va.iter().map(|(h, _)| *h).collect();
        let hb: HashSet<u64> = vb.iter().map(|(h, _)| *h).collect();
        let added: Vec<_> = vb.iter().filter(|(h, _)| !ha.contains(h)).collect();
        let removed: Vec<_> = va.iter().filter(|(h, _)| !hb.contains(h)).collect();
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        // Layer anchor: the stack-order union of layers the CHANGED vias span, so the
        // row's focus isolation shows every affected layer, not just one.
        let changed_layers: HashSet<&str> = added
            .iter()
            .chain(&removed)
            .flat_map(|(_, ls)| ls.iter().map(String::as_str))
            .collect();
        let layers: Vec<String> = b
            .layers
            .iter()
            .chain(&a.layers)
            .map(|l| l.name.as_str())
            .filter(|n| changed_layers.contains(n))
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut acc, n| {
                if !acc.iter().any(|x: &String| x == n) {
                    acc.push(n.to_string());
                }
                acc
            });
        let net_anchor = (!net.is_empty()).then(|| net.clone());
        let net_disp = if net.is_empty() { "(no net)".to_string() } else { net.clone() };
        let noun = if added.len().max(removed.len()) == 1 { "via" } else { "vias" };
        let (kind, verb) = if !added.is_empty() && !removed.is_empty() {
            (Kind::Modified, "changed")
        } else if !added.is_empty() {
            (Kind::Added, "added")
        } else {
            (Kind::Removed, "removed")
        };
        let mut bits = Vec::new();
        if !added.is_empty() {
            bits.push(format!("+{}", added.len()));
        }
        if !removed.is_empty() {
            bits.push(format!("−{}", removed.len()));
        }
        out.push(Change {
            group: Group::Routing,
            kind,
            impact: Impact::Electrical,
            title: format!("{net_disp} {noun} {verb} ({})", bits.join(" ")),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                schematic_a: None,
                pcb: Some(PcbAnchor {
                    bbox: None,
                    layers,
                    comp: None,
                    net: net_anchor,
                    vias: true,
                }),
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

/// (layer-name, net-name) -> set of track content hashes (segments + arcs; vias are
/// `via_hashes`'s job). Layer & net indices are dereferenced to names for cross-board
/// identity. Coordinates are already rounded 1e-4 by the extractor, so a stable
/// string hash is exact.
fn routing_hashes(g: &Geometry) -> HashMap<(String, String), HashSet<u64>> {
    let mut m: HashMap<(String, String), HashSet<u64>> = HashMap::new();
    let ln = |i: u16| g.layers.get(i as usize).map(|l| l.name.clone()).unwrap_or_default();
    let nn = |i: u32| g.nets.get(i as usize).cloned().unwrap_or_default();

    // seg = [x1,y1,x2,y2] (stride 4), arc = [sx,sy,mx,my,ex,ey] (stride 6). Same column
    // shape, so one loop parameterized by (kind, stride) covers both.
    for (kind, col, stride) in [("seg", &g.tracks.seg, 4), ("arc", &g.tracks.arc, 6)] {
        for (i, w) in col.w.iter().enumerate() {
            let o = i * stride;
            if o + stride > col.xy.len() {
                break;
            }
            let layer = ln(*col.layer.get(i).unwrap_or(&0));
            let net = nn(*col.net.get(i).unwrap_or(&0));
            let coords = &col.xy[o..o + stride];
            let h = hash_prim(kind, &layer, &net, coords, *w);
            m.entry((layer, net)).or_default().insert(h);
        }
    }
    m
}

/// net-name -> each via's (content hash, spanned layer names). The hash folds the
/// whole layer span (a via re-stitched to different layers is a change) and mirrors
/// the overlay's via identity (`glDiff.buildKeys`).
fn via_hashes(g: &Geometry) -> HashMap<String, Vec<(u64, Vec<String>)>> {
    let mut m: HashMap<String, Vec<(u64, Vec<String>)>> = HashMap::new();
    let ln = |i: u16| g.layers.get(i as usize).map(|l| l.name.clone()).unwrap_or_default();
    let nn = |i: u32| g.nets.get(i as usize).cloned().unwrap_or_default();
    for v in &g.vias {
        let net = nn(v.net);
        let layers: Vec<String> = v.layers.iter().map(|&li| ln(li)).collect();
        let span = layers.join("+");
        let h = hash_prim("via", &span, &net, &[v.x, v.y, v.size], 0.0);
        m.entry(net).or_default().push((h, layers));
    }
    m
}

/// A stable, order-independent content hash of a primitive tuple. FNV-1a over a
/// canonical byte encoding — no HashMap randomization, so it's byte-deterministic.
fn hash_prim(kind: &str, layer: &str, net: &str, coords: &[f64], width: f64) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    mix(kind.as_bytes());
    mix(&[0]);
    mix(layer.as_bytes());
    mix(&[0]);
    mix(net.as_bytes());
    mix(&[0]);
    for c in coords {
        // Coords are pre-rounded to 1e-4; encode as a fixed-point integer so -0.0/0.0
        // and float bit noise can't split identical geometry.
        let q = (c * 1e4).round() as i64;
        mix(&q.to_le_bytes());
    }
    let wq = (width * 1e4).round() as i64;
    mix(&wq.to_le_bytes());
    h
}

// ================================================================ zone diff

/// Zone (copper pour) diff per (layer, net), on two signals both gated by the same
/// ~1 mm² noise floor:
///   • area delta — a pour added / removed / grown / shrunk.
///   • shape      — a pour that kept its area but RE-FLOWED, e.g. around a re-routed
///     track: the copper notch moves, so the total area barely changes yet the pour is
///     genuinely different. Area-only comparison is blind to this (the bug this fixes).
/// Comparing fill polygons vertex-by-vertex would trip on refill jitter, so the shape
/// signal measures the actual differing area (A △ B) and holds it to the same floor.
fn diff_zones(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    let polys_a = zone_polys(a);
    let polys_b = zone_polys(b);
    let area = |m: &HashMap<(String, String), Vec<&[f64]>>, k: &(String, String)| {
        m.get(k).map(|ps| ps.iter().map(|p| ring_area(p)).sum::<f64>()).unwrap_or(0.0)
    };

    let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
    keys.extend(polys_a.keys().cloned());
    keys.extend(polys_b.keys().cloned());

    for key in keys {
        let (layer, net) = key.clone();
        let aa = area(&polys_a, &key);
        let ab = area(&polys_b, &key);
        let delta = ab - aa;
        let net_disp = if net.is_empty() { "(no net)".to_string() } else { net.clone() };

        let (kind, title) = if delta.abs() >= ZONE_AREA_EPS_MM2 {
            // Area moved enough on its own — the classic add/remove/grow/shrink row.
            let verb = if aa == 0.0 {
                "added on"
            } else if ab == 0.0 {
                "removed from"
            } else if delta > 0.0 {
                "grew on"
            } else {
                "shrank on"
            };
            let kind = if aa == 0.0 {
                Kind::Added
            } else if ab == 0.0 {
                Kind::Removed
            } else {
                Kind::Modified
            };
            (kind, format!("{net_disp} pour {verb} {layer} ({:+.0} mm²)", delta))
        } else {
            // Area held steady — did the pour re-flow? Measure the moved copper. Both
            // sides present here (a pour that only exists on one side has |delta| ≥ its
            // whole area, caught above unless it's sub-threshold tiny — then symdiff is
            // that same tiny area and stays below the floor too).
            let moved = zone_symdiff_area(
                polys_a.get(&key).map(Vec::as_slice).unwrap_or(&[]),
                polys_b.get(&key).map(Vec::as_slice).unwrap_or(&[]),
            );
            if moved < ZONE_AREA_EPS_MM2 {
                continue;
            }
            (Kind::Modified, format!("{net_disp} pour reshaped on {layer} (~{:.0} mm²)", moved))
        };

        out.push(Change {
            group: Group::Zone,
            kind,
            impact: Impact::Placement,
            title,
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                schematic_a: None,
                pcb: Some(PcbAnchor {
                    bbox: None,
                    layers: vec![layer],
                    comp: None,
                    net: (!net.is_empty()).then(|| net.clone()),
                    vias: false,
                }),
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

/// (layer-name, net-name) -> the filled fill polygons (each a flat `[x,y,…]` ring),
/// borrowed from the geometry. Unfilled/keepout zones and rings with fewer than three
/// points carry no measurable copper and are dropped.
fn zone_polys(g: &Geometry) -> HashMap<(String, String), Vec<&[f64]>> {
    let mut m: HashMap<(String, String), Vec<&[f64]>> = HashMap::new();
    for z in &g.zones {
        if !z.filled || z.pts.len() < 6 {
            continue;
        }
        let layer = g.layers.get(z.layer as usize).map(|l| l.name.clone()).unwrap_or_default();
        let net = g.nets.get(z.net as usize).cloned().unwrap_or_default();
        m.entry((layer, net)).or_default().push(z.pts.as_slice());
    }
    m
}

/// Estimated area (mm²) of the symmetric difference of two fill-polygon sets on one
/// (layer, net) — the copper filled on exactly one side. Byte-identical polygons appear
/// on both sides and cancel out of A △ B (fill islands never overlap within a net), so
/// they're dropped first: a localized re-flow then only measures its own neighbourhood,
/// and whole-board refill jitter (near-identical rings) nets close to zero. The
/// remainder is integrated over horizontal scanlines — exact in x (each side's filled
/// spans come from even-odd pairing of its edge crossings), sampled every
/// `ZONE_SCAN_STEP_MM` in y.
fn zone_symdiff_area(pa: &[&[f64]], pb: &[&[f64]]) -> f64 {
    // Drop polygons present identically on both sides (multiset-aware).
    let mut bcount: HashMap<u64, usize> = HashMap::new();
    for p in pb {
        *bcount.entry(poly_hash(p)).or_insert(0) += 1;
    }
    let mut ua: Vec<&[f64]> = Vec::new();
    for &p in pa {
        match bcount.get_mut(&poly_hash(p)) {
            Some(c) if *c > 0 => *c -= 1, // cancels an identical B polygon
            _ => ua.push(p),
        }
    }
    let mut ub: Vec<&[f64]> = Vec::new();
    for &p in pb {
        if let Some(c) = bcount.get_mut(&poly_hash(p)) {
            if *c > 0 {
                *c -= 1;
                ub.push(p);
            }
        }
    }
    if ua.is_empty() && ub.is_empty() {
        return 0.0;
    }

    let (mut ymin, mut ymax) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in ua.iter().chain(ub.iter()) {
        let mut i = 1;
        while i < p.len() {
            ymin = ymin.min(p[i]);
            ymax = ymax.max(p[i]);
            i += 2;
        }
    }
    if ymax <= ymin {
        return 0.0; // degenerate (zero-height) — no area to sweep
    }

    let step = ZONE_SCAN_STEP_MM;
    let mut area = 0.0;
    let mut y = ymin + step * 0.5;
    while y < ymax {
        area += span_symdiff_len(&scan_spans(&ua, y), &scan_spans(&ub, y)) * step;
        y += step;
    }
    area
}

/// FNV-1a over a fill ring's fixed-point (1e-4 mm) coordinates — the same rounding the
/// extractor and overlay use, so "identical polygon" means the same thing everywhere.
fn poly_hash(pts: &[f64]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for c in pts {
        let q = (c * 1e4).round() as i64;
        for &byte in &q.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// The sorted, pairwise-disjoint x-intervals filled by `polys` at height `y`. Every
/// polygon is a simple ring and islands are mutually disjoint, so even-odd pairing of
/// all edge crossings yields the filled spans directly. Edges are counted half-open in y
/// (`(y1 > y) != (y2 > y)`) so a scanline grazing a shared vertex isn't double-counted.
fn scan_spans(polys: &[&[f64]], y: f64) -> Vec<(f64, f64)> {
    let mut xs: Vec<f64> = Vec::new();
    for p in polys {
        let n = p.len() / 2;
        for i in 0..n {
            let j = (i + 1) % n;
            let (x1, y1) = (p[2 * i], p[2 * i + 1]);
            let (x2, y2) = (p[2 * j], p[2 * j + 1]);
            if (y1 > y) != (y2 > y) {
                xs.push(x1 + (x2 - x1) * (y - y1) / (y2 - y1));
            }
        }
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut spans = Vec::with_capacity(xs.len() / 2);
    let mut i = 0;
    while i + 1 < xs.len() {
        spans.push((xs[i], xs[i + 1]));
        i += 2;
    }
    spans
}

/// Total length covered by exactly one of two span lists — the 1-D symmetric difference.
fn span_symdiff_len(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
    // Sweep interval endpoints; accumulate length while exactly one side is "inside".
    let mut ev: Vec<(f64, i8)> = Vec::with_capacity((a.len() + b.len()) * 2);
    for &(s, e) in a {
        ev.push((s, 1));
        ev.push((e, -1));
    }
    for &(s, e) in b {
        ev.push((s, 2));
        ev.push((e, -2));
    }
    ev.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let (mut da, mut db) = (0i32, 0i32);
    let mut last = 0.0;
    let mut total = 0.0;
    let mut started = false;
    for (x, d) in ev {
        if started && (da > 0) != (db > 0) {
            total += x - last;
        }
        if d.abs() == 1 {
            da += d.signum() as i32;
        } else {
            db += d.signum() as i32;
        }
        last = x;
        started = true;
    }
    total
}

/// Shoelace area of a flat `[x,y,…]` ring (absolute value, mm²).
fn ring_area(pts: &[f64]) -> f64 {
    let n = pts.len() / 2;
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        let (xi, yi) = (pts[2 * i], pts[2 * i + 1]);
        let (xj, yj) = (pts[2 * j], pts[2 * j + 1]);
        sum += xi * yj - xj * yi;
    }
    (sum / 2.0).abs()
}

// ==================================================== silk / text / outline diff

/// Graphics + texts set-diff on non-copper layers. Silk/text/outline groups all come
/// from here: a graphic on Edge.Cuts is an outline change; text is a silk/text change;
/// other graphics are silk. (plan §2)
fn diff_graphics_and_text(a: &Geometry, b: &Geometry, comp_delta: &CompDelta, out: &mut Vec<Change>) {
    // Layer-name → manifest role, merged over both sides, for impact classing of
    // rows built after their layer index is gone (paired texts, graphics set-diff).
    let mut roles: HashMap<String, String> = HashMap::new();
    for l in a.layers.iter().chain(b.layers.iter()) {
        roles.entry(l.name.clone()).or_insert_with(|| l.role.clone());
    }
    let impact_of =
        |layer: &str| layer_impact(layer, roles.get(layer).map(String::as_str).unwrap_or(""));

    // Refdes a component-level row (placement move / add / remove / rename) already
    // explains. Its footprint's art and text moved WITH it — the overlay hands those
    // primitives to that row, and one user action must read as ONE row — so they sit
    // out of the loose-graphics/text diff below. Without this, every footprint move
    // spawned phantom "changed on F.CrtYd / F.Fab / silk" and "text moved" rows that
    // owned nothing on the board (feedback: clicking them showed no change).
    let mut explained: HashSet<String> = out
        .iter()
        .filter(|c| matches!(c.group, Group::Placement | Group::Component))
        .filter_map(|c| c.anchors.pcb.as_ref().and_then(|p| p.comp.clone()))
        .collect();
    // A re-annotation's row anchors the NEW refdes; the A side's art still carries the
    // old one — cover it too, or the old refdes text surfaces as a phantom "removed".
    explained.extend(comp_delta.renamed.iter().map(|(old, _)| old.clone()));
    let comp_explained = |g: &Geometry, idx: Option<i64>| -> bool {
        idx.and_then(|i| usize::try_from(i).ok())
            .and_then(|i| g.components.get(i))
            .is_some_and(|c| explained.contains(&c.reference))
    };

    // --- text diff. Three-stage pairing on indices so ONE authoring action reads as
    // ONE row instead of an add+remove pair:
    //   0. identical texts (layer + position + string + style) are unchanged — masked;
    //   1. leftovers at the SAME spot pair as an in-place edit: a changed string
    //      ("'REV A' → 'REV B'") or, string intact, a restyle (size/pen/font/bold);
    //   2. leftovers with the SAME string on the same layer pair as a move
    //      (nearest candidate first), anchored on a box covering BOTH positions so
    //      each side's render frames its own copy;
    //   3. whatever is still unpaired really is an add / remove.
    let pos_of = |t: &GeomText| ((t.x * 1e4).round() as i64, (t.y * 1e4).round() as i64);
    let full_key = |g: &Geometry, t: &GeomText| (layer_of(g, t.layer), pos_of(t), t.text.clone(), style_key(t));

    // Stage 0: texts identical on both sides are unchanged — mask them.
    let b_full: HashSet<_> = b.texts.iter().map(|t| full_key(b, t)).collect();
    let a_full: HashSet<_> = a.texts.iter().map(|t| full_key(a, t)).collect();
    let mut a_left: Vec<usize> = (0..a.texts.len())
        .filter(|&i| !comp_explained(a, a.texts[i].comp))
        .filter(|&i| !b_full.contains(&full_key(a, &a.texts[i])))
        .collect();
    let mut b_left: Vec<usize> = (0..b.texts.len())
        .filter(|&j| !comp_explained(b, b.texts[j].comp))
        .filter(|&j| !a_full.contains(&full_key(b, &b.texts[j])))
        .collect();

    // Stage 1: pair by position — a string edit or a restyle at the same spot.
    let pos_key = |g: &Geometry, t: &GeomText| (layer_of(g, t.layer), pos_of(t));
    let mut a_by_pos: HashMap<(String, (i64, i64)), Vec<usize>> = HashMap::new();
    for &i in &a_left {
        a_by_pos.entry(pos_key(a, &a.texts[i])).or_default().push(i);
    }
    let mut consumed_a: HashSet<usize> = HashSet::new();
    let mut consumed_b: HashSet<usize> = HashSet::new();
    // (title, detail, layer, anchor bbox [x, y, w, h])
    let mut paired: Vec<(String, String, String, [f64; 4])> = Vec::new();
    for &j in &b_left {
        let bt = &b.texts[j];
        if let Some(cands) = a_by_pos.get(&pos_key(b, bt)) {
            if let Some(&i) = cands.iter().find(|&&i| !consumed_a.contains(&i)) {
                consumed_a.insert(i);
                consumed_b.insert(j);
                let at = &a.texts[i];
                let layer = layer_of(b, bt.layer);
                let (title, detail) = if at.text != bt.text {
                    (format!("Text '{}' → '{}'", at.text, bt.text), format!("on {layer}"))
                } else {
                    (format!("Text '{}' restyled on {layer}", bt.text), style_delta(at, bt))
                };
                paired.push((title, detail, layer, point_box(bt.x, bt.y)));
            }
        }
    }
    a_left.retain(|i| !consumed_a.contains(i));
    b_left.retain(|j| !consumed_b.contains(j));

    // Stage 2: pair by string — the same text at a new spot is a move, not add+remove.
    // Nearest surviving candidate wins so several same-string labels pair sensibly.
    let mut a_by_text: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for &i in &a_left {
        let at = &a.texts[i];
        a_by_text.entry((layer_of(a, at.layer), at.text.clone())).or_default().push(i);
    }
    let mut moves: Vec<(String, String, String, [f64; 4])> = Vec::new();
    for &j in &b_left {
        let bt = &b.texts[j];
        let layer = layer_of(b, bt.layer);
        let Some(cands) = a_by_text.get(&(layer.clone(), bt.text.clone())) else { continue };
        let nearest = cands
            .iter()
            .filter(|&&i| !consumed_a.contains(&i))
            .min_by(|&&p, &&q| {
                let d = |i: usize| (a.texts[i].x - bt.x).hypot(a.texts[i].y - bt.y);
                d(p).total_cmp(&d(q)).then(p.cmp(&q))
            });
        let Some(&i) = nearest else { continue };
        consumed_a.insert(i);
        consumed_b.insert(j);
        let at = &a.texts[i];
        let dist = (at.x - bt.x).hypot(at.y - bt.y);
        let mut detail = format!("by {dist:.2} mm");
        let restyle = style_delta(at, bt);
        if !restyle.is_empty() {
            detail = format!("{detail} · {restyle}");
        }
        // Anchor covers both the old and the new position so either side's render
        // shows its copy of the text when the change is focused.
        let (x0, y0) = (at.x.min(bt.x), at.y.min(bt.y));
        let (x1, y1) = (at.x.max(bt.x), at.y.max(bt.y));
        let bbox = [x0 - 5.0, y0 - 5.0, (x1 - x0) + 10.0, (y1 - y0) + 10.0];
        moves.push((format!("Text '{}' moved on {layer}", bt.text), detail, layer, bbox));
    }
    a_left.retain(|i| !consumed_a.contains(i));
    b_left.retain(|j| !consumed_b.contains(j));

    let by_title = |x: &(String, String, String, [f64; 4]), y: &(String, String, String, [f64; 4])| {
        (x.2.as_str(), x.0.as_str(), x.1.as_str()).cmp(&(y.2.as_str(), y.0.as_str(), y.1.as_str()))
    };
    paired.sort_by(by_title);
    moves.sort_by(by_title);
    for (kind, rows) in [(Kind::Modified, paired), (Kind::Moved, moves)] {
        for (title, detail, layer, bbox) in rows {
            out.push(Change {
                group: Group::Text,
                kind,
                impact: impact_of(&layer),
                title,
                detail,
                anchors: Anchors {
                    schematic: None,
                    schematic_a: None,
                    pcb: Some(PcbAnchor { bbox: Some(bbox), layers: vec![layer], comp: None, net: None, vias: false }),
                    bom: None,
                },
                side: Side::Both,
                ..Default::default()
            });
        }
    }
    // Stage 3: remaining unpaired texts → add / remove.
    push_text_addremove(out, a, &a_left, Kind::Removed, Side::A);
    push_text_addremove(out, b, &b_left, Kind::Added, Side::B);

    // --- graphics set-diff, split into outline (Edge.Cuts) vs silk (everything else) ---
    let ga = graphic_hashes(a, &comp_explained);
    let gb = graphic_hashes(b, &comp_explained);
    let mut layers: BTreeSet<String> = BTreeSet::new();
    layers.extend(ga.keys().cloned());
    layers.extend(gb.keys().cloned());
    for layer in layers {
        let empty = HashSet::new();
        let ha = ga.get(&layer).unwrap_or(&empty);
        let hb = gb.get(&layer).unwrap_or(&empty);
        let added = hb.difference(ha).count();
        let removed = ha.difference(hb).count();
        if added == 0 && removed == 0 {
            continue;
        }
        let is_edge = layer == "Edge.Cuts";
        let group = if is_edge { Group::Outline } else { Group::Silk };
        let impact = impact_of(&layer);
        let mut bits = Vec::new();
        if added > 0 {
            bits.push(format!("+{added}"));
        }
        if removed > 0 {
            bits.push(format!("−{removed}"));
        }
        let title = if is_edge {
            format!("Board outline changed on {layer} ({})", bits.join(" "))
        } else {
            // Name the layer's function — "Silk" is only right for silkscreen
            // (feedback: a courtyard/fab change is not "silk").
            let noun = layer_noun(&layer, roles.get(&layer).map(String::as_str).unwrap_or(""));
            format!("{noun} changed on {layer} ({})", bits.join(" "))
        };
        out.push(Change {
            group,
            kind: Kind::Modified,
            impact,
            title,
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                schematic_a: None,
                pcb: Some(PcbAnchor { bbox: None, layers: vec![layer], comp: None, net: None, vias: false }),
                bom: None,
            },
            side: Side::Both,
            ..Default::default()
        });
    }
}

/// Resolve a layer index to its name (empty when out of range).
fn layer_of(g: &Geometry, idx: u16) -> String {
    g.layers.get(idx as usize).map(|l| l.name.clone()).unwrap_or_default()
}

/// Impact class for a change on a non-copper layer. Silkscreen, solder mask, paste
/// and the board outline are manufactured features — a change there alters the
/// physical board (assembly marking, soldering, fit), so it is never merely
/// cosmetic. Fab / courtyard / user-drawing layers stay Cosmetic. Matches by
/// manifest role, with a KiCad layer-name fallback for bundles missing roles.
fn layer_impact(name: &str, role: &str) -> Impact {
    let fab_role = matches!(role, "silkscreen" | "mask" | "paste" | "edge");
    let fab_name = name == "Edge.Cuts"
        || name.ends_with(".SilkS")
        || name.ends_with(".Silkscreen")
        || name.ends_with(".Mask")
        || name.ends_with(".Paste");
    if fab_role || fab_name {
        Impact::Placement
    } else {
        Impact::Cosmetic
    }
}

/// Resolve a layer index to its manifest role (empty when out of range).
fn role_of(g: &Geometry, idx: u16) -> String {
    g.layers.get(idx as usize).map(|l| l.role.clone()).unwrap_or_default()
}

/// Human noun for a loose-graphics change on a non-copper layer, by manifest role
/// with a KiCad layer-name fallback. "Silk" is only right for silkscreen — a
/// courtyard or fab-drawing change must say what it is.
fn layer_noun(name: &str, role: &str) -> &'static str {
    match role {
        "silkscreen" => "Silkscreen",
        "mask" => "Solder mask",
        "paste" => "Paste",
        "courtyard" => "Courtyard",
        "fab" => "Fab drawing",
        _ => {
            if name.ends_with(".SilkS") || name.ends_with(".Silkscreen") {
                "Silkscreen"
            } else if name.ends_with(".Mask") {
                "Solder mask"
            } else if name.ends_with(".Paste") {
                "Paste"
            } else if name.ends_with(".CrtYd") || name.ends_with(".Courtyard") {
                "Courtyard"
            } else if name.ends_with(".Fab") {
                "Fab drawing"
            } else {
                "Drawing"
            }
        }
    }
}

/// Emit one text add/remove change per leftover text index, in a deterministic order
/// (by layer, then the text string).
fn push_text_addremove(out: &mut Vec<Change>, g: &Geometry, idxs: &[usize], kind: Kind, side: Side) {
    let mut items: Vec<(String, String, String, [f64; 2])> = idxs
        .iter()
        .map(|&i| {
            let t = &g.texts[i];
            (layer_of(g, t.layer), role_of(g, t.layer), t.text.clone(), [t.x, t.y])
        })
        .collect();
    items.sort_by(|a, b| (a.0.as_str(), a.2.as_str()).cmp(&(b.0.as_str(), b.2.as_str())));
    let verb = match kind {
        Kind::Added => "added",
        Kind::Removed => "removed",
        _ => "changed",
    };
    for (layer, role, text, at) in items {
        out.push(Change {
            group: Group::Text,
            kind,
            impact: layer_impact(&layer, &role),
            title: format!("Text '{text}' {verb} on {layer}"),
            detail: String::new(),
            anchors: pcb_point_anchor(&layer, at),
            side,
            ..Default::default()
        });
    }
}

/// A text's comparable style, floats rounded to 1 µm so re-serialization noise never
/// reads as a restyle. Absent options are the extractor defaults on both sides, so
/// `None == None` correctly means "same".
fn style_key(t: &GeomText) -> (Option<i64>, Option<i64>, Option<i64>, bool, bool, Option<String>) {
    let um = |v: Option<f64>| v.map(|v| (v * 1e3).round() as i64);
    (um(t.size), um(t.width), um(t.thickness), t.bold, t.italic, t.font.clone())
}

/// Human-readable summary of what changed between two same-string texts' styles
/// (empty when the styles match).
fn style_delta(a: &GeomText, b: &GeomText) -> String {
    let mut bits = Vec::new();
    let mm = |v: Option<f64>| v.map(|v| format!("{v} mm")).unwrap_or_else(|| "default".into());
    if style_key(a).0 != style_key(b).0 {
        bits.push(format!("size {} → {}", mm(a.size), mm(b.size)));
    }
    if style_key(a).1 != style_key(b).1 {
        bits.push(format!("width {} → {}", mm(a.width), mm(b.width)));
    }
    if style_key(a).2 != style_key(b).2 {
        bits.push(format!("thickness {} → {}", mm(a.thickness), mm(b.thickness)));
    }
    if a.bold != b.bold {
        bits.push(format!("bold {}", if b.bold { "on" } else { "off" }));
    }
    if a.italic != b.italic {
        bits.push(format!("italic {}", if b.italic { "on" } else { "off" }));
    }
    if a.font != b.font {
        let name = |f: &Option<String>| f.clone().unwrap_or_else(|| "KiCad stroke".into());
        bits.push(format!("font {} → {}", name(&a.font), name(&b.font)));
    }
    bits.join(", ")
}

/// The small camera-landing box `pcb_point_anchor` uses, as a raw `[x, y, w, h]`.
fn point_box(x: f64, y: f64) -> [f64; 4] {
    [x - 5.0, y - 5.0, 10.0, 10.0]
}

fn pcb_point_anchor(layer: &str, at: [f64; 2]) -> Anchors {
    Anchors {
        schematic: None,
        schematic_a: None,
        pcb: Some(PcbAnchor {
            // A small landing box around the text/point.
            bbox: Some([at[0] - 5.0, at[1] - 5.0, 10.0, 10.0]),
            layers: vec![layer.to_string()],
            comp: None,
            net: None,
            vias: false,
        }),
        bom: None,
    }
}

/// layer-name -> set of graphic content hashes. `comp_explained` filters out
/// footprint art whose component already has its own change row (see
/// diff_graphics_and_text) — that art moved with the footprint, not on its own.
fn graphic_hashes(
    g: &Geometry,
    comp_explained: &dyn Fn(&Geometry, Option<i64>) -> bool,
) -> HashMap<String, HashSet<u64>> {
    let mut m: HashMap<String, HashSet<u64>> = HashMap::new();
    for gr in &g.graphics {
        if comp_explained(g, gr.comp) {
            continue;
        }
        let layer = g.layers.get(gr.layer as usize).map(|l| l.name.clone()).unwrap_or_default();
        // Skip copper — routing already covers it; graphics on copper are rare (dimensions).
        let is_copper = g.layers.get(gr.layer as usize).map(|l| l.role == "copper").unwrap_or(false);
        if is_copper {
            continue;
        }
        let h = hash_prim(&gr.kind, &layer, "", &gr.data, gr.width);
        m.entry(layer).or_default().insert(h);
    }
    m
}

// ================================================================ small helpers

fn disp(s: &str) -> &str {
    if s.is_empty() {
        "∅"
    } else {
        s
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

// ============================================================ machine-local cache
//
// The diff document is regenerable (a pure function of two cache bundles), so it
// lives in the machine-local tier under `project::local_data_root`, NEVER the synced
// project folder — same rule as `cache::cache_root`. Keyed by the blake3 of the two
// input cache keys so an equal pair reuses one file and it self-invalidates when
// either side re-extracts.

use std::path::{Path, PathBuf};

/// Root of the machine-local diff cache.
pub fn diff_cache_root(project_dir: &Path) -> PathBuf {
    crate::project::local_data_root(project_dir).join("diffs")
}

/// Cache key for a `(cache_key_a, cache_key_b)` pair — blake3 of the two, order
/// preserved (A is base, B is target; a swap is a different diff).
pub fn diff_key(cache_key_a: &str, cache_key_b: &str) -> String {
    // Engine revision: bump whenever the changeset a given bundle pair produces
    // changes shape (new rows, anchors, fields) — the key only hashes the inputs,
    // so without this a cached diff.json would keep serving the old shape.
    // 5: manufactured-layer impact reclass, footprint art/text folds into its
    //    component row, layer-function nouns ("Courtyard changed", not "Silk").
    // 6: via changes get one per-net row (`vias` anchor marker) instead of folding
    //    into every spanned layer's "segments" row.
    // 7: zones also diff on shape — a pour that re-flowed at constant area (e.g. around
    //    a re-routed track) now emits a "reshaped" row the area-only test missed.
    // 8: component property edits (Package, Manufacturer, Tolerance, …) get their own rows.
    // 9: pin electrical-type edits get their own row, and the schematic element signature
    //    folds in the library body (pin text sizes, pin geometry) so a lib-side restyle
    //    no longer reads as "no change".
    // 10: BOM changeset (group "bom" rows + BomAnchor) derived from the component
    //     changeset (phase 3).
    // 11: BOM rows carry the line's symbol-property edits (MSL, Automotive Grade, …) on
    //     the anchor, and a line whose members only edited such a property gets its own
    //     BOM row instead of showing up in the component pass alone.
    const DIFF_ENGINE_VERSION: &str = "11";
    let mut h = blake3::Hasher::new();
    h.update(DIFF_ENGINE_VERSION.as_bytes());
    h.update(b" ");
    h.update(cache_key_a.as_bytes());
    h.update(b"\0");
    h.update(cache_key_b.as_bytes());
    h.finalize().to_hex().as_str()[..16].to_string()
}

/// Absolute path of a cached `diff.json` for the given pair key.
pub fn diff_cache_path(project_dir: &Path, key: &str) -> PathBuf {
    diff_cache_root(project_dir).join(format!("{key}.json"))
}

/// Bound the diff cache: keep `{keep}.json` plus the newest `max_entries` files, delete
/// the rest. `keep` protects the doc just written and handed to the frontend, so a
/// concurrent prepare_diff's gc (>`max_entries` distinct pairs compared) can't evict the
/// file the caller is about to serve. Regenerable, so eviction is otherwise safe.
/// Best-effort; never errors. Mirrors `cache::gc` (which keeps the same protection for
/// bundle dirs).
pub fn gc(project_dir: &Path, keep: &str, max_entries: usize) {
    let root = diff_cache_root(project_dir);
    let Ok(rd) = std::fs::read_dir(&root) else {
        return;
    };
    let keep_file = format!("{keep}.json");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        // Only published `.json` docs are cache entries; leave a concurrent writer's
        // short-lived `.json.tmp` alone (neither count it nor delete it out from under
        // the rename).
        if !e.file_name().to_string_lossy().ends_with(".json") {
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        files.push((mtime, e.path()));
    }
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (i, (_, path)) in files.iter().enumerate() {
        let is_keep = path.file_name().map(|n| n == keep_file.as_str()).unwrap_or(false);
        if is_keep || i < max_entries {
            continue;
        }
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests;
