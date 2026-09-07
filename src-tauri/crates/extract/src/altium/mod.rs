//! Altium front-end for the extraction pipeline.
//!
//! Everything here builds the *same* public structs the KiCad path builds
//! (`design::Component`, `netlist::Frag`, `design::Design`), so the two sources
//! cannot drift on schema: a schema change breaks both builders at once.

pub mod design;
pub mod dump;
pub mod netlist;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use eda_parse_altium::{sch, CompileOptions, Doc, Kind, Project, SchDoc};
use serde::Serialize;

use crate::design::SheetInfo;

/// Maximum hierarchy depth walked — a backstop against pathological cycles.
const MAX_SHEET_DEPTH: usize = 32;

/// Harness record types (§4.4). Recorded, not expanded, until M5.
const HARNESS_RECORDS: [i64; 4] = [215, 216, 217, 218];

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
    /// Harness objects seen. Recorded, not expanded.
    pub harness_objects: usize,
    /// Parameter sets (net-class and other directives) seen but not applied to
    /// `net_name_to_classes`.
    pub directives_not_applied: usize,
    /// `RECORD=211` regions. The plan reads these as compile masks; in this
    /// corpus every component inside one is also on the board, so applying them
    /// would drop 65 real parts. They are reported, not applied — see
    /// docs/altium-extraction-notes.md.
    pub compile_mask_candidates_not_applied: usize,
    /// Project variants defined but not applied, so the DNP column is empty.
    pub variants_not_applied: Vec<String>,
    /// Component pins on a net of their own — no wire, label or port at their
    /// location. A connectivity-model problem shows up here as a number.
    pub unconnected_pins: usize,
    /// Pins connected implicitly as hidden supply pins.
    pub hidden_supply_pins: usize,
    /// Record types seen but not modelled, by type and count.
    pub skipped_records: BTreeMap<String, usize>,
}

/// Source-tool block added to the design model for Altium designs.
#[derive(Debug, Clone, Serialize)]
pub struct SourceInfo {
    pub tool: String,
    pub compile: CompileSettings,
    pub unresolved: Unresolved,
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
pub fn load_hierarchy(
    project: &Path,
    emit: &mut dyn FnMut(crate::pipeline::Msg),
) -> Result<(String, CompileOptions, Vec<LoadedSheet>, Unresolved), String> {
    use crate::pipeline::Msg;
    let mut unresolved = Unresolved::default();
    let dir = project.parent().map(Path::to_path_buf).unwrap_or_default();
    let is_prj = project
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("PrjPcb"))
        .unwrap_or(false);

    let (name, options, candidates) = if is_prj {
        let prj = Project::open(project)?;
        unresolved.variants_not_applied = prj.variant_names();
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
        unresolved.directives_not_applied += s.sch.param_sets.len();
        unresolved.compile_mask_candidates_not_applied += s.sch.regions.len();
        for (t, n) in &s.sch.skipped {
            if HARNESS_RECORDS.contains(t) {
                unresolved.harness_objects += n;
            }
            *unresolved
                .skipped_records
                .entry(format!("RECORD={t}"))
                .or_default() += n;
        }
    }
    Ok((name, options, out, unresolved))
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
pub fn build_design(
    project_name: &str,
    project_path: &str,
    project_filename: &str,
    sheets: &[LoadedSheet],
    options: &CompileOptions,
    unresolved: Unresolved,
    free_document: bool,
) -> (crate::design::Design, SourceInfo) {
    let channels = channels_of(sheets, options);
    let mut components = Vec::new();
    let mut extra_svg_ids: Vec<(String, String)> = Vec::new();
    let mut frags = Vec::new();
    let mut diag = netlist::SheetDiagnostics::default();
    for (s, channel) in sheets.iter().zip(&channels) {
        let (mut c, extra) = design::build_components_on(
            &s.sch,
            &s.info.sheet_path,
            &s.info.sheet_path_uuids,
            channel.as_ref(),
        );
        components.append(&mut c);
        extra_svg_ids.extend(extra);
        frags.extend(netlist::fragments(
            &s.sch,
            &s.info.sheet_path_uuids,
            options,
            channel.as_ref(),
            &mut diag,
        ));
    }
    let mut nets = crate::netlist::merge_frags(frags);
    netlist::consolidate_case(&mut nets);

    let mut unresolved = unresolved;
    unresolved.unconnected_pins = diag.unconnected_pins;
    unresolved.hidden_supply_pins = diag.hidden_supply_pins;

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
        unresolved,
    };
    (model, source)
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
fn bus_range(text: &str) -> Option<(String, Vec<String>)> {
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
) -> Vec<crate::design::Component> {
    let channels = channels_of(sheets, options);
    let mut out = Vec::new();
    for (s, channel) in sheets.iter().zip(&channels) {
        let (mut c, _) = design::build_components_on(
            &s.sch,
            &s.info.sheet_path,
            &s.info.sheet_path_uuids,
            channel.as_ref(),
        );
        out.append(&mut c);
    }
    out
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
