//! The projected design model — the structured view emitted as `*_design.json`
//! and consumed by the viewer app and the AI review skills. JSON field names are
//! the contract those consumers already read; the Rust types are our own.

use std::collections::{BTreeMap, BTreeSet};

use eda_parse_kicad::schematic::{Pt, Schematic, SymbolInstance};
use serde::Serialize;

use crate::geom;
use crate::netlist::{self, Graphical, Terminal};

/// An axis-aligned region on a sheet, in schematic millimetres. Fills the
/// previously-always-NULL component bbox and lets the viewer crop to a region
/// instead of rasterizing a whole sheet.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Bbox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bbox {
    fn from_points(pts: &[(f64, f64)]) -> Option<Bbox> {
        let (mut minx, mut miny) = *pts.first()?;
        let (mut maxx, mut maxy) = (minx, miny);
        for &(x, y) in &pts[1..] {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        // Trim to micrometre precision to keep the JSON compact.
        let r = |v: f64| (v * 1000.0).round() / 1000.0;
        Some(Bbox { x: r(minx), y: r(miny), w: r(maxx - minx), h: r(maxy - miny) })
    }

    fn corners(&self) -> [(f64, f64); 2] {
        [(self.x, self.y), (self.x + self.w, self.y + self.h)]
    }

    /// Smallest box covering both.
    fn union(self, other: Bbox) -> Bbox {
        let [(ax0, ay0), (ax1, ay1)] = self.corners();
        let [(bx0, by0), (bx1, by1)] = other.corners();
        Bbox::from_points(&[
            (ax0.min(bx0), ay0.min(by0)),
            (ax1.max(bx1), ay1.max(by1)),
        ])
        .unwrap()
    }
}

/// Bounding box of a placed symbol: transform the library body box corners
/// through the placement and take their extent.
fn placed_bbox(sym: &SymbolInstance, min: Pt, max: Pt) -> Bbox {
    let corners = [
        (min.x, min.y),
        (max.x, min.y),
        (max.x, max.y),
        (min.x, max.y),
    ];
    let pts: Vec<(f64, f64)> = corners
        .iter()
        .map(|&(px, py)| {
            geom::place_mm(sym.at.x, sym.at.y, sym.at.angle, sym.mirror.as_deref(), px, py)
        })
        .collect();
    Bbox::from_points(&pts).expect("four corners")
}

/// How a symbol is bucketed for review heuristics.
#[derive(Debug, Clone, Serialize)]
pub struct Classification {
    pub prefix: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub pin_count: u32,
}

/// Where a component sits in the sheet hierarchy.
#[derive(Debug, Clone, Serialize)]
pub struct Hierarchy {
    pub base_designator: String,
    pub channel: Option<String>,
    pub channel_index: Option<i64>,
    pub sheet: String,
    pub sheet_path: String,
    pub sheet_path_uuids: String,
}

/// A placed component in the design model.
#[derive(Debug, Clone, Serialize)]
pub struct Component {
    pub designator: String,
    pub svg_id: String,
    pub value: String,
    pub footprint: String,
    pub library_ref: String,
    pub description: String,
    pub hierarchy: Hierarchy,
    pub classification: Classification,
    pub parameters: BTreeMap<String, String>,
    /// Placed bounding box on its sheet, when the library symbol has geometry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Bbox>,
}

/// Leading non-digit run of a designator (`"MH2" -> "MH"`, `"C12" -> "C"`).
pub fn prefix_of(designator: &str) -> String {
    designator
        .chars()
        .take_while(|c| !c.is_ascii_digit())
        .collect()
}

/// Bucket a symbol by prefix and pin count, matching the reference scheme.
///
/// Component families are recognised by designator prefix first (a `D` diode
/// array can carry more than two pins yet is still a passive part); only parts
/// with an unrecognised prefix fall back to the pin-count heuristic.
pub fn classify(prefix: &str, pin_count: u32) -> &'static str {
    match prefix {
        "MH" => "mounting_hole",
        "U" | "IC" | "A" | "AR" => "ic",
        "J" | "P" | "CN" | "CON" => "connector",
        "X" | "Y" | "XTAL" => "crystal",
        "R" | "C" | "L" | "D" | "CR" | "LED" | "FB" | "RV" => "passive_2pin",
        _ if pin_count == 0 => "unknown",
        _ if pin_count == 2 => "passive_2pin",
        _ => "unknown",
    }
}

/// True for power-flag symbols, which are net namers rather than components.
fn is_power_symbol(sch: &Schematic, sym: &SymbolInstance) -> bool {
    sym.lib_id.starts_with("power:")
        || sch.lib_for(sym).map(|l| l.power).unwrap_or(false)
}

/// Number of distinct pins on the placed unit of a symbol.
fn pin_count(sch: &Schematic, sym: &SymbolInstance) -> u32 {
    let Some(lib) = sch.lib_for(sym) else {
        return 0;
    };
    let mut nums: Vec<&str> = lib
        .pins
        .iter()
        .filter(|p| p.unit == 0 || p.unit == sym.unit)
        .map(|p| p.number.as_str())
        .collect();
    nums.sort_unstable();
    nums.dedup();
    nums.len() as u32
}

/// Resolve a placed symbol's designator for a specific sheet instance.
///
/// In a hierarchy the same symbol is instantiated once per sheet placement, each
/// with its own reference recorded in the `(instances)` block keyed by the chain
/// of sheet-instance uuids. We match the instance whose path ends with this
/// sheet's uuid chain (so repeated instantiations resolve to different refs),
/// falling back to the lone instance, then to the `Reference` property.
pub(crate) fn reference_for<'a>(sym: &'a SymbolInstance, sheet_path_uuids: &str) -> Option<&'a str> {
    let want: Vec<&str> = sheet_path_uuids.split('/').filter(|s| !s.is_empty()).collect();
    // Root sheet (`/`): the `Reference` property is authoritative and matches the
    // flat-design golden bundles exactly.
    if want.is_empty() {
        return sym.reference().or_else(|| {
            sym.instances
                .first()
                .map(|i| i.reference.as_str())
                .filter(|r| !r.is_empty())
        });
    }
    // Child sheet: pick the instance whose uuid chain ends with this sheet's, so
    // repeated instantiations resolve to their own reference.
    if let Some(ip) = sym.instances.iter().find(|i| {
        !i.reference.is_empty() && {
            let have: Vec<&str> = i.path.split('/').filter(|s| !s.is_empty()).collect();
            have.ends_with(&want)
        }
    }) {
        return Some(&ip.reference);
    }
    sym.reference()
}

/// Build the component list for one sheet instance. `sheet_path` is the human
/// path (e.g. `/Power/`) and `sheet_path_uuids` the uuid chain used to pick the
/// per-instance reference.
pub fn build_components_on(
    sch: &Schematic,
    sheet_path: &str,
    sheet_path_uuids: &str,
) -> Vec<Component> {
    let mut out = Vec::new();
    for sym in &sch.symbols {
        if is_power_symbol(sch, sym) {
            continue;
        }
        let designator = reference_for(sym, sheet_path_uuids).unwrap_or("").to_string();
        if designator.is_empty() || designator.starts_with('#') {
            continue;
        }
        let prefix = prefix_of(&designator);
        let pins = pin_count(sch, sym);
        let bbox = sch
            .lib_for(sym)
            .and_then(|l| l.bbox)
            .map(|(min, max)| placed_bbox(sym, min, max));
        let mut parameters: BTreeMap<String, String> = BTreeMap::new();
        for p in &sym.properties {
            // Reference/Value/Footprint/Datasheet/Description are surfaced as
            // first-class fields, but every property is retained for review.
            parameters.insert(p.key.clone(), p.value.clone());
        }
        parameters.insert("kicad_dnp".into(), bool_str(sym.dnp));
        parameters.insert("kicad_in_bom".into(), bool_str(sym.in_bom));
        parameters.insert("kicad_on_board".into(), bool_str(sym.on_board));

        out.push(Component {
            designator: designator.clone(),
            svg_id: sym.uuid.clone(),
            value: sym.property("Value").unwrap_or("").to_string(),
            footprint: sym.property("Footprint").unwrap_or("").to_string(),
            library_ref: sym.lib_id.clone(),
            description: sym.property("Description").unwrap_or("").to_string(),
            hierarchy: Hierarchy {
                base_designator: designator,
                channel: None,
                channel_index: None,
                sheet: sheet_path.to_string(),
                sheet_path: sheet_path.to_string(),
                sheet_path_uuids: sheet_path_uuids.to_string(),
            },
            classification: Classification {
                prefix: prefix.clone(),
                kind: classify(&prefix, pins).to_string(),
                pin_count: pins,
            },
            parameters,
            bbox,
        });
    }
    out.sort_by(|a, b| a.designator.cmp(&b.designator));
    out
}

/// Build the component list for a root/single sheet (`/` uuid path). Thin
/// wrapper over [`build_components_on`] kept for the BOM path and single-sheet
/// callers.
pub fn build_components(sch: &Schematic, sheet_path: &str) -> Vec<Component> {
    build_components_on(sch, sheet_path, sheet_path)
}

fn bool_str(b: bool) -> String {
    if b { "true".into() } else { "false".into() }
}

/// A bus alias surfaced in the design model: a named bundle and its member nets.
/// Provided as review context (and a foothold for future bus support); the netlist
/// does NOT synthesise nets from members — see [`eda_parse_kicad::schematic::BusAlias`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BusAliasInfo {
    pub name: String,
    pub members: Vec<String>,
}

/// Project identity in the design model.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub filename: String,
    pub path: String,
}

/// A sheet entry in the design model.
#[derive(Debug, Clone, Serialize)]
pub struct SheetInfo {
    pub filename: String,
    pub path: String,
    pub sheet_number: i64,
    pub sheet_path: String,
    pub sheet_path_uuids: String,
    pub title: String,
    /// KiCad page label for this sheet instance (`(sheet_instances … (page "N"))`).
    /// Empty when the project leaves page numbering automatic — the viewer then
    /// falls back to `sheet_number`.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub page: String,
    /// Free-text designer notes on the sheet — design-intent context for review.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<String>,
    /// Title-block doc-control fields, when present.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub company: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub rev: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub date: String,
}

/// A net as serialized into the design model.
#[derive(Debug, Clone, Serialize)]
pub struct DesignNet {
    pub uid: String,
    pub name: String,
    pub auto_named: bool,
    pub driver_kind: String,
    /// sheet_path_uuids of the sheets this net appears on (the viewer resolves
    /// these to sheet numbers).
    pub source_sheets: Vec<String>,
    pub terminals: Vec<Terminal>,
    /// SVG element uuids of this net, bucketed by kind, for cross-probing.
    pub graphical: Graphical,
    /// Region covering the net's connected components (approximate; per-sheet
    /// geometry is not unioned). Absent when no terminal has a bbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<Bbox>,
}

/// Lookup tables consumers use for cross-probing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Indexes {
    /// Schematic group uuid -> component designator (the component's own group).
    pub svg_to_component: BTreeMap<String, String>,
    /// Component designator -> the nets it connects to.
    pub component_to_nets: BTreeMap<String, Vec<String>>,
    /// Graphical element uuid -> net name (single-valued; last writer wins for a
    /// uuid shared across repeated sheet instances).
    pub svg_to_net: BTreeMap<String, String>,
    /// Graphical element uuid -> every net it maps to (multi-valued: a uuid reused
    /// across instantiations of one source sheet maps to one net per instance).
    pub svg_to_nets: BTreeMap<String, Vec<String>>,
    /// sheet_path_uuids -> (element uuid -> net names on that sheet).
    pub sheet_svg_to_nets: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

/// The full projected design model emitted as `*_design.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Design {
    pub schema: String,
    pub generator: String,
    pub project: ProjectInfo,
    pub sheets: Vec<SheetInfo>,
    pub components: Vec<Component>,
    pub nets: Vec<DesignNet>,
    pub net_name_to_classes: BTreeMap<String, Vec<String>>,
    /// Bus alias definitions (named bundles → member nets) gathered across all
    /// sheets, deduped. Not expanded into `nets`.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bus_aliases: Vec<BusAliasInfo>,
    pub indexes: Indexes,
    /// The user's active KiCad colour theme — the palette the viewer themes the
    /// monochrome SVGs with, derived from KiCad instead of hand-mirrored in CSS.
    /// Filled by the pipeline; empty when no KiCad config is reachable.
    pub theme: crate::theme::Theme,
    /// Project drawing defaults (line width, text size) from the `.kicad_pro`.
    pub drawing: crate::theme::Drawing,
}

/// Assemble the design model for a single (root) schematic sheet.
pub fn build_design(project_name: &str, project_path: &str, sheet: &SheetInfo, sch: &Schematic) -> Design {
    build_design_multi(project_name, project_path, &[(sheet.clone(), sch)], &BTreeMap::new())
}

/// Rename merged nets to the board's canonical net names. Each schematic net is
/// matched to the PCB by its component-pin terminals (`(designator, pin)` →
/// KiCad net name); when every matched terminal agrees on one board net, adopt
/// that name. A terminal set that maps to *two* board nets signals a real
/// connectivity discrepancy between schematic and layout — we leave such a net
/// under its schematic name rather than pick one arbitrarily. Nets with no board
/// terminal (schematic-only) keep their schematic name. No-op without a board.
fn rename_nets_from_pcb(
    nets: &mut [netlist::Net],
    pad_net: &BTreeMap<(String, String), String>,
) {
    if pad_net.is_empty() {
        return;
    }
    for n in nets.iter_mut() {
        let mut found: Option<&str> = None;
        let mut conflict = false;
        for t in &n.terminals {
            let Some(name) = pad_net.get(&(t.designator.clone(), t.pin.clone())) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            match found {
                None => found = Some(name.as_str()),
                Some(f) if f == name.as_str() => {}
                Some(_) => {
                    conflict = true;
                    break;
                }
            }
        }
        if let (Some(name), false) = (found, conflict) {
            if n.name != name {
                n.name = name.to_string();
            }
        }
    }
}

/// Assemble the design model from every sheet instance in the hierarchy.
///
/// Connectivity is computed globally: each sheet contributes connected
/// fragments, and fragments are merged across the whole hierarchy by shared
/// keys — global/power labels (by name), local labels (sheet-scoped), and
/// hierarchical sheet-pin ↔ label connections (matching KiCad's structural
/// joins, not just name coincidence).
pub fn build_design_multi(
    project_name: &str,
    project_path: &str,
    sheets: &[(SheetInfo, &Schematic)],
    pcb_pad_net: &BTreeMap<(String, String), String>,
) -> Design {
    let mut components = Vec::new();
    let mut indexes = Indexes::default();

    // Collect fragments from every sheet. Each fragment carries its sheet instance
    // (`Frag::sheet`), so the merged net's `by_sheet` keeps a correct per-instance
    // attribution — no flat uuid→sheet map (which collapses repeated sheet instances
    // like gate_driver U/V/W and mosfet_temp_1..6 onto whichever was processed last).
    let mut frags = Vec::new();
    for (sheet, sch) in sheets {
        components.extend(build_components_on(
            sch,
            &sheet.sheet_path,
            &sheet.sheet_path_uuids,
        ));
        frags.extend(netlist::fragments(sch, &sheet.sheet_path, &sheet.sheet_path_uuids));
    }
    // designator -> sheet, for net source_sheets.
    let comp_sheet: BTreeMap<String, String> = components
        .iter()
        .map(|c| (c.designator.clone(), c.hierarchy.sheet_path_uuids.clone()))
        .collect();
    let comp_bbox: BTreeMap<String, Bbox> = components
        .iter()
        .filter_map(|c| c.bbox.map(|b| (c.designator.clone(), b)))
        .collect();

    // Merge fragments into nets across the whole hierarchy, then adopt KiCad's
    // canonical net names from the board (when present) so the schematic, the
    // PCB, and the project's net-class rules all share one namespace — the basis
    // for cross-probing and correct net-class colouring.
    let mut merged = netlist::merge_frags(frags);
    rename_nets_from_pcb(&mut merged, pcb_pad_net);

    let mut net_name_to_classes = BTreeMap::new();
    let mut component_to_nets: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut nets = Vec::new();
    for (i, n) in merged.into_iter().enumerate() {
        let mut source_sheets: BTreeSet<String> = BTreeSet::new();
        let mut bbox: Option<Bbox> = None;
        for uuid in n.graphical.all_uuids() {
            indexes.svg_to_net.insert(uuid.clone(), n.name.clone());
            indexes.svg_to_nets.entry(uuid.clone()).or_default().push(n.name.clone());
        }
        // Per-instance attribution: on each sheet instance the net occupies, map its
        // graphical uuids → this net for that instance. This is what lets the viewer
        // resolve a click and cross-probe on repeated sheet instances (gate_driver
        // U/V/W, mosfet_temp_1..6), where one uuid means a different net per instance.
        for (sheet, g) in &n.by_sheet {
            source_sheets.insert(sheet.clone());
            for uuid in g.all_uuids() {
                indexes
                    .sheet_svg_to_nets
                    .entry(sheet.clone())
                    .or_default()
                    .entry(uuid.clone())
                    .or_default()
                    .push(n.name.clone());
            }
        }
        for t in &n.terminals {
            component_to_nets
                .entry(t.designator.clone())
                .or_default()
                .insert(n.name.clone());
            if let Some(spu) = comp_sheet.get(&t.designator) {
                source_sheets.insert(spu.clone());
            }
            if let Some(b) = comp_bbox.get(&t.designator) {
                bbox = Some(bbox.map_or(*b, |acc| acc.union(*b)));
            }
        }
        net_name_to_classes
            .entry(n.name.clone())
            .or_insert_with(|| vec!["Default".to_string()]);
        nets.push(DesignNet {
            uid: format!("{:012x}", i + 1),
            auto_named: n.driver_kind == "pin",
            name: n.name,
            driver_kind: n.driver_kind,
            source_sheets: source_sheets.into_iter().collect(),
            terminals: n.terminals,
            graphical: n.graphical,
            bbox,
        });
    }

    components.sort_by(|a, b| a.designator.cmp(&b.designator));
    for c in &components {
        if !c.svg_id.is_empty() {
            indexes
                .svg_to_component
                .insert(c.svg_id.clone(), c.designator.clone());
        }
    }
    indexes.component_to_nets = component_to_nets
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect();

    // Bus aliases gathered across every sheet, deduped + sorted so the JSON stays
    // byte-deterministic. Surfaced as review context only — never synthesised into
    // `nets` (member names are bus-local and reused across buses, so a bare-name
    // union would silently short unrelated nets).
    let mut bus_aliases: Vec<BusAliasInfo> = sheets
        .iter()
        .flat_map(|(_, sch)| sch.bus_aliases.iter())
        .map(|a| BusAliasInfo { name: a.name.clone(), members: a.members.clone() })
        .collect();
    bus_aliases.sort_by(|a, b| (&a.name, &a.members).cmp(&(&b.name, &b.members)));
    bus_aliases.dedup();

    Design {
        schema: crate::DESIGN_SCHEMA.to_string(),
        generator: crate::GENERATOR.to_string(),
        project: ProjectInfo {
            name: project_name.to_string(),
            filename: format!("{project_name}.kicad_pro"),
            path: project_path.to_string(),
        },
        sheets: sheets.iter().map(|(s, _)| s.clone()).collect(),
        components,
        nets,
        net_name_to_classes,
        bus_aliases,
        indexes,
        // Filled by the pipeline (KiCad theme is application-global, the project
        // file gives drawing defaults) — both default to empty here.
        theme: crate::theme::Theme::default(),
        drawing: crate::theme::Drawing::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_kicad::Schematic;

    const SCH: &str = r#"
    (kicad_sch
      (lib_symbols
        (symbol "Device:R"
          (symbol "R_1_1"
            (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
            (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (pin "1" (uuid "r1p1")) (pin "2" (uuid "r1p2")) (instances))
      (symbol (lib_id "Device:R") (at 100 110 0) (unit 1) (uuid "r2")
        (property "Reference" "R2") (pin "1" (uuid "r2p1")) (pin "2" (uuid "r2p2")) (instances))
      (wire (pts (xy 100 102.54) (xy 100 107.46)) (uuid "w1"))
      (label "MID" (at 100 102.54 0) (uuid "lbl1")))
    "#;

    fn root_sheet() -> SheetInfo {
        SheetInfo {
            filename: "demo.kicad_sch".into(),
            path: "demo.kicad_sch".into(),
            sheet_number: 1,
            sheet_path: "/".into(),
            sheet_path_uuids: "/".into(),
            title: "demo".into(),
            page: String::new(),
            notes: Vec::new(),
            company: String::new(),
            rev: String::new(),
            date: String::new(),
        }
    }

    const ROOT_GLOBAL: &str = r#"
    (kicad_sch
      (lib_symbols (symbol "Device:R" (symbol "R_1_1"
        (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
        (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (pin "1" (uuid "r1p1")) (pin "2" (uuid "r1p2")) (instances))
      (global_label "VBUS" (at 100 97.46 0) (uuid "g1")))
    "#;

    const CHILD_GLOBAL: &str = r#"
    (kicad_sch
      (lib_symbols (symbol "Device:C" (symbol "C_1_1"
        (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
        (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:C") (at 100 100 0) (unit 1) (uuid "c1")
        (property "Reference" "C1") (pin "1" (uuid "c1p1")) (pin "2" (uuid "c1p2")) (instances))
      (global_label "VBUS" (at 100 97.46 0) (uuid "g2")))
    "#;

    #[test]
    fn merges_global_net_across_sheets() {
        let root = Schematic::parse_str(ROOT_GLOBAL).unwrap();
        let child = Schematic::parse_str(CHILD_GLOBAL).unwrap();
        let root_info = root_sheet();
        let child_info = SheetInfo {
            filename: "power.kicad_sch".into(),
            path: "power.kicad_sch".into(),
            sheet_number: 2,
            sheet_path: "/Power/".into(),
            sheet_path_uuids: "/pw/".into(),
            title: "Power".into(),
            page: String::new(),
            notes: Vec::new(),
            company: String::new(),
            rev: String::new(),
            date: String::new(),
        };
        let d = build_design_multi(
            "demo",
            "demo.kicad_pro",
            &[(root_info, &root), (child_info, &child)],
            &BTreeMap::new(),
        );
        // R1 and C1 both attach to the single global net VBUS.
        let vbus = d.nets.iter().find(|n| n.name == "VBUS").expect("VBUS net");
        assert_eq!(vbus.driver_kind, "global_label");
        let mut who: Vec<_> = vbus.terminals.iter().map(|t| t.designator.as_str()).collect();
        who.sort();
        assert_eq!(who, vec!["C1", "R1"]);
        // It spans both sheets, and both global-label glyphs are recorded.
        assert_eq!(vbus.source_sheets, vec!["/".to_string(), "/pw/".to_string()]);
        assert_eq!(vbus.graphical.ports, vec!["g1".to_string(), "g2".to_string()]);
        // Both components are present and the child's per-sheet index exists.
        assert!(d.components.iter().any(|c| c.designator == "C1"));
        assert!(d.indexes.sheet_svg_to_nets.contains_key("/pw/"));
    }

    #[test]
    fn joins_via_hierarchical_sheet_pin() {
        // Root: R1.1 sits on a sheet symbol pin "SIG" (child sheet uuid "C").
        let root = Schematic::parse_str(
            r#"
            (kicad_sch
              (lib_symbols (symbol "Device:R" (symbol "R_1_1"
                (pin passive line (at 0 0 0) (length 0) (name "~") (number "1")))))
              (symbol (lib_id "Device:R") (at 50 50 0) (unit 1) (uuid "r1")
                (property "Reference" "R1") (pin "1" (uuid "r1p1")) (instances))
              (sheet (uuid "C")
                (property "Sheetname" "Sub") (property "Sheetfile" "sub.kicad_sch")
                (pin "SIG" input (at 50 50 0) (uuid "sp1"))))
            "#,
        )
        .unwrap();
        // Child: C1.1 carries hierarchical label "SIG".
        let child = Schematic::parse_str(
            r#"
            (kicad_sch
              (lib_symbols (symbol "Device:C" (symbol "C_1_1"
                (pin passive line (at 0 0 0) (length 0) (name "~") (number "1")))))
              (symbol (lib_id "Device:C") (at 70 70 0) (unit 1) (uuid "c1")
                (property "Reference" "C1") (pin "1" (uuid "c1p1")) (instances))
              (hierarchical_label "SIG" (at 70 70 0) (uuid "hl1")))
            "#,
        )
        .unwrap();
        let root_info = root_sheet();
        let child_info = SheetInfo {
            filename: "sub.kicad_sch".into(),
            path: "sub.kicad_sch".into(),
            sheet_number: 2,
            sheet_path: "/Sub/".into(),
            sheet_path_uuids: "/C/".into(),
            title: "Sub".into(),
            page: String::new(),
            notes: Vec::new(),
            company: String::new(),
            rev: String::new(),
            date: String::new(),
        };
        let d = build_design_multi(
            "demo",
            "demo.kicad_pro",
            &[(root_info, &root), (child_info, &child)],
            &BTreeMap::new(),
        );
        // R1 and C1 are one net, bridged by the sheet pin ↔ hierarchical label.
        let sig = d.nets.iter().find(|n| n.name == "SIG").expect("hier net SIG");
        let mut who: Vec<_> = sig.terminals.iter().map(|t| t.designator.as_str()).collect();
        who.sort();
        assert_eq!(who, vec!["C1", "R1"]);
        assert_eq!(sig.driver_kind, "hier_label");
    }

    #[test]
    fn resolves_per_instance_reference_on_child_sheet() {
        // One symbol instantiated on two sheets, each with its own reference.
        let sch = Schematic::parse_str(
            r#"
            (kicad_sch
              (lib_symbols (symbol "Device:R" (symbol "R_1_1"
                (pin passive line (at 0 0 0) (length 1) (name "~") (number "1")))))
              (symbol (lib_id "Device:R") (at 10 10 0) (unit 1) (uuid "sym")
                (property "Reference" "R?")
                (instances (project "demo"
                  (path "/root/instA" (reference "R10") (unit 1))
                  (path "/root/instB" (reference "R20") (unit 1))))))
            "#,
        )
        .unwrap();
        let a = build_components_on(&sch, "/A/", "/instA/");
        let b = build_components_on(&sch, "/B/", "/instB/");
        assert_eq!(a[0].designator, "R10");
        assert_eq!(b[0].designator, "R20");
        assert_eq!(a[0].hierarchy.sheet_path_uuids, "/instA/");
    }

    #[test]
    fn build_design_populates_svg_indexes() {
        let sch = Schematic::parse_str(SCH).unwrap();
        let d = build_design("demo", "demo.kicad_pro", &root_sheet(), &sch);

        // Every addressable element resolves to a net, with the wire/label on /MID.
        assert_eq!(d.indexes.svg_to_net.get("w1"), Some(&"/MID".to_string()));
        assert_eq!(d.indexes.svg_to_net.get("lbl1"), Some(&"/MID".to_string()));
        assert_eq!(d.indexes.svg_to_net.get("r1p2"), Some(&"/MID".to_string()));
        // Multi-valued mirror covers at least the single-valued map.
        assert_eq!(d.indexes.svg_to_nets.get("w1"), Some(&vec!["/MID".to_string()]));
        // The root sheet's per-sheet index carries the wire too.
        let sheet = d.indexes.sheet_svg_to_nets.get("/").expect("root sheet index");
        assert_eq!(sheet.get("w1"), Some(&vec!["/MID".to_string()]));
        // The net carries its graphical buckets through to the model.
        let mid = d.nets.iter().find(|n| n.name == "/MID").unwrap();
        assert_eq!(mid.graphical.wires, vec!["w1".to_string()]);
        assert_eq!(mid.source_sheets, vec!["/".to_string()]);
        // Components and the net get region bboxes from the symbol geometry.
        assert!(d.components.iter().all(|c| c.bbox.is_some()));
        assert!(mid.bbox.is_some());
    }

    #[test]
    fn prefix_and_classify() {
        assert_eq!(prefix_of("MH2"), "MH");
        assert_eq!(prefix_of("C12"), "C");
        assert_eq!(prefix_of("#PWR01"), "#PWR");
        assert_eq!(classify("MH", 1), "mounting_hole");
        assert_eq!(classify("FM", 0), "unknown");
        assert_eq!(classify("C", 2), "passive_2pin");
        assert_eq!(classify("D", 2), "passive_2pin");
        assert_eq!(classify("D", 6), "passive_2pin"); // diode array, >2 pins
        assert_eq!(classify("FM", 0), "unknown"); // fiducial
        assert_eq!(classify("U", 48), "ic");
        assert_eq!(classify("J", 12), "connector");
        assert_eq!(classify("X", 4), "crystal");
    }
}
