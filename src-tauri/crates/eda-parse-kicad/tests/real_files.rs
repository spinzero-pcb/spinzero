//! Validation against real reference boards. These are developer-machine designs
//! outside the repo, so each test no-ops when its file is absent.

use std::path::Path;

use eda_parse_kicad::Schematic;

// Differential fixture: set SPINZERO_TI_TUTORIAL to a checkout of the (public)
// KiCad 9 TI-MSPM0 tutorial board, with a crunched `.pcbcache` next to it, to run
// this test; it skips when the variable is unset.
fn ti_root() -> Option<std::path::PathBuf> {
    std::env::var("SPINZERO_TI_TUTORIAL").ok().map(std::path::PathBuf::from)
}

#[test]
fn ti_tutorial_schematic_parses() {
    let Some(root) = ti_root() else {
        eprintln!("skipping: SPINZERO_TI_TUTORIAL not set");
        return;
    };
    let path = root.join("TI-MSP-KICAD9-TUTORIAL.kicad_sch");
    if !path.exists() {
        eprintln!("skipping: reference board not present");
        return;
    }
    let src = std::fs::read_to_string(&path).unwrap();
    let sch = Schematic::parse_str(&src).unwrap();

    // Real placements were found and carry resolved properties.
    assert!(sch.symbols.len() > 20, "got {} symbols", sch.symbols.len());

    let c12 = sch
        .symbols
        .iter()
        .find(|s| s.reference() == Some("C12"))
        .expect("C12 present");
    assert_eq!(c12.property("Value"), Some("470n"));
    assert_eq!(c12.lib_id, "Device:C");
    // svg_id cross-reference: the placement uuid matches the golden design.json.
    assert_eq!(c12.uuid, "1277ffc1-946b-4105-a00f-ddc1d88fe351");

    // Library pin electrical types are recovered (needed for net terminals).
    let dev_c = sch.lib_symbol("Device:C").expect("Device:C lib symbol");
    assert_eq!(dev_c.pins.len(), 2);
    assert!(dev_c.pins.iter().all(|p| p.etype == "passive"));

    // Wiring primitives are populated for the netlist stage.
    assert!(!sch.wires.is_empty());
    assert!(!sch.labels.is_empty());
}
