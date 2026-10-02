//! End-to-end checks on real OrCAD designs.
//!
//! The corpus is public repositories gathered for development and may not be
//! redistributed, so these skip with a message when it is absent.
//! `SPINZERO_ORCAD_CORPUS` points at the corpus root; each test names the
//! design it needs relative to it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use extract::orcad::pcb;
use extract::pipeline::{run_design, Msg};

fn corpus(rel: &str) -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("SPINZERO_ORCAD_CORPUS").ok()?);
    let p = root.join(rel);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping: {} not in the corpus", p.display());
        None
    }
}

fn out_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sz_orcad_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn design_json(out: &Path) -> serde_json::Value {
    let m: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("design_review_manifest.json")).unwrap()).unwrap();
    let f = m["design_json"].as_str().unwrap();
    serde_json::from_str(&std::fs::read_to_string(out.join(f)).unwrap()).unwrap()
}

/// The FEDEVEL tutorial: a one-page Capture project with its Allegro board.
/// Expectations are Capture's own netlist and Allegro's own placement file,
/// both shipped beside the design.
#[test]
fn the_led_tutorial_extracts_with_its_board() {
    let Some(opj) = corpus("FEDEVEL_youtube-cadence-quick-tutorial/LED_Youtube.opj") else { return };
    let out = out_dir("led");
    run_design(&opj, &out, &mut |_: Msg| {}).expect("extracts");
    let d = design_json(&out);

    let refs: BTreeSet<&str> = d["components"].as_array().unwrap().iter().map(|c| c["designator"].as_str().unwrap()).collect();
    assert_eq!(refs, ["D1", "D2", "J1", "R1", "R2"].into_iter().collect());
    let nets: BTreeSet<&str> = d["nets"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(nets, ["+3V3", "DIODE1", "DIODE2", "GND"].into_iter().collect());
    assert_eq!(d["source"]["tool"], "orcad");
    assert_eq!(d["source"]["board"]["format"], "Allegro 17.4");

    // Placements from Allegro's place_txt.txt for the same board.
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("pcb/geometry.json")).unwrap()).unwrap();
    let expect = [("D1", 6.0, -6.5, -90.0), ("D2", 16.5, -7.5, -270.0), ("J1", 12.0, -11.5, 0.0), ("R1", 6.0, -13.5, -270.0), ("R2", 16.5, -13.0, -90.0)];
    for (r, x, y, a) in expect {
        let c = g["components"].as_array().unwrap().iter().find(|c| c["ref"] == r).expect(r);
        assert!((c["x"].as_f64().unwrap() - x).abs() < 1e-3, "{r} x");
        assert!((c["y"].as_f64().unwrap() - y).abs() < 1e-3, "{r} y");
        assert!((c["angle"].as_f64().unwrap() - a).abs() < 1e-3, "{r} angle");
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// Every id the design model and the geometry name is drawn on a sheet.
#[test]
fn every_handle_resolves_on_a_hierarchical_design() {
    let Some(dsn) = corpus("beagleboard_beagleboard-xm/SCH/BeagleBoard-xM_ORCAD.DSN") else { return };
    let out = out_dir("bbxm");
    run_design(&dsn, &out, &mut |_: Msg| {}).expect("extracts");
    let d = design_json(&out);
    let mut drawn = BTreeSet::new();
    for e in std::fs::read_dir(out.join("schematics")).unwrap().flatten() {
        let t = std::fs::read_to_string(e.path()).unwrap();
        for part in t.split("data-uuid=\"").skip(1) {
            drawn.insert(part.split('"').next().unwrap().to_string());
        }
    }
    for c in d["components"].as_array().unwrap() {
        assert!(drawn.contains(c["svg_id"].as_str().unwrap()), "{} has no group", c["designator"]);
    }
    for n in d["nets"].as_array().unwrap() {
        for (_, ids) in n["graphical"].as_object().unwrap() {
            for id in ids.as_array().unwrap() {
                assert!(drawn.contains(id.as_str().unwrap()), "{} names undrawn {id}", n["name"]);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// Pad connectivity agrees with the Allegro netlist imported into the board.
#[test]
fn board_pads_carry_the_nets_of_the_allegro_netlist() {
    let Some(brd) = corpus("GreenHandLW_Mic_Array/Hardware/8Mics/allegro/8MICS_V021.brd") else { return };
    let b = pcb::load_board(&brd).unwrap();
    let ours = pcb::board_netlist(&b);
    let netlist = std::fs::read_to_string(brd.parent().unwrap().join("pstxnet.dat")).unwrap();
    let mut cur = String::new();
    let (mut seen, mut same) = (0, 0);
    let mut lines = netlist.lines();
    while let Some(l) = lines.next() {
        let l = l.trim();
        if l == "NET_NAME" {
            cur = lines.next().unwrap_or("").trim().trim_matches('\'').to_string();
        } else if let Some(rest) = l.strip_prefix("NODE_NAME") {
            let mut it = rest.split_whitespace();
            let (r, p) = (it.next().unwrap().to_string(), it.next().unwrap().to_string());
            if let Some((name, _)) = ours.iter().find(|(_, t)| t.contains(&(r.clone(), p.clone()))) {
                seen += 1;
                if name.eq_ignore_ascii_case(&cur) {
                    same += 1;
                }
            }
        }
    }
    assert!(seen >= 160, "pins found on the board: {seen}");
    assert_eq!(same, seen, "every pin on its netlist net");
}
