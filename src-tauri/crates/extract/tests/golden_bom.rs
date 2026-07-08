//! Differential validation of the BOM: the grouped output must coalesce parts
//! into the same line items (by designator membership) as the golden bundle, and
//! the enriched resolver must surface distributor / datasheet / manufacturer data.

use std::collections::BTreeSet;

use eda_parse_kicad::Schematic;
use extract::bom;
use extract::design;

// Differential fixture: set SPINZERO_TI_TUTORIAL to a checkout of the (public)
// KiCad 9 TI-MSPM0 tutorial board, with a crunched `.pcbcache` next to it, to run
// this test; it skips when the variable is unset.
fn ti_root() -> Option<std::path::PathBuf> {
    std::env::var("SPINZERO_TI_TUTORIAL").ok().map(std::path::PathBuf::from)
}

type DesSet = BTreeSet<String>;

#[test]
fn ti_tutorial_bom_matches_golden() {
    let Some(root) = ti_root() else {
        eprintln!("skipping: SPINZERO_TI_TUTORIAL not set");
        return;
    };
    let sch_p = root.join("TI-MSP-KICAD9-TUTORIAL.kicad_sch");
    let bom_p = root.join(".pcbcache/bom/TI-MSP-KICAD9-TUTORIAL_bom.json");
    if !sch_p.exists() || !bom_p.exists() {
        eprintln!("skipping: reference board not present");
        return;
    }
    let sch = Schematic::parse_str(&std::fs::read_to_string(&sch_p).unwrap()).unwrap();
    let components = design::build_components(&sch, "/");
    let mapping = bom::resolve_mapping(&components);

    // --- grouped: same line items by designator membership ---
    let grouped = bom::build_grouped(&components, &mapping, "p", "TI-MSP-KICAD9-TUTORIAL");
    assert_eq!(grouped.line_count, 16);
    assert_eq!(grouped.component_count, 29);

    let gold: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bom_p).unwrap()).unwrap();
    let gold_groups: BTreeSet<DesSet> = gold["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["designators"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    let json = serde_json::to_value(&grouped).unwrap();
    let mine_groups: BTreeSet<DesSet> = json["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["designators"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_str().unwrap().to_string())
                .collect()
        })
        .collect();
    let only_gold: Vec<_> = gold_groups.difference(&mine_groups).collect();
    let only_mine: Vec<_> = mine_groups.difference(&gold_groups).collect();
    eprintln!("bom groups only_gold={:?} only_mine={:?}", only_gold, only_mine);
    assert!(only_gold.is_empty() && only_mine.is_empty(), "BOM grouping diverges");

    // --- enriched: resolver surfaced real sourcing data ---
    let (rows, dist) = bom::build_enriched(&components, &mapping);
    assert_eq!(rows.len(), 16);
    assert!(dist.contains(&"LCSC".to_string()), "LCSC distributor detected");

    let u3 = rows.iter().find(|r| r.references.contains(&"U3".to_string())).unwrap();
    assert_eq!(u3.manufacturer, "Texas Instruments");
    assert_eq!(u3.mpn, "MSPM0G3507SPTR");

    let d1 = rows.iter().find(|r| r.references.contains(&"D1".to_string())).unwrap();
    assert!(d1.datasheet.starts_with("http"), "D1 datasheet URL resolved");

    let caps = rows
        .iter()
        .find(|r| r.value == "470n" && r.footprint.contains("0603"))
        .unwrap();
    assert_eq!(caps.distributors.get("LCSC").map(String::as_str), Some("C1623"));
}
