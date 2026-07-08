//! Differential validation of the component list against the golden bundle:
//! same designators, and matching value / footprint / library_ref /
//! classification / svg_id per component.

use std::collections::BTreeMap;

use eda_parse_kicad::Schematic;
use extract::design;

// Differential fixture: set SPINZERO_TI_TUTORIAL to a checkout of the (public)
// KiCad 9 TI-MSPM0 tutorial board, with a crunched `.pcbcache` next to it, to run
// this test; it skips when the variable is unset.
fn ti_root() -> Option<std::path::PathBuf> {
    std::env::var("SPINZERO_TI_TUTORIAL").ok().map(std::path::PathBuf::from)
}

#[test]
fn ti_tutorial_components_match_golden() {
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
    let mine = design::build_components(&sch, "/");

    let gold_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_p).unwrap()).unwrap();
    let gold: BTreeMap<String, &serde_json::Value> = gold_json["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["designator"].as_str().unwrap().to_string(), c))
        .collect();

    let mine_des: Vec<&str> = mine.iter().map(|c| c.designator.as_str()).collect();
    let gold_des: Vec<&str> = gold.keys().map(String::as_str).collect();

    let mut problems = Vec::new();
    for d in &gold_des {
        if !mine_des.contains(d) {
            problems.push(format!("missing component {d}"));
        }
    }
    for d in &mine_des {
        if !gold.contains_key(*d) {
            problems.push(format!("extra component {d}"));
        }
    }

    for c in &mine {
        let Some(g) = gold.get(&c.designator) else {
            continue;
        };
        let g_str = |k: &str| g[k].as_str().unwrap_or("").to_string();
        if c.value != g_str("value") {
            problems.push(format!("{}: value {:?} != {:?}", c.designator, c.value, g_str("value")));
        }
        if c.footprint != g_str("footprint") {
            problems.push(format!("{}: footprint mismatch", c.designator));
        }
        if c.library_ref != g_str("library_ref") {
            problems.push(format!("{}: library_ref mismatch", c.designator));
        }
        if c.svg_id != g_str("svg_id") {
            problems.push(format!("{}: svg_id mismatch", c.designator));
        }
        let gk = g["classification"]["type"].as_str().unwrap_or("");
        if c.classification.kind != gk {
            problems.push(format!(
                "{}: class {:?} != {:?} (pins={})",
                c.designator, c.classification.kind, gk, c.classification.pin_count
            ));
        }
    }

    eprintln!(
        "components: mine={} golden={} problems={}",
        mine.len(),
        gold.len(),
        problems.len()
    );
    for p in problems.iter().take(40) {
        eprintln!("  {p}");
    }
    assert!(problems.is_empty(), "{} component mismatches", problems.len());
}
