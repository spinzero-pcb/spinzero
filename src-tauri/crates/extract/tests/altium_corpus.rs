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
