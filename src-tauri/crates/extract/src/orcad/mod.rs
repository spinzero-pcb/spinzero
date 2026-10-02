//! OrCAD front-end for the extraction pipeline.
//!
//! Capture schematics (`.OPJ` / `.DSN`) and OrCAD/Allegro boards (`.brd`) are
//! projected into the same public structs the KiCad and Altium paths build —
//! `design::Component`, `netlist::Frag`, `design::Design`, `ir::Geometry` — so
//! everything downstream of the model is shared code.
//!
//! A Capture page is the unit the bundle calls a sheet. A schematic folder
//! holds several pages; a hierarchical block instantiates a folder, and a
//! folder placed by several blocks is instantiated once per OCCURRENCE, each
//! occurrence its own set of sheets with its own designators and net names.

pub mod design;
pub mod netlist;
pub mod pcb;
pub mod pipeline;
pub mod sch_geom;
pub mod sch_svg;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eda_parse_orcad::capture::hierarchy::{Occurrence, Scope};
use eda_parse_orcad::capture::opj::Project;
use eda_parse_orcad::capture::page::Page;
use eda_parse_orcad::capture::CaptureDoc;
use serde::Serialize;

use crate::design::SheetInfo;
use crate::pipeline::Msg;

/// Maximum hierarchy depth walked — a backstop against pathological cycles.
const MAX_DEPTH: usize = 32;

/// Capture's database unit (10 mil) in millimetres.
pub const DBU_MM: f64 = 0.254;

pub fn mm(v: i32) -> f64 {
    v as f64 * DBU_MM
}

/// True when a path names something this front-end handles: a Capture project,
/// a Capture design, or an OrCAD/Allegro board.
pub fn is_orcad_project(p: &Path) -> bool {
    orcad_rank(p).is_some()
}

/// How strongly a file stands for an OrCAD design: the project (`.OPJ`) first,
/// then a Capture design (`.DSN`), then a board (`.brd`). `None` for anything
/// else. `.dsn` is also Specctra's text extension and `.brd` Eagle's, so both
/// are confirmed by their content, not their name.
pub fn orcad_rank(p: &Path) -> Option<u8> {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "opj" => Some(0),
        "dsn" => head(p).map(|b| ole_cfb_magic(&b)).unwrap_or(false).then_some(1),
        "brd" => eda_parse_orcad::allegro::sniff_path(p).then_some(2),
        _ => None,
    }
}

fn head(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut b = vec![0u8; 8];
    std::fs::File::open(p).ok()?.read_exact(&mut b).ok()?;
    Some(b)
}

fn ole_cfb_magic(b: &[u8]) -> bool {
    b.len() >= 8 && b[..8] == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
}

/// What the extraction could not resolve. Every gap lands here.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Unresolved {
    /// Hierarchical blocks whose child folder is not in the design.
    pub missing_folders: Vec<String>,
    /// Folders no block reaches. Emitted as their own top sheets so their parts
    /// still reach the BOM.
    pub orphan_folders: Vec<String>,
    /// Block pins with no port of that name on the child folder.
    pub block_pins_without_port: Vec<String>,
    /// Ports on a child folder with no pin of that name on the block above it.
    pub ports_without_block_pin: Vec<String>,
    /// Pages Capture could not be read past, with the reason. What was read
    /// before the failure is kept.
    pub pages_truncated: Vec<String>,
    /// Pages that could not be read at all.
    pub pages_unreadable: Vec<String>,
    /// The occurrence tree could not be read: designators come from the
    /// placed instances, which may carry the `U?` template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_tree: Option<String>,
    /// Placed parts whose cached symbol is missing from the design cache.
    pub parts_without_symbol: Vec<String>,
    /// Placed parts whose package (pin numbers) is missing; their pins are
    /// numbered by pin name instead.
    pub parts_without_package: Vec<String>,
    /// Parts still carrying an unannotated designator (`R?`).
    pub unannotated_parts: Vec<String>,
    /// Bus wires seen. Buses are recorded, not expanded into member nets.
    pub buses_not_expanded: usize,
    /// Component pins on a net of their own — no wire, pin, power symbol or
    /// connector at their connection point, and no no-connect marker.
    pub unconnected_pins: usize,
    /// Pins the designer marked no-connect.
    pub pins_marked_no_connect: usize,
    /// Hidden power pins connected implicitly to the net their name names.
    pub hidden_supply_pins: usize,
    /// Places where the stored wire net ids and the drawn geometry disagree
    /// (two stored nets touching). Geometry wins; the count says how often.
    pub stored_nets_joined_by_geometry: usize,
    /// Records the reader skipped, by stream.
    pub skipped: Vec<String>,
}

/// A parsed Capture design and everything read alongside it.
pub struct Loaded {
    pub name: String,
    pub dsn: PathBuf,
    pub project: Option<Project>,
    pub doc: CaptureDoc,
}

/// One sheet instance: a page in the context of one occurrence of its folder.
pub struct SheetInstance {
    pub info: SheetInfo,
    pub folder: usize,
    pub page: usize,
    /// The occurrence scope of this folder instance, when the design has an
    /// occurrence tree.
    pub scope: Option<Scope>,
    /// Identity of the folder instance: every page of one occurrence shares
    /// it. Off-page connectors and net names are scoped by it.
    pub folder_key: String,
    /// For a child folder instance: the key the block on the parent page uses
    /// for its pins, so ports and block pins meet.
    pub hier_key: Option<String>,
    /// Depth in the block hierarchy (0 for the root folder and orphans).
    pub depth: usize,
    /// What Capture appends to a net name local to this folder instance:
    /// `_` and the block's name, for every block on the way down.
    pub suffix: String,
}

/// The compile context echoed into the design model.
#[derive(Debug, Clone, Serialize)]
pub struct CompileSettings {
    /// Library stream version, e.g. `3.2`.
    pub format_version: String,
    /// The root schematic folder.
    pub root_folder: String,
    /// True when the design carries Capture's occurrence tree (designators and
    /// generated net names are per occurrence).
    pub occurrence_annotated: bool,
    /// True when only a `.DSN` was given (no `.OPJ`).
    pub free_document: bool,
}

/// The source block of the design model.
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub tool: String,
    pub compile: CompileSettings,
    pub board: Option<BoardInfo>,
    pub variants: Vec<design::VariantInfo>,
    pub unresolved: Unresolved,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct BoardInfo {
    pub file: String,
    pub format: String,
    pub layers: usize,
    pub components: usize,
    pub nets: usize,
    /// The board's physical constraint sets (track width, spacing, ...).
    pub constraint_sets: Vec<pcb::ConstraintSet>,
    /// What reading the board skipped or approximated.
    pub stats: pcb::BoardStats,
}

/// Find the design a project argument names: an `.OPJ` (its first schematic
/// design), or a `.DSN` directly.
pub fn resolve_design(project: &Path) -> Result<(PathBuf, Option<Project>), String> {
    let ext = project.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "dsn" {
        // A design beside an .OPJ that names it belongs to that project.
        let opj = sibling_project(project);
        return Ok((project.to_path_buf(), opj));
    }
    if ext != "opj" {
        return Err(format!("not an OrCAD Capture project or design: {}", project.display()));
    }
    let p = Project::open(project)?;
    let dir = project.parent().unwrap_or(Path::new("."));
    let found = p.designs().find_map(|f| Project::resolve(dir, &f.path));
    if let Some(path) = found {
        return Ok((path, Some(p)));
    }
    // An .OPJ whose design path does not resolve: fall back to a .DSN of the
    // project's own name beside it, then any single .DSN there.
    let stem = project.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let dsns: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|q| {
                    q.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("dsn")).unwrap_or(false)
                        && std::fs::read(q).map(|b| ole_cfb_magic(&b)).unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(d) = dsns.iter().find(|d| {
        d.file_stem().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case(stem)).unwrap_or(false)
    }) {
        return Ok((d.clone(), Some(p)));
    }
    if dsns.len() == 1 {
        return Ok((dsns[0].clone(), Some(p)));
    }
    Err(format!("the project names no design that exists: {}", project.display()))
}

/// The `.OPJ` beside a `.DSN` that lists it, if any.
fn sibling_project(dsn: &Path) -> Option<Project> {
    let dir = dsn.parent()?;
    let name = dsn.file_name()?.to_str()?.to_string();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if !p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("opj")).unwrap_or(false) {
            continue;
        }
        let Ok(prj) = Project::open(&p) else { continue };
        if prj.designs().any(|f| {
            f.path.replace('\\', "/").rsplit('/').next().map(|n| n.eq_ignore_ascii_case(&name)).unwrap_or(false)
        }) {
            return Some(prj);
        }
    }
    None
}

pub fn load(project: &Path, emit: &mut dyn FnMut(Msg)) -> Result<Loaded, String> {
    let (dsn, prj) = resolve_design(project)?;
    let doc = eda_parse_orcad::capture::open(&dsn)?;
    if !doc.is_design() {
        return Err(format!("{} is an OrCAD Capture library, not a design", dsn.display()));
    }
    emit(Msg::Progress(format!(
        "parsed {} (Capture {}.{}, {} folders, {} pages)",
        dsn.display(),
        doc.lib.version.0,
        doc.lib.version.1,
        doc.folders.len(),
        doc.folders.iter().map(|f| f.pages.len()).sum::<usize>()
    )));
    let name = match &prj {
        Some(p) if !p.name.is_empty() => p.name.clone(),
        _ => project.file_stem().and_then(|s| s.to_str()).unwrap_or("design").to_string(),
    };
    Ok(Loaded { name, dsn, project: prj, doc })
}

/// The occurrence a block or part on a page stands for, within one scope.
pub fn occurrence_of<'a>(scope: Option<&'a Scope>, db_id: u32) -> Option<&'a Occurrence> {
    scope?.occurrences.iter().find(|o| o.target_db_id == db_id)
}

fn page_label(p: &Page) -> String {
    if p.name.is_empty() {
        p.stream.clone()
    } else {
        p.name.clone()
    }
}

/// A path segment safe inside the slash-separated sheet paths.
fn seg(s: &str) -> String {
    s.replace('/', "∕")
}

/// Walk the design's folders from the root, one sheet per page per folder
/// occurrence, and report what the walk could not reach.
pub fn walk(doc: &CaptureDoc, unresolved: &mut Unresolved) -> Vec<SheetInstance> {
    let mut out = Vec::new();
    let mut counter = 0i64;
    let mut reached: BTreeSet<usize> = BTreeSet::new();
    let root_scope = doc.tree.as_ref().map(|t| t.root.clone());
    if let Some(r) = doc.root {
        visit(doc, r, root_scope, "/", "/", "", None, "", 0, &mut counter, &mut reached, &mut out, unresolved);
    }
    // Folders nothing reaches still hold parts; emit them as their own top
    // sheets, after the hierarchy, in display order.
    for (i, f) in doc.folders.iter().enumerate() {
        if reached.contains(&i) || f.pages.is_empty() {
            continue;
        }
        unresolved.orphan_folders.push(f.name.clone());
        let human = format!("/{}/", seg(&f.name));
        let ids = format!("/f:{}/", seg(&f.name));
        // Capture netlists only the root; a name an orphan folder shares with
        // another top sheet is still another net, so it takes the folder's
        // name as its suffix when the bare name would collide.
        let suffix = format!("_{}", f.name);
        visit(doc, i, None, &human, &ids, &format!("orphan:{}", f.name), None, &suffix, 0, &mut counter, &mut reached, &mut out, unresolved);
    }
    for f in &doc.folders {
        for p in &f.pages {
            if let Some(t) = &p.truncated {
                unresolved.pages_truncated.push(format!("{}/{}: {t}", f.name, page_label(p)));
            }
        }
        for (p, e) in &f.page_errors {
            unresolved.pages_unreadable.push(format!("{}/{p}: {e}", f.name));
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn visit(
    doc: &CaptureDoc,
    folder: usize,
    scope: Option<Scope>,
    human: &str,
    ids: &str,
    folder_key: &str,
    hier_key: Option<String>,
    suffix: &str,
    depth: usize,
    counter: &mut i64,
    reached: &mut BTreeSet<usize>,
    out: &mut Vec<SheetInstance>,
    unresolved: &mut Unresolved,
) {
    if depth > MAX_DEPTH {
        return;
    }
    reached.insert(folder);
    let f = &doc.folders[folder];
    let single = f.pages.len() == 1;
    for (pi, page) in f.pages.iter().enumerate() {
        *counter += 1;
        let label = page_label(page);
        // A one-page folder is the sheet; a multi-page folder adds the page.
        let sheet_path = if single && depth > 0 { human.to_string() } else { format!("{human}{}/", seg(&label)) };
        let sheet_ids = format!("{ids}p:{}/", seg(&page.stream));
        let title = page.props.iter().find(|(k, _)| k.eq_ignore_ascii_case("Title")).map(|(_, v)| v.clone());
        let tb = |key: &str| -> String {
            page.title_blocks
                .iter()
                .find_map(|t| t.prop(key).map(str::to_string))
                .unwrap_or_default()
        };
        let notes: Vec<String> = page
            .graphics
            .iter()
            .filter(|g| g.kind == 61)
            .filter_map(|g| g.body.as_ref())
            .flat_map(|b| b.prims.iter())
            .filter_map(|p| match p {
                eda_parse_orcad::capture::symbol::Prim::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
            .collect();
        let info = SheetInfo {
            filename: format!("{}/{}", f.name, label),
            path: format!("{}/Pages/{}", f.name, page.stream),
            sheet_number: *counter,
            sheet_path: sheet_path.clone(),
            sheet_path_uuids: sheet_ids.clone(),
            title: title.unwrap_or_else(|| tb("Title")),
            page: tb("Page Number"),
            notes,
            company: tb("OrgName"),
            rev: tb("RevCode"),
            date: tb("Page Modify Date"),
        };
        out.push(SheetInstance {
            info,
            folder,
            page: pi,
            scope: scope.clone(),
            folder_key: folder_key.to_string(),
            hier_key: hier_key.clone(),
            depth,
            suffix: suffix.to_string(),
        });

        // Hierarchical blocks on this page instantiate their child folders.
        for b in &page.blocks {
            let occ = occurrence_of(scope.as_ref(), b.db_id);
            let child_name = occ
                .map(|o| o.child_folder.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| b.implementation.clone());
            let Some(ci) = doc.folders.iter().position(|x| x.name.eq_ignore_ascii_case(&child_name)) else {
                unresolved.missing_folders.push(format!(
                    "{} on {}: {}",
                    if b.reference.is_empty() { &b.name } else { &b.reference },
                    sheet_path,
                    child_name
                ));
                continue;
            };
            let block_label = if !b.reference.is_empty() {
                b.reference.clone()
            } else if !b.name.is_empty() {
                b.name.clone()
            } else {
                child_name.clone()
            };
            let child_human = format!("{sheet_path}{}/", seg(&block_label));
            let child_ids = format!("{sheet_ids}b:{}/", b.db_id);
            let child_key = format!("{folder_key}/{}", b.db_id);
            let child_scope = occ.map(|o| o.nested.clone());
            let block_name = if !b.name.is_empty() { b.name.clone() } else { block_label.clone() };
            let child_suffix = format!("{suffix}_{block_name}");
            visit(
                doc,
                ci,
                child_scope,
                &child_human,
                &child_ids,
                &child_key,
                Some(format!("hier:{child_key}")),
                &child_suffix,
                depth + 1,
                counter,
                reached,
                out,
                unresolved,
            );
        }
    }
}

/// The board a project pairs with, when one can be found: the `.OPJ`'s netlist
/// target by name, then a `.brd` of the design's own name, then the only
/// `.brd` in the project folder or its conventional `allegro/` subfolder.
pub fn board_beside(project: &Path, prj: Option<&Project>, dsn: &Path) -> Option<PathBuf> {
    let ext = project.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "brd" {
        return Some(project.to_path_buf());
    }
    let dir = dsn.parent()?;
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(pd) = project.parent() {
        if pd != dir {
            dirs.push(pd.to_path_buf());
        }
    }
    for d in dirs.clone() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
                if p.is_dir() && (n == "allegro" || n == "pcb" || n == "layout" || n == "board") {
                    dirs.push(p);
                }
            }
        }
    }
    let boards: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("brd")).unwrap_or(false))
        .filter(|p| eda_parse_orcad::allegro::sniff_path(p))
        .collect();
    let by_name = |name: &str| {
        boards.iter().find(|b| {
            b.file_name().and_then(|n| n.to_str()).map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)
        })
    };
    if let Some(n) = prj.and_then(|p| p.board_name()) {
        if let Some(b) = by_name(&n) {
            return Some(b.clone());
        }
    }
    let stem = dsn.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some(b) = by_name(&format!("{stem}.brd")) {
        return Some(b.clone());
    }
    (boards.len() == 1).then(|| boards[0].clone())
}

/// Build the design model and its source block for a loaded design.
pub fn build_design(
    l: &Loaded,
    project_path: &str,
    project_filename: &str,
    free_document: bool,
) -> (crate::design::Design, SourceInfo, Vec<SheetInstance>) {
    let mut unresolved = Unresolved::default();
    if let Some(e) = &l.doc.tree_error {
        unresolved.occurrence_tree = Some(e.clone());
    }
    unresolved.skipped = l.doc.notes.clone();
    let sheets = walk(&l.doc, &mut unresolved);
    let (components, extra_svg_ids) = design::build_components(&l.doc, &sheets, &mut unresolved);
    let power_names = netlist::power_names(&l.doc);
    let mut frags = Vec::new();
    for s in &sheets {
        frags.extend(netlist::fragments(&l.doc, s, &power_names, &mut unresolved));
    }
    let mut nets = crate::netlist::merge_frags(frags);
    netlist::finalize_names(&mut nets);
    let infos: Vec<SheetInfo> = sheets.iter().map(|s| s.info.clone()).collect();
    let mut model = crate::design::assemble(
        &l.name,
        project_path,
        project_filename,
        infos,
        components,
        nets,
        Vec::new(),
    );
    for (uuid, designator) in extra_svg_ids {
        model.indexes.svg_to_component.insert(uuid, designator);
    }
    let root_folder = l.doc.root_folder().map(|f| f.name.clone()).unwrap_or_default();
    let source = SourceInfo {
        tool: "orcad".to_string(),
        compile: CompileSettings {
            format_version: format!("{}.{}", l.doc.lib.version.0, l.doc.lib.version.1),
            root_folder,
            occurrence_annotated: l.doc.tree.is_some(),
            free_document,
        },
        board: None,
        variants: Vec::new(),
        unresolved,
    };
    (model, source, sheets)
}
