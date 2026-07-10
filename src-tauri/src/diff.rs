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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pcb: Option<PcbAnchor>,
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
    #[serde(default)]
    pub seg: GeomSegCol,
    #[serde(default)]
    pub arc: GeomArcCol,
}

#[derive(Deserialize, Default, Clone)]
pub struct GeomSegCol {
    #[serde(default)]
    pub xy: Vec<f64>,
    #[serde(default)]
    pub w: Vec<f64>,
    #[serde(default)]
    pub layer: Vec<u16>,
    #[serde(default)]
    pub net: Vec<u32>,
}

#[derive(Deserialize, Default, Clone)]
pub struct GeomArcCol {
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
}

#[derive(Deserialize, Clone)]
pub struct GeomText {
    pub layer: u16,
    pub text: String,
    pub x: f64,
    pub y: f64,
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
    /// The `.kicad_pcb` source file name, for PCB-pass pruning.
    pub pcb_file: Option<String>,
}

// ============================================================ tuning constants

/// Position tolerance (mm) below which two component placements count as "same
/// position" — used by re-annotation rename folding.
const POS_EPS_MM: f64 = 0.001;

/// A component move under this distance (mm) isn't worth a placement change row.
const MOVE_EPS_MM: f64 = 0.05;

/// An angle delta under this (deg) isn't a rotation.
const ANGLE_EPS_DEG: f64 = 0.01;

/// Net-rename fold threshold: terminal-set Jaccard similarity at/above this folds a
/// remove+add of two differently-named nets into one rename (plan §2, "~0.7").
const NET_RENAME_JACCARD: f64 = 0.7;

/// Zone area deltas below this (mm²) are refill noise, not a change (plan §2, "~1 mm²").
const ZONE_AREA_EPS_MM2: f64 = 1.0;

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
    diff_components(a, b, &mut raw);
    diff_nets(a, b, &mut raw);
    diff_sheets(a, b, &mut raw);
    diff_docs(a, b, &mut raw);

    // --- PCB geometry groups, unless the whole board is source-identical ---
    if pcb_pass_needed(a, b, &changed_sources) {
        if let (Some(ga), Some(gb)) = (a.geometry.as_ref(), b.geometry.as_ref()) {
            diff_placement(ga, gb, &mut raw);
            diff_routing(ga, gb, &mut raw);
            diff_zones(ga, gb, &mut raw);
            diff_graphics_and_text(ga, gb, &mut raw);
        }
    }

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
            match c.impact {
                Impact::Electrical => stats.electrical += 1,
                Impact::Placement => stats.placement += 1,
                Impact::Cosmetic => stats.cosmetic += 1,
                Impact::Doc => stats.doc += 1,
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
        if same_on_b && !changed_sources.contains(file.as_str()) {
            pruned.push(*num);
        }
    }
    pruned
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

// ============================================================ component diff

/// Component field comparison + re-annotation rename folding (plan §2).
fn diff_components(a: &Bundle, b: &Bundle, out: &mut Vec<Change>) {
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
                && placements_match(a, b, ra, rb)
            {
                best = Some(*rb);
                break;
            }
        }
        if let Some(rb) = best {
            consumed_b.insert(rb);
            folded_a.insert(*ra);
            let anchors = comp_anchors(b, rb);
            out.push(Change {
                id: String::new(),
                group: Group::Component,
                kind: Kind::Renamed,
                impact: Impact::Cosmetic,
                title: format!("{ra} re-annotated → {rb}"),
                detail: format!("same value {} / footprint {}", disp(&comp_a.value), disp(&comp_a.fp)),
                anchors,
                side: Side::Both,
            });
        }
    }

    // Genuine adds/removes (those not folded into a rename).
    for rb in &only_b {
        if consumed_b.contains(rb) {
            continue;
        }
        let c = &cb[*rb];
        out.push(Change {
            id: String::new(),
            group: Group::Component,
            kind: Kind::Added,
            impact: Impact::Electrical,
            title: format!("{rb} added"),
            detail: comp_summary(c),
            anchors: comp_anchors(b, rb),
            side: Side::B,
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
        let c = &ca[*ra];
        out.push(Change {
            id: String::new(),
            group: Group::Component,
            kind: Kind::Removed,
            impact: Impact::Electrical,
            title: format!("{ra} removed"),
            detail: comp_summary(c),
            anchors: comp_anchors(a, ra),
            side: Side::A,
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
        let dnp_flip = x.dnp != y.dnp;
        if fields.is_empty() && !dnp_flip {
            continue;
        }
        // The headline uses the first (most electrical) field; the rest go to detail.
        let (title, detail, impact);
        if let Some((label, old, new)) = fields.first().copied() {
            title = format!("{r} {label} {} → {}", disp(old), disp(new));
            let mut extra: Vec<String> = fields
                .iter()
                .skip(1)
                .map(|(l, o, n)| format!("{l} {} → {}", disp(o), disp(n)))
                .collect();
            if dnp_flip {
                extra.push(if y.dnp { "marked DNP".into() } else { "un-marked DNP".into() });
            }
            detail = extra.join("; ");
            // value/footprint/mpn are electrical-ish; footprint is placement-relevant
            // but treated electrical here (it changes the land pattern / part).
            impact = Impact::Electrical;
        } else {
            // DNP-only flip.
            title = format!("{r} {}", if y.dnp { "marked DNP" } else { "un-marked DNP" });
            detail = String::new();
            impact = Impact::Electrical;
        }
        out.push(Change {
            id: String::new(),
            group: Group::Component,
            kind: Kind::Modified,
            impact,
            title,
            detail,
            anchors: comp_anchors(b, r),
            side: Side::Both,
        });
    }
}

/// Two refdes occupy ~the same board position (for re-annotation folding). If neither
/// side has PCB geometry, position is unknown → treat as matching (value+fp equality
/// alone then carries the fold, which is the schematic-only case).
fn placements_match(a: &Bundle, b: &Bundle, ra: &str, rb: &str) -> bool {
    let pa = comp_pos(a, ra);
    let pb = comp_pos(b, rb);
    match (pa, pb) {
        (Some((xa, ya)), Some((xb, yb))) => (xa - xb).abs() < POS_EPS_MM && (ya - yb).abs() < POS_EPS_MM,
        _ => true,
    }
}

fn comp_pos(bundle: &Bundle, refdes: &str) -> Option<(f64, f64)> {
    let g = bundle.geometry.as_ref()?;
    g.components.iter().find(|c| c.reference == refdes).map(|c| (c.x, c.y))
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
            });
        }
    }
    anchors
}

// ================================================================= net diff

/// Net membership diff with name-rename folding (Jaccard on the terminal set) and
/// per-pin membership-move changes (plan §2).
fn diff_nets(a: &Bundle, b: &Bundle, out: &mut Vec<Change>) {
    let na = &a.indexes.nets;
    let nb = &b.indexes.nets;

    let mut only_a: Vec<&String> = na.keys().filter(|k| !nb.contains_key(*k)).collect();
    let mut only_b: Vec<&String> = nb.keys().filter(|k| !na.contains_key(*k)).collect();
    only_a.sort();
    only_b.sort();

    // --- rename folding: an unmatched name on A + one on B with terminal-set Jaccard
    // >= threshold is one rename. Score every candidate pair; take the highest, then
    // the next, greedily but deterministically (sorted candidate order breaks ties).
    let mut candidates: Vec<(String, &String, &String)> = Vec::new(); // (jac-key, a, b)
    for ra in &only_a {
        let ta = terminal_set(&na[*ra]);
        for rb in &only_b {
            let tb = terminal_set(&nb[*rb]);
            let j = jaccard(&ta, &tb);
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
        let ta = terminal_set(&na[*ra]);
        let n_pins = ta.len();
        out.push(Change {
            id: String::new(),
            group: Group::Net,
            kind: Kind::Renamed,
            impact: Impact::Electrical,
            title: format!("net {ra} → {rb}"),
            detail: format!("same {n_pins} pin{}", plural(n_pins)),
            anchors: net_anchors(b, rb),
            side: Side::Both,
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
            id: String::new(),
            group: Group::Net,
            kind: Kind::Added,
            impact: Impact::Electrical,
            title: format!("net {rb} added"),
            detail: format!("{cnt} pin{}", plural(cnt)),
            anchors: net_anchors(b, rb),
            side: Side::B,
        });
    }
    for ra in &only_a {
        if consumed_a.contains(ra) {
            continue;
        }
        let n = &na[*ra];
        let cnt = n.terminals.len();
        out.push(Change {
            id: String::new(),
            group: Group::Net,
            kind: Kind::Removed,
            impact: Impact::Electrical,
            title: format!("net {ra} removed"),
            detail: format!("{cnt} pin{}", plural(cnt)),
            anchors: net_anchors(a, ra),
            side: Side::A,
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
    let a_of = terminal_owner_map(na, &common_set);
    let b_of = terminal_owner_map(nb, &common_set);

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
    for (term, from, to) in moved {
        let (d, p) = split_terminal(&term);
        out.push(Change {
            id: String::new(),
            group: Group::Net,
            kind: Kind::Modified,
            impact: Impact::Electrical,
            title: format!("{d}.{p} moved from {from} to {to}"),
            detail: String::new(),
            anchors: net_anchors(b, &to),
            side: Side::Both,
        });
    }

    // --- pin-count changes for a net whose membership changed but wasn't FULLY
    // explained by the per-pin moves above (pins gained/lost to/from add/remove nets,
    // or ground rework). The move statement is strictly better, so we only emit this
    // when a net has added/removed pins that are NOT already covered by a move — else
    // a single U1.4 hop would spam both its from- and to-nets with a redundant row.
    for r in common {
        let ta = terminal_set(&na[r.as_str()]);
        let tb = terminal_set(&nb[r.as_str()]);
        if ta == tb {
            continue;
        }
        // Count only pins whose change isn't a move already reported.
        let added = tb.difference(&ta).filter(|t| !moved_terms.contains(*t)).count();
        let removed = ta.difference(&tb).filter(|t| !moved_terms.contains(*t)).count();
        if added == 0 && removed == 0 {
            continue; // every delta pin is a reported move — the move row is enough
        }
        let mut bits = Vec::new();
        if added > 0 {
            bits.push(format!("+{added}"));
        }
        if removed > 0 {
            bits.push(format!("−{removed}"));
        }
        out.push(Change {
            id: String::new(),
            group: Group::Net,
            kind: Kind::Modified,
            impact: Impact::Electrical,
            title: format!("net {r} membership changed ({})", bits.join(" ")),
            detail: format!("{} pin{} now", tb.len(), plural(tb.len())),
            anchors: net_anchors(b, r),
            side: Side::Both,
        });
    }
}

/// The terminal set of a net as sorted `designator.pin` strings.
fn terminal_set(n: &crate::design::NetLite) -> BTreeSet<String> {
    n.terminals.iter().map(|t| format!("{}\u{0}{}", t.d, t.p)).collect()
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
) -> BTreeMap<String, String> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, net) in nets {
        if !restrict.contains(name) {
            continue;
        }
        for t in &net.terminals {
            owners
                .entry(format!("{}\u{0}{}", t.d, t.p))
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
            anchors.pcb = Some(PcbAnchor { bbox: None, layers: Vec::new(), comp: None, net: Some(name.to_string()) });
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
                id: String::new(),
                group: Group::Sheet,
                kind: Kind::Added,
                impact: Impact::Doc,
                title: format!("Sheet '{name}' added"),
                detail: String::new(),
                anchors: Anchors {
                    schematic: Some(SchematicAnchor { sheet: *num, uuids: Vec::new() }),
                    pcb: None,
                },
                side: Side::B,
            });
        }
    }
    for (name, num) in &names_a {
        if !names_b.contains_key(name) {
            out.push(Change {
                id: String::new(),
                group: Group::Sheet,
                kind: Kind::Removed,
                impact: Impact::Doc,
                title: format!("Sheet '{name}' removed"),
                detail: String::new(),
                anchors: Anchors {
                    schematic: Some(SchematicAnchor { sheet: *num, uuids: Vec::new() }),
                    pcb: None,
                },
                side: Side::A,
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
                id: String::new(),
                group: Group::Doc,
                kind: Kind::Modified,
                impact: Impact::Doc,
                title: format!("{label} field {} → {}", disp(old), disp(new)),
                detail: String::new(),
                anchors: Anchors::default(),
                side: Side::Both,
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
            id: String::new(),
            group: Group::Placement,
            kind: Kind::Moved,
            impact: Impact::Placement,
            title: format!("{} {}", cb.reference, bits.join(", ")),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                pcb: Some(PcbAnchor {
                    bbox: cb.bbox,
                    layers,
                    comp: Some(cb.reference.clone()),
                    net: None,
                }),
            },
            side: Side::Both,
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

/// Copper track/via set-diff, grouped per (layer, net): identity = a content hash of
/// the primitive tuple, so a moved track reads as removed+added — correct for review —
/// but the per-(layer,net) grouping keeps it one row ("rerouted: +N −M"). (plan §2)
fn diff_routing(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    // Per (layer-name, net-name) buckets of primitive hashes on each side. Layer/net
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
            id: String::new(),
            group: Group::Routing,
            kind,
            impact: Impact::Electrical,
            title: format!("{net_disp} {verb} on {layer} ({} segments)", bits.join(" ")),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                pcb: Some(PcbAnchor {
                    bbox: None,
                    layers: vec![layer],
                    comp: None,
                    net: net_anchor,
                }),
            },
            side: Side::Both,
        });
    }
}

/// (layer-name, net-name) -> set of primitive content hashes (tracks + vias). Layer &
/// net indices are dereferenced to names for cross-board identity. Coordinates are
/// already rounded 1e-4 by the extractor, so a stable string hash is exact.
fn routing_hashes(g: &Geometry) -> HashMap<(String, String), HashSet<u64>> {
    let mut m: HashMap<(String, String), HashSet<u64>> = HashMap::new();
    let ln = |i: u16| g.layers.get(i as usize).map(|l| l.name.clone()).unwrap_or_default();
    let nn = |i: u32| g.nets.get(i as usize).cloned().unwrap_or_default();

    // straight segments: [x1,y1,x2,y2] per seg.
    for (i, w) in g.tracks.seg.w.iter().enumerate() {
        let o = i * 4;
        if o + 4 > g.tracks.seg.xy.len() {
            break;
        }
        let layer = ln(*g.tracks.seg.layer.get(i).unwrap_or(&0));
        let net = nn(*g.tracks.seg.net.get(i).unwrap_or(&0));
        let coords = &g.tracks.seg.xy[o..o + 4];
        let h = hash_prim("seg", &layer, &net, coords, *w);
        m.entry((layer, net)).or_default().insert(h);
    }
    // arcs: [sx,sy,mx,my,ex,ey] per arc.
    for (i, w) in g.tracks.arc.w.iter().enumerate() {
        let o = i * 6;
        if o + 6 > g.tracks.arc.xy.len() {
            break;
        }
        let layer = ln(*g.tracks.arc.layer.get(i).unwrap_or(&0));
        let net = nn(*g.tracks.arc.net.get(i).unwrap_or(&0));
        let coords = &g.tracks.arc.xy[o..o + 6];
        let h = hash_prim("arc", &layer, &net, coords, *w);
        m.entry((layer, net)).or_default().insert(h);
    }
    // vias: one row per copper layer spanned (keyed under each layer so a via that
    // appears/moves shows on every affected layer's group).
    for v in &g.vias {
        let net = nn(v.net);
        for &li in &v.layers {
            let layer = ln(li);
            let h = hash_prim("via", &layer, &net, &[v.x, v.y, v.size], 0.0);
            m.entry((layer, net.clone())).or_default().insert(h);
        }
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

/// Zone (copper pour) diff per (layer, net): filled-area delta with a noise threshold.
/// Comparing fill polygons vertex-by-vertex is refill jitter, so we compare total area.
fn diff_zones(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    let area_a = zone_areas(a);
    let area_b = zone_areas(b);

    let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
    keys.extend(area_a.keys().cloned());
    keys.extend(area_b.keys().cloned());

    for (layer, net) in keys {
        let aa = area_a.get(&(layer.clone(), net.clone())).copied().unwrap_or(0.0);
        let ab = area_b.get(&(layer.clone(), net.clone())).copied().unwrap_or(0.0);
        let delta = ab - aa;
        if delta.abs() < ZONE_AREA_EPS_MM2 {
            continue;
        }
        let net_disp = if net.is_empty() { "(no net)".to_string() } else { net.clone() };
        let (kind, verb) = if aa == 0.0 {
            (Kind::Added, "added on")
        } else if ab == 0.0 {
            (Kind::Removed, "removed from")
        } else if delta > 0.0 {
            (Kind::Modified, "grew on")
        } else {
            (Kind::Modified, "shrank on")
        };
        out.push(Change {
            id: String::new(),
            group: Group::Zone,
            kind,
            impact: Impact::Placement,
            title: format!("{net_disp} pour {verb} {layer} ({:+.0} mm²)", delta),
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                pcb: Some(PcbAnchor {
                    bbox: None,
                    layers: vec![layer],
                    comp: None,
                    net: (!net.is_empty()).then(|| net.clone()),
                }),
            },
            side: Side::Both,
        });
    }
}

/// (layer-name, net-name) -> total filled area (mm²), summed over filled zones.
fn zone_areas(g: &Geometry) -> HashMap<(String, String), f64> {
    let mut m: HashMap<(String, String), f64> = HashMap::new();
    for z in &g.zones {
        if !z.filled {
            continue;
        }
        let layer = g.layers.get(z.layer as usize).map(|l| l.name.clone()).unwrap_or_default();
        let net = g.nets.get(z.net as usize).cloned().unwrap_or_default();
        *m.entry((layer, net)).or_insert(0.0) += ring_area(&z.pts);
    }
    m
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
fn diff_graphics_and_text(a: &Geometry, b: &Geometry, out: &mut Vec<Change>) {
    // --- text diff. We work on indices so an in-place edit (same layer+position,
    // changed string) folds to one "modified" instead of add+remove, and the
    // still-unpaired texts on each side become add/remove. Identity for pairing is
    // (layer, rounded x, rounded y); identity for "unchanged" additionally includes
    // the string, so an untouched text is dropped on both sides.
    let pos_key = |g: &Geometry, t: &GeomText| (t.layer, (t.x * 1e4).round() as i64, (t.y * 1e4).round() as i64, layer_of(g, t.layer));
    let full_key = |_g: &Geometry, t: &GeomText| (t.layer, (t.x * 1e4).round() as i64, (t.y * 1e4).round() as i64, t.text.clone());

    // Texts identical on both sides (same position AND string) are unchanged — mask them.
    let b_full: HashSet<(u16, i64, i64, String)> = b.texts.iter().map(|t| full_key(b, t)).collect();
    let a_full: HashSet<(u16, i64, i64, String)> = a.texts.iter().map(|t| full_key(a, t)).collect();
    let mut a_left: Vec<usize> = (0..a.texts.len()).filter(|&i| !b_full.contains(&full_key(a, &a.texts[i]))).collect();
    let mut b_left: Vec<usize> = (0..b.texts.len()).filter(|&j| !a_full.contains(&full_key(b, &b.texts[j]))).collect();

    // Pair by position (an edit): a leftover A and leftover B at the same spot = a modify.
    let mut a_by_pos: HashMap<(u16, i64, i64, String), Vec<usize>> = HashMap::new();
    for &i in &a_left {
        a_by_pos.entry(pos_key(a, &a.texts[i])).or_default().push(i);
    }
    let mut consumed_a: HashSet<usize> = HashSet::new();
    let mut consumed_b: HashSet<usize> = HashSet::new();
    let mut edits: Vec<(String, String, String, [f64; 2])> = Vec::new(); // (old, new, layer, at)
    for &j in &b_left {
        let bt = &b.texts[j];
        if let Some(cands) = a_by_pos.get(&pos_key(b, bt)) {
            if let Some(&i) = cands.iter().find(|&&i| !consumed_a.contains(&i)) {
                consumed_a.insert(i);
                consumed_b.insert(j);
                let layer = layer_of(b, bt.layer);
                edits.push((a.texts[i].text.clone(), bt.text.clone(), layer, [bt.x, bt.y]));
            }
        }
    }
    a_left.retain(|i| !consumed_a.contains(i));
    b_left.retain(|j| !consumed_b.contains(j));

    edits.sort_by(|x, y| (x.2.as_str(), x.0.as_str(), x.1.as_str()).cmp(&(y.2.as_str(), y.0.as_str(), y.1.as_str())));
    for (old, new, layer, at) in edits {
        out.push(Change {
            id: String::new(),
            group: Group::Text,
            kind: Kind::Modified,
            impact: Impact::Cosmetic,
            title: format!("Text '{}' → '{}'", old, new),
            detail: format!("on {layer}"),
            anchors: pcb_point_anchor(&layer, at),
            side: Side::Both,
        });
    }
    // Remaining unpaired texts → add / remove.
    push_text_addremove(out, a, &a_left, Kind::Removed, Side::A);
    push_text_addremove(out, b, &b_left, Kind::Added, Side::B);

    // --- graphics set-diff, split into outline (Edge.Cuts) vs silk (everything else) ---
    let ga = graphic_hashes(a);
    let gb = graphic_hashes(b);
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
        let impact = if is_edge { Impact::Placement } else { Impact::Cosmetic };
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
            format!("Silk changed on {layer} ({})", bits.join(" "))
        };
        out.push(Change {
            id: String::new(),
            group,
            kind: Kind::Modified,
            impact,
            title,
            detail: String::new(),
            anchors: Anchors {
                schematic: None,
                pcb: Some(PcbAnchor { bbox: None, layers: vec![layer], comp: None, net: None }),
            },
            side: Side::Both,
        });
    }
}

/// Resolve a layer index to its name (empty when out of range).
fn layer_of(g: &Geometry, idx: u16) -> String {
    g.layers.get(idx as usize).map(|l| l.name.clone()).unwrap_or_default()
}

/// Emit one text add/remove change per leftover text index, in a deterministic order
/// (by layer, then the text string).
fn push_text_addremove(out: &mut Vec<Change>, g: &Geometry, idxs: &[usize], kind: Kind, side: Side) {
    let mut items: Vec<(String, String, [f64; 2])> = idxs
        .iter()
        .map(|&i| {
            let t = &g.texts[i];
            (layer_of(g, t.layer), t.text.clone(), [t.x, t.y])
        })
        .collect();
    items.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
    let verb = match kind {
        Kind::Added => "added",
        Kind::Removed => "removed",
        _ => "changed",
    };
    for (layer, text, at) in items {
        out.push(Change {
            id: String::new(),
            group: Group::Text,
            kind,
            impact: Impact::Cosmetic,
            title: format!("Text '{text}' {verb} on {layer}"),
            detail: String::new(),
            anchors: pcb_point_anchor(&layer, at),
            side,
        });
    }
}

fn pcb_point_anchor(layer: &str, at: [f64; 2]) -> Anchors {
    Anchors {
        schematic: None,
        pcb: Some(PcbAnchor {
            // A small landing box around the text/point.
            bbox: Some([at[0] - 5.0, at[1] - 5.0, 10.0, 10.0]),
            layers: vec![layer.to_string()],
            comp: None,
            net: None,
        }),
    }
}

/// layer-name -> set of graphic content hashes.
fn graphic_hashes(g: &Geometry) -> HashMap<String, HashSet<u64>> {
    let mut m: HashMap<String, HashSet<u64>> = HashMap::new();
    for gr in &g.graphics {
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
    let mut h = blake3::Hasher::new();
    h.update(cache_key_a.as_bytes());
    h.update(b"\0");
    h.update(cache_key_b.as_bytes());
    h.finalize().to_hex().as_str()[..16].to_string()
}

/// Absolute path of a cached `diff.json` for the given pair key.
pub fn diff_cache_path(project_dir: &Path, key: &str) -> PathBuf {
    diff_cache_root(project_dir).join(format!("{key}.json"))
}

/// Bound the diff cache: keep the newest `max_entries` files, delete the rest.
/// Regenerable, so eviction is always safe. Best-effort; never errors. Mirrors
/// `cache::gc`.
pub fn gc(project_dir: &Path, max_entries: usize) {
    let root = diff_cache_root(project_dir);
    let Ok(rd) = std::fs::read_dir(&root) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
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
        if i >= max_entries {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests;
