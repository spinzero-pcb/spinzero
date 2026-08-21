//! The detailed review's upload — and the promise about what is *not* in it.
//!
//! The plan's upload-minimization rule (§4.2) says a BOM review sends the enriched
//! BOM, its column-mapping sidecar, and a small metadata blob — and never geometry,
//! gerbers, project files or datasheets. That rule is only worth anything if the
//! app can show the user exactly what will leave the machine, so this module builds
//! the bundle as an explicit `name -> text` map that the pre-flight dialog renders
//! verbatim before anything is sent.
//!
//! Two consequences of building it here rather than in the frontend:
//!
//! * the file set is decided by code the user can read, not by whatever the UI
//!   happened to attach;
//! * `design_meta.json` carries a HASH of the project name, never the name — the
//!   service is supposed to know a job by its shape, not by the customer's board.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::design;

/// Bundle spec v1.0 (`schemas/bundle-1.0.json`). The service enforces the same list.
pub const BOM_CSV: &str = "bom_enriched.csv";
pub const MAPPING_JSON: &str = "bom_enriched.mapping.json";
pub const DESIGN_META: &str = "design_meta.json";

#[derive(Serialize)]
pub struct ReviewBundle {
    /// Bundle-relative name -> file text. This IS what gets uploaded.
    pub files: BTreeMap<String, String>,
    /// Per-file byte counts, so the dialog can show sizes without re-measuring.
    pub sizes: BTreeMap<String, usize>,
    pub bom_rows: usize,
    /// Files deliberately NOT included, with the reason — shown in the dialog.
    pub excluded: Vec<&'static str>,
}

#[derive(Serialize)]
struct DesignMeta {
    /// blake3 of the project name, truncated — identifies re-runs of the same board
    /// to nobody but this machine.
    project_hash: String,
    profile: String,
    design_tool: String,
    /// Board shape only: counts, never names.
    component_count: usize,
    bom_line_count: usize,
    /// Extraction revision id, so a finding can be traced to a design state locally.
    extraction_id: Option<String>,
    app_version: String,
}

/// Build the bundle from the extraction cache. Fails loudly if the enriched BOM is
/// absent rather than sending a lesser file: the review's quality depends on it.
pub fn build(
    cache: Option<PathBuf>,
    profile: &str,
    project_name: &str,
    design_tool: &str,
    extraction_id: Option<String>,
    component_count: usize,
) -> Result<ReviewBundle, String> {
    let dir = design::cache_dir(cache)?;
    let bom_dir = dir.join("bom");
    let csv_path = find_enriched_csv(&bom_dir)
        .ok_or("no enriched review BOM in the extraction cache — run an extraction first")?;
    let csv = std::fs::read_to_string(&csv_path).map_err(|e| format!("read enriched BOM: {e}"))?;
    let bom_rows = csv.lines().filter(|l| !l.trim().is_empty()).count().saturating_sub(1);
    if bom_rows == 0 {
        return Err("the enriched review BOM has no rows".into());
    }

    let mut files = BTreeMap::new();
    files.insert(BOM_CSV.to_string(), csv);

    // The mapping sidecar is what lets the engine tell "this BOM has no MPN column"
    // apart from "the extractor did not recognize the MPN column" — worth sending,
    // and it contains column names and coverage numbers only.
    let mapping_path = PathBuf::from(format!("{}.mapping.json", csv_path.to_string_lossy()));
    if let Ok(text) = std::fs::read_to_string(&mapping_path) {
        files.insert(MAPPING_JSON.to_string(), text);
    }

    let meta = DesignMeta {
        project_hash: blake3::hash(project_name.as_bytes()).to_hex()[..16].to_string(),
        profile: profile.to_string(),
        design_tool: design_tool.to_string(),
        component_count,
        bom_line_count: bom_rows,
        extraction_id,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    files.insert(
        DESIGN_META.to_string(),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    );

    let sizes = files.iter().map(|(k, v)| (k.clone(), v.len())).collect();
    Ok(ReviewBundle {
        files,
        sizes,
        bom_rows,
        excluded: vec![
            "schematic and PCB geometry",
            "gerbers and fabrication outputs",
            "the KiCad project and source files",
            "datasheet PDFs",
            "review comments and project history",
        ],
    })
}

/// The extractor writes `<name>_bom_enriched.csv` next to the grouped BOM.
fn find_enriched_csv(bom_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(bom_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_bom_enriched.csv"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(csv: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinzero_bundle_{}", std::process::id()));
        let bom = dir.join("bom");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&bom).expect("temp dir");
        std::fs::write(bom.join("board_bom_enriched.csv"), csv).expect("csv");
        std::fs::write(
            bom.join("board_bom_enriched.csv.mapping.json"),
            r#"{"unmapped_columns":[]}"#,
        )
        .expect("mapping");
        dir
    }

    #[test]
    fn bundle_holds_only_the_three_spec_files_and_no_project_name() {
        let dir = fixture("Reference,Value\nR1,10k\nR2,4k7\n");
        let bundle = build(Some(dir.clone()), "automotive", "Secret Board", "kicad", None, 2)
            .expect("bundle");
        let names: Vec<&String> = bundle.files.keys().collect();
        assert_eq!(names, vec![BOM_CSV, MAPPING_JSON, DESIGN_META]);
        assert_eq!(bundle.bom_rows, 2);
        let all = bundle.files.values().cloned().collect::<String>();
        assert!(
            !all.contains("Secret Board"),
            "the project name must never leave the machine — only its hash"
        );
        assert!(all.contains("automotive"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_bom_is_refused_before_anything_is_uploaded() {
        let dir = fixture("Reference,Value\n");
        assert!(build(Some(dir.clone()), "default", "b", "kicad", None, 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
