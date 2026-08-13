//! Drives the extraction commands end to end: parse the source files, build the
//! design model, and write the bundle the app and review skills consume.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use eda_parse_kicad::pcb::Pcb;
use eda_parse_kicad::Schematic;
use serde::Serialize;

use crate::bom;
use crate::design::{self, Component, SheetInfo};

/// Progress messages surfaced to the caller (CLI prints NDJSON; the app turns
/// these into crunch events).
pub enum Msg {
    Progress(String),
    Artifact(String),
}

/// Manifest entry for a rendered schematic sheet.
#[derive(Serialize)]
struct SchematicSvg {
    file: String,
    sheet_number: i64,
    sheet_name: String,
    sheet_path: String,
    /// KiCad page label (`(page "N")`); empty when the project uses automatic
    /// numbering, so the viewer falls back to `sheet_number`.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    page: String,
}

/// The bundle manifest the app reads.
#[derive(Serialize)]
struct Manifest {
    schema: String,
    design_json: String,
    schematic_svgs: Vec<SchematicSvg>,
    pcb_svgs: Vec<serde_json::Value>,
    /// Cache-relative path of the structured PCB geometry IR (the GPU renderer's
    /// input). Absent for schematic-only bundles or older extractions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pcb_geometry: Option<String>,
    /// Cache-relative path of the per-element schematic geometry (the diff engine's
    /// input for splitting + anchoring graphical edits). Absent for older extractions.
    #[serde(skip_serializing_if = "Option::is_none")]
    schematic_geometry: Option<String>,
}

/// Resolve the root schematic path for a project argument (`.kicad_pro` or a
/// `.kicad_sch` directly).
fn root_schematic(project: &Path) -> Result<PathBuf, String> {
    let sch = if project.extension().and_then(|e| e.to_str()) == Some("kicad_sch") {
        project.to_path_buf()
    } else {
        project.with_extension("kicad_sch")
    };
    if sch.exists() {
        Ok(sch)
    } else {
        Err(format!("root schematic not found: {}", sch.display()))
    }
}

/// One parsed sheet instance: its place in the hierarchy plus the schematic.
struct LoadedSheet {
    info: SheetInfo,
    sch: Schematic,
}

/// Maximum hierarchy depth walked — a backstop against pathological cycles.
const MAX_SHEET_DEPTH: usize = 32;

/// Parse a sheet file and recurse into its child sheets, depth-first, assigning
/// sequential sheet numbers. Read/parse failures below the root are skipped (a
/// missing generated subsheet shouldn't sink the whole extraction); the root's
/// success is enforced by the caller.
fn add_sheet(
    root_uuid: &str,
    page: &str,
    sch_path: &Path,
    sheet_path: &str,
    sheet_path_uuids: &str,
    out: &mut Vec<LoadedSheet>,
    counter: &mut i64,
    visited: &mut HashSet<String>,
    depth: usize,
    emit: &mut dyn FnMut(Msg),
) {
    if depth > MAX_SHEET_DEPTH || !visited.insert(sheet_path_uuids.to_string()) {
        return;
    }
    let Ok(src) = std::fs::read_to_string(sch_path) else {
        emit(Msg::Progress(format!("skipped (unreadable): {}", sch_path.display())));
        return;
    };
    let Ok(sch) = Schematic::parse_str(&src) else {
        emit(Msg::Progress(format!("skipped (parse error): {}", sch_path.display())));
        return;
    };
    emit(Msg::Progress(format!("parsed {}", sch_path.display())));

    *counter += 1;
    let info = SheetInfo {
        filename: sch_path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string(),
        path: sch_path.to_string_lossy().into_owned(),
        sheet_number: *counter,
        sheet_path: sheet_path.to_string(),
        sheet_path_uuids: sheet_path_uuids.to_string(),
        title: sch.title.clone().unwrap_or_default(),
        // KiCad ≥7 page label for this placement — the parent passes it in (resolved
        // from the child's `(instances …)`). The root and any unlabelled sheet are
        // filled from the root's `sheet_instances` after the walk, else fall back to
        // the sequential sheet number in the viewer.
        page: page.to_string(),
        notes: sch.notes.iter().map(|t| t.text.clone()).collect(),
        company: sch.company.clone().unwrap_or_default(),
        rev: sch.rev.clone().unwrap_or_default(),
        date: sch.date.clone().unwrap_or_default(),
    };
    // KiCad instance paths begin with the *root* schematic's own uuid. The root call
    // can't know it yet (passes ""), so adopt this file's uuid at depth 0 and thread
    // it down; deeper calls keep the value handed in.
    let root_uuid = if depth == 0 {
        sch.uuid.clone().unwrap_or_default()
    } else {
        root_uuid.to_string()
    };
    let parent_path = kicad_instance_path(&root_uuid, sheet_path_uuids);
    let children = sch.sheets.clone();
    let dir = sch_path.parent().map(Path::to_path_buf).unwrap_or_default();
    out.push(LoadedSheet { info, sch });

    for child in &children {
        if child.file.is_empty() {
            continue;
        }
        let child_path = dir.join(&child.file);
        let child_human = format!("{sheet_path}{}/", child.name);
        let child_uuids = format!("{sheet_path_uuids}{}/", child.uuid);
        // This child placement's page label is keyed by the parent's (this sheet's)
        // full KiCad instance path inside the child's `(instances …)` block — so a
        // re-used sheet picks the right page for each instance context.
        let child_page = child
            .instances
            .iter()
            .find(|sp| sp.path == parent_path)
            .map(|sp| sp.page.as_str())
            .unwrap_or("");
        add_sheet(
            &root_uuid,
            child_page,
            &child_path,
            &child_human,
            &child_uuids,
            out,
            counter,
            visited,
            depth + 1,
            emit,
        );
    }
}

/// Load every sheet in a project's hierarchy (root first, then DFS children).
fn load_hierarchy(
    project: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<(String, Vec<LoadedSheet>), String> {
    let name = project
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("invalid project path")?
        .to_string();
    let root_path = root_schematic(project)?;
    let mut out = Vec::new();
    let mut counter = 0;
    let mut visited = HashSet::new();
    add_sheet("", "", &root_path, "/", "/", &mut out, &mut counter, &mut visited, 0, emit);
    if out.is_empty() {
        return Err(format!("failed to parse root schematic: {}", root_path.display()));
    }

    // Fallback page labels from the root's `sheet_instances` block, for the root
    // sheet itself and any sheet the per-placement walk left unlabelled (e.g. older
    // KiCad files that store every page number here instead of per placement). Only
    // fills empty pages, so a per-placement label always wins. Designs on automatic
    // numbering have no useful map (only the root's own `/`), so children stay empty
    // and the viewer falls back to the sequential sheet number.
    let page_map: BTreeMap<String, String> = out
        .first()
        .map(|root| {
            root.sch
                .sheet_instances
                .iter()
                .filter(|p| !p.page.is_empty())
                .map(|p| (normalize_instance_path(&p.path), p.page.clone()))
                .collect()
        })
        .unwrap_or_default();
    if !page_map.is_empty() {
        for s in out.iter_mut().filter(|s| s.info.page.is_empty()) {
            if let Some(pg) = page_map.get(&normalize_instance_path(&s.info.sheet_path_uuids)) {
                s.info.page = pg.clone();
            }
        }
    }

    // Project text variables (`${PCB_REVISION}` &c.) are a project-file concept, so the
    // parser can't resolve them — do it once here, before anything reads a sheet. That
    // way the expanded text reaches the design model, the BOM and the rendered SVG
    // alike (the board self-resolves in the parser: it mirrors the vars into itself).
    let vars = read_text_vars(&project.with_extension("kicad_pro"));
    if !vars.is_empty() {
        for s in &mut out {
            expand_sheet_text_vars(&mut s.sch, &vars);
            // SheetInfo copies the title-block/note text out of the sheet, so refresh
            // the copies from the now-expanded source.
            s.info.title = s.sch.title.clone().unwrap_or_default();
            s.info.company = s.sch.company.clone().unwrap_or_default();
            s.info.rev = s.sch.rev.clone().unwrap_or_default();
            s.info.date = s.sch.date.clone().unwrap_or_default();
            s.info.notes = s.sch.notes.iter().map(|t| t.text.clone()).collect();
        }
    }
    Ok((name, out))
}

/// Expand `${KEY}` project text variables in place, everywhere KiCad substitutes them
/// on a schematic sheet: symbol fields (which is what the BOM reads), free text notes
/// and the title block. Deliberately NOT applied to labels or sheet names — those name
/// nets and hierarchy paths, and expanding them would silently re-key connectivity.
fn expand_sheet_text_vars(sch: &mut Schematic, vars: &BTreeMap<String, String>) {
    for sym in &mut sch.symbols {
        for p in &mut sym.properties {
            if p.value.contains("${") {
                p.value = expand_text_vars(&p.value, vars);
            }
        }
    }
    for n in &mut sch.notes {
        if n.text.contains("${") {
            n.text = expand_text_vars(&n.text, vars);
        }
    }
    for f in [&mut sch.title, &mut sch.company, &mut sch.rev, &mut sch.date] {
        if let Some(s) = f.as_mut() {
            if s.contains("${") {
                *s = expand_text_vars(s, vars);
            }
        }
    }
    for c in &mut sch.comments {
        if c.contains("${") {
            *c = expand_text_vars(c, vars);
        }
    }
}

/// The KiCad instance path of a sheet, rebuilt from the walker's `sheet_path_uuids`.
/// KiCad prefixes the root schematic's own uuid and drops the trailing slash, so the
/// walker's `/` (root) becomes `/<root_uuid>` and `/A/B/` becomes `/<root_uuid>/A/B`
/// — the exact form a child placement's `(instances … (path …))` uses to key its page.
fn kicad_instance_path(root_uuid: &str, walker_path: &str) -> String {
    format!("/{}{}", root_uuid, walker_path.trim_end_matches('/'))
}

/// Normalise an instance-path key so the walker's `sheet_path_uuids` (which appends
/// a trailing `/`) matches KiCad's `sheet_instances` paths (`/` for the root,
/// `/uuid/uuid` for descendants — no trailing slash). The root stays `/`.
fn normalize_instance_path(p: &str) -> String {
    if p == "/" {
        "/".to_string()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

/// Human display name for a sheet: the last segment of its path, else its title,
/// else the file stem.
fn sheet_display_name(info: &SheetInfo) -> String {
    let last = info.sheet_path.trim_matches('/').rsplit('/').next().unwrap_or("");
    if !last.is_empty() {
        return last.to_string();
    }
    if !info.title.is_empty() {
        return info.title.clone();
    }
    info.filename.trim_end_matches(".kicad_sch").to_string()
}

/// Filesystem-safe slug for a sheet file name.
fn slug(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' { ch } else { '_' })
        .collect();
    let trimmed = s.trim_matches('_');
    if trimmed.is_empty() { "sheet".into() } else { trimmed.to_string() }
}

/// Run the `design` command: walk the full sheet hierarchy and emit
/// `<name>_design.json` plus the manifest.
///
/// Schematic/PCB SVG rendering is layered in next; the manifest's SVG lists are
/// empty until then, which the app tolerates.
pub fn run_design(project: &Path, out_dir: &Path, emit: &mut dyn FnMut(Msg)) -> Result<(), String> {
    let (name, sheets) = load_hierarchy(project, emit)?;
    let refs: Vec<(SheetInfo, &Schematic)> =
        sheets.iter().map(|s| (s.info.clone(), &s.sch)).collect();
    // Harvest the board's canonical net names (when a board sits next to the
    // project) so the design model adopts KiCad's own net namespace — this is
    // what makes schematic↔PCB cross-probe and net-class colouring line up.
    let pcb_path = project.with_extension("kicad_pcb");
    let pcb_pad_net = read_pcb_pad_net(&pcb_path);
    if !pcb_pad_net.is_empty() {
        emit(Msg::Progress(format!("board nets: {} pad assignments harvested", pcb_pad_net.len())));
    }
    let mut model =
        design::build_design_multi(&name, &project.to_string_lossy(), &refs, &pcb_pad_net);

    // Resolve each net's real net class from the project's class rules (patterns +
    // per-net assignments) before serialising — `build_design_multi` only seeds the
    // implicit "Default". Without this every net's card reads "Default" even when the
    // project puts it in 50-ohm / Isolated / etc.
    let net_classes = crate::netclass::NetClassColors::from_pro(&project.with_extension("kicad_pro"));
    if net_classes.has_classes() {
        let resolved: Vec<(String, Vec<String>)> = model
            .nets
            .iter()
            .filter_map(|net| {
                let classes = net_classes.classes_for(&net.name);
                (!classes.is_empty()).then(|| (net.name.clone(), classes))
            })
            .collect();
        let reclassed = resolved.len();
        for (name, classes) in resolved {
            model.net_name_to_classes.insert(name, classes);
        }
        emit(Msg::Progress(format!("net classes: {reclassed} nets assigned a non-default class")));
    }

    // Derive the colour palette from the user's active KiCad theme and the
    // project's drawing defaults, so the viewer themes the monochrome SVGs with
    // KiCad's real colours instead of a hand-mirrored copy. When no KiCad config is
    // reachable the palette stays empty and the viewer keeps its KiCad-Default fallback.
    model.theme = crate::theme::load();
    model.drawing = crate::theme::drawing(&project.with_extension("kicad_pro"));
    if !model.theme.is_empty() {
        emit(Msg::Progress(format!(
            "theme: {} schematic + {} board colours from KiCad",
            model.theme.schematic.len(),
            model.theme.board.len()
        )));
    }
    let sch_style = crate::svg::SchStyle::from_kicad(&model.theme, &model.drawing);

    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let design_file = format!("{name}_design.json");
    let design_path = out_dir.join(&design_file);
    let json = serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?;
    std::fs::write(&design_path, json).map_err(|e| e.to_string())?;
    emit(Msg::Artifact(design_file.clone()));

    // Resolve per-net schematic colours from the project's net classes, then map
    // each net's graphical element uuids to its colour so the SVGs can paint
    // wires/labels/pins the way KiCad does (e.g. the CAN bus in its net colour).
    let net_colors = build_net_colors(&net_classes, &model, &sheets, emit);

    // Lean per-sheet SVGs, keyed back to sheets via the manifest.
    let sch_dir = out_dir.join("schematics");
    std::fs::create_dir_all(&sch_dir).map_err(|e| e.to_string())?;
    let mut schematic_svgs = Vec::new();
    // Title-block fields are per-sheet in KiCad — a sub-sheet shows ONLY its own
    // fields (an empty title stays empty; it does NOT inherit the root's). Project
    // text variables were already expanded by `load_hierarchy`.
    let total = sheets.len() as i64;
    for s in &sheets {
        let display = sheet_display_name(&s.info);
        let file_name = format!("{:02}_{}.svg", s.info.sheet_number, slug(&display));
        let own = |f: &Option<String>| f.clone().unwrap_or_default();
        let (title, company, rev, date) =
            (own(&s.sch.title), own(&s.sch.company), own(&s.sch.rev), own(&s.sch.date));
        let version = s.sch.generator_version.clone().unwrap_or_default();
        let comments: Vec<String> = s.sch.comments.clone();
        let frame = crate::svg::SheetFrame {
            number: s.info.sheet_number,
            page: &s.info.page,
            total,
            // KiCad's "Sheet:" line is the hierarchical path, "File:" the source file.
            name: &s.info.sheet_path,
            file: &s.info.filename,
            title: &title,
            company: &company,
            rev: &rev,
            date: &date,
            version: &version,
            comments: &comments,
        };
        let svg = crate::svg::render_sheet(&s.sch, &net_colors, Some(frame), &sch_style);
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

    // Per-element schematic geometry (the diff engine's input for splitting +
    // anchoring graphical edits). Best-effort: a write failure logs and leaves the
    // manifest key absent, so the diff falls back to its one-row-per-sheet behaviour.
    let schematic_geometry = {
        let geom = crate::sch_geom::build_sch_geometry(&refs);
        let rel = "schematics/geometry.json";
        match serde_json::to_string(&geom)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(sch_dir.join("geometry.json"), json).map_err(|e| e.to_string()))
        {
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

    // Board artifacts (3D model refs + geometry IR + lean PCB SVGs), when a board sits
    // next to the project. Absent or unreadable boards just leave these empty.
    let (pcb_svgs, pcb_geometry) = if pcb_path.exists() {
        match crate::pcb::extract_pcb(&pcb_path, out_dir, &model.theme, emit) {
            Ok(a) => (a.svgs, a.geometry),
            Err(e) => {
                emit(Msg::Progress(format!("pcb skipped: {e}")));
                (Vec::new(), None)
            }
        }
    } else {
        (Vec::new(), None)
    };

    let manifest = Manifest {
        schema: crate::MANIFEST_SCHEMA.to_string(),
        design_json: design_file,
        schematic_svgs,
        pcb_svgs,
        pcb_geometry,
        schematic_geometry,
    };
    let manifest_path = out_dir.join("design_review_manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    emit(Msg::Artifact("design_review_manifest.json".to_string()));

    emit(Msg::Progress(format!(
        "design: {} components, {} nets",
        model.components.len(),
        model.nets.len()
    )));
    Ok(())
}

/// Build the element-uuid → colour map for the SVG renderer: read the project's
/// net classes and, for every net that resolves to a colour, paint all of its
/// graphical element uuids that colour. Empty when the project sets no colours.
fn build_net_colors(
    classes: &crate::netclass::NetClassColors,
    model: &design::Design,
    sheets: &[LoadedSheet],
    emit: &mut dyn FnMut(Msg),
) -> crate::svg::NetColors {
    let mut colors = crate::svg::NetColors::new();
    if classes.is_empty() {
        return colors;
    }
    let mut painted = 0usize;
    for net in &model.nets {
        if let Some(hex) = classes.color_for(&net.name) {
            for uuid in net.graphical.all_uuids() {
                colors.insert(uuid.clone(), hex.clone());
            }
            painted += 1;
        }
    }
    // Directive-flag glyphs carry their net-class name directly, not a net name,
    // and aren't part of any net's graphical elements — colour them from the class.
    for s in sheets {
        for f in &s.sch.netclass_flags {
            if f.netclass.is_empty() || f.uuid.is_empty() {
                continue;
            }
            if let Some(hex) = classes.class_hex(&f.netclass) {
                colors.insert(f.uuid.clone(), hex);
            }
        }
    }
    emit(Msg::Progress(format!("net colours: {painted} nets coloured from net classes")));
    colors
}

/// Read the project's `text_variables` map (`${KEY}` substitutions used in the
/// title block, e.g. `PCB_REVISION`). Empty when absent/unreadable.
fn read_text_vars(pro: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let Ok(txt) = std::fs::read_to_string(pro) else {
        return m;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return m;
    };
    if let Some(obj) = v.get("text_variables").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            if let Some(s) = val.as_str() {
                m.insert(k.clone(), s.to_string());
            }
        }
    }
    m
}

/// Expand `${KEY}` references against the project text variables. An unknown key
/// is left as its literal `${KEY}` (KiCad's own behaviour) rather than dropped.
fn expand_text_vars(s: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => {
                let key = &rest[i + 2..i + j];
                match vars.get(key) {
                    Some(val) => out.push_str(val),
                    None => out.push_str(&rest[i..i + j + 1]),
                }
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Build `(designator, pad_number) → KiCad net name` from the board, the join
/// key for adopting canonical net names. Absent/unreadable boards yield an empty
/// map (the rename then no-ops). A footprint's first pad per number wins.
fn read_pcb_pad_net(pcb_path: &Path) -> BTreeMap<(String, String), String> {
    let mut map = BTreeMap::new();
    let Ok(src) = std::fs::read_to_string(pcb_path) else {
        return map;
    };
    let Ok(pcb) = Pcb::parse_str(&src) else {
        return map;
    };
    for fp in &pcb.footprints {
        if fp.reference.is_empty() {
            continue;
        }
        for pad in &fp.pads {
            if pad.number.is_empty() || pad.net_name.is_empty() {
                continue;
            }
            map.entry((fp.reference.clone(), pad.number.clone()))
                .or_insert_with(|| pad.net_name.clone());
        }
    }
    map
}

/// Parse a project's full hierarchy into the component list (shared by `bom`),
/// so subsheet parts are not missing from the BOM.
pub fn load_components(project: &Path) -> Result<(String, Vec<Component>), String> {
    let mut sink = |_: Msg| {};
    let (name, sheets) = load_hierarchy(project, &mut sink)?;
    let mut components = Vec::new();
    for s in &sheets {
        components.extend(design::build_components_on(
            &s.sch,
            &s.info.sheet_path,
            &s.info.sheet_path_uuids,
        ));
    }
    components.sort_by(|a, b| a.designator.cmp(&b.designator));
    Ok((name, components))
}

/// Run the `bom` command for one output format.
pub fn run_bom(
    project: &Path,
    out_dir: &Path,
    format: &str,
    emit: &mut dyn FnMut(Msg),
) -> Result<(), String> {
    let (name, components) = load_components(project)?;
    let mapping = bom::resolve_mapping(&components);
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let project_str = project.to_string_lossy().into_owned();

    let write = |file: &str, contents: &str, emit: &mut dyn FnMut(Msg)| -> Result<(), String> {
        std::fs::write(out_dir.join(file), contents).map_err(|e| e.to_string())?;
        emit(Msg::Artifact(file.to_string()));
        Ok(())
    };

    match format {
        "grouped-json" => {
            // Ungrouped on purpose: the app's BOM table groups per the active KiCad
            // preset, so the extractor must not pre-collapse the lines.
            let flat = bom::build_flat(&components, &mapping, &project_str, &name);
            let json = serde_json::to_string_pretty(&flat).map_err(|e| e.to_string())?;
            write(&format!("{name}_bom.json"), &json, emit)?;
        }
        "grouped-csv" => {
            let grouped = bom::build_grouped(&components, &mapping, &project_str, &name);
            write(&format!("{name}_bom.csv"), &bom::grouped_csv(&grouped), emit)?;
        }
        "enriched-csv" => {
            let (rows, dist) = bom::build_enriched(&components, &mapping);
            write(&format!("{name}_bom_enriched.csv"), &bom::enriched_csv(&rows, &dist), emit)?;
            let report = bom::mapping_report(&mapping, &components);
            write(
                &format!("{name}_bom_enriched.csv.mapping.json"),
                &serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
                emit,
            )?;
            emit(Msg::Progress(format!("enriched BOM: {} lines", rows.len())));
        }
        other => return Err(format!("unknown bom format '{other}'")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Any real KiCad project on the dev machine (set SPINZERO_TEST_PROJECT to
    // its .kicad_pro); skips when unset/absent (CI has no boards).
    fn test_project() -> Option<PathBuf> {
        let p = PathBuf::from(std::env::var("SPINZERO_TEST_PROJECT").ok()?);
        p.exists().then_some(p)
    }

    /// A `${VAR}` in a symbol field (e.g. an MPN of `EX-…-Rev ${PCB_REVISION}`) must
    /// read expanded — that field is what both the BOM table and the rendered
    /// schematic show. Notes and the title block go the same way; an unknown key stays
    /// literal, and labels stay untouched so connectivity isn't re-keyed.
    #[test]
    fn expands_text_vars_in_symbol_fields_notes_and_title_block() {
        let src = r#"
        (kicad_sch
          (uuid "root")
          (title_block (title "Rev ${PCB_REVISION}") (comment 1 "c ${PCB_REVISION}"))
          (symbol (lib_id "Device:R") (at 0 0 0) (uuid "r1")
            (property "Reference" "R1")
            (property "MPN" "EX-0000035-00-Rev ${PCB_REVISION}")
            (property "Note" "${NOPE}"))
          (text "n ${PCB_REVISION}" (at 0 0 0))
          (label "NET_${PCB_REVISION}" (at 0 0 0)))
        "#;
        let mut sch = Schematic::parse_str(src).expect("parse");
        let vars = BTreeMap::from([("PCB_REVISION".to_string(), "B".to_string())]);
        expand_sheet_text_vars(&mut sch, &vars);

        let field = |k: &str| {
            sch.symbols[0].properties.iter().find(|p| p.key == k).unwrap().value.clone()
        };
        assert_eq!(field("MPN"), "EX-0000035-00-Rev B");
        assert_eq!(field("Note"), "${NOPE}", "unknown key stays literal, as KiCad does");
        assert_eq!(sch.notes[0].text, "n B");
        assert_eq!(sch.title.as_deref(), Some("Rev B"));
        assert_eq!(sch.comments[0], "c B");
        assert_eq!(sch.labels[0].text, "NET_${PCB_REVISION}", "labels name nets — untouched");
    }

    /// `design.json` must be byte-identical across runs — the runtime cache key and
    /// raw-blob dedupe both rely on it. Each `run_design` builds fresh HashMaps with
    /// different seeds, so a non-total net ordering (the old `sort_by(name)`) would
    /// diverge here; the canonical `net_order_key` sort keeps it stable.
    #[test]
    fn design_json_is_byte_deterministic() {
        let Some(pro) = test_project() else { return };
        let tmp = std::env::temp_dir().join(format!("extract_det_{}", std::process::id()));
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut sink = |_: Msg| {};
        run_design(&pro, &a, &mut sink).expect("run a");
        run_design(&pro, &b, &mut sink).expect("run b");
        let stem = pro.file_stem().unwrap().to_string_lossy().into_owned();
        let ja = std::fs::read(a.join(format!("{stem}_design.json"))).unwrap();
        let jb = std::fs::read(b.join(format!("{stem}_design.json"))).unwrap();
        assert_eq!(ja, jb, "design.json must be byte-identical across runs");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
