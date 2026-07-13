//! Fixture-based unit tests for the diff engine. Each test builds two small
//! `Bundle`s by hand (design indexes + optional geometry) and asserts on the change
//! rows produced. Determinism (byte-identical serialization across runs) and empty
//! diff on identical bundles are covered explicitly, per the plan's test list.

use super::*;
use crate::design::{CompLite, DesignIndexes, NetLite, SheetLite, TerminalLite};
use crate::rawstore::RevisionDiff;
use std::collections::HashMap;

// -------------------------------------------------------------- fixture builders

fn empty_indexes() -> DesignIndexes {
    DesignIndexes {
        sheets: Vec::new(),
        layers: Vec::new(),
        svg_to_net: HashMap::new(),
        svg_to_nets: HashMap::new(),
        svg_to_comp: HashMap::new(),
        elem_kind: HashMap::new(),
        nets: HashMap::new(),
        components: HashMap::new(),
        theme: serde_json::Value::Null,
        pcb_geometry: None,
    }
}

fn comp(value: &str, fp: &str, dnp: bool) -> CompLite {
    CompLite {
        value: value.into(),
        mpn: String::new(),
        mfr: String::new(),
        fp: fp.into(),
        desc: String::new(),
        sheet: Some(1),
        dnp,
        nets: Vec::new(),
        svg_id: String::new(),
        bbox: None,
    }
}

fn comp_full(value: &str, fp: &str, mpn: &str, sheet: i64, svg_id: &str) -> CompLite {
    CompLite {
        value: value.into(),
        mpn: mpn.into(),
        mfr: String::new(),
        fp: fp.into(),
        desc: String::new(),
        sheet: Some(sheet),
        dnp: false,
        nets: Vec::new(),
        svg_id: svg_id.into(),
        bbox: None,
    }
}

fn term(d: &str, p: &str) -> TerminalLite {
    TerminalLite { d: d.into(), p: p.into(), pn: String::new(), pt: String::new() }
}

fn net(terms: Vec<TerminalLite>) -> NetLite {
    NetLite {
        class: "Default".into(),
        terminals: terms,
        sheets: vec![1],
        by_sheet: HashMap::new(),
    }
}

fn sheet(num: i64, name: &str) -> SheetLite {
    SheetLite { num, name: name.into(), svg: None }
}

fn bundle(indexes: DesignIndexes) -> Bundle {
    Bundle {
        rev: "r_a".into(),
        label: "A".into(),
        indexes,
        sheet_files: HashMap::new(),
        geometry: None,
        sch_geometry: None,
        pcb_file: None,
    }
}

/// A schematic-geometry element `(uuid, kind, [x,y,w,h], sig)`.
fn sch_elem(uuid: &str, kind: &str, bbox: [f64; 4], sig: &str) -> SchElem {
    SchElem { uuid: uuid.into(), kind: kind.into(), bbox, sig: sig.into() }
}

/// A one-file schematic geometry with the given elements.
fn sch_geom(file: &str, elems: Vec<SchElem>) -> SchGeometry {
    SchGeometry { sheets: vec![SchSheetGeom { file: file.into(), elements: elems }] }
}

fn no_source_diff() -> RevisionDiff {
    RevisionDiff::default()
}

/// A RevisionDiff whose `changed` names one file — forces the PCB pass / marks sheets
/// as touched.
fn changed(files: &[&str]) -> RevisionDiff {
    RevisionDiff {
        added: Vec::new(),
        removed: Vec::new(),
        changed: files.iter().map(|s| s.to_string()).collect(),
    }
}

// ------------------------------------------------------------------- the tests

#[test]
fn value_change_is_one_electrical_modify() {
    let mut a = empty_indexes();
    a.components.insert("C14".into(), comp("100n", "C_0402", false));
    let mut b = empty_indexes();
    b.components.insert("C14".into(), comp("1u", "C_0402", false));
    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let comp_changes: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Component).collect();
    assert_eq!(comp_changes.len(), 1, "one component change");
    let c = comp_changes[0];
    assert_eq!(c.kind, Kind::Modified);
    assert_eq!(c.impact, Impact::Electrical);
    assert!(c.title.contains("C14"), "title names the part: {}", c.title);
    assert!(c.title.contains("100n") && c.title.contains("1u"), "title shows both values: {}", c.title);
    assert_eq!(doc.stats.electrical, 1);
}

#[test]
fn component_add_and_remove() {
    let mut a = empty_indexes();
    a.components.insert("R1".into(), comp("10k", "R_0402", false));
    let mut b = empty_indexes();
    b.components.insert("R1".into(), comp("10k", "R_0402", false));
    b.components.insert("R7".into(), comp("4k7", "R_0402", false)); // added
    // Remove a part that only existed on A.
    a.components.insert("D3".into(), comp("LED", "LED_0603", false));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert!(doc.changes.iter().any(|c| c.title == "R7 added" && c.kind == Kind::Added));
    assert!(doc.changes.iter().any(|c| c.title == "D3 removed" && c.kind == Kind::Removed));
}

#[test]
fn reannotation_folds_to_one_rename() {
    // R12 removed + R15 added with identical value/footprint (schematic-only, no
    // geometry so position is "match") folds into a single rename, not add+remove.
    let mut a = empty_indexes();
    a.components.insert("R12".into(), comp("10k", "R_0402", false));
    let mut b = empty_indexes();
    b.components.insert("R15".into(), comp("10k", "R_0402", false));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let comp_changes: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Component).collect();
    assert_eq!(comp_changes.len(), 1, "exactly one change (the rename), got {comp_changes:?}");
    assert_eq!(comp_changes[0].kind, Kind::Renamed);
    assert!(comp_changes[0].title.contains("R12") && comp_changes[0].title.contains("R15"));
    // A rename is cosmetic, not electrical (nothing about the circuit changed).
    assert_eq!(comp_changes[0].impact, Impact::Cosmetic);
}

#[test]
fn reannotation_fold_keeps_second_identical_removal() {
    // Two identical parts removed, ONE added with the same value/footprint
    // (schematic-only, so position always "matches"): exactly one removal may fold
    // into the rename — the other must still be reported as removed, not silently
    // dropped because it also satisfies the fold predicate against the consumed
    // B-side candidate.
    let mut a = empty_indexes();
    a.components.insert("R1".into(), comp("10k", "R_0402", false));
    a.components.insert("R2".into(), comp("10k", "R_0402", false));
    let mut b = empty_indexes();
    b.components.insert("R9".into(), comp("10k", "R_0402", false));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let renames: Vec<_> = doc.changes.iter().filter(|c| c.kind == Kind::Renamed).collect();
    let removals: Vec<_> = doc.changes.iter().filter(|c| c.kind == Kind::Removed).collect();
    assert_eq!(renames.len(), 1, "one rename, got {renames:?}");
    assert_eq!(removals.len(), 1, "the unfolded twin is still a removal, got {removals:?}");
    assert!(removals[0].title.contains("R2"), "R1 folds (sorted greedy), R2 remains: {}", removals[0].title);
}

#[test]
fn net_rename_folds_via_jaccard() {
    // /SW1 on A and /PHASE_A on B share the same 5 terminals → one rename.
    let terms = || vec![term("U1", "1"), term("U1", "2"), term("R3", "1"), term("R3", "2"), term("C9", "1")];
    let mut a = empty_indexes();
    a.nets.insert("/SW1".into(), net(terms()));
    let mut b = empty_indexes();
    b.nets.insert("/PHASE_A".into(), net(terms()));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let net_changes: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Net).collect();
    assert_eq!(net_changes.len(), 1, "one net change, got {net_changes:?}");
    assert_eq!(net_changes[0].kind, Kind::Renamed);
    assert!(net_changes[0].title.contains("/SW1") && net_changes[0].title.contains("/PHASE_A"));
}

#[test]
fn low_jaccard_stays_add_remove() {
    // Two nets sharing only 1 of many pins are NOT a rename.
    let mut a = empty_indexes();
    a.nets.insert("/AAA".into(), net(vec![term("U1", "1"), term("U1", "2"), term("U1", "3"), term("U1", "4")]));
    let mut b = empty_indexes();
    b.nets.insert("/BBB".into(), net(vec![term("U1", "1"), term("Q9", "5"), term("Q9", "6"), term("Q9", "7")]));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert!(doc.changes.iter().any(|c| c.title == "net /AAA removed"));
    assert!(doc.changes.iter().any(|c| c.title == "net /BBB added"));
    assert!(!doc.changes.iter().any(|c| c.kind == Kind::Renamed && c.group == Group::Net));
}

#[test]
fn pin_membership_move_between_two_nets() {
    // U1.4 leaves /SDA and joins /SCL; both nets exist on both sides.
    let mut a = empty_indexes();
    a.nets.insert("/SDA".into(), net(vec![term("U1", "3"), term("U1", "4")]));
    a.nets.insert("/SCL".into(), net(vec![term("U1", "5")]));
    let mut b = empty_indexes();
    b.nets.insert("/SDA".into(), net(vec![term("U1", "3")]));
    b.nets.insert("/SCL".into(), net(vec![term("U1", "5"), term("U1", "4")]));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let net_changes: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Net).collect();
    // Exactly one net change: the move. No redundant "membership changed" rows on
    // either the from- or to-net, since the move fully explains both deltas.
    assert_eq!(net_changes.len(), 1, "just the move, no redundant rows: {net_changes:?}");
    let m = net_changes[0];
    assert!(m.title.contains("U1.4") && m.title.contains("moved"), "{}", m.title);
    assert!(m.title.contains("/SDA") && m.title.contains("/SCL"), "names both nets: {}", m.title);
    assert_eq!(m.impact, Impact::Electrical);
}

#[test]
fn pin_move_anchors_to_component_not_net() {
    // C1.2 leaves /GND and joins /P5V. C1 is a placed part (sheet 3, svg_id "u_c1"), so
    // the move must anchor to the COMPONENT's symbol (batch 3): clicking focuses the pin
    // on C1's home sheet, not the /GND or /P5V net geometry (which would land on whatever
    // sheet those rails bottom out on, often the root page).
    let mut a = empty_indexes();
    a.components.insert("C1".into(), comp_full("100n", "C_0402", "", 3, "u_c1"));
    a.nets.insert("/GND".into(), net(vec![term("C1", "1"), term("C1", "2")]));
    a.nets.insert("/P5V".into(), net(vec![term("R1", "1")]));
    let mut b = empty_indexes();
    b.components.insert("C1".into(), comp_full("100n", "C_0402", "", 3, "u_c1"));
    b.nets.insert("/GND".into(), net(vec![term("C1", "1")]));
    b.nets.insert("/P5V".into(), net(vec![term("R1", "1"), term("C1", "2")]));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let m = doc
        .changes
        .iter()
        .find(|c| c.title.contains("C1.2") && c.title.contains("moved"))
        .expect("pin-move change");
    let sch = m.anchors.schematic.as_ref().expect("schematic anchor");
    assert_eq!(sch.sheet, 3, "lands on C1's home sheet, not the net's lowest sheet");
    assert_eq!(sch.uuids, vec!["u_c1".to_string()], "anchors to the component symbol");
}

// -------------------------------------------------- geometry-backed fixtures

fn geom_with_comp(refdes: &str, x: f64, y: f64, angle: f64, layer: i32) -> Geometry {
    Geometry {
        layers: vec![
            GeomLayer { name: "F.Cu".into(), role: "copper".into() },
            GeomLayer { name: "B.Cu".into(), role: "copper".into() },
        ],
        nets: vec![String::new()],
        components: vec![GeomComp {
            reference: refdes.into(),
            layer,
            x,
            y,
            angle,
            bbox: Some([x - 1.0, y - 1.0, 2.0, 2.0]),
            uuid: String::new(),
        }],
        ..Default::default()
    }
}

/// A geometry with `n` same-designator footprints at 1 mm x-steps (stitching-via
/// style), each with uuid `"u<i>"` (or empty when `uuids` is false).
fn geom_with_dups(refdes: &str, n: usize, uuids: bool, moved: Option<(usize, f64)>) -> Geometry {
    let mut g = Geometry {
        layers: vec![GeomLayer { name: "F.Cu".into(), role: "copper".into() }],
        nets: vec![String::new()],
        ..Default::default()
    };
    for i in 0..n {
        let mut x = 10.0 + i as f64;
        if let Some((mi, dx)) = moved {
            if mi == i {
                x += dx;
            }
        }
        g.components.push(GeomComp {
            reference: refdes.into(),
            layer: 0,
            x,
            y: 5.0,
            angle: 0.0,
            bbox: Some([x - 0.5, 4.5, 1.0, 1.0]),
            uuid: if uuids { format!("u{i}") } else { String::new() },
        });
    }
    g
}

#[test]
fn placement_uuid_pairs_repeated_designators() {
    // 136-style repeated designator: 6 STITCH1 footprints, one shifted 2 mm. UUID
    // pairing must yield exactly ONE placement row (the shifted instance), not a
    // flood of one row per sibling (the old designator-map collapse).
    let mut a = bundle(empty_indexes());
    a.geometry = Some(geom_with_dups("STITCH1", 6, true, None));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(geom_with_dups("STITCH1", 6, true, Some((3, 2.0))));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let rows: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Placement).collect();
    assert_eq!(rows.len(), 1, "one accurate row, no designator collapse: {rows:?}");
    assert!(rows[0].title.contains("STITCH1") && rows[0].title.contains("2.00"), "{}", rows[0].title);
}

#[test]
fn placement_legacy_fallback_no_flood() {
    // Same board but extracted before the uuid field existed (empty uuids): the
    // per-designator positional fallback still reports exactly one moved instance —
    // exact-position siblings consume each other, the moved pair zips leftover-to-
    // leftover.
    let mut a = bundle(empty_indexes());
    a.geometry = Some(geom_with_dups("STITCH1", 6, false, None));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(geom_with_dups("STITCH1", 6, false, Some((3, 2.0))));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let rows: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Placement).collect();
    assert_eq!(rows.len(), 1, "positional fallback must not flood: {rows:?}");
    assert!(rows[0].title.contains("moved"), "{}", rows[0].title);
}

#[test]
fn placement_uuid_survives_reannotation() {
    // The SAME footprint instance (same uuid) renamed R12 → R15 and moved: uuid
    // pairing still produces one placement row (under the new name), even though
    // the designators differ on the two sides.
    let mut ga = geom_with_comp("R12", 10.0, 10.0, 0.0, 0);
    ga.components[0].uuid = "fixed-uuid".into();
    let mut gb = geom_with_comp("R15", 14.0, 10.0, 0.0, 0);
    gb.components[0].uuid = "fixed-uuid".into();
    let mut a = bundle(empty_indexes());
    a.geometry = Some(ga);
    let mut b = bundle(empty_indexes());
    b.geometry = Some(gb);

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let rows: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Placement).collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].title.contains("R15") && rows[0].title.contains("4.00"), "{}", rows[0].title);
}

#[test]
fn placement_move_and_rotate() {
    let mut a = bundle(empty_indexes());
    a.geometry = Some(geom_with_comp("R7", 10.0, 10.0, 0.0, 0));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(geom_with_comp("R7", 13.2, 10.0, 90.0, 0));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let p = doc
        .changes
        .iter()
        .find(|c| c.group == Group::Placement)
        .expect("a placement change");
    assert_eq!(p.kind, Kind::Moved);
    assert_eq!(p.impact, Impact::Placement);
    assert!(p.title.contains("R7"), "{}", p.title);
    assert!(p.title.contains("moved") && p.title.contains("3.20"), "distance: {}", p.title);
    assert!(p.title.contains("rotated") && p.title.contains("90"), "rotation: {}", p.title);
}

#[test]
fn placement_side_flip() {
    let mut a = bundle(empty_indexes());
    a.geometry = Some(geom_with_comp("U2", 20.0, 20.0, 0.0, 0)); // F.Cu
    let mut b = bundle(empty_indexes());
    b.geometry = Some(geom_with_comp("U2", 20.0, 20.0, 0.0, 1)); // B.Cu

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let p = doc.changes.iter().find(|c| c.group == Group::Placement).expect("placement change");
    assert!(p.title.contains("back"), "reports side flip: {}", p.title);
}

fn geom_with_seg(_net_name: &str, xy: [f64; 4]) -> GeomTracks {
    GeomTracks {
        seg: GeomSegCol {
            xy: xy.to_vec(),
            w: vec![0.25],
            layer: vec![0],
            net: vec![1],
        },
        ..Default::default()
    }
}

fn routing_geom(nets: Vec<&str>, tracks: GeomTracks, extra: usize) -> Geometry {
    let mut g = Geometry {
        layers: vec![GeomLayer { name: "In2.Cu".into(), role: "copper".into() }],
        nets: std::iter::once(String::new()).chain(nets.iter().map(|s| s.to_string())).collect(),
        tracks,
        ..Default::default()
    };
    // Append `extra` extra segments (all net index 1) at offset positions.
    for i in 0..extra {
        let base = 100.0 + i as f64;
        g.tracks.seg.xy.extend([base, base, base + 1.0, base]);
        g.tracks.seg.w.push(0.25);
        g.tracks.seg.layer.push(0);
        g.tracks.seg.net.push(1);
    }
    g
}

#[test]
fn routing_setdiff_groups_per_layer_net() {
    // A has 3 segments on /VBUS (In2.Cu); B has those 3 replaced by 12 different ones
    // → one "rerouted (+12 −3)" row, not 15 rows.
    let a = {
        let mut b = bundle(empty_indexes());
        let g = routing_geom(vec!["/VBUS"], geom_with_seg("/VBUS", [0.0, 0.0, 5.0, 0.0]), 2); // 3 total
        b.geometry = Some(g);
        b
    };
    let bb = {
        let mut b = bundle(empty_indexes());
        // 12 fresh segments, all at coordinates disjoint from A's.
        let mut t = GeomTracks::default();
        for i in 0..12 {
            let y = 500.0 + i as f64;
            t.seg.xy.extend([0.0, y, 5.0, y]);
            t.seg.w.push(0.25);
            t.seg.layer.push(0);
            t.seg.net.push(1);
        }
        let g = routing_geom(vec!["/VBUS"], t, 0);
        b.geometry = Some(g);
        b
    };
    let doc = diff_bundles(&a, &bb, &changed(&["board.kicad_pcb"]));
    let r: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Routing).collect();
    assert_eq!(r.len(), 1, "one grouped routing row, got {r:?}");
    assert_eq!(r[0].kind, Kind::Modified);
    assert!(r[0].title.contains("/VBUS") && r[0].title.contains("In2.Cu"), "{}", r[0].title);
    assert!(r[0].title.contains("+12") && r[0].title.contains("3"), "counts: {}", r[0].title);
}

#[test]
fn zone_area_delta_with_threshold() {
    // A GND pour on B.Cu grows well past the 1 mm² noise floor → one zone change; a
    // sub-threshold jitter on another pour is ignored.
    let make = |big_side: f64| Geometry {
        layers: vec![GeomLayer { name: "B.Cu".into(), role: "copper".into() }],
        nets: vec![String::new(), "GND".into()],
        zones: vec![GeomZone {
            layer: 0,
            net: 1,
            filled: true,
            // A square of side `big_side` → area big_side².
            pts: vec![0.0, 0.0, big_side, 0.0, big_side, big_side, 0.0, big_side],
        }],
        ..Default::default()
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make(10.0)); // 100 mm²
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make(20.0)); // 400 mm²

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let z: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Zone).collect();
    assert_eq!(z.len(), 1, "one zone change, got {z:?}");
    assert!(z[0].title.contains("GND") && z[0].title.contains("B.Cu"), "{}", z[0].title);
    assert!(z[0].title.contains("grew"), "{}", z[0].title);

    // Below-threshold: a 0.5 mm² change (10.0 vs sqrt(100.5)) must NOT appear.
    let mut a2 = bundle(empty_indexes());
    a2.geometry = Some(make(10.0)); // 100
    let mut b2 = bundle(empty_indexes());
    b2.geometry = Some(make((100.5_f64).sqrt())); // 100.5, delta 0.5 mm²
    let doc2 = diff_bundles(&a2, &b2, &changed(&["board.kicad_pcb"]));
    assert!(!doc2.changes.iter().any(|c| c.group == Group::Zone), "sub-threshold zone jitter ignored");
}

#[test]
fn sheet_add() {
    let mut a = empty_indexes();
    a.sheets = vec![sheet(1, "root"), sheet(2, "power")];
    let mut b = empty_indexes();
    b.sheets = vec![sheet(1, "root"), sheet(2, "power"), sheet(3, "gate_driver_W")];

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let s = doc.changes.iter().find(|c| c.group == Group::Sheet).expect("sheet change");
    assert_eq!(s.kind, Kind::Added);
    assert!(s.title.contains("gate_driver_W"), "{}", s.title);
    assert_eq!(s.impact, Impact::Doc);
}

#[test]
fn doc_titleblock_change() {
    let frame = |rev: &str| GeomFrame {
        title: "Board".into(),
        company: "Acme".into(),
        rev: rev.into(),
        date: String::new(),
        paper: "A4".into(),
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(Geometry { frame: Some(frame("1.2")), ..Default::default() });
    a.pcb_file = Some("board.kicad_pcb".into());
    let mut b = bundle(empty_indexes());
    b.geometry = Some(Geometry { frame: Some(frame("1.3")), ..Default::default() });
    b.pcb_file = Some("board.kicad_pcb".into());

    // Board file changed so the PCB pass (which also produces docs? no — docs come from
    // diff_docs, always run). Use no_source_diff to confirm docs run regardless.
    let doc = diff_bundles(&a, &b, &no_source_diff());
    let d = doc.changes.iter().find(|c| c.group == Group::Doc).expect("doc change");
    assert!(d.title.contains("Rev") && d.title.contains("1.2") && d.title.contains("1.3"), "{}", d.title);
    assert_eq!(d.impact, Impact::Doc);
}

#[test]
fn unchanged_sheet_is_pruned_and_skipped() {
    // Two sheets present on both sides, mapped to source files; only power.kicad_sch
    // changed. root.kicad_sch is source-identical → pruned + reported.
    let mut a = empty_indexes();
    a.sheets = vec![sheet(1, "root"), sheet(2, "power")];
    let mut b = empty_indexes();
    b.sheets = vec![sheet(1, "root"), sheet(2, "power")];
    let mut ba = bundle(a);
    ba.sheet_files = HashMap::from([(1, "root.kicad_sch".to_string()), (2, "power.kicad_sch".to_string())]);
    let mut bb = bundle(b);
    bb.sheet_files = HashMap::from([(1, "root.kicad_sch".to_string()), (2, "power.kicad_sch".to_string())]);

    let doc = diff_bundles(&ba, &bb, &changed(&["power.kicad_sch"]));
    assert!(doc.sheets_pruned.contains(&1), "root sheet (unchanged file) pruned");
    assert!(!doc.sheets_pruned.contains(&2), "power sheet (changed file) not pruned");
}

#[test]
fn pcb_pass_skipped_when_board_unchanged() {
    // Placement differs, but the .kicad_pcb blob is unchanged → the whole PCB pass is
    // skipped, so no placement change appears.
    let mut a = bundle(empty_indexes());
    a.geometry = Some(geom_with_comp("R7", 10.0, 10.0, 0.0, 0));
    a.pcb_file = Some("board.kicad_pcb".into());
    let mut b = bundle(empty_indexes());
    b.geometry = Some(geom_with_comp("R7", 50.0, 50.0, 90.0, 0));
    b.pcb_file = Some("board.kicad_pcb".into());

    // Only a schematic file changed; the pcb is not in the delta.
    let doc = diff_bundles(&a, &b, &changed(&["root.kicad_sch"]));
    assert!(!doc.changes.iter().any(|c| c.group == Group::Placement), "PCB pass skipped when board unchanged");
}

#[test]
fn empty_diff_on_identical_bundles() {
    let make = || {
        let mut a = empty_indexes();
        a.components.insert("R1".into(), comp_full("10k", "R_0402", "MPN1", 1, "u1"));
        a.nets.insert("/N".into(), net(vec![term("R1", "1")]));
        a.sheets = vec![sheet(1, "root")];
        a
    };
    let a = make();
    let b = make();

    let mut ba = bundle(a);
    ba.geometry = Some(geom_with_comp("R1", 5.0, 5.0, 0.0, 0));
    ba.pcb_file = Some("board.kicad_pcb".into());
    let mut bb = bundle(b);
    bb.geometry = Some(geom_with_comp("R1", 5.0, 5.0, 0.0, 0));
    bb.pcb_file = Some("board.kicad_pcb".into());

    let doc = diff_bundles(&ba, &bb, &changed(&["board.kicad_pcb"]));
    assert!(doc.changes.is_empty(), "identical bundles produce no changes, got {:?}", doc.changes);
    assert_eq!(doc.stats, Stats::default());
}

#[test]
fn deterministic_serialization() {
    // Same inputs twice ⇒ byte-identical serialized doc (load-bearing invariant).
    let build = || {
        let mut a = empty_indexes();
        a.components.insert("C1".into(), comp("100n", "C_0402", false));
        a.components.insert("R1".into(), comp("10k", "R_0402", false));
        a.nets.insert("/A".into(), net(vec![term("R1", "1"), term("C1", "1")]));
        let mut b = empty_indexes();
        b.components.insert("C1".into(), comp("1u", "C_0402", false)); // value change
        b.components.insert("R1".into(), comp("10k", "R_0402", false));
        b.components.insert("R2".into(), comp("22k", "R_0402", false)); // add
        b.nets.insert("/A".into(), net(vec![term("R1", "1")])); // pin removed
        let mut ba = bundle(a);
        ba.geometry = Some(geom_with_comp("R1", 3.0, 3.0, 0.0, 0));
        ba.pcb_file = Some("board.kicad_pcb".into());
        let mut bb = bundle(b);
        bb.geometry = Some(geom_with_comp("R1", 6.0, 3.0, 90.0, 0));
        bb.pcb_file = Some("board.kicad_pcb".into());
        diff_bundles(&ba, &bb, &changed(&["board.kicad_pcb"]))
    };
    let d1 = serde_json::to_string(&build()).unwrap();
    let d2 = serde_json::to_string(&build()).unwrap();
    assert_eq!(d1, d2, "diff doc serialization must be byte-identical across runs");
    // Ids are ordinal and stable.
    let doc = build();
    for (i, c) in doc.changes.iter().enumerate() {
        assert_eq!(c.id, format!("ch_{i:04}"));
    }
}

#[test]
fn silk_and_outline_from_graphics() {
    // A silk line added on F.SilkS and an Edge.Cuts change → one Silk + one Outline row.
    let make = |edge_len: f64, silk: bool| {
        let mut g = Geometry {
            layers: vec![
                GeomLayer { name: "Edge.Cuts".into(), role: "edge".into() },
                GeomLayer { name: "F.SilkS".into(), role: "silkscreen".into() },
            ],
            nets: vec![String::new()],
            graphics: vec![GeomGraphic {
                layer: 0,
                width: 0.1,
                kind: "seg".into(),
                data: vec![0.0, 0.0, edge_len, 0.0],
            }],
            ..Default::default()
        };
        if silk {
            g.graphics.push(GeomGraphic {
                layer: 1,
                width: 0.12,
                kind: "seg".into(),
                data: vec![1.0, 1.0, 2.0, 2.0],
            });
        }
        g
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make(100.0, false));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make(120.0, true)); // edge changed + silk added

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    assert!(
        doc.changes.iter().any(|c| c.group == Group::Outline && c.impact == Impact::Placement),
        "outline change present"
    );
    assert!(
        doc.changes.iter().any(|c| c.group == Group::Silk && c.impact == Impact::Cosmetic),
        "silk change present"
    );
}

#[test]
fn pcb_text_edit_folds_to_modify() {
    let make = |rev: &str| Geometry {
        layers: vec![GeomLayer { name: "F.SilkS".into(), role: "silkscreen".into() }],
        nets: vec![String::new()],
        texts: vec![GeomText { layer: 0, text: rev.into(), x: 10.0, y: 10.0, ..Default::default() }],
        ..Default::default()
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make("REV A"));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make("REV B"));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let t: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Text).collect();
    assert_eq!(t.len(), 1, "one text modify, not add+remove: {t:?}");
    assert_eq!(t[0].kind, Kind::Modified);
    assert!(t[0].title.contains("REV A") && t[0].title.contains("REV B"), "{}", t[0].title);
}

#[test]
fn pcb_text_move_folds_to_moved() {
    // The same string at a new spot is ONE moved row (anchored over both positions),
    // not a remove + add pair.
    let make = |x: f64, y: f64| Geometry {
        layers: vec![GeomLayer { name: "F.SilkS".into(), role: "silkscreen".into() }],
        nets: vec![String::new()],
        texts: vec![GeomText { layer: 0, text: "TDO".into(), x, y, ..Default::default() }],
        ..Default::default()
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make(10.0, 10.0));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make(14.0, 13.0));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let t: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Text).collect();
    assert_eq!(t.len(), 1, "one moved row, not add+remove: {t:?}");
    assert_eq!(t[0].kind, Kind::Moved);
    assert!(t[0].title.contains("TDO") && t[0].title.contains("moved"), "{}", t[0].title);
    assert_eq!(t[0].side, Side::Both);
    // Anchor spans BOTH positions so either side's render frames its copy.
    let bbox = t[0].anchors.pcb.as_ref().unwrap().bbox.unwrap();
    assert!(bbox[0] <= 5.0 && bbox[0] + bbox[2] >= 19.0, "bbox covers old+new x: {bbox:?}");
    assert!(bbox[1] <= 5.0 && bbox[1] + bbox[3] >= 18.0, "bbox covers old+new y: {bbox:?}");
}

#[test]
fn pcb_text_restyle_surfaces_as_modify() {
    // A style-only edit (size / pen thickness / font, string and position intact)
    // must surface as ONE modified row — not mask as unchanged, not add+remove.
    let make = |size: f64, thickness: Option<f64>, font: Option<&str>| Geometry {
        layers: vec![GeomLayer { name: "F.SilkS".into(), role: "silkscreen".into() }],
        nets: vec![String::new()],
        texts: vec![
            GeomText { layer: 0, text: "EN/FLT".into(), x: 10.0, y: 10.0, size: Some(size), font: font.map(Into::into), ..Default::default() },
            GeomText { layer: 0, text: "V_phs".into(), x: 20.0, y: 20.0, size: Some(1.0), thickness, ..Default::default() },
        ],
        ..Default::default()
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make(1.0, Some(0.15), None));
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make(0.8, Some(0.3), Some("Calibri")));

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    let t: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Text).collect();
    assert_eq!(t.len(), 2, "one restyle row per text: {t:?}");
    for c in &t {
        assert_eq!(c.kind, Kind::Modified);
        assert!(c.title.contains("restyled"), "{}", c.title);
    }
    let en = t.iter().find(|c| c.title.contains("EN/FLT")).expect("EN/FLT row");
    assert!(en.detail.contains("size") && en.detail.contains("font"), "{}", en.detail);
    let vp = t.iter().find(|c| c.title.contains("V_phs")).expect("V_phs row");
    assert!(vp.detail.contains("thickness 0.15 mm → 0.3 mm"), "{}", vp.detail);
}

#[test]
fn pcb_text_untouched_style_stays_masked() {
    // Identical texts (including style) on both sides must produce NO text rows.
    let make = || Geometry {
        layers: vec![GeomLayer { name: "F.SilkS".into(), role: "silkscreen".into() }],
        nets: vec![String::new()],
        texts: vec![GeomText { layer: 0, text: "B-".into(), x: 5.0, y: 5.0, size: Some(1.0), thickness: Some(0.15), ..Default::default() }],
        ..Default::default()
    };
    let mut a = bundle(empty_indexes());
    a.geometry = Some(make());
    let mut b = bundle(empty_indexes());
    b.geometry = Some(make());

    let doc = diff_bundles(&a, &b, &changed(&["board.kicad_pcb"]));
    assert!(
        !doc.changes.iter().any(|c| c.group == Group::Text),
        "no text rows for identical texts: {:?}",
        doc.changes
    );
}

// ----------------------------------------------- one-action-one-row + schematic moves

#[test]
fn removed_component_does_not_spam_net_membership() {
    // Removing C1 must read as ONE change (the component row); the nets it sat on
    // must not add "membership changed" rows for the same action.
    let mut a = empty_indexes();
    a.components.insert("R1".into(), comp("10k", "R_0402", false));
    a.components.insert("C1".into(), comp("100n", "C_0402", false));
    a.nets.insert("/N".into(), net(vec![term("R1", "1"), term("C1", "1")]));
    let mut b = empty_indexes();
    b.components.insert("R1".into(), comp("10k", "R_0402", false));
    b.nets.insert("/N".into(), net(vec![term("R1", "1")]));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    let comps: Vec<_> = doc.changes.iter().filter(|c| c.group == Group::Component).collect();
    assert_eq!(comps.len(), 1, "one component row: {:?}", doc.changes);
    assert_eq!(comps[0].kind, Kind::Removed);
    assert!(
        !doc.changes.iter().any(|c| c.group == Group::Net),
        "no net rows for a component removal: {:?}",
        doc.changes
    );
}

#[test]
fn reannotation_suppresses_net_churn_and_anchors_both_sides() {
    // C1 -> C2 re-annotation: one renamed row carrying BOTH sides' anchors (the
    // symbol uuid changed), and zero net rows (C2.1 is C1.1 canonicalized).
    let mut a = empty_indexes();
    a.components.insert("C1".into(), comp_full("100n", "C_0402", "", 1, "u_old"));
    a.nets.insert("/N".into(), net(vec![term("C1", "1")]));
    let mut b = empty_indexes();
    b.components.insert("C2".into(), comp_full("100n", "C_0402", "", 1, "u_new"));
    b.nets.insert("/N".into(), net(vec![term("C2", "1")]));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert_eq!(doc.changes.len(), 1, "exactly the rename row: {:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!(c.kind, Kind::Renamed);
    let sch = c.anchors.schematic.as_ref().expect("B anchor");
    assert_eq!(sch.uuids, vec!["u_new".to_string()]);
    let sch_a = c.anchors.schematic_a.as_ref().expect("A anchor for the old uuid");
    assert_eq!(sch_a.uuids, vec!["u_old".to_string()]);
}

#[test]
fn schematic_symbol_move_emits_cosmetic_row() {
    let mk = |x: f64| {
        let mut c = comp("100n", "C_0402", false);
        c.bbox = Some([x, 5.0, 2.0, 2.0]);
        c
    };
    let mut a = empty_indexes();
    a.components.insert("C1".into(), mk(10.0));
    let mut b = empty_indexes();
    b.components.insert("C1".into(), mk(22.7));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!((c.group, c.kind, c.impact), (Group::Component, Kind::Moved, Impact::Cosmetic));
    assert!(c.title.contains("moved on schematic"), "{}", c.title);

    // Sub-grid jitter must NOT emit a row.
    let mut a = empty_indexes();
    a.components.insert("C1".into(), mk(10.0));
    let mut b = empty_indexes();
    b.components.insert("C1".into(), mk(10.3));
    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert!(doc.changes.is_empty(), "0.3 mm is jitter: {:?}", doc.changes);
}

#[test]
fn graphical_only_sheet_edit_falls_back_to_one_row() {
    // Source changed (a nudged GND symbol), semantics identical: one cosmetic
    // sheet row, so the edit isn't invisible.
    let make = || {
        let mut ix = empty_indexes();
        ix.components.insert("R1".into(), comp_full("10k", "R_0402", "", 1, "u_r1"));
        ix.nets.insert("/N".into(), net(vec![term("R1", "1")]));
        ix.sheets = vec![sheet(1, "root")];
        ix
    };
    let mut ba = bundle(make());
    ba.sheet_files.insert(1, "root.kicad_sch".into());
    let mut bb = bundle(make());
    bb.sheet_files.insert(1, "root.kicad_sch".into());

    // The source-hash delta carries design-folder-prefixed paths; sheet_files carries
    // the bare design.json filename — the fallback must match across that difference.
    let doc = diff_bundles(&ba, &bb, &changed(&["PROJ/root.kicad_sch"]));
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!((c.group, c.kind, c.impact), (Group::Sheet, Kind::Modified, Impact::Cosmetic));
    assert!(c.title.contains("graphical edits"), "{}", c.title);
    assert_eq!(c.anchors.schematic.as_ref().map(|s| s.sheet), Some(1));

    // But when a semantic change explains the source delta, no fallback row rides along.
    let mut ba = bundle(make());
    ba.sheet_files.insert(1, "root.kicad_sch".into());
    let mut ix = make();
    ix.components.get_mut("R1").unwrap().value = "22k".into();
    let mut bb = bundle(ix);
    bb.sheet_files.insert(1, "root.kicad_sch".into());
    let doc = diff_bundles(&ba, &bb, &changed(&["root.kicad_sch"]));
    assert!(
        !doc.changes.iter().any(|c| c.group == Group::Sheet),
        "no fallback next to the value row: {:?}",
        doc.changes
    );
}

#[test]
fn moved_power_symbol_splits_into_one_anchored_row() {
    // A nudged GND power symbol (no semantic row — power symbols aren't components).
    // With per-element geometry it becomes ONE cosmetic row anchored to its uuid, and
    // the clubbed fallback row is suppressed.
    let make_ix = || {
        let mut ix = empty_indexes();
        ix.sheets = vec![sheet(1, "power")];
        ix
    };
    let mut ba = bundle(make_ix());
    ba.sheet_files.insert(1, "power.kicad_sch".into());
    ba.sch_geometry =
        Some(sch_geom("power.kicad_sch", vec![sch_elem("gnd1", "power", [100.0, 100.0, 2.0, 2.0], "power:GND|u1|a0|m")]));
    let mut bb = bundle(make_ix());
    bb.sheet_files.insert(1, "power.kicad_sch".into());
    bb.sch_geometry =
        Some(sch_geom("power.kicad_sch", vec![sch_elem("gnd1", "power", [120.0, 130.0, 2.0, 2.0], "power:GND|u1|a0|m")]));

    let doc = diff_bundles(&ba, &bb, &changed(&["power.kicad_sch"]));
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!((c.group, c.kind, c.impact), (Group::Sheet, Kind::Moved, Impact::Cosmetic));
    assert!(c.title.contains("power symbol moved"), "{}", c.title);
    assert_eq!(c.anchors.schematic.as_ref().unwrap().uuids, vec!["gnd1".to_string()]);
    assert!(!c.title.contains("graphical edits"), "no clubbed fallback row: {}", c.title);
}

#[test]
fn colocated_edits_cluster_but_distant_ones_split() {
    let make_ix = || {
        let mut ix = empty_indexes();
        ix.sheets = vec![sheet(1, "root")];
        ix
    };
    // A dragged symbol + its stretched wire (co-located) plus a far-away moved note.
    let a = vec![
        sch_elem("s1", "symbol", [50.0, 50.0, 4.0, 4.0], "Device:R|u1|a0|m"),
        sch_elem("w1", "wire", [54.0, 52.0, 3.0, 0.0], "wire|w0"),
        sch_elem("n1", "text", [200.0, 200.0, 0.0, 0.0], "text|hi"),
    ];
    let b = vec![
        sch_elem("s1", "symbol", [60.0, 50.0, 4.0, 4.0], "Device:R|u1|a0|m"),
        sch_elem("w1", "wire", [64.0, 52.0, 3.0, 0.0], "wire|w0"),
        sch_elem("n1", "text", [200.0, 250.0, 0.0, 0.0], "text|hi"),
    ];
    let mut ba = bundle(make_ix());
    ba.sheet_files.insert(1, "root.kicad_sch".into());
    ba.sch_geometry = Some(sch_geom("root.kicad_sch", a));
    let mut bb = bundle(make_ix());
    bb.sheet_files.insert(1, "root.kicad_sch".into());
    bb.sch_geometry = Some(sch_geom("root.kicad_sch", b));

    let doc = diff_bundles(&ba, &bb, &changed(&["root.kicad_sch"]));
    // The symbol+wire cluster is one row; the distant note is another → two rows.
    assert_eq!(doc.changes.len(), 2, "{:?}", doc.changes);
    let sym_row = doc.changes.iter().find(|c| c.title.contains("symbol")).expect("symbol row");
    let mut u = sym_row.anchors.schematic.as_ref().unwrap().uuids.clone();
    u.sort();
    assert_eq!(u, vec!["s1".to_string(), "w1".to_string()], "drag clusters the wire in");
    assert!(doc.changes.iter().any(|c| c.title.contains("note moved")), "distant note is its own row");
}

#[test]
fn removed_schematic_element_anchors_a_side() {
    // A wire present only on A (deleted): a Side::A removed row anchored to its uuid, so
    // the A-island landing (frontend) can frame it.
    let make_ix = || {
        let mut ix = empty_indexes();
        ix.sheets = vec![sheet(1, "root")];
        ix
    };
    let mut ba = bundle(make_ix());
    ba.sheet_files.insert(1, "root.kicad_sch".into());
    ba.sch_geometry =
        Some(sch_geom("root.kicad_sch", vec![sch_elem("w9", "wire", [10.0, 10.0, 5.0, 0.0], "wire|w0")]));
    let mut bb = bundle(make_ix());
    bb.sheet_files.insert(1, "root.kicad_sch".into());
    bb.sch_geometry = Some(sch_geom("root.kicad_sch", vec![]));

    let doc = diff_bundles(&ba, &bb, &changed(&["root.kicad_sch"]));
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!((c.group, c.kind, c.side), (Group::Sheet, Kind::Removed, Side::A));
    assert_eq!(c.anchors.schematic.as_ref().unwrap().uuids, vec!["w9".to_string()]);
}

#[test]
fn field_presentation_edit_surfaces_alongside_semantic_row() {
    // C68: the MPN is cleared (a semantic component row) AND a visible field's font size
    // changes (a geometry-signature edit, body bbox untouched). The MPN row must NOT
    // swallow the presentation edit — both appear, and the edit anchors to the symbol so
    // clicking it lands on C68.
    let mut a = empty_indexes();
    a.sheets = vec![sheet(6, "current_sense")];
    a.components.insert("C68".into(), comp_full("100n", "C_0402", "GCM155", 6, "sym68"));
    let mut b = empty_indexes();
    b.sheets = vec![sheet(6, "current_sense")];
    b.components.insert("C68".into(), comp_full("100n", "C_0402", "", 6, "sym68"));

    let mut ba = bundle(a);
    ba.sheet_files.insert(6, "current_sense.kicad_sch".into());
    ba.sch_geometry = Some(sch_geom(
        "current_sense.kicad_sch",
        vec![sch_elem("sym68", "symbol", [41.0, 92.0, 4.0, 7.0], "Device:C|u1|a0|m|Value:z1.27:00:5,2,0")],
    ));
    let mut bb = bundle(b);
    bb.sheet_files.insert(6, "current_sense.kicad_sch".into());
    bb.sch_geometry = Some(sch_geom(
        "current_sense.kicad_sch",
        vec![sch_elem("sym68", "symbol", [41.0, 92.0, 4.0, 7.0], "Device:C|u1|a0|m|Value:z2.54:00:5,2,0")],
    ));

    let doc = diff_bundles(&ba, &bb, &changed(&["current_sense.kicad_sch"]));
    assert!(
        doc.changes.iter().any(|c| c.group == Group::Component && c.title.contains("C68") && c.title.contains("MPN")),
        "the semantic MPN row is present: {:?}",
        doc.changes
    );
    let edit = doc
        .changes
        .iter()
        .find(|c| c.group == Group::Sheet && c.title.contains("edited"))
        .unwrap_or_else(|| panic!("field-edit row is NOT suppressed by the MPN row: {:?}", doc.changes));
    assert_eq!(edit.anchors.schematic.as_ref().unwrap().uuids, vec!["sym68".to_string()]);
}

#[test]
fn simultaneous_move_and_edit_reads_as_both() {
    // A note that was reworded AND dragged (the "close→near" note that also shifted a grid
    // step) reads as "edited & moved", not a plain "edited" that hides the move.
    let make_ix = || {
        let mut ix = empty_indexes();
        ix.sheets = vec![sheet(1, "root")];
        ix
    };
    let mut ba = bundle(make_ix());
    ba.sheet_files.insert(1, "root.kicad_sch".into());
    ba.sch_geometry =
        Some(sch_geom("root.kicad_sch", vec![sch_elem("n1", "text", [10.0, 10.0, 0.0, 0.0], "text|close")]));
    let mut bb = bundle(make_ix());
    bb.sheet_files.insert(1, "root.kicad_sch".into());
    bb.sch_geometry =
        Some(sch_geom("root.kicad_sch", vec![sch_elem("n1", "text", [10.0, 11.27, 0.0, 0.0], "text|near")]));

    let doc = diff_bundles(&ba, &bb, &changed(&["root.kicad_sch"]));
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    assert!(doc.changes[0].title.contains("edited & moved"), "{}", doc.changes[0].title);
}

#[test]
fn value_change_carries_per_side_emphasis() {
    let mut a = empty_indexes();
    a.components.insert("C134".into(), comp_full("1nF", "C_0402", "", 1, "u1"));
    let mut b = empty_indexes();
    b.components.insert("C134".into(), comp_full("10nF", "C_0402", "", 1, "u1"));

    let doc = diff_bundles(&bundle(a), &bundle(b), &no_source_diff());
    assert_eq!(doc.changes.len(), 1, "{:?}", doc.changes);
    let c = &doc.changes[0];
    assert_eq!(c.emph_a.as_deref(), Some("1nF"));
    assert_eq!(c.emph_b.as_deref(), Some("10nF"));
    // Same symbol uuid on both sides: no duplicate A anchor.
    assert!(c.anchors.schematic_a.is_none());
}
