//! Altium front-end for the extraction pipeline.
//!
//! Everything here builds the *same* public structs the KiCad path builds
//! (`design::Component`, `netlist::Frag`, `design::Design`), so the two sources
//! cannot drift on schema: a schema change breaks both builders at once.

pub mod design;
pub mod dump;
pub mod layers;
pub mod netlist;
pub mod pcb;
pub mod pcb_svg;
pub mod sch_geom;
pub mod sch_svg;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use eda_parse_altium::{sch, CompileOptions, Doc, Kind, Project, SchDoc};
use serde::Serialize;

use crate::design::SheetInfo;

/// Maximum hierarchy depth walked — a backstop against pathological cycles.
const MAX_SHEET_DEPTH: usize = 32;

/// A signal harness the design draws, as the review reads it.
///
/// A harness is Altium's bus with named members instead of a range, and it is
/// treated the same way here: the BUNDLE is modelled, and its members are NOT
/// expanded into nets. The rule is the plan's own (§4.4) and the reason is the
/// one that governs buses — member names are bundle-local, so `SDA` inside an
/// `I2C` harness and `SDA` inside a `SENSOR` harness are different signals, and
/// a bare-name union would short them together. Altium's own compiled net name
/// for a harness member is not documented anywhere this project can cite, and
/// guessing at the spelling would put an invented net in a review.
#[derive(Debug, Clone, Serialize)]
pub struct HarnessInfo {
    /// The type the connector declares (`I2C`), or empty when it draws none.
    pub harness_type: String,
    /// The member signals, in the order the connector lists them.
    pub entries: Vec<String>,
    /// Sheet the connector is drawn on.
    pub sheet: String,
    /// Names of the ports on that sheet carrying this harness type. This is the
    /// link the plan asks for: a harness connector reaches the hierarchy
    /// through a port, and without it the bundle stops at the sheet edge.
    pub ports: Vec<String>,
    /// Whether a `.Harness` definition file declares the same members. `None`
    /// when the project ships no definition for this type; `Some(false)` is a
    /// locked definition that has drifted from the drawing, which Altium itself
    /// reports as a violation.
    pub matches_definition: Option<bool>,
}

/// One parsed sheet instance: its place in the hierarchy plus the document.
pub struct LoadedSheet {
    pub info: SheetInfo,
    pub sch: SchDoc,
}

/// The compile options actually used, echoed into the design model so a review
/// can cite them — the same schematic compiles to a different netlist under
/// different settings, and a KiCad project has no analogue for them.
#[derive(Debug, Clone, Serialize)]
pub struct CompileSettings {
    pub hierarchy_mode: String,
    pub allow_port_net_names: bool,
    pub allow_sheet_entry_net_names: bool,
    pub power_port_names_take_priority: bool,
    pub netlist_single_pin_nets: bool,
    pub name_nets_hierarchically: bool,
    /// True when no `.PrjPcb` was found, so free-document defaults applied.
    pub free_document: bool,
}

impl CompileSettings {
    fn new(o: &CompileOptions, free_document: bool) -> CompileSettings {
        CompileSettings {
            hierarchy_mode: o.hierarchy_mode.as_str().to_string(),
            allow_port_net_names: o.allow_port_net_names,
            allow_sheet_entry_net_names: o.allow_sheet_entry_net_names,
            power_port_names_take_priority: o.power_port_names_take_priority,
            netlist_single_pin_nets: o.netlist_single_pin_nets,
            name_nets_hierarchically: o.name_nets_hierarchically,
            free_document,
        }
    }
}

/// What the extraction could not resolve. A review that silently omits
/// something is worse than one that says it did, so every gap lands here.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Unresolved {
    /// Sheet symbols whose child document could not be found or read.
    pub missing_sheets: Vec<String>,
    /// Documents in the project that no sheet symbol reaches. They are emitted
    /// as their own top sheets so their parts still reach the BOM.
    pub orphan_sheets: Vec<String>,
    /// Sheet entries with no matching port on the child sheet.
    pub sheet_entries_without_port: Vec<String>,
    /// Child ports with no matching entry on the parent's sheet symbol.
    pub ports_without_sheet_entry: Vec<String>,
    /// Buses seen. Bus ranges are not expanded into member nets.
    pub buses_not_expanded: usize,
    /// Harness member signals left inside their bundle. A harness is a bundle
    /// like a bus, its member names are bundle-local, and Altium's compiled
    /// name for a member is not documented — so the bundle is modelled
    /// (`harnesses`) and the members are not made into nets.
    pub harness_members_not_expanded: usize,
    /// Harness connectors reaching no port on their own sheet. The bundle stops
    /// at the sheet edge, so nothing carries it up the hierarchy.
    pub harnesses_without_a_port: usize,
    /// Directives (`RECORD=43`) that state no class this reader knows. Their
    /// class names reach `net_name_to_classes`; these state none.
    pub directives_without_a_class: usize,
    /// Directives that state a class but sit where no net reaches them, so the
    /// class lands on nothing.
    pub directives_without_a_net: usize,
    /// `RECORD=211` regions. The plan reads these as compile masks; in this
    /// corpus every component inside one is also on the board, so applying them
    /// would drop 65 real parts. They are reported, not applied — see
    /// docs/altium-extraction-notes.md.
    pub compile_mask_candidates_not_applied: usize,
    /// Variations naming a designator the design does not place. Altium keeps
    /// stale entries in a variant, and an unannotated one reads `R?`.
    pub variations_without_a_part: Vec<String>,
    /// Parameter overrides inside a variant this reader does not decode. Every
    /// corpus project declares none, so the layout could not be confirmed.
    pub variant_parameter_overrides_unread: usize,
    /// Images a sheet LINKS to rather than embeds. The file lives on the
    /// designer's own machine and is not in the design, so the frame is drawn
    /// and the picture is not.
    pub linked_images_not_embedded: usize,
    /// Component pins on a net of their own — no wire, label or port at their
    /// location, and no marker saying so. A connectivity-model problem shows up
    /// here as a number.
    pub unconnected_pins: usize,
    /// Pins the designer marked no-connect: open on purpose. Counted apart from
    /// the ones above, because a review reads the two very differently.
    pub pins_marked_no_connect: usize,
    /// Pins connected implicitly as hidden supply pins.
    pub hidden_supply_pins: usize,
    /// Hidden pins naming no supply whose pad a visible pin already draws. They
    /// are a second connection point on one pad, so they get no terminal. A
    /// hidden pin with a pad of its own keeps one, and reads as unconnected.
    pub hidden_pins_without_net: usize,
    /// Pins joined to a wire by the sheet's snap radius rather than exactly,
    /// because the wire drawn to them ends just short of the pin.
    pub pins_joined_by_hot_spot: usize,
    /// Pins left unconnected because two wire vertices were equally near, so
    /// joining either would be a guess between two nets.
    pub hot_spot_ambiguous: usize,
    /// Net labels carrying no text. Altium keeps them, they draw nothing, and
    /// they name nothing.
    pub unnamed_labels: usize,
    /// Ports and sheet entries whose name is a bus RANGE. Linking on the bundle
    /// name would short every member together, so no link is made and the
    /// members meet by name instead.
    pub bus_range_links_not_made: usize,
    /// Rigid-flex regions of the board outline bound to no substack the board
    /// declares. The region draws, and which stack builds it is unknown — so its
    /// layer count is the master stack's, which on a flex ribbon is wrong. The
    /// stack itself is modelled in `board_stackup`; this counts what did not
    /// resolve.
    pub board_regions_without_substack: usize,
    /// Board special strings drawn verbatim because the extraction has no value
    /// for them — a drill legend, a print date.
    pub board_unresolved_specials: usize,
    /// Record types seen but not modelled, by type and count.
    pub skipped_records: BTreeMap<String, usize>,
}

/// Parameter naming the variants that leave a part off. `kicad_dnp` stays
/// false: in the schematic as drawn every part is fitted, and the base design is
/// what a review reads.
pub const NOT_FITTED_PARAM: &str = "altium_not_fitted_in";

/// Parameter naming the variants that fit a different part instead.
pub const ALTERNATE_IN_PARAM: &str = "altium_alternate_in";

/// One project variant, resolved against the design's own components.
///
/// Altium has no per-symbol DNP flag: a build that leaves parts off is a
/// VARIANT, and one project can define several. Every one of them is modelled
/// here, and the base design — the schematic as drawn — stays what
/// `components` and `nets` describe. That keeps one design to review and lets a
/// finding name the variant it applies to, rather than picking one build and
/// silently reviewing that.
#[derive(Debug, Clone, Serialize)]
pub struct VariantInfo {
    /// The variant's own name, which is what a finding cites.
    pub name: String,
    pub uid: String,
    /// Whether the project allows this variant to be fabricated.
    pub allow_fabrication: bool,
    /// Designators this variant does not fit, sorted.
    pub not_fitted: Vec<String>,
    /// Designator -> the part fitted in place of the base one.
    pub alternate_parts: BTreeMap<String, String>,
}

/// One of the board's design rules, as the review reads it.
///
/// A rule is what the board is CHECKED against — a clearance, a track width, a
/// hole size, a mask expansion — so it belongs in the model beside the geometry
/// it constrains. The scope expressions are Altium's own query language, kept
/// verbatim: `InNet('DESAT_2_2')` means nothing to this extractor and
/// everything to a reviewer.
#[derive(Debug, Clone, Serialize)]
pub struct RuleInfo {
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i64,
    pub scope: String,
    pub against: String,
    pub values: BTreeMap<String, String>,
}

/// One layer-stack region of a rigid-flex board, as the review reads it.
///
/// GUIDs are resolved to the names the designer typed, because a reviewer
/// reasons about `EAST_FLEX_EXTENTION`, not `{8E2616F1-…}`.
#[derive(Debug, Clone, Serialize)]
pub struct SubstackInfo {
    pub name: String,
    /// True when the region bends.
    pub is_flex: bool,
    /// Copper layer names this region builds, in stack order. The count is the
    /// number a fabricator would call this part of the board.
    pub copper_layers: Vec<String>,
    /// Every stack entry the region enables — dielectrics, coverlay and
    /// adhesive included, which is what makes a flex ribbon a flex ribbon.
    pub layers: Vec<String>,
}

/// A drill span and the regions it is drilled in.
#[derive(Debug, Clone, Serialize)]
pub struct DrillPairInfo {
    pub low: String,
    pub high: String,
    /// Substack names, or empty on a board with one stack.
    pub substacks: Vec<String>,
}

/// A fold in a flex region.
#[derive(Debug, Clone, Serialize)]
pub struct BendInfo {
    pub angle_deg: f64,
    pub radius_mm: f64,
    /// The fold line, millimetres, in the bundle's Y-down space.
    pub from: [f64; 2],
    pub to: [f64; 2],
}

/// One region of the board outline, bound to the stack that builds it.
#[derive(Debug, Clone, Serialize)]
pub struct BoardRegionInfo {
    pub name: String,
    /// Name of the substack this region is built from, empty when unbound.
    pub substack: String,
    pub bends: Vec<BendInfo>,
}

/// A whole child board placed inside this one — Altium's board-in-board, and
/// how an assembly panel is drawn.
///
/// The child document is referenced by a path on the designer's own machine and
/// is NOT followed, so what is modelled is the placement. It matters to a review
/// because a panel's real part count is the child board's multiplied by
/// `instances`, and nothing else in the extraction knows that.
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddedBoardInfo {
    pub document_path: String,
    /// Placement origin, millimetres, in the bundle's Y-down space.
    pub at: [f64; 2],
    pub rotation: f64,
    pub mirrored: bool,
    pub rows: i64,
    pub columns: i64,
    pub row_spacing_mm: f64,
    pub column_spacing_mm: f64,
    /// Copies of the child board this placement puts on the panel.
    pub instances: i64,
}

/// The board's layer stack when it has more than one — the rigid-flex model.
///
/// A rigid-flex board is several stackups sharing one outline, and a review that
/// reads only the master stack cannot see that the ribbon joining two halves of
/// a ten-layer board is two layers of polyimide with a bend radius. Absent for a
/// board with a single stack, which is every ordinary board.
#[derive(Debug, Clone, Serialize)]
pub struct StackupInfo {
    pub substacks: Vec<SubstackInfo>,
    pub drill_pairs: Vec<DrillPairInfo>,
    pub regions: Vec<BoardRegionInfo>,
}

/// Source-tool block added to the design model for Altium designs.
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub tool: String,
    pub compile: CompileSettings,
    /// The board's design rules. Empty for a design with no board.
    pub board_rules: Vec<RuleInfo>,
    /// The rigid-flex layer stack, when the board declares substacks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board_stackup: Option<StackupInfo>,
    /// The signal harnesses the design draws. Empty for a design with none.
    pub harnesses: Vec<HarnessInfo>,
    /// Child boards placed inside this one. Empty for an ordinary board.
    pub embedded_boards: Vec<EmbeddedBoardInfo>,
    /// Every variant the project defines. Empty for a design with none.
    pub variants: Vec<VariantInfo>,
    pub unresolved: Unresolved,
}

/// A stable handle for a drawable object, falling back to its position when
/// the file names none.
///
/// Altium writes no `UniqueID` on a junction at all (1786 of them in the
/// corpus) and drops it on a few percent of graphics and pins. Those objects
/// would otherwise be unaddressable: no `data-uuid` in the SVG, no row in the
/// schematic geometry, so a junction added or removed between two revisions
/// would never reach the diff. The position IS the identity for an object the
/// file does not name — a junction that "moved" really is one removed and one
/// added — and the renderer and the geometry builder derive the same string, so
/// a change still anchors to the group the viewer draws.
pub fn oid(uuid: &str, tag: &str, at: sch::Pt) -> String {
    if uuid.is_empty() {
        format!("~{tag}:{},{}", at.x, at.y)
    } else {
        uuid.to_string()
    }
}

/// True when a path names a project or document this front-end handles.
pub fn is_altium_project(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    ext.eq_ignore_ascii_case("PrjPcb") || matches!(Kind::of(p), Kind::Schematic | Kind::Board)
}

/// Load a project's whole sheet hierarchy.
///
/// `project` is a `.PrjPcb`, or a loose `.SchDoc` / `.PcbDoc` — a design with no
/// project file compiles with **free-document** defaults, notably global net
/// scope, which is not the board-project default.
/// A project's sheets and everything read alongside them.
pub struct Hierarchy {
    pub name: String,
    pub options: CompileOptions,
    pub sheets: Vec<LoadedSheet>,
    pub unresolved: Unresolved,
    /// The project file's `[ParameterN]` entries — where a title block's
    /// `=PRJ_Title` and its siblings resolve. Empty for a loose document.
    pub project_params: BTreeMap<String, String>,
    /// Every `[ProjectVariantN]` the project defines, unresolved. Empty for a
    /// loose document, which has no project file to define one.
    pub variants: Vec<eda_parse_altium::Variant>,
    /// The signal harnesses the design draws, resolved against the project's
    /// `.Harness` definitions.
    pub harnesses: Vec<HarnessInfo>,
}

pub fn load_hierarchy(
    project: &Path,
    emit: &mut dyn FnMut(crate::pipeline::Msg),
) -> Result<Hierarchy, String> {
    use crate::pipeline::Msg;
    let mut unresolved = Unresolved::default();
    let dir = project.parent().map(Path::to_path_buf).unwrap_or_default();
    let is_prj = project
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("PrjPcb"))
        .unwrap_or(false);

    let mut project_params: BTreeMap<String, String> = BTreeMap::new();
    let mut variants: Vec<eda_parse_altium::Variant> = Vec::new();
    let (name, options, candidates) = if is_prj {
        let prj = Project::open(project)?;
        variants = prj.variants();
        project_params = prj.parameters();
        for d in &prj.duplicate_documents {
            emit(Msg::Progress(format!("project names a document twice, later entry ignored: {d}")));
        }
        let docs = prj.documents_with_ext(&dir, "SchDoc");
        (prj.name.clone(), prj.options.clone(), docs)
    } else {
        // A loose document. Its own stem names the project, and the
        // free-document compile defaults apply. Every `.SchDoc` beside it is a
        // candidate sheet: a project-less design is still a design, and taking
        // only the named file would drop its other sheets from the netlist and
        // the BOM.
        let stem = project
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid project path")?
            .to_string();
        let mut docs: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| Kind::of(p) == Kind::Schematic)
            .collect();
        docs.sort();
        if docs.is_empty() {
            docs.push(project.with_extension("SchDoc"));
        }
        (stem, CompileOptions::free_document(), docs)
    };

    // Parse every candidate once, then pick the root: the document no other
    // document references by file name.
    let mut parsed: Vec<(PathBuf, SchDoc)> = Vec::new();
    for p in &candidates {
        let Some(p) = resolve_case_insensitive(p) else {
            unresolved.missing_sheets.push(display(p));
            emit(Msg::Progress(format!("skipped (missing): {}", p.display())));
            continue;
        };
        match Doc::open(&p).map(|d| sch::parse(&d)) {
            Ok(s) => {
                emit(Msg::Progress(format!("parsed {}", p.display())));
                parsed.push((p, s));
            }
            Err(e) => {
                unresolved.missing_sheets.push(display(&p));
                emit(Msg::Progress(format!("skipped (parse error): {}: {e}", p.display())));
            }
        }
    }
    if parsed.is_empty() {
        return Err(format!("no readable schematic for {}", project.display()));
    }
    // The root is a document no other document references by file name. A
    // project can leave several unreferenced — a disclaimer or revision sheet
    // nobody links to is still a sheet whose parts belong in the BOM — so the
    // one that drives the hierarchy (the most sheet symbols) leads and the rest
    // are walked after it as their own top sheets. Choosing the *first*
    // unreferenced document instead makes a leaf the root and silently drops
    // every other sheet in the project.
    let referenced: HashSet<String> = parsed
        .iter()
        .flat_map(|(_, s)| s.sheet_symbols.iter())
        .map(|s| lower_stem(&s.filename))
        .collect();
    let mut roots: Vec<usize> = (0..parsed.len())
        .filter(|&i| !referenced.contains(&lower_stem(&display(&parsed[i].0))))
        .collect();
    roots.sort_by_key(|&i| (std::cmp::Reverse(parsed[i].1.sheet_symbols.len()), i));
    if roots.is_empty() {
        roots.push(0);
    }

    let by_stem: BTreeMap<String, usize> = parsed
        .iter()
        .enumerate()
        .map(|(i, (p, _))| (lower_stem(&display(p)), i))
        .collect();

    let mut out = Vec::new();
    let mut counter = 0i64;
    let mut visited = HashSet::new();
    let mut seen_docs = HashSet::new();
    for (n, &root) in roots.iter().enumerate() {
        let (path, uuids) = if n == 0 {
            ("/".to_string(), "/".to_string())
        } else {
            let stem = stem_of(&display(&parsed[root].0));
            (format!("/{stem}/"), format!("/{stem}/"))
        };
        add_sheet(
            root,
            &parsed,
            &by_stem,
            &path,
            &uuids,
            &mut out,
            &mut counter,
            &mut visited,
            &mut seen_docs,
            0,
            &mut unresolved,
            emit,
        );
    }
    // Anything still unwalked (a document referenced only by an unreachable
    // sheet) is emitted as its own top sheet rather than dropped.
    for i in 0..parsed.len() {
        if seen_docs.contains(&i) {
            continue;
        }
        let stem = stem_of(&display(&parsed[i].0));
        unresolved.orphan_sheets.push(stem.clone());
        add_sheet(
            i,
            &parsed,
            &by_stem,
            &format!("/{stem}/"),
            &format!("/{stem}/"),
            &mut out,
            &mut counter,
            &mut visited,
            &mut seen_docs,
            0,
            &mut unresolved,
            emit,
        );
    }
    check_hierarchy_links(&out, &mut unresolved);
    for s in &out {
        unresolved.buses_not_expanded += s.sch.buses.len();

        // Two record types mask a sheet: the FileHeader region and the blanket
        // in the Additional stream. Counting only the first reports zero on a
        // sheet that is visibly masked in Altium.
        unresolved.compile_mask_candidates_not_applied += s.sch.regions.len() + s.sch.blankets.len();
        unresolved.linked_images_not_embedded +=
            s.sch.images.iter().filter(|i| i.data.is_empty()).count();
        for (t, n) in &s.sch.skipped {
            *unresolved
                .skipped_records
                .entry(format!("RECORD={t}"))
                .or_default() += n;
        }
    }
    let definitions = harness_definitions(project);
    let harnesses = resolve_harnesses(&out, &definitions);
    unresolved.harness_members_not_expanded =
        harnesses.iter().map(|h| h.entries.len()).sum();
    unresolved.harnesses_without_a_port =
        harnesses.iter().filter(|h| h.ports.is_empty()).count();
    Ok(Hierarchy { name, options, sheets: out, unresolved, project_params, variants, harnesses })
}

/// Every `TYPE=MEMBER,MEMBER` line of every `.Harness` file beside the project.
///
/// Altium keeps the definition in its own file rather than in the schematic, so
/// a design can declare a harness type no sheet draws, and a locked definition
/// can disagree with the connector that generated it.
fn harness_definitions(project: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let Some(dir) = project.parent() else { return out };
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("harness")) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for (name, members) in sch::parse_harness_definition(&text) {
            out.insert(name.to_ascii_uppercase(), members);
        }
    }
    out
}

/// The harnesses the design draws, each linked to the ports that carry it off
/// the sheet and checked against the project's own definition.
///
/// The port link is by harness TYPE, which is how Altium itself matches them: a
/// port declares `HarnessType=I2C` and any connector on that sheet declaring
/// `I2C` is what it carries. A connector with no type declared can be matched to
/// nothing, and says so by reaching no port.
fn resolve_harnesses(
    sheets: &[LoadedSheet],
    definitions: &BTreeMap<String, Vec<String>>,
) -> Vec<HarnessInfo> {
    let mut out = Vec::new();
    for s in sheets {
        for c in &s.sch.harness_connectors {
            let entries: Vec<String> = c.entries.iter().map(|e| e.name.clone()).collect();
            let key = c.harness_type.to_ascii_uppercase();
            let ports = if key.is_empty() {
                Vec::new()
            } else {
                s.sch
                    .ports
                    .iter()
                    .filter(|p| p.harness_type.eq_ignore_ascii_case(&c.harness_type))
                    .map(|p| p.name.clone())
                    .collect()
            };
            // Order is the drawing's, so compare as sets: the definition file
            // lists members in its own order and a reorder is not a drift.
            let matches_definition = definitions.get(&key).map(|d| {
                let a: BTreeSet<&str> = d.iter().map(String::as_str).collect();
                let b: BTreeSet<&str> = entries.iter().map(String::as_str).collect();
                a == b
            });
            out.push(HarnessInfo {
                harness_type: c.harness_type.clone(),
                entries,
                sheet: s.info.filename.clone(),
                ports,
                matches_definition,
            });
        }
    }
    out
}

/// Depth-first walk of the sheet hierarchy, assigning sequential sheet numbers.
#[allow(clippy::too_many_arguments)]
fn add_sheet(
    idx: usize,
    parsed: &[(PathBuf, SchDoc)],
    by_stem: &BTreeMap<String, usize>,
    sheet_path: &str,
    sheet_path_uuids: &str,
    out: &mut Vec<LoadedSheet>,
    counter: &mut i64,
    visited: &mut HashSet<String>,
    seen_docs: &mut HashSet<usize>,
    depth: usize,
    unresolved: &mut Unresolved,
    emit: &mut dyn FnMut(crate::pipeline::Msg),
) {
    if depth > MAX_SHEET_DEPTH || !visited.insert(sheet_path_uuids.to_string()) {
        return;
    }
    seen_docs.insert(idx);
    let (path, sch) = &parsed[idx];
    *counter += 1;
    let params: BTreeMap<String, String> = design::sheet_params(sch);
    let param = |k: &str| {
        params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    out.push(LoadedSheet {
        info: SheetInfo {
            filename: display(path),
            path: path.to_string_lossy().into_owned(),
            sheet_number: *counter,
            sheet_path: sheet_path.to_string(),
            sheet_path_uuids: sheet_path_uuids.to_string(),
            title: param("Title"),
            page: param("SheetNumber"),
            notes: sch.notes.clone(),
            company: param("Organization"),
            rev: param("Revision"),
            date: param("ApprovedDate"),
        },
        sch: sch.clone(),
    });

    let children: Vec<(String, String, String)> = parsed[idx]
        .1
        .sheet_symbols
        .iter()
        .map(|s| (s.filename.clone(), s.name.clone(), s.uuid.clone()))
        .collect();
    for (filename, name, uuid) in children {
        if filename.is_empty() {
            continue;
        }
        let Some(&child) = by_stem.get(&lower_stem(&filename)) else {
            unresolved.missing_sheets.push(filename.clone());
            emit(crate::pipeline::Msg::Progress(format!(
                "sheet symbol references a document not in the project: {filename}"
            )));
            continue;
        };
        let label = if name.is_empty() { stem_of(&filename) } else { name };
        add_sheet(
            child,
            parsed,
            by_stem,
            &format!("{sheet_path}{label}/"),
            &format!("{sheet_path_uuids}{uuid}/"),
            out,
            counter,
            visited,
            seen_docs,
            depth + 1,
            unresolved,
            emit,
        );
    }
}

/// Name every sheet entry with no matching child port, and every child port with
/// no matching entry — hierarchical links that silently do not connect.
fn check_hierarchy_links(sheets: &[LoadedSheet], unresolved: &mut Unresolved) {
    // Ports each sheet instance offers, keyed by its uuid chain.
    let ports: BTreeMap<&str, BTreeSet<String>> = sheets
        .iter()
        .map(|s| {
            (
                s.info.sheet_path_uuids.as_str(),
                s.sch.ports.iter().map(|p| p.name.to_ascii_uppercase()).collect(),
            )
        })
        .collect();
    for s in sheets {
        for sym in &s.sch.sheet_symbols {
            let child = format!("{}{}/", s.info.sheet_path_uuids, sym.uuid);
            let Some(child_ports) = ports.get(child.as_str()) else {
                continue;
            };
            let names: BTreeSet<String> =
                sym.entries.iter().map(|e| e.name.to_ascii_uppercase()).collect();
            for e in &sym.entries {
                if !child_ports.contains(&e.name.to_ascii_uppercase()) {
                    unresolved
                        .sheet_entries_without_port
                        .push(format!("{}{}:{}", s.info.sheet_path, sym.name, e.name));
                }
            }
            for p in child_ports.difference(&names) {
                unresolved
                    .ports_without_sheet_entry
                    .push(format!("{}{}:{p}", s.info.sheet_path, sym.name));
            }
        }
    }
    unresolved.sheet_entries_without_port.sort();
    unresolved.sheet_entries_without_port.dedup();
    unresolved.ports_without_sheet_entry.sort();
    unresolved.ports_without_sheet_entry.dedup();
}

/// Assemble the design model from every sheet instance.
#[allow(clippy::too_many_arguments)]
pub fn build_design(
    project_name: &str,
    project_path: &str,
    project_filename: &str,
    sheets: &[LoadedSheet],
    options: &CompileOptions,
    unresolved: Unresolved,
    free_document: bool,
    variants: &[eda_parse_altium::Variant],
    project_params: &BTreeMap<String, String>,
) -> (crate::design::Design, SourceInfo) {
    let channels = channels_of(sheets, options);
    let mut placements = Vec::new();
    let mut frags = Vec::new();
    let mut diag = netlist::SheetDiagnostics::default();
    for (s, channel) in sheets.iter().zip(&channels) {
        placements.extend(design::build_components_on(
            &s.sch,
            &s.info.sheet_path,
            &s.info.sheet_path_uuids,
            channel.as_ref(),
            project_params,
        ));
        frags.extend(netlist::fragments(
            &s.sch,
            &s.info.sheet_path_uuids,
            options,
            channel.as_ref(),
            &mut diag,
        ));
    }
    // Parts of one multi-part symbol can sit on different sheets, so the
    // grouping runs over the whole design rather than per sheet.
    let (mut components, extra_svg_ids) = design::group_parts(placements);
    let mut nets = crate::netlist::merge_frags(frags);
    netlist::consolidate_case(&mut nets);

    let mut unresolved = unresolved;
    let resolved = resolve_variants(variants, &mut components, &mut unresolved);
    unresolved.unconnected_pins = diag.unconnected_pins;
    unresolved.pins_marked_no_connect = diag.pins_marked_no_connect;
    unresolved.hidden_supply_pins = diag.hidden_supply_pins;
    unresolved.hidden_pins_without_net = diag.hidden_pins_without_net;
    unresolved.pins_joined_by_hot_spot = diag.pins_joined_by_hot_spot;
    unresolved.hot_spot_ambiguous = diag.hot_spot_ambiguous;
    unresolved.unnamed_labels = diag.unnamed_labels;
    unresolved.directives_without_a_class = diag.directives_without_a_class;
    unresolved.directives_without_a_net = diag.directives_without_a_net;
    unresolved.bus_range_links_not_made = diag.bus_range_links_not_made;

    let infos: Vec<SheetInfo> = sheets.iter().map(|s| s.info.clone()).collect();
    let mut model = crate::design::assemble(
        project_name,
        project_path,
        project_filename,
        infos,
        components,
        nets,
        bus_aliases(sheets),
    );
    for (uuid, designator) in extra_svg_ids {
        model.indexes.svg_to_component.insert(uuid, designator);
    }
    let source = SourceInfo {
        tool: "altium".to_string(),
        compile: CompileSettings::new(options, free_document),
        board_rules: Vec::new(),
        board_stackup: None,
        harnesses: Vec::new(),
        embedded_boards: Vec::new(),
        variants: resolved,
        unresolved,
    };
    (model, source)
}

/// Resolve every variant against the components the design actually places.
///
/// A variation names a placed designator (`R37_1`, channel suffix included) and
/// the sheet-symbol chain that reaches it. Altium keeps stale entries — a part
/// deleted from the schematic leaves its variation behind, and an unannotated
/// one reads `R?` — so a variation naming nothing is reported rather than
/// silently carried into a build list.
///
/// The base design is not changed by any of this. `kicad_dnp` stays false on
/// every component, because in the schematic as drawn every part is fitted; a
/// review that wants a specific build reads the variant it names.
fn resolve_variants(
    variants: &[eda_parse_altium::Variant],
    components: &mut [crate::design::Component],
    unresolved: &mut Unresolved,
) -> Vec<VariantInfo> {
    let placed: BTreeSet<&str> = components.iter().map(|c| c.designator.as_str()).collect();
    let mut out = Vec::new();
    for v in variants {
        let mut not_fitted: Vec<String> = Vec::new();
        let mut alternate_parts: BTreeMap<String, String> = BTreeMap::new();
        for x in &v.variations {
            if !placed.contains(x.designator.as_str()) {
                unresolved
                    .variations_without_a_part
                    .push(format!("{}:{}", v.name, x.designator));
                continue;
            }
            match x.fitting {
                eda_parse_altium::Fitting::NotFitted => not_fitted.push(x.designator.clone()),
                eda_parse_altium::Fitting::Alternate if !x.alternate_part.is_empty() => {
                    alternate_parts.insert(x.designator.clone(), x.alternate_part.clone());
                }
                _ => {}
            }
        }
        not_fitted.sort();
        not_fitted.dedup();
        unresolved.variant_parameter_overrides_unread += v.parameter_overrides_unread;
        out.push(VariantInfo {
            name: v.name.clone(),
            uid: v.uid.clone(),
            allow_fabrication: v.allow_fabrication,
            not_fitted,
            alternate_parts,
        });
    }
    unresolved.variations_without_a_part.sort();
    unresolved.variations_without_a_part.dedup();

    // Say it on the part as well as in the variant list. A BOM row that reads
    // "not fitted in Default_Assembly" needs no cross-reference, and a review
    // that only ever sees the row still sees the fact.
    for c in components.iter_mut() {
        let names = |f: &dyn Fn(&VariantInfo) -> bool| -> String {
            out.iter()
                .filter(|v| f(v))
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let off = names(&|v: &VariantInfo| v.not_fitted.iter().any(|d| *d == c.designator));
        if !off.is_empty() {
            c.parameters.insert(NOT_FITTED_PARAM.into(), off);
        }
        let alt = names(&|v: &VariantInfo| v.alternate_parts.contains_key(&c.designator));
        if !alt.is_empty() {
            c.parameters.insert(ALTERNATE_IN_PARAM.into(), alt);
        }
    }
    out
}

/// Bus alias definitions from bus-range labels (`GATE_[1...12]`).
///
/// Surfaced as review context only — never synthesised into `nets`, the same
/// contract the KiCad path keeps, because member names are bus-local and a
/// bare-name union would silently short unrelated nets.
fn bus_aliases(sheets: &[LoadedSheet]) -> Vec<crate::design::BusAliasInfo> {
    let mut out: Vec<crate::design::BusAliasInfo> = Vec::new();
    for s in sheets {
        // A bus range is written wherever a bus is named: on a net label, on a
        // port, and on the sheet entry that carries it between sheets — which is
        // where most of them are, so scanning labels alone finds almost none.
        let labels = s.sch.net_labels.iter().map(|l| l.text.as_str());
        let ports = s.sch.ports.iter().map(|p| p.name.as_str());
        let entries = s
            .sch
            .sheet_symbols
            .iter()
            .flat_map(|y| y.entries.iter())
            .map(|e| e.name.as_str());
        for text in labels.chain(ports).chain(entries) {
            let Some((name, members)) = bus_range(text) else {
                continue;
            };
            out.push(crate::design::BusAliasInfo { name, members });
        }
    }
    out.sort_by(|a, b| (&a.name, &a.members).cmp(&(&b.name, &b.members)));
    out.dedup();
    out
}

/// Split a bus-range label into its name and members. `GATE_[1...12]` is
/// `GATE_` over `GATE_1 … GATE_12`. Ranges are capped so a malformed label
/// cannot allocate without bound.
pub(crate) fn bus_range(text: &str) -> Option<(String, Vec<String>)> {
    const MAX_MEMBERS: i64 = 1024;
    let open = text.find('[')?;
    let close = text.rfind(']')?;
    if close < open {
        return None;
    }
    let inner = &text[open + 1..close];
    let dots = inner.find("..")?;
    let lo: i64 = inner[..dots].trim().parse().ok()?;
    let hi: i64 = inner[dots..].trim_start_matches('.').trim().parse().ok()?;
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    if hi - lo + 1 > MAX_MEMBERS {
        return None;
    }
    let stem = &text[..open];
    Some((
        text.to_string(),
        (lo..=hi).map(|i| format!("{stem}{i}")).collect(),
    ))
}

/// The component list for a whole hierarchy, channels applied. Shared by the
/// design model and the BOM so the two cannot disagree on a designator.
pub fn build_components(
    sheets: &[LoadedSheet],
    options: &CompileOptions,
    project_params: &BTreeMap<String, String>,
) -> Vec<crate::design::Component> {
    let channels = channels_of(sheets, options);
    let mut placements = Vec::new();
    for (s, channel) in sheets.iter().zip(&channels) {
        placements.extend(design::build_components_on(
            &s.sch,
            &s.info.sheet_path,
            &s.info.sheet_path_uuids,
            channel.as_ref(),
            project_params,
        ));
    }
    design::group_parts(placements).0
}

/// Assign a channel to every sheet placement whose document is placed more than
/// once, in walk order.
///
/// This is the second of Altium's two channel mechanisms — multiple sheet
/// symbols referencing the same child document, with no `REPEAT` anywhere — and
/// it is the one the corpus uses. Missing it collapses every channel onto one
/// designator, and the BOM loses a whole channel's parts.
fn channels_of(sheets: &[LoadedSheet], options: &CompileOptions) -> Vec<Option<design::Channel>> {
    let mut placements: BTreeMap<&str, usize> = BTreeMap::new();
    for s in sheets {
        *placements.entry(s.info.path.as_str()).or_default() += 1;
    }
    let mut seen: BTreeMap<&str, i64> = BTreeMap::new();
    sheets
        .iter()
        .map(|s| {
            if placements.get(s.info.path.as_str()).copied().unwrap_or(0) < 2 {
                return None;
            }
            let n = seen.entry(s.info.path.as_str()).or_default();
            *n += 1;
            Some(design::Channel {
                index: *n,
                name: s
                    .info
                    .sheet_path
                    .trim_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string(),
                format: options.channel_designator_format.clone(),
            })
        })
        .collect()
}

/// File name of a path, as a display string.
fn display(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

fn stem_of(file: &str) -> String {
    Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .to_string()
}

/// Sheet symbols name their child by file name; match on the lower-cased stem so
/// `Gate_Drv.SchDoc` finds `gate_drv.schdoc` on a case-sensitive filesystem.
fn lower_stem(file: &str) -> String {
    stem_of(&file.replace('\\', "/")).to_ascii_lowercase()
}

/// Altium writes Windows paths with Windows casing; a case-sensitive host needs
/// the directory scanned to find the real file.
fn resolve_case_insensitive(p: &Path) -> Option<PathBuf> {
    if p.exists() {
        return Some(p.to_path_buf());
    }
    let dir = p.parent()?;
    let want = p.file_name()?.to_str()?.to_ascii_lowercase();
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|c| {
            c.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase() == want)
                .unwrap_or(false)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_altium_inputs() {
        assert!(is_altium_project(Path::new("a/B.PrjPcb")));
        assert!(is_altium_project(Path::new("a/B.SchDoc")));
        assert!(is_altium_project(Path::new("a/B.pcbdoc")));
        assert!(!is_altium_project(Path::new("a/B.kicad_pro")));
    }

    /// A bus-range label is review context: the bundle records the bundle and
    /// its members, and never unions them into one net.
    #[test]
    fn bus_range_labels_become_aliases() {
        let (name, members) = bus_range("GATE_[1...12]").expect("range");
        assert_eq!(name, "GATE_[1...12]");
        assert_eq!(members.len(), 12);
        assert_eq!(members[0], "GATE_1");
        assert_eq!(members[11], "GATE_12");
        assert_eq!(bus_range("D[0..7]").unwrap().1, ["D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7"]);
        assert!(bus_range("PLAIN_LABEL").is_none());
        assert!(bus_range("HUGE[0...99999]").is_none(), "a malformed range is refused");
    }

    #[test]
    fn child_documents_match_on_the_lower_cased_stem() {
        assert_eq!(lower_stem("Gate_Drv.SchDoc"), "gate_drv");
        assert_eq!(lower_stem("sub\\Gate Driver_A.SchDoc"), "gate driver_a");
    }
}
