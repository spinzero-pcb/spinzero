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
        pcb_file: None,
    }
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
        }],
        ..Default::default()
    }
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
        texts: vec![GeomText { layer: 0, text: rev.into(), x: 10.0, y: 10.0 }],
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
