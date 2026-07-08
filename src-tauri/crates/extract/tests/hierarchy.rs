//! End-to-end multi-sheet walk: a root schematic that references a child sheet
//! file is loaded, both sheets are netlisted, and a global net spanning the two
//! is merged. Uses temp files so it runs anywhere (no reference designs needed).

use std::path::PathBuf;

use extract::pipeline::{run_design, Msg};

const ROOT: &str = r#"
(kicad_sch
  (uuid "root-uuid")
  (lib_symbols (symbol "Device:R" (symbol "R_1_1"
    (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
    (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
  (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
    (property "Reference" "R1") (pin "1" (uuid "r1p1")) (pin "2" (uuid "r1p2")) (instances))
  (global_label "VBUS" (at 100 97.46 0) (uuid "g1"))
  (text "DNP R1 on the 48V build" (at 130 100 0) (uuid "note1"))
  (sheet (at 50 50) (uuid "pw")
    (property "Sheetname" "Power")
    (property "Sheetfile" "power.kicad_sch")))
"#;

const CHILD: &str = r#"
(kicad_sch
  (uuid "child-uuid")
  (lib_symbols (symbol "Device:C" (symbol "C_1_1"
    (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
    (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
  (symbol (lib_id "Device:C") (at 100 100 0) (unit 1) (uuid "c1")
    (property "Reference" "C1") (pin "1" (uuid "c1p1")) (pin "2" (uuid "c1p2")) (instances))
  (global_label "VBUS" (at 100 97.46 0) (uuid "g2"))
  (label "LOCAL" (at 100 102.54 0) (uuid "l2")))
"#;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("extract_hier_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn walks_child_sheet_and_merges_global_net() {
    let work = temp_dir("walk");
    std::fs::write(work.join("board.kicad_sch"), ROOT).unwrap();
    std::fs::write(work.join("power.kicad_sch"), CHILD).unwrap();

    let out = work.join("out");
    let mut log: Vec<String> = Vec::new();
    let mut emit = |m: Msg| {
        if let Msg::Progress(s) = m {
            log.push(s);
        }
    };
    run_design(&work.join("board.kicad_sch"), &out, &mut emit).expect("design");

    let design: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("board_design.json")).unwrap())
            .unwrap();

    // Two sheets were walked.
    let sheets = design["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 2);
    // The root sheet's designer note is surfaced for the reviewer.
    let root_notes = sheets[0]["notes"].as_array().unwrap();
    assert!(root_notes.iter().any(|n| n.as_str() == Some("DNP R1 on the 48V build")));

    // VBUS is a single net spanning both sheets, carrying R1 and C1.
    let vbus = design["nets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "VBUS")
        .expect("VBUS net");
    let mut refs: Vec<&str> = vbus["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["designator"].as_str().unwrap())
        .collect();
    refs.sort();
    assert_eq!(refs, vec!["C1", "R1"]);
    assert_eq!(vbus["source_sheets"].as_array().unwrap().len(), 2);

    // The child's local label is sheet-path scoped.
    assert!(design["nets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["name"] == "/Power/LOCAL"));

    // Both components survive into the model.
    let designators: Vec<&str> = design["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["designator"].as_str().unwrap())
        .collect();
    assert!(designators.contains(&"R1") && designators.contains(&"C1"));

    // The manifest lists one lean SVG per sheet, and each file exists.
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("design_review_manifest.json")).unwrap(),
    )
    .unwrap();
    let svgs = manifest["schematic_svgs"].as_array().unwrap();
    assert_eq!(svgs.len(), 2);
    for entry in svgs {
        let rel = entry["file"].as_str().unwrap();
        let body = std::fs::read_to_string(out.join(rel)).unwrap();
        assert!(body.starts_with("<svg") && body.contains("data-uuid"));
        assert!(!body.contains("<metadata"));
    }

    let _ = std::fs::remove_dir_all(&work);
}
