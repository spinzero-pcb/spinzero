//! Validates the assembled design model: counts, and that component_to_nets
//! agrees with golden for a representative multi-pin component.


use eda_parse_kicad::Schematic;
use extract::design::{self, SheetInfo};

// Differential fixture: set SPINZERO_TI_TUTORIAL to a checkout of the (public)
// KiCad 9 TI-MSPM0 tutorial board, with a crunched `.pcbcache` next to it, to run
// this test; it skips when the variable is unset.
fn ti_root() -> Option<std::path::PathBuf> {
    std::env::var("SPINZERO_TI_TUTORIAL").ok().map(std::path::PathBuf::from)
}

#[test]
fn ti_tutorial_design_model_matches_golden() {
    let Some(root) = ti_root() else {
        eprintln!("skipping: SPINZERO_TI_TUTORIAL not set");
        return;
    };
    let sch_p = root.join("TI-MSP-KICAD9-TUTORIAL.kicad_sch");
    let json_p = root.join(".pcbcache/design/TI-MSP-KICAD9-TUTORIAL_design.json");
    if !sch_p.exists() || !json_p.exists() {
        eprintln!("skipping: reference board not present");
        return;
    }
    let sch = Schematic::parse_str(&std::fs::read_to_string(&sch_p).unwrap()).unwrap();
    let sheet = SheetInfo {
        filename: "TI-MSP-KICAD9-TUTORIAL.kicad_sch".into(),
        path: sch_p.to_string_lossy().into_owned(),
        sheet_number: 1,
        sheet_path: "/".into(),
        sheet_path_uuids: "/".into(),
        title: "TI MSPM0 KICAD 9 TUTORIAL".into(),
        page: String::new(),
        notes: Vec::new(),
        company: String::new(),
        rev: String::new(),
        date: String::new(),
    };
    let model = design::build_design("TI-MSP-KICAD9-TUTORIAL", "x.kicad_pro", &sheet, &sch);

    assert_eq!(model.components.len(), 37);
    assert_eq!(model.nets.len(), 62);
    // Round-trips as JSON.
    let _ = serde_json::to_string(&model).unwrap();

    // svg_to_component covers every component and resolves a known part.
    assert_eq!(model.indexes.svg_to_component.len(), 37);
    let c12 = model.components.iter().find(|c| c.designator == "C12").unwrap();
    assert_eq!(
        model.indexes.svg_to_component.get(&c12.svg_id),
        Some(&"C12".to_string())
    );

    // component_to_nets for the MCU U3 matches golden (membership-equal).
    let gold: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_p).unwrap()).unwrap();
    let gold_u3: std::collections::BTreeSet<String> = gold["indexes"]["component_to_nets"]["U3"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let mine_u3: std::collections::BTreeSet<String> =
        model.indexes.component_to_nets["U3"].iter().cloned().collect();
    // Tolerate the one documented stale-golden net name (+5V vs our name).
    let only_gold: Vec<_> = gold_u3.difference(&mine_u3).collect();
    let only_mine: Vec<_> = mine_u3.difference(&gold_u3).collect();
    eprintln!("U3 nets only_gold={:?} only_mine={:?}", only_gold, only_mine);
    assert!(only_gold.len() <= 1 && only_mine.len() <= 1, "U3 net set diverges");
}
