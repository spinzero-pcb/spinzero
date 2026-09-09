//! Tests against the *generated* Altium corpus.
//!
//! `reference_designs` holds five real boards and not one harness, substack,
//! embedded board or library file, which is why those readers went unbuilt: a
//! parser for a binary layout with nothing to test it against is a silent wrong
//! answer with a passing build (docs/altium-extraction-notes.md, D9.4).
//!
//! These fixtures close that gap. They are authored by the §8.3 reference
//! parser (`altium-monkey`) and checked in, so they are redistributable in a way
//! the real corpus is not — regenerate with
//! `python scripts/altium-fixtures/generate.py`.
//!
//! What they prove is framing and field decoding against an INDEPENDENT writer
//! of the format, not that Altium itself emits the same bytes. Where the two
//! could differ the reader is written to accept both, and a claim that needs
//! Altium's own hand is left to the real corpus.

use std::path::PathBuf;

use eda_parse_altium::{pcb, Doc};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generated").join(name)
}

fn rigid_flex() -> pcb::PcbDoc {
    let doc = Doc::open(&fixture("rigid_flex.PcbDoc")).expect("rigid-flex fixture opens");
    pcb::parse(&doc)
}

/// The board declares seven layer-stack regions, three of them flex.
#[test]
fn a_rigid_flex_board_declares_its_substacks() {
    let b = rigid_flex();
    let names: Vec<&str> = b.substacks.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "MAIN_RIGID_STACK",
            "EAST_FLEX_EXTENTION",
            "EAST_RIGID_EXTENTION",
            "WEST_FLEX_EXTENTION",
            "WEST_EXTENTION_RIGID_STACK",
            "WEST_BRANCH_EXTENTION_BRANCH_FLEX_EXTENTION",
            "WEST_BRANCH_EXTENTION_RIGID_STACK",
        ]
    );
    assert_eq!(b.substacks.iter().filter(|s| s.is_flex).count(), 3);
}

/// The membership flag is INVERTED — `CONTEXT=0` means the layer is in the
/// substack — and reading it the obvious way is not a subtle error: it makes
/// every flex ribbon the thickest part of the board. The physical check is the
/// one that catches it, so that is the assertion: a flex ribbon carries FEWER
/// copper layers than the rigid stack it joins.
#[test]
fn a_flex_region_is_thinner_than_the_rigid_stack_it_joins() {
    let b = rigid_flex();
    let by = |n: &str| {
        b.substacks.iter().find(|s| s.name == n).unwrap_or_else(|| panic!("no substack {n}"))
    };
    let main = by("MAIN_RIGID_STACK").layers.len();
    assert!(main >= 8, "the master rigid stack is a many-layer board, got {main}");
    for s in b.substacks.iter().filter(|s| s.is_flex) {
        assert!(
            s.layers.len() < main,
            "{} is flex and reads {} copper layers against the rigid stack's {main} \
             — the CONTEXT flag has been read the wrong way round",
            s.name,
            s.layers.len()
        );
    }
}

/// Every outline region names a stack the board declares. This is the join that
/// turns "a substack exists" into "this part of the board is built like that".
#[test]
fn every_board_region_binds_to_a_declared_substack() {
    let b = rigid_flex();
    assert_eq!(b.board_regions.len(), 7, "one region per substack");
    for r in &b.board_regions {
        assert!(
            b.substacks.iter().any(|s| s.id == r.substack_id),
            "region {:?} names substack {:?}, which the board does not declare",
            r.name,
            r.substack_id
        );
    }
}

/// A drill span belongs to the substacks it is drilled in. On a rigid-flex
/// board a plain "top to bottom" reading is wrong, because the regions have
/// different bottoms.
#[test]
fn drill_spans_name_the_substacks_they_are_drilled_in() {
    let b = rigid_flex();
    assert!(!b.drill_pairs.is_empty(), "the board defines drill spans");
    let declared: Vec<&str> = b.substacks.iter().map(|s| s.id.as_str()).collect();
    for d in &b.drill_pairs {
        assert!(!d.low.is_empty() && !d.high.is_empty(), "a span names both ends");
        for g in &d.substacks {
            assert!(declared.contains(&g.as_str()), "span {}..{} names unknown stack {g}", d.low, d.high);
        }
    }
    assert!(
        b.drill_pairs.iter().any(|d| !d.substacks.is_empty()),
        "at least one span is bound to a substack"
    );
}

/// The folds. A bend line is where the flex actually bends, so its radius is a
/// manufacturing constraint a review can check — and it has to arrive in
/// millimetres, not Altium's 1/10000 mil.
#[test]
fn bend_lines_arrive_in_millimetres() {
    let b = rigid_flex();
    let bends: Vec<&pcb::BendLine> = b.board_regions.iter().flat_map(|r| r.bends.iter()).collect();
    assert!(!bends.is_empty(), "the board draws bend lines");
    for d in &bends {
        assert!(
            d.radius_mm > 0.0 && d.radius_mm < 25.0,
            "a bend radius of {} mm is not a radius a flex circuit is folded to",
            d.radius_mm
        );
        assert!(d.a != d.b, "a bend line has length");
        for (x, y) in [d.a, d.b] {
            assert!((0.0..=2540.0).contains(&x) && (0.0..=2540.0).contains(&y), "({x}, {y}) is off the workspace");
        }
    }
}

/// The harness connector, its entries and its type. All four harness records
/// live in the `Additional` stream, which is why a sheet whose `FileHeader`
/// histogram shows none can still draw a bundle.
#[test]
fn a_harness_connector_carries_its_entries_and_its_type() {
    let doc = Doc::open(&fixture("harness.SchDoc")).expect("harness fixture opens");
    let s = eda_parse_altium::sch::parse(&doc);

    assert_eq!(s.harness_connectors.len(), 1);
    let c = &s.harness_connectors[0];
    assert_eq!(c.harness_type, "I2C", "the RECORD=217 label names the bundle");
    let names: Vec<&str> = c.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["SDA", "SCL"], "entries in the order the connector lists them");
    assert!(c.w > 0 && c.h > 0, "the connector has a body to draw");

    assert_eq!(s.signal_harnesses.len(), 1, "one polyline carries the bundle");
    assert!(s.signal_harnesses[0].pts.len() >= 2);

    // The port is the link out of the sheet, and it names the bundle by type
    // rather than by any one member.
    let carriers: Vec<&str> = s
        .ports
        .iter()
        .filter(|p| !p.harness_type.is_empty())
        .map(|p| p.harness_type.as_str())
        .collect();
    assert_eq!(carriers, ["I2C"]);
}

/// A harness is a bundle, so no member of it becomes a net here. The bundle is
/// modelled and the members stay inside it — the same contract buses keep,
/// because `SDA` inside `I2C` and `SDA` inside `SENSOR` are different signals.
#[test]
fn a_harness_member_is_not_a_net_label() {
    let doc = Doc::open(&fixture("harness.SchDoc")).expect("harness fixture opens");
    let s = eda_parse_altium::sch::parse(&doc);
    for l in &s.net_labels {
        assert!(
            !["SDA", "SCL"].contains(&l.text.as_str()),
            "harness member {:?} reached the sheet as a net label",
            l.text
        );
    }
    // And the records are no longer counted as dropped.
    for t in [215i64, 216, 217, 218] {
        assert!(!s.skipped.contains_key(&t), "RECORD={t} is parsed, not skipped");
    }
}

/// The `.Harness` sidecar is the definition Altium generates and the designer
/// may lock, after which it can disagree with the drawing.
#[test]
fn a_harness_definition_file_lists_its_members() {
    let d = eda_parse_altium::sch::parse_harness_definition("I2C=SDA,SCL\r\n");
    assert_eq!(d, vec![("I2C".to_string(), vec!["SDA".to_string(), "SCL".to_string()])]);
    assert!(eda_parse_altium::sch::parse_harness_definition("\n=nothing\n").is_empty());
}
