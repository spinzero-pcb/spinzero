//! The `design` and `bom` commands for an OrCAD project.
//!
//! Only the front half is OrCAD's: everything from `design::assemble` onward —
//! the indexes, the uid numbering, the BOM mapping, the manifest — is the code
//! the KiCad and Altium paths already run.

use std::path::Path;

use crate::design::Component;
use crate::pipeline::{slug, Manifest, Msg, SchematicSvg};

use super::{board_beside, build_design, load, BoardInfo};

fn is_free_document(project: &Path) -> bool {
    !project
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("opj"))
        .unwrap_or(false)
}

pub fn load_components(project: &Path) -> Result<(String, Vec<Component>), String> {
    let mut sink = |_: Msg| {};
    if is_board_only(project) {
        let b = super::pcb::load_board(project)?;
        let name = project.file_stem().and_then(|s| s.to_str()).unwrap_or("board").to_string();
        let mut components = super::pcb::board_components(&b);
        components.sort_by(|a, b| a.designator.cmp(&b.designator));
        return Ok((name, components));
    }
    let l = load(project, &mut sink)?;
    let (model, _, _) = build_design(&l, "", "", is_free_document(project));
    Ok((l.name, model.components))
}

/// Add the board's net classes to the design model's. A net named by a real
/// class drops the implicit `Default` it was seeded with.
fn merge_classes(map: &mut std::collections::BTreeMap<String, Vec<String>>, classes: &[(String, Vec<String>)]) {
    for (class, members) in classes {
        for net in members {
            map.entry(net.clone()).or_default().push(class.clone());
        }
    }
    for v in map.values_mut() {
        v.sort();
        v.dedup();
        if v.len() > 1 {
            v.retain(|c| c != "Default");
        }
    }
}

fn is_board_only(project: &Path) -> bool {
    project.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("brd")).unwrap_or(false)
}

fn write_json<T: serde::Serialize>(path: &Path, v: &T, pretty: bool) -> Result<(), String> {
    let json = if pretty { serde_json::to_string_pretty(v) } else { serde_json::to_string(v) }
        .map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

pub fn run_design(project: &Path, out_dir: &Path, emit: &mut dyn FnMut(Msg)) -> Result<(), String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    if is_board_only(project) {
        return run_board_only(project, out_dir, emit);
    }
    let l = load(project, emit)?;
    let filename = project.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let (mut model, mut source, sheets) =
        build_design(&l, &project.to_string_lossy(), &filename, is_free_document(project));
    let u = &source.unresolved;
    emit(Msg::Progress(format!(
        "unresolved: {} missing folders, {} orphan folders, {} unconnected pins, {} buses, {} unannotated parts",
        u.missing_folders.len(),
        u.orphan_folders.len(),
        u.unconnected_pins,
        u.buses_not_expanded,
        u.unannotated_parts.len(),
    )));

    // Variants (CIS), when the design defines any.
    source.variants = super::design::variants(&l.doc, &model.components);

    // The board, when one pairs with this project.
    let mut pcb_svgs: Vec<serde_json::Value> = Vec::new();
    let pcb_geometry = match board_beside(project, l.project.as_ref(), &l.dsn) {
        Some(board) => match super::pcb::extract_board(&board, out_dir, emit) {
            Ok(a) => {
                pcb_svgs = a.svgs;
                model.theme.board = a.theme;
                merge_classes(&mut model.net_name_to_classes, &a.net_classes);
                source.board = Some(BoardInfo {
                    file: board.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    format: a.format.clone(),
                    layers: a.summary.layers,
                    components: a.summary.components,
                    nets: a.summary.nets,
                });
                Some(a.geometry)
            }
            Err(e) => {
                emit(Msg::Progress(format!("pcb skipped: {e}")));
                None
            }
        },
        None => {
            emit(Msg::Progress("no board found beside the project".to_string()));
            None
        }
    };
    model.source = serde_json::to_value(&source).ok();

    // Sheets, in Capture's own colours.
    model.theme.schematic = super::sch_svg::palette();
    let sch_dir = out_dir.join("schematics");
    std::fs::create_dir_all(&sch_dir).map_err(|e| e.to_string())?;
    let total = sheets.len() as i64;
    let mut schematic_svgs = Vec::new();
    for s in &sheets {
        let display = s.info.filename.clone();
        let file_name = format!("{:02}_{}.svg", s.info.sheet_number, slug(&display));
        let svg = super::sch_svg::render_sheet(&l.doc, s, total, &model);
        std::fs::write(sch_dir.join(&file_name), svg).map_err(|e| e.to_string())?;
        let rel = format!("schematics/{file_name}");
        emit(Msg::Artifact(rel.clone()));
        schematic_svgs.push(SchematicSvg {
            file: rel,
            sheet_number: s.info.sheet_number,
            sheet_name: display,
            sheet_path: s.info.sheet_path.clone(),
            page: s.info.page.clone(),
        });
    }
    emit(Msg::Progress(format!("schematics: {} sheet svgs", schematic_svgs.len())));

    let schematic_geometry = {
        let geom = super::sch_geom::build(&l.doc, &sheets);
        let rel = "schematics/geometry.json";
        match write_json(&sch_dir.join("geometry.json"), &geom, false) {
            Ok(()) => {
                emit(Msg::Artifact(rel.to_string()));
                Some(rel.to_string())
            }
            Err(e) => {
                emit(Msg::Progress(format!("schematic geometry skipped: {e}")));
                None
            }
        }
    };

    let design_file = format!("{}_design.json", l.name);
    write_json(&out_dir.join(&design_file), &model, true)?;
    emit(Msg::Artifact(design_file.clone()));
    let manifest = Manifest {
        schema: crate::MANIFEST_SCHEMA.to_string(),
        design_json: design_file,
        schematic_svgs,
        pcb_svgs,
        pcb_geometry,
        schematic_geometry,
    };
    write_json(&out_dir.join("design_review_manifest.json"), &manifest, true)?;
    emit(Msg::Artifact("design_review_manifest.json".to_string()));
    emit(Msg::Progress(format!(
        "design: {} sheets, {} components, {} nets",
        model.sheets.len(),
        model.components.len(),
        model.nets.len()
    )));
    Ok(())
}

/// A loose board with no schematic: the board's own components and nets make
/// the design model, so the BOM and a board review still work.
fn run_board_only(board: &Path, out_dir: &Path, emit: &mut dyn FnMut(Msg)) -> Result<(), String> {
    let a = super::pcb::extract_board(board, out_dir, emit)?;
    let b = super::pcb::load_board(board)?;
    let name = board.file_stem().and_then(|s| s.to_str()).unwrap_or("board").to_string();
    let filename = board.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let mut model = super::pcb::board_design(&b, &name, &board.to_string_lossy(), &filename);
    model.theme.board = a.theme;
    merge_classes(&mut model.net_name_to_classes, &a.net_classes);
    let source = super::SourceInfo {
        tool: "orcad".to_string(),
        compile: super::CompileSettings {
            format_version: String::new(),
            root_folder: String::new(),
            occurrence_annotated: false,
            free_document: true,
        },
        board: Some(BoardInfo {
            file: filename.clone(),
            format: a.format.clone(),
            layers: a.summary.layers,
            components: a.summary.components,
            nets: a.summary.nets,
        }),
        variants: Vec::new(),
        unresolved: super::Unresolved::default(),
    };
    model.source = serde_json::to_value(&source).ok();
    let design_file = format!("{name}_design.json");
    write_json(&out_dir.join(&design_file), &model, true)?;
    emit(Msg::Artifact(design_file.clone()));
    let manifest = Manifest {
        schema: crate::MANIFEST_SCHEMA.to_string(),
        design_json: design_file,
        schematic_svgs: Vec::new(),
        pcb_svgs: a.svgs,
        pcb_geometry: Some(a.geometry),
        schematic_geometry: None,
    };
    write_json(&out_dir.join("design_review_manifest.json"), &manifest, true)?;
    emit(Msg::Artifact("design_review_manifest.json".to_string()));
    Ok(())
}
