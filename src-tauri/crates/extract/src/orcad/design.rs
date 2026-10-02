//! Capture parts projected into `design::Component`.
//!
//! A placed part is one SECTION of a package (`U1A`, `U1B`); the BOM wants the
//! package. Sections are grouped by designator across the whole design with the
//! grouping the Altium path already uses, so a gate placed on three pages is
//! one component with one pin count.

use std::collections::BTreeMap;

use eda_parse_orcad::capture::cache::Package;
use eda_parse_orcad::capture::page::PlacedPart;
use eda_parse_orcad::capture::symbol::SymbolDef;
use eda_parse_orcad::capture::CaptureDoc;
use serde::Serialize;

use super::{mm, occurrence_of, SheetInstance, Unresolved};
use crate::altium::design::{group_parts, Placement};
use crate::design::{Bbox, Classification, Component, Hierarchy};

/// Parameter key naming the Capture library a part came from.
pub const SOURCE_LIB_PARAM: &str = "orcad_source_library";
/// Parameter key naming the package section a placement draws.
pub const SECTION_PARAM: &str = "orcad_section";

/// One variant the design defines (CIS), as the design model reports it.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VariantInfo {
    pub name: String,
    /// Designators the variant leaves unfitted: the members of the part
    /// groups it selects whose name says do-not-fit (see [`is_dnf_group`]).
    pub not_fitted: Vec<String>,
    /// Designator -> property overrides the variant applies.
    pub overrides: BTreeMap<String, BTreeMap<String, String>>,
    /// The part groups the variant selects, as stored.
    pub groups: Vec<VariantGroup>,
    /// Occurrence ids the CIS data names that no part in the design carries
    /// (CIS data left behind by deleted parts).
    pub stale_ids: usize,
}

/// One CIS part group, its members resolved to designators.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VariantGroup {
    pub name: String,
    /// Members whose stored state is 1.
    pub members: Vec<String>,
    /// Members whose stored state is 0. What the state means beyond the
    /// do-not-fit groups is not established (see the notes).
    pub state_zero: Vec<String>,
}

/// A group whose name marks its parts as not fitted. Measured on TI's
/// LAUNCHXL-CC1310: the members of its `DNM` group are exactly the parts TI's
/// released BOM lists as DNM.
pub fn is_dnf_group(name: &str) -> bool {
    let n = name.trim().to_ascii_uppercase().replace(['_', '-', ' '], "");
    matches!(n.as_str(), "DNM" | "DNP" | "DNS" | "DNI" | "DNF" | "DONOTMOUNT" | "DONOTPOPULATE" | "DONOTSTUFF" | "DONOTFIT" | "NOTFITTED")
}

/// The stable handle of a placed object on a page.
pub fn part_id(db_id: u32) -> String {
    format!("oc:{db_id}")
}

/// Everything known about one placed part in one sheet instance.
pub struct Resolved<'a> {
    pub part: &'a PlacedPart,
    /// The package section this placement draws in this occurrence. An
    /// occurrence may override the instance's own section: a gate placed once
    /// in a reused folder is section A in one occurrence and B in the next.
    pub device: usize,
    pub designator: String,
    /// Section suffix (`A`), empty for a single-section package.
    pub section: String,
    pub symbol: Option<&'a SymbolDef>,
    pub package: Option<&'a Package>,
    /// Effective properties, lowest precedence first: package, device, the
    /// placed instance, then the occurrence's overrides.
    pub props: BTreeMap<String, String>,
}

impl Resolved<'_> {
    /// Pin number of a symbol pin slot: the package section's number, else
    /// the symbol pin's own name.
    pub fn pin_number(&self, slot: usize) -> Option<String> {
        if let Some(pkg) = self.package {
            if let Some(dev) = pkg.devices.get(self.device) {
                if let Some(Some(n)) = dev.pins.get(slot) {
                    if !n.number.is_empty() {
                        return Some(n.number.clone());
                    }
                }
            }
        }
        let p = self.symbol?.pins.iter().find(|p| p.slot == slot)?;
        Some(p.name.clone())
    }

    pub fn prop(&self, k: &str) -> Option<&str> {
        self.props.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

/// Resolve a placed part in the context of its sheet instance.
pub fn resolve<'a>(doc: &'a CaptureDoc, inst: &SheetInstance, part: &'a PlacedPart) -> Resolved<'a> {
    let occ = occurrence_of(inst.scope.as_ref(), part.db_id);
    let symbol = doc.symbol_for(part);
    let package = doc.cache.package(&part.package);
    let device = occ
        .and_then(|o| o.unit.as_deref())
        .and_then(|u| package.and_then(|p| p.devices.iter().position(|d| d.unit == u)))
        .unwrap_or(part.unit_index as usize);
    let mut props: BTreeMap<String, String> = BTreeMap::new();
    let mut put = |k: &str, v: &str| {
        if k.is_empty() {
            return;
        }
        // Keys are case-insensitive in Capture; keep the first spelling seen.
        let key = props.keys().find(|x| x.eq_ignore_ascii_case(k)).cloned().unwrap_or_else(|| k.to_string());
        props.insert(key, v.to_string());
    };
    if let Some(p) = package {
        for (k, v) in &p.props {
            put(k, v);
        }
        if let Some(d) = p.devices.get(device) {
            for (k, v) in &d.props {
                put(k, v);
            }
        }
    }
    for (k, v) in &part.props {
        put(k, v);
    }
    if let Some(o) = occ {
        for (k, v) in &o.props {
            put(k, v);
        }
    }
    let designator = occ
        .map(|o| o.reference.clone())
        .filter(|r| !r.is_empty())
        .or_else(|| props.get("Part Reference").cloned().filter(|r| !r.is_empty()))
        .unwrap_or_else(|| part.reference.clone());
    // A displayed reference carries the section suffix (`U1A`); the package
    // designator does not.
    let section = occ
        .and_then(|o| o.unit.clone())
        .or_else(|| package.and_then(|p| p.devices.get(device)).map(|d| d.unit.clone()))
        .unwrap_or_default();
    let multi = package.map(|p| p.devices.len() > 1).unwrap_or(false);
    let designator = if multi && !section.is_empty() && designator.ends_with(&section) && designator.len() > section.len() {
        let base = &designator[..designator.len() - section.len()];
        if base.ends_with(|c: char| c.is_ascii_digit()) {
            base.to_string()
        } else {
            designator
        }
    } else {
        designator
    };
    // Capture's netlister upper-cases every designator, and the board the
    // netlist feeds carries that spelling; the bundle uses it so schematic and
    // board name a part the same way. The sheet still draws it as typed.
    let designator = designator.to_uppercase();
    Resolved {
        part,
        device,
        designator,
        section: if multi { section } else { String::new() },
        symbol,
        package,
        props,
    }
}

/// Placed body box in bundle millimetres: the cached symbol's body box taken
/// through the placement.
pub fn body_bbox(part: &PlacedPart, symbol: Option<&SymbolDef>) -> Option<Bbox> {
    let s = symbol?;
    let (x1, y1, x2, y2) = s.bbox;
    if x1 == x2 && y1 == y2 {
        return None;
    }
    let body = (x1 as i32, y1 as i32, x2 as i32, y2 as i32);
    let pts: Vec<(i32, i32)> = [(x1, y1), (x2, y1), (x2, y2), (x1, y2)]
        .iter()
        .map(|&(x, y)| part.orient.place((x as i32, y as i32), part.pos, body))
        .collect();
    let (mut a, mut b) = (pts[0], pts[0]);
    for p in &pts {
        a = (a.0.min(p.0), a.1.min(p.1));
        b = (b.0.max(p.0), b.1.max(p.1));
    }
    let r = |v: f64| (v * 1000.0).round() / 1000.0;
    Some(Bbox { x: r(mm(a.0)), y: r(mm(a.1)), w: r(mm(b.0 - a.0)), h: r(mm(b.1 - a.1)) })
}

fn library_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// Every placed part of every sheet instance, sections grouped by designator.
pub fn build_components(
    doc: &CaptureDoc,
    sheets: &[SheetInstance],
    unresolved: &mut Unresolved,
) -> (Vec<Component>, Vec<(String, String)>) {
    let mut placements = Vec::new();
    for inst in sheets {
        let page = &doc.folders[inst.folder].pages[inst.page];
        for part in &page.parts {
            let r = resolve(doc, inst, part);
            if r.symbol.is_none() {
                unresolved.parts_without_symbol.push(format!("{} ({})", r.designator, part.cache_name));
            }
            if r.package.is_none() {
                unresolved.parts_without_package.push(format!("{} ({})", r.designator, part.package));
            }
            if r.designator.ends_with('?') || r.designator.is_empty() {
                unresolved.unannotated_parts.push(format!("{} on {}", r.designator, inst.info.sheet_path));
            }
            let pins: Vec<String> = part.pins.iter().filter_map(|p| r.pin_number(p.slot())).collect();
            let prefix = crate::design::prefix_of(&r.designator);
            let value = r.prop("Value").map(str::to_string).filter(|v| !v.is_empty()).unwrap_or_else(|| part.value.clone());
            let footprint = r
                .prop("PCB Footprint")
                .map(str::to_string)
                .filter(|v| !v.is_empty())
                .or_else(|| r.package.map(|p| p.footprint.clone()))
                .unwrap_or_default();
            let library_ref = if !part.package.is_empty() {
                part.package.clone()
            } else {
                part.cache_name.trim_end_matches(".Normal").trim_end_matches(".Convert").to_string()
            };
            // An empty description collapses unrelated parts onto one BOM
            // line, so the library part's own name stands in for a missing one.
            let description = r
                .prop("Description")
                .map(str::to_string)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| library_ref.clone());
            let mut parameters: BTreeMap<String, String> = r.props.clone();
            parameters.insert("Value".into(), value.clone());
            parameters.insert("Footprint".into(), footprint.clone());
            if !part.source_lib.is_empty() {
                parameters.insert(SOURCE_LIB_PARAM.into(), library_name(&part.source_lib));
            }
            if !r.section.is_empty() {
                parameters.insert(SECTION_PARAM.into(), r.section.clone());
            }
            parameters.insert("kicad_in_bom".into(), "true".into());
            parameters.insert("kicad_dnp".into(), "false".into());
            parameters.insert("kicad_on_board".into(), "true".into());
            let pin_count = {
                let mut v = pins.clone();
                v.sort();
                v.dedup();
                v.len() as u32
            };
            placements.push(Placement {
                component: Component {
                    designator: r.designator.clone(),
                    svg_id: part_id(part.db_id),
                    value,
                    footprint,
                    library_ref,
                    description,
                    hierarchy: Hierarchy {
                        base_designator: r.designator.clone(),
                        channel: None,
                        channel_index: None,
                        sheet: inst.info.sheet_path.clone(),
                        sheet_path: inst.info.sheet_path.clone(),
                        sheet_path_uuids: inst.info.sheet_path_uuids.clone(),
                    },
                    classification: Classification {
                        prefix: prefix.clone(),
                        kind: crate::design::classify(&prefix, pin_count).to_string(),
                        pin_count,
                    },
                    parameters,
                    bbox: body_bbox(part, r.symbol),
                },
                part_id: r.device as i64 + 1,
                pins,
            });
        }
    }
    unresolved.unannotated_parts.sort();
    unresolved.unannotated_parts.dedup();
    group_parts(placements)
}

/// Parameter naming the CIS variants that leave a part off. The base design
/// (every part fitted) stays what `components` describes, as on the Altium
/// path.
pub const NOT_FITTED_PARAM: &str = "orcad_not_fitted_in";

/// Occurrence id -> designator, for every placed part in every sheet
/// instance. CIS keys its records by occurrence id: the occurrence's own id in
/// an occurrence-annotated design, the placed part's id otherwise.
pub fn occurrence_designators(doc: &CaptureDoc, sheets: &[SheetInstance]) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    for inst in sheets {
        let page = &doc.folders[inst.folder].pages[inst.page];
        for part in &page.parts {
            let r = resolve(doc, inst, part);
            if let Some(o) = occurrence_of(inst.scope.as_ref(), part.db_id) {
                out.insert(o.own_db_id, r.designator.clone());
            }
            out.entry(part.db_id).or_insert(r.designator);
        }
    }
    out
}

/// The CIS variants a design defines, resolved to designators. Each part a
/// variant leaves off also names the variant in [`NOT_FITTED_PARAM`].
pub fn variants(doc: &CaptureDoc, sheets: &[SheetInstance], components: &mut [Component]) -> Vec<VariantInfo> {
    let Some(cis) = &doc.cis else { return Vec::new() };
    let ids = occurrence_designators(doc, sheets);
    let mut out = Vec::new();
    for v in &cis.variants {
        let mut info = VariantInfo { name: v.name.clone(), ..Default::default() };
        for g in v.groups.iter().filter_map(|g| cis.groups.get(g)) {
            let mut vg = VariantGroup { name: g.name.clone(), ..Default::default() };
            for (on, id) in &g.members {
                match ids.get(id) {
                    Some(d) if *on => vg.members.push(d.clone()),
                    Some(d) => vg.state_zero.push(d.clone()),
                    None => info.stale_ids += 1,
                }
            }
            if is_dnf_group(&g.name) {
                info.not_fitted.extend(vg.members.iter().chain(&vg.state_zero).cloned());
            }
            for (id, props) in &g.updates {
                if let Some(d) = ids.get(id) {
                    info.overrides.entry(d.clone()).or_default().extend(props.iter().cloned());
                }
            }
            vg.members.sort();
            vg.state_zero.sort();
            info.groups.push(vg);
        }
        info.not_fitted.sort();
        info.not_fitted.dedup();
        for c in components.iter_mut().filter(|c| info.not_fitted.binary_search(&c.designator).is_ok()) {
            let e = c.parameters.entry(NOT_FITTED_PARAM.to_string()).or_default();
            if !e.is_empty() {
                e.push(';');
            }
            e.push_str(&v.name);
        }
        out.push(info);
    }
    out
}
