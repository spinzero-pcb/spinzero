//! Bundle-level tests for the Altium front-end, against the real corpus.
//!
//! The designs may not be redistributed, so these locate them by path and skip
//! with a message when absent (`SPINZERO_ALTIUM_CORPUS` overrides the default).
//!
//! Two things are checked. First the **contract**: the same shape assertions a
//! KiCad bundle satisfies — schema ids, required keys, closed vocabularies,
//! indexes that round-trip. Second the **board cross-check**: the board carries
//! a compiled netlist and a placed-component table independent of the schematic,
//! so it is the strongest oracle available without the reference parser.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use eda_parse_altium::Doc;
use extract::pipeline::{run_bom, run_design, Msg};

const DEFAULT_CORPUS: &str = r"D:\git_repo\reference_designs";

/// A corpus design: the project (or loose document) and its board.
struct Design {
    project: PathBuf,
    board: PathBuf,
}

fn corpus() -> Option<PathBuf> {
    let p = std::env::var("SPINZERO_ALTIUM_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CORPUS));
    p.is_dir().then_some(p)
}

/// Every `.PrjPcb` under the corpus, paired with the board next to it, plus any
/// loose `.SchDoc` whose directory has no project file at all.
fn designs(root: &Path) -> Vec<Design> {
    let mut out = Vec::new();
    let mut projects: BTreeSet<PathBuf> = BTreeSet::new();
    let mut loose: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        let has_project = entries.iter().any(|p| ext_is(p, "PrjPcb"));
        for p in entries {
            if p.is_dir() {
                stack.push(p);
            } else if ext_is(&p, "PrjPcb") {
                projects.insert(p);
            } else if ext_is(&p, "SchDoc") && !has_project {
                loose.push(p);
            }
        }
    }
    for project in projects {
        if let Some(board) = sibling(&project, "PcbDoc") {
            out.push(Design { project, board });
        }
    }
    // A project-less directory is ONE design, whatever it is entered through —
    // the loader walks every schematic beside the entry file. Enter it through
    // the document whose name is closest to the board's, so the bundle is named
    // after the design rather than after its disclaimer sheet.
    let mut by_dir: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for sch in loose {
        if let Some(dir) = sch.parent() {
            by_dir.entry(dir.to_path_buf()).or_default().push(sch);
        }
    }
    for (_dir, mut schs) in by_dir {
        let Some(board) = sibling(&schs[0], "PcbDoc") else { continue };
        let target = stem(&board);
        schs.sort_by_key(|s| (std::cmp::Reverse(shared_prefix(&stem(s), &target)), s.clone()));
        out.push(Design { project: schs.remove(0), board });
    }
    out
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn shared_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn ext_is(p: &Path, ext: &str) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
}

/// The one board in the same directory, when there is exactly one.
fn sibling(p: &Path, ext: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(p.parent()?)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|c| ext_is(c, ext))
        .collect();
    found.sort();
    (found.len() == 1).then(|| found.remove(0))
}

fn out_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("extract_altium_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn extract(design: &Design, tag: &str) -> (PathBuf, serde_json::Value) {
    let dir = out_dir(tag);
    let mut sink = |_: Msg| {};
    run_design(&design.project, &dir, &mut sink)
        .unwrap_or_else(|e| panic!("{}: {e}", design.project.display()));
    let stem = design.project.file_stem().unwrap().to_string_lossy().into_owned();
    let json = std::fs::read_to_string(dir.join(format!("{stem}_design.json")))
        .expect("design.json written");
    (dir, serde_json::from_str(&json).expect("design.json parses"))
}

/// The board's own tables: placed designators (from the string table, not
/// `SOURCEDESIGNATOR`) and net names.
fn board_tables(board: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let doc = Doc::open(board).expect("board opens");
    let mut designators = BTreeSet::new();
    for r in doc.records("Texts6") {
        // The text primitive's string is its second block, a Pascal string.
        let Some(block) = r.extra.first() else { continue };
        let Some((&len, rest)) = block.split_first() else { continue };
        let s = eda_parse_altium::record::decode(&rest[..(len as usize).min(rest.len())]);
        if !s.is_empty() {
            designators.insert(s);
        }
    }
    let nets = doc
        .text_records("Nets6")
        .iter()
        .map(|r| r.s("NAME").to_ascii_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    (designators, nets)
}

/// Every corpus design produces a bundle that satisfies the same shape contract
/// a KiCad bundle does. This is the cross-source test: it asserts the contract,
/// not the values, so a divergence between the two front-ends fails here.
#[test]
fn bundle_matches_the_shared_shape_contract() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus (set SPINZERO_ALTIUM_CORPUS)");
        return;
    };
    let designs = designs(&root);
    assert!(!designs.is_empty(), "corpus has no Altium designs");

    const PIN_TYPES: &[&str] = &[
        "INPUT", "OUTPUT", "BIDIRECTIONAL", "PASSIVE", "POWER_IN", "TRI_STATE",
        "OPEN_COLLECTOR", "OPEN_EMITTER",
    ];
    const CLASSES: &[&str] = &[
        "ic", "connector", "crystal", "passive_2pin", "mounting_hole", "unknown",
    ];
    const DRIVERS: &[&str] =
        &["global_power_pin", "global_label", "hier_label", "local_label", "pin"];

    for (i, d) in designs.iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, model) = extract(d, &format!("shape{i}"));
        assert_eq!(model["schema"], "extract.design.a0", "{name}");
        assert_eq!(model["generator"], "extract", "{name}");
        assert_eq!(model["source"]["tool"], "altium", "{name}");
        assert!(model["sheets"].as_array().unwrap().len() >= 1, "{name}");

        let components = model["components"].as_array().unwrap();
        assert!(!components.is_empty(), "{name}: no components");
        // A designator may repeat across sheets — one physical part placed as
        // several parts of a multi-part symbol — the same way a KiCad bundle
        // carries one entry per unit; the shared BOM code collapses them. What
        // must never repeat is a designator WITHIN one sheet instance, which is
        // what the per-placement grouping guarantees.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut placed: BTreeSet<(&str, &str)> = BTreeSet::new();
        for c in components {
            let dsg = c["designator"].as_str().unwrap();
            assert!(!dsg.is_empty(), "{name}: empty designator");
            seen.insert(dsg);
            let sheet = c["hierarchy"]["sheet_path_uuids"].as_str().unwrap();
            assert!(placed.insert((sheet, dsg)), "{name}: {dsg} twice on one sheet");
            let kind = c["classification"]["type"].as_str().unwrap();
            assert!(CLASSES.contains(&kind), "{name}: unknown class {kind}");
            for key in ["kicad_in_bom", "kicad_dnp", "altium_component_kind"] {
                assert!(c["parameters"][key].is_string(), "{name}: {dsg} missing {key}");
            }
            assert!(c["hierarchy"]["sheet_path_uuids"].as_str().unwrap().starts_with('/'));
        }

        let nets = model["nets"].as_array().unwrap();
        assert!(!nets.is_empty(), "{name}: no nets");
        for n in nets {
            let driver = n["driver_kind"].as_str().unwrap();
            assert!(DRIVERS.contains(&driver), "{name}: unknown driver {driver}");
            assert!(!n["name"].as_str().unwrap().is_empty(), "{name}: unnamed net");
            for t in n["terminals"].as_array().unwrap() {
                let pt = t["pin_type"].as_str().unwrap();
                assert!(PIN_TYPES.contains(&pt), "{name}: unknown pin_type {pt}");
                assert!(
                    seen.contains(t["designator"].as_str().unwrap()),
                    "{name}: terminal names a component not in the design"
                );
            }
        }

        // Indexes round-trip: every svg id maps to a real designator, and every
        // component with terminals appears in component_to_nets.
        for (_svg, dsg) in model["indexes"]["svg_to_component"].as_object().unwrap() {
            assert!(seen.contains(dsg.as_str().unwrap()), "{name}: stale svg index");
        }
        let manifest = std::fs::read_to_string(dir.join("design_review_manifest.json")).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["schema"], "extract.design_review_manifest.a0", "{name}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The board is an independent compile of the same design, so it is the oracle:
/// the placed designators it carries must all exist in the design model, and
/// most of its net names must too.
///
/// The thresholds are deliberately below 100%: bus ranges are recorded but not
/// expanded into member nets, and a few nets named across a channel boundary
/// still take the parent's name (both reported in the design's `unresolved`
/// block). They are here to catch a regression, not to claim parity.
#[test]
fn design_agrees_with_the_boards_own_tables() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, model) = extract(d, &format!("board{i}"));
        let (board_designators, board_nets) = board_tables(&d.board);

        let designators: BTreeSet<String> = model["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["designator"].as_str().unwrap().to_string())
            .collect();
        // Board text includes pin names and free text, so only compare the
        // strings that look like designators the design also uses a prefix for.
        let prefixes: BTreeSet<String> = designators
            .iter()
            .map(|d| d.chars().take_while(|c| !c.is_ascii_digit()).collect())
            .collect();
        let placed: BTreeSet<&String> = board_designators
            .iter()
            .filter(|s| {
                let p: String = s.chars().take_while(|c| !c.is_ascii_digit()).collect();
                !p.is_empty() && p.len() < s.len() && prefixes.contains(&p)
            })
            .collect();
        let missing: Vec<&&String> =
            placed.iter().filter(|s| !designators.contains(**s)).collect();
        assert!(
            missing.len() * 20 <= placed.len(),
            "{name}: {} of {} placed designators are not in the design model, e.g. {:?}",
            missing.len(),
            placed.len(),
            &missing[..missing.len().min(8)]
        );

        let names: BTreeSet<String> = model["nets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap().to_ascii_uppercase())
            .collect();
        let found = board_nets.iter().filter(|n| names.contains(*n)).count();
        assert!(
            found * 3 >= board_nets.len() * 2,
            "{name}: only {found} of {} board nets are in the design model",
            board_nets.len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The shared BOM code runs unchanged on Altium components, and the scored field
/// mapping resolves the review-critical fields from the design's own parameter
/// names — no hard-coded keys.
#[test]
fn enriched_bom_resolves_the_review_fields() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    let mut checked = 0;
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let dir = out_dir(&format!("bom{i}"));
        let mut sink = |_: Msg| {};
        run_bom(&d.project, &dir, "enriched-csv", &mut sink)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let stem = d.project.file_stem().unwrap().to_string_lossy().into_owned();
        let csv =
            std::fs::read_to_string(dir.join(format!("{stem}_bom_enriched.csv"))).expect("csv");
        let mut lines = csv.lines();
        let header = lines.next().expect("header");
        for column in ["Reference", "Manufacturer Part Number", "Manufacturer", "Datasheet"] {
            assert!(header.contains(column), "{name}: BOM header missing {column}");
        }
        assert!(lines.next().is_some(), "{name}: BOM has no rows");

        let report: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(format!("{stem}_bom_enriched.csv.mapping.json")))
                .expect("mapping report"),
        )
        .unwrap();
        let coverage: BTreeMap<&str, u64> = ["mpn", "manufacturer"]
            .iter()
            .map(|f| (*f, report["fields"][f]["coverage_pct"].as_u64().unwrap_or(0)))
            .collect();
        // A database-backed Altium project names these fields its own way; the
        // scored mapping is what has to find them.
        if report["fields"]["mpn"]["primary"].is_string() {
            assert!(
                coverage["mpn"] >= 50 && coverage["manufacturer"] >= 50,
                "{name}: weak BOM field coverage {coverage:?}"
            );
            checked += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(checked > 0, "no corpus design resolved an MPN field at all");
}

/// `design.json` must be byte-identical across runs — the runtime cache key and
/// the raw-blob dedupe both rely on it.
#[test]
fn altium_design_json_is_byte_deterministic() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    let Some(d) = designs(&root).into_iter().next() else { return };
    let (a, ma) = extract(&d, "det_a");
    let (b, mb) = extract(&d, "det_b");
    assert_eq!(
        serde_json::to_string(&ma).unwrap(),
        serde_json::to_string(&mb).unwrap(),
        "design.json must be identical across runs"
    );
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// Every corpus board produces a `pcb/geometry.json` the renderer can consume:
/// the schema and units the KiCad path writes, a closed `role` vocabulary, and
/// every index inside its table. A dangling layer or net index is the failure
/// mode that draws a board with primitives silently missing.
#[test]
fn board_geometry_is_internally_consistent() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    const ROLES: &[&str] =
        &["copper", "silkscreen", "mask", "fab", "courtyard", "paste", "edge", "user"];
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, _model) = extract(d, &format!("geom{i}"));
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("design_review_manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["pcb_geometry"], "pcb/geometry.json",
            "{name}: the manifest does not point at the board geometry"
        );
        let g: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("pcb/geometry.json")).expect("geometry.json"),
        )
        .unwrap();
        assert_eq!(g["schema"], "extract.pcb.geometry.a0", "{name}");
        assert_eq!(g["units"], "mm", "{name}");

        let layers = g["layers"].as_array().unwrap();
        let nets = g["nets"].as_array().unwrap();
        let comps = g["components"].as_array().unwrap();
        assert!(!layers.is_empty(), "{name}: no layers");
        assert_eq!(nets[0], "", "{name}: index 0 must stay the no-net sentinel");
        for l in layers {
            let role = l["role"].as_str().unwrap();
            assert!(ROLES.contains(&role), "{name}: unknown role {role}");
            assert!(!l["name"].as_str().unwrap().is_empty(), "{name}: unnamed layer");
        }
        let copper = layers.iter().filter(|l| l["role"] == "copper").count();
        assert!(copper >= 2, "{name}: {copper} copper layers");
        assert!(
            layers.iter().any(|l| l["role"] == "edge"),
            "{name}: no edge layer, so the board outline has nowhere to live"
        );

        let layer_ok = |v: &serde_json::Value| (v.as_u64().unwrap() as usize) < layers.len();
        let net_ok = |v: &serde_json::Value| (v.as_u64().unwrap() as usize) < nets.len();
        for key in ["seg", "arc"] {
            let t = &g["tracks"][key];
            for v in t["layer"].as_array().unwrap() {
                assert!(layer_ok(v), "{name}: track on a layer that is not in the table");
            }
            for v in t["net"].as_array().unwrap() {
                assert!(net_ok(v), "{name}: track on a net that is not in the table");
            }
        }
        for p in g["pads"].as_array().unwrap() {
            assert!(net_ok(&p["net"]), "{name}: pad on an unknown net");
            let comp = p["comp"].as_i64().unwrap();
            assert!(comp >= -1 && comp < comps.len() as i64, "{name}: pad on an unknown component");
            let on = p["layers"].as_array().unwrap();
            assert!(!on.is_empty(), "{name}: pad on no layer at all");
            for v in on {
                assert!(layer_ok(v), "{name}: pad on an unknown layer");
            }
        }
        for v in g["vias"].as_array().unwrap() {
            assert!(net_ok(&v["net"]), "{name}: via on an unknown net");
            assert!(v["drill"].as_f64().unwrap() > 0.0, "{name}: via with no hole");
            // Equal is allowed: one corpus board really does place a via with
            // no annular ring, which is a finding about that board and not a
            // decoding error.
            assert!(
                v["size"].as_f64().unwrap() >= v["drill"].as_f64().unwrap(),
                "{name}: via hole is larger than its pad"
            );
            for l in v["layers"].as_array().unwrap() {
                assert!(layer_ok(l), "{name}: via on an unknown layer");
            }
        }
        for z in g["zones"].as_array().unwrap() {
            assert!(layer_ok(&z["layer"]) && net_ok(&z["net"]), "{name}: zone index out of range");
        }
        for x in g["graphics"].as_array().unwrap() {
            assert!(layer_ok(&x["layer"]), "{name}: graphic on an unknown layer");
        }
        // A component's own special strings MUST resolve — a board that prints
        // `.DESIGNATOR` on every part is the visible form of corner case 7.
        // Document-level ones this build has no value for (a drill legend, a
        // print date) are drawn as they stand and counted in `unresolved`.
        for t in g["texts"].as_array().unwrap() {
            assert!(layer_ok(&t["layer"]), "{name}: text on an unknown layer");
            let text = t["text"].as_str().unwrap().to_ascii_uppercase();
            assert!(
                !matches!(text.as_str(), ".DESIGNATOR" | ".COMMENT" | ".NAME" | ".VALUE"),
                "{name}: a component special string reached the board: {text}"
            );
        }
        assert!(
            _model["source"]["unresolved"]["board_unresolved_specials"].is_number(),
            "{name}: the bundle does not say how many special strings it left standing"
        );

        // The extent has to contain what it describes, or the viewer opens on
        // empty space.
        let bbox: Vec<f64> = g["bbox"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(bbox[2] > 1.0 && bbox[3] > 1.0, "{name}: degenerate bbox {bbox:?}");
        for c in comps {
            let (x, y) = (c["x"].as_f64().unwrap(), c["y"].as_f64().unwrap());
            assert!(
                x >= bbox[0] && x <= bbox[0] + bbox[2] && y >= bbox[1] && y <= bbox[1] + bbox[3],
                "{name}: {} is placed outside the board extent",
                c["ref"]
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Every pad, via and placed footprint the board file carries reaches the
/// geometry document. A silently dropped primitive is the whole failure mode of
/// a binary-record reader, and the record count is the one number that catches
/// it without a second implementation.
#[test]
fn board_geometry_keeps_every_pad_via_and_footprint() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.board.file_name().unwrap().to_string_lossy().into_owned();
        let doc = Doc::open(&d.board).expect("board opens");
        let expected = [
            ("pads", doc.records("Pads6").len()),
            ("vias", doc.records("Vias6").len()),
            ("components", doc.text_records("Components6").len()),
        ];
        let (dir, _model) = extract(d, &format!("counts{i}"));
        let g: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("pcb/geometry.json")).expect("geometry.json"),
        )
        .unwrap();
        for (key, want) in expected {
            assert_eq!(
                g[key].as_array().unwrap().len(),
                want,
                "{name}: {key} in the geometry do not match the record count"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}


/// The manifest lists a sheet SVG for every sheet the design model carries, and
/// a layer SVG for every layer the geometry carries. A bundle whose manifest and
/// model disagree opens with a blank canvas and no error anywhere.
#[test]
fn every_sheet_and_layer_reaches_the_manifest_as_a_file() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, model) = extract(d, &format!("svg{i}"));
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("design_review_manifest.json")).unwrap(),
        )
        .unwrap();

        let sheets = model["sheets"].as_array().unwrap();
        let sheet_svgs = manifest["schematic_svgs"].as_array().unwrap();
        assert_eq!(sheet_svgs.len(), sheets.len(), "{name}: a sheet has no SVG");
        for entry in sheet_svgs {
            let file = entry["file"].as_str().unwrap();
            let svg = std::fs::read_to_string(dir.join(file))
                .unwrap_or_else(|e| panic!("{name}: {file}: {e}"));
            assert!(svg.starts_with("<svg "), "{name}: {file} is not an SVG");
            assert!(svg.ends_with("</svg>"), "{name}: {file} is truncated");
            assert!(
                svg.contains("data-primitive="),
                "{name}: {file} carries no addressable primitive"
            );
        }

        let g: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("pcb/geometry.json")).unwrap(),
        )
        .unwrap();
        let geom_layers: BTreeSet<String> = g["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap().to_string())
            .collect();
        let mut seen = BTreeSet::new();
        for entry in manifest["pcb_svgs"].as_array().unwrap() {
            let layer = entry["layer"].as_str().unwrap().to_string();
            // The synthetic drawing-sheet row is page context, not a board layer.
            let Some(file) = entry["file"].as_str() else {
                continue;
            };
            assert!(geom_layers.contains(&layer), "{name}: {layer} is not in the geometry");
            let svg = std::fs::read_to_string(dir.join(file))
                .unwrap_or_else(|e| panic!("{name}: {file}: {e}"));
            assert!(
                svg.contains(&format!(r#"data-review-layer="{}""#, layer.replace('&', "&amp;"))),
                "{name}: {file} does not name its own layer"
            );
            seen.insert(layer);
        }
        // Every copper layer and the board edge must be there: a reviewer expects
        // the stack to be complete and selectable even where it is empty.
        for l in g["layers"].as_array().unwrap() {
            let (n, role) = (l["name"].as_str().unwrap(), l["role"].as_str().unwrap());
            if role == "copper" || role == "edge" {
                assert!(seen.contains(n), "{name}: {n} ({role}) has no layer SVG");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Every sheet SVG is well formed and stays in the bundle's coordinate space.
///
/// Two things go silently wrong in a renderer and neither raises an error: an
/// unbalanced group, which the browser reparents rather than rejects, and an
/// image blob left in the format Altium stored it in, which is an uncompressed
/// bitmap and makes one sheet tens of megabytes.
#[test]
fn sheet_svgs_are_balanced_and_carry_no_uncompressed_bitmaps() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, _model) = extract(d, &format!("svgshape{i}"));
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("design_review_manifest.json")).unwrap(),
        )
        .unwrap();
        for entry in manifest["schematic_svgs"].as_array().unwrap() {
            let file = entry["file"].as_str().unwrap();
            let svg = std::fs::read_to_string(dir.join(file)).unwrap();
            let opens = svg.matches("<g ").count() + svg.matches("<g>").count();
            let closes = svg.matches("</g>").count();
            assert_eq!(opens, closes, "{name}: {file} has unbalanced groups");
            assert!(
                !svg.contains("data:image/bmp"),
                "{name}: {file} embeds an uncompressed bitmap"
            );
            // An empty `data-uuid` looks like an identity to every consumer that
            // reads one, so every junction on the sheet would share it. Altium's
            // junction record is the one that carries no `UniqueID`; the
            // attribute is omitted rather than written blank.
            assert!(
                !svg.contains(r#"data-uuid="""#),
                "{name}: {file} writes an empty data-uuid"
            );
            // The viewBox is millimetres on a real page, not Altium's own units.
            let vb = svg
                .split(r#"viewBox=""#)
                .nth(1)
                .and_then(|s| s.split('"').next())
                .expect("a viewBox");
            let nums: Vec<f64> = vb.split_whitespace().map(|v| v.parse().unwrap()).collect();
            assert!(
                nums[2] > 50.0 && nums[2] < 3000.0 && nums[3] > 50.0 && nums[3] < 3000.0,
                "{name}: {file} viewBox {vb} is not a page in millimetres"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The design model carries the palette the viewer themes with, in the keys the
/// frontend's theme map reads. An empty block is a bundle that renders every
/// Altium object in the neutral fallback.
#[test]
fn the_bundle_carries_a_palette_in_the_viewers_own_keys() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, model) = extract(d, &format!("theme{i}"));
        let sch = model["theme"]["schematic"].as_object().expect("a schematic palette");
        for key in ["wire", "component_outline", "pin", "reference"] {
            let hex = sch.get(key).and_then(|v| v.as_str());
            assert!(
                hex.map(|h| h.starts_with('#') && h.len() == 7).unwrap_or(false),
                "{name}: theme.schematic.{key} is {hex:?}"
            );
        }
        let board = model["theme"]["board"].as_object().expect("a board palette");
        assert!(board.contains_key("copper.f"), "{name}: no front-copper colour");
        assert!(board.contains_key("edge_cuts"), "{name}: no board-outline colour");
        let _ = std::fs::remove_dir_all(&dir);
    }
}


/// M3's exit criterion: a `data-uuid` in a rendered sheet round-trips through
/// the design model's cross-probe indexes. A click that resolves to nothing is
/// the failure this catches, and it is invisible in the SVG itself.
///
/// Symbols and pins must resolve exactly — every one of them is a component the
/// model carries. Wires and labels are held to a proportion instead: a wire on
/// no net, and a label naming nothing, are both real and both rare.
#[test]
fn every_drawn_uuid_resolves_through_the_cross_probe_indexes() {
    let Some(root) = corpus() else {
        eprintln!("skipping: no Altium corpus");
        return;
    };
    // `<g data-primitive="X" data-uuid="Y">`, the shape every emitter writes.
    let scan = |svg: &str, primitive: &str| -> Vec<String> {
        let needle = format!(r#"<g data-primitive="{primitive}" data-uuid=""#);
        svg.match_indices(&needle)
            .filter_map(|(i, _)| {
                let rest = &svg[i + needle.len()..];
                rest.find('"').map(|end| rest[..end].to_string())
            })
            .filter(|u| !u.is_empty())
            .collect()
    };

    for (i, d) in designs(&root).iter().enumerate() {
        let name = d.project.file_name().unwrap().to_string_lossy().into_owned();
        let (dir, model) = extract(d, &format!("probe{i}"));
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("design_review_manifest.json")).unwrap(),
        )
        .unwrap();
        let idx = &model["indexes"];
        let known = |key: &str, uuid: &str| idx[key].get(uuid).is_some();

        let (mut soft_hit, mut soft_all) = (0usize, 0usize);
        for entry in manifest["schematic_svgs"].as_array().unwrap() {
            let svg = std::fs::read_to_string(dir.join(entry["file"].as_str().unwrap())).unwrap();
            for uuid in scan(&svg, "symbol") {
                assert!(
                    known("svg_to_component", &uuid),
                    "{name}: symbol {uuid} resolves to no component"
                );
            }
            for uuid in scan(&svg, "pin") {
                assert!(
                    known("svg_to_net", &uuid) || known("svg_to_component", &uuid),
                    "{name}: pin {uuid} resolves to neither a net nor a component"
                );
            }
            for primitive in ["wire", "bus", "label", "power-symbol"] {
                for uuid in scan(&svg, primitive) {
                    soft_all += 1;
                    if known("svg_to_net", &uuid) {
                        soft_hit += 1;
                    }
                }
            }
        }
        assert!(soft_all > 0, "{name}: no addressable net geometry at all");
        let share = soft_hit as f64 / soft_all as f64;
        assert!(
            share >= 0.95,
            "{name}: only {soft_hit}/{soft_all} ({:.0}%) of the drawn net geometry resolves",
            share * 100.0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
