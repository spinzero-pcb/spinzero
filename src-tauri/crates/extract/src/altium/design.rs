//! Altium components projected into `design::Component`.
//!
//! The output structs are the ones the KiCad path fills, so a schema change
//! breaks both builders at once. Only the *sources* of the fields differ.

use std::collections::BTreeMap;

use eda_parse_altium::sch::{self, Component as SchComponent, Param, SchDoc, UNIT};

use crate::design::{Bbox, Classification, Component, Hierarchy};

/// Parameter key carrying the source-truth `ComponentKind`.
pub const KIND_PARAM: &str = "altium_component_kind";

/// Which channel a sheet placement is, when its document is placed more than
/// once. Both mechanisms Altium offers produce this — a `REPEAT(...)` sheet
/// symbol and, as in the corpus, simply placing the same child document twice.
#[derive(Debug, Clone)]
pub struct Channel {
    /// 1-based index in placement order.
    pub index: i64,
    /// The placement's display name, e.g. `Gate_Drv_HS`.
    pub name: String,
    /// `ChannelDesignatorFormatString` from the project.
    pub format: String,
}

impl Channel {
    /// The placed designator for a base designator on this channel.
    ///
    /// Without this every channel of a repeated block collapses onto one
    /// designator and the BOM loses a whole channel's parts. The board's own
    /// text records show the synthesised names (`C31_1`, `C31_2`), which is what
    /// this reproduces.
    pub fn designator(&self, base: &str) -> String {
        self.format
            .replace("$Component", base)
            .replace("$ChannelIndex", &self.index.to_string())
    }

    /// Suffix a local net name picks up on this channel.
    pub fn net_suffix(&self) -> String {
        format!("_{}", self.index)
    }
}

/// Resolve a field that may be an expression.
///
/// A value beginning `=` is a parameter reference (`Comment==Value`,
/// `Text==SheetNumber`) resolved against component, sheet and project
/// parameters. When it resolves empty we fall back to the literal text, which is
/// what Altium itself displays.
pub fn evaluate(text: &str, component: &[Param], sheet: &BTreeMap<String, String>) -> String {
    let Some(name) = text.strip_prefix('=') else {
        return text.to_string();
    };
    let name = name.trim();
    let from_component = component
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.text.clone());
    let resolved = from_component
        .or_else(|| {
            sheet
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_default();
    if resolved.is_empty() || resolved.starts_with('=') {
        text.to_string()
    } else {
        resolved
    }
}

/// Pins of the placement actually drawn: the current part, in the active display
/// mode. A symbol with several display modes stores every mode's pins, so taking
/// them all double-counts every terminal.
pub fn placed_pins(c: &SchComponent) -> impl Iterator<Item = &sch::Pin> {
    c.pins.iter().filter(move |p| {
        p.display_mode == c.display_mode && (p.part_id == c.current_part_id || p.part_id == -1)
    })
}

/// Sheet-level parameters as a name -> text map, for expression resolution.
pub fn sheet_params(sch: &SchDoc) -> BTreeMap<String, String> {
    sch.parameters
        .iter()
        .map(|p| (p.name.clone(), p.text.clone()))
        .collect()
}

/// Altium's Y-up sheet coordinates as a bundle bbox: millimetres, Y-down.
fn bbox_mm(min: sch::Pt, max: sch::Pt, sheet_height: i64) -> Bbox {
    let mm = |v: i64| eda_parse_altium::units::sch_mm(0, v);
    let r = |v: f64| (v * 1000.0).round() / 1000.0;
    // Flip about the sheet height, so the max corner becomes the top edge.
    let (y0, y1) = (sheet_height - max.y, sheet_height - min.y);
    Bbox {
        x: r(mm(min.x)),
        y: r(mm(y0)),
        w: r(mm(max.x - min.x)),
        h: r(mm(y1 - y0)),
    }
}

/// One placed symbol record, before the parts of a component are grouped.
///
/// A multi-part symbol writes one record per part, and Altium places those
/// records on whatever sheet the designer wants. So grouping cannot happen here:
/// see [`group_parts`].
#[derive(Debug, Clone)]
pub struct Placement {
    pub component: Component,
    /// `CurrentPartId`. Altium's primary part is 1, and it leads the group.
    pub part_id: i64,
    /// This placement's pin designators, in file order. The component's pin
    /// count is the distinct union of these over every placement.
    pub pins: Vec<String>,
}

/// Build the placement list for one sheet instance.
///
/// One entry per placed symbol record. The parts of a multi-part component are
/// still separate here, because the other parts can be on other sheets.
pub fn build_components_on(
    sch: &SchDoc,
    sheet_path: &str,
    sheet_path_uuids: &str,
    channel: Option<&Channel>,
) -> Vec<Placement> {
    let params = sheet_params(sch);
    let mut out: Vec<Placement> = Vec::new();

    for c in &sch.components {
        let base = c.designator.trim().to_string();
        if base.is_empty() {
            continue;
        }
        let designator = match channel {
            Some(ch) => ch.designator(&base),
            None => base.clone(),
        };
        let pins: Vec<&sch::Pin> = placed_pins(c).collect();

        let prefix = crate::design::prefix_of(&designator);
        // A pin COUNT is a pad count, so records sharing a designator count
        // once. A symbol may draw one pad as two connection points — `TR1` on
        // the 5BR design has 16 pin records under 11 designators — and two
        // placements of one ground symbol both draw pad 1. `group_parts` takes
        // the distinct union over every placement, which is what the reference
        // publishes: `DGND` on the 10KW gate-driver board is 1 pin, not 2.
        let numbers: Vec<String> = pins.iter().map(|p| p.number.clone()).collect();
        let pin_count = distinct(&numbers);
        out.push(Placement {
            part_id: c.current_part_id,
            pins: numbers,
            component: Component {
            designator: designator.clone(),
            svg_id: c.uuid.clone(),
            // Altium's "Comment" is KiCad's "Value", and it is routinely an
            // expression pointing at another parameter.
            value: evaluate(c.param("Comment").unwrap_or(""), &c.parameters, &params),
            footprint: c.footprint.clone(),
            library_ref: library_ref(c),
            description: c.description.clone(),
            hierarchy: Hierarchy {
                base_designator: base.clone(),
                channel: channel.map(|c| c.name.clone()),
                channel_index: channel.map(|c| c.index),
                sheet: sheet_path.to_string(),
                sheet_path: sheet_path.to_string(),
                sheet_path_uuids: sheet_path_uuids.to_string(),
            },
            classification: Classification {
                prefix: prefix.clone(),
                kind: crate::design::classify(&prefix, pin_count).to_string(),
                pin_count,
            },
                parameters: parameters_of(c, &params),
                bbox: c.bbox.map(|(min, max)| bbox_mm(min, max, sch.sheet.height)),
            },
        });
    }
    out
}

/// Group the placements of a whole design into one component per designator.
///
/// A multi-part symbol is one physical part written as one record per part, and
/// Altium places the parts wherever the designer wants. `U1` on the MCU144E1
/// design is four placements on three different sheets, so grouping per sheet
/// leaves three components under one designator, three BOM lines, and a
/// `pin_count` of 9 against the part's real 256. Annotation makes a designator
/// unique across the design, so the designator is the key.
///
/// The **primary part leads**. Altium numbers the parts from 1, and part 1 is
/// the placement whose sheet the reference publishes as the component's own.
/// The extra placements' uuids are returned alongside, so every one of them
/// still indexes back to the designator for cross-probing.
pub fn group_parts(placements: Vec<Placement>) -> (Vec<Component>, Vec<(String, String)>) {
    let mut out: Vec<Component> = Vec::new();
    // designator -> (index in `out`, the leader's part id)
    let mut at: BTreeMap<String, (usize, i64)> = BTreeMap::new();
    // designator -> every pin designator any placement drew, for the pad count.
    let mut pins: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut extra_svg_ids: Vec<(String, String)> = Vec::new();

    for p in placements {
        let Placement { component: c, part_id, pins: numbers } = p;
        pins.entry(c.designator.clone()).or_default().extend(numbers);
        let Some(&(i, leader_part)) = at.get(&c.designator) else {
            at.insert(c.designator.clone(), (out.len(), part_id));
            out.push(c);
            continue;
        };
        let existing = &mut out[i];
        existing.bbox = match (existing.bbox, c.bbox) {
            (Some(a), Some(b)) => Some(union(a, b)),
            (a, b) => a.or(b),
        };
        for (k, v) in c.parameters {
            existing.parameters.entry(k).or_insert(v);
        }
        // A later placement with a lower part id is the primary one: it takes
        // over the identity, the sheet and the artwork, and the placement it
        // displaces becomes an extra id.
        let displaced = if part_id < leader_part {
            at.insert(existing.designator.clone(), (i, part_id));
            let old = std::mem::replace(&mut existing.svg_id, c.svg_id);
            existing.hierarchy = c.hierarchy;
            old
        } else {
            c.svg_id
        };
        if !displaced.is_empty() {
            extra_svg_ids.push((displaced, existing.designator.clone()));
        }
    }
    for c in &mut out {
        c.classification.pin_count =
            distinct(pins.get(&c.designator).map(Vec::as_slice).unwrap_or(&[]));
        c.classification.kind =
            crate::design::classify(&c.classification.prefix, c.classification.pin_count)
                .to_string();
    }
    out.sort_by(|a, b| a.designator.cmp(&b.designator));
    (out, extra_svg_ids)
}

/// How many distinct strings a slice holds.
fn distinct(v: &[String]) -> u32 {
    let mut s: Vec<&str> = v.iter().map(String::as_str).collect();
    s.sort_unstable();
    s.dedup();
    s.len() as u32
}

fn union(a: Bbox, b: Bbox) -> Bbox {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
    Bbox { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// `LibReference`, qualified by the source library when the file names one.
fn library_ref(c: &SchComponent) -> String {
    if c.source_library.is_empty() || c.source_library == "*" {
        c.library_ref.clone()
    } else {
        format!("{}:{}", c.source_library, c.library_ref)
    }
}

/// Every parameter of the component, expression-resolved, plus the two flags the
/// shared BOM code reads.
///
/// `kicad_in_bom` is the bundle's existing contract key for BOM inclusion, so
/// `bom.rs` runs unchanged; `altium_component_kind` carries the source truth
/// next to it. Hidden parameters are kept (KiCad hidden properties are too) —
/// they are just not drawn.
fn parameters_of(c: &SchComponent, sheet: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = BTreeMap::new();
    for p in &c.parameters {
        if p.name.is_empty() {
            continue;
        }
        m.insert(p.name.clone(), evaluate(&p.text, &c.parameters, sheet));
    }
    m.insert(KIND_PARAM.into(), c.kind.as_str().into());
    m.insert("kicad_in_bom".into(), c.kind.in_bom().to_string());
    // Altium expresses "not fitted" through project variants, not a per-symbol
    // flag, and variants are not applied yet — so DNP is left explicitly false
    // and the diagnostics block says the project defines variants.
    m.insert("kicad_dnp".into(), "false".into());
    m.insert("kicad_on_board".into(), "true".into());
    m
}

/// Millimetre conversion for a raw schematic coordinate, for callers building
/// geometry from the same model.
pub fn mm(v: i64) -> f64 {
    eda_parse_altium::units::sch_mm(v / UNIT, v % UNIT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_altium::sch::{ComponentKind, Pt};

    fn param(name: &str, text: &str) -> Param {
        Param { name: name.into(), text: text.into(), ..Default::default() }
    }

    fn comp(designator: &str, params: Vec<Param>, kind: ComponentKind) -> SchComponent {
        SchComponent {
            library_ref: "R".into(),
            description: "Resistor".into(),
            designator: designator.into(),
            designator_uuid: "D".into(),
            part_count: 1,
            current_part_id: 1,
            display_mode: 0,
            kind,
            at: Pt::default(),
            uuid: format!("U-{designator}"),
            source_library: "PCBLibraryData.SVNDbLib".into(),
            database_table: "TPartsAll".into(),
            footprint: "RESC1608X55N".into(),
            parameters: params,
            ..Default::default()
        }
    }

    fn doc(components: Vec<SchComponent>) -> SchDoc {
        SchDoc { components, ..SchDoc::default() }
    }

    /// One sheet, built and grouped the way the pipeline builds it.
    fn built(d: &SchDoc, sheet: &str, ch: Option<&Channel>) -> (Vec<Component>, Vec<(String, String)>) {
        group_parts(build_components_on(d, sheet, sheet, ch))
    }

    /// Corner case 5: a field whose text is `=Name` is a parameter reference.
    /// Reading it literally puts `=Value` in the BOM's value column.
    #[test]
    fn expression_fields_resolve_against_parameters() {
        let params = vec![param("Value", "10k"), param("Comment", "=Value")];
        let sheet = BTreeMap::from([("Revision".to_string(), "B".to_string())]);
        assert_eq!(evaluate("=Value", &params, &sheet), "10k");
        assert_eq!(evaluate("=Revision", &params, &sheet), "B", "sheet params resolve too");
        assert_eq!(evaluate("=Missing", &params, &sheet), "=Missing", "empty falls back");
        assert_eq!(evaluate("plain", &params, &sheet), "plain");
    }

    #[test]
    fn component_value_comes_from_the_evaluated_comment() {
        let d = doc(vec![comp("R1", vec![param("Value", "10k"), param("Comment", "=Value")], ComponentKind::Standard)]);
        let (out, _) = built(&d, "/", None);
        assert_eq!(out[0].value, "10k");
        assert_eq!(out[0].library_ref, "PCBLibraryData.SVNDbLib:R");
        assert_eq!(out[0].footprint, "RESC1608X55N");
        assert_eq!(out[0].hierarchy.sheet_path_uuids, "/");
    }

    /// Corner case 13 reaching the BOM: a graphical decoration must not become a
    /// BOM line, and the shared `bom.rs` reads `kicad_in_bom` to decide.
    #[test]
    fn component_kind_drives_bom_inclusion() {
        let d = doc(vec![
            comp("R1", vec![], ComponentKind::Standard),
            comp("LOGO1", vec![], ComponentKind::Graphical),
            comp("MP1", vec![], ComponentKind::Mechanical),
        ]);
        let (out, _) = built(&d, "/", None);
        let kind = |dsg: &str| {
            let c = out.iter().find(|c| c.designator == dsg).unwrap();
            (
                c.parameters["kicad_in_bom"].clone(),
                c.parameters[KIND_PARAM].clone(),
            )
        };
        assert_eq!(kind("R1"), ("true".into(), "standard".into()));
        assert_eq!(kind("LOGO1"), ("false".into(), "graphical".into()));
        assert_eq!(kind("MP1"), ("true".into(), "mechanical".into()));
    }

    fn upin(n: &str, part: i64) -> sch::Pin {
        sch::Pin {
            number: n.into(),
            name: n.into(),
            electrical: 4,
            conglomerate: 32,
            length: 10,
            part_id: part,
            uuid: format!("p{n}"),
            ..Default::default()
        }
    }

    /// One physical part placed as several records is one component, with its
    /// pins unioned — not several BOM lines.
    #[test]
    fn multi_part_placements_group_into_one_component() {
        let mut a = comp("U1", vec![], ComponentKind::Standard);
        a.pins = vec![upin("1", 1), upin("2", 1)];
        let mut b = comp("U1", vec![], ComponentKind::Standard);
        b.current_part_id = 2;
        b.uuid = "U-U1-part2".into();
        b.pins = vec![upin("3", 2), upin("4", 2)];
        let (out, extra) = built(&doc(vec![a, b]), "/", None);
        assert_eq!(out.len(), 1, "one designator, one component");
        assert_eq!(out[0].classification.pin_count, 4);
        assert_eq!(out[0].classification.kind, "ic");
        assert_eq!(extra, vec![("U-U1-part2".to_string(), "U1".to_string())]);
    }

    /// The parts of one symbol can be on DIFFERENT sheets. `U1` on the MCU144E1
    /// design is four placements across three of them, and grouping per sheet
    /// left three components under one designator, three BOM lines and a pin
    /// count of 9 against the part's real 256.
    #[test]
    fn multi_part_placements_group_across_sheets() {
        let part = |n: i64, uuid: &str, pins: Vec<sch::Pin>| {
            let mut c = comp("U1", vec![], ComponentKind::Standard);
            c.current_part_id = n;
            c.uuid = uuid.into();
            c.pins = pins;
            c
        };
        // Walk order puts part 2 first, the way the corpus design does.
        let two = part(2, "U-p2", vec![upin("3", 2), upin("4", 2)]);
        let one = part(1, "U-p1", vec![upin("1", 1), upin("2", 1)]);
        let three = part(3, "U-p3", vec![upin("5", 3)]);
        let mut placements = build_components_on(&doc(vec![two]), "/GPIO/", "/g/", None);
        placements.extend(build_components_on(&doc(vec![one]), "/ADC/", "/a/", None));
        placements.extend(build_components_on(&doc(vec![three]), "/Clock/", "/c/", None));
        let (out, extra) = group_parts(placements);

        assert_eq!(out.len(), 1, "one designator, one component");
        assert_eq!(out[0].classification.pin_count, 5, "every part's pins");
        // The primary part leads even when another part is walked first: the
        // reference publishes part 1's sheet as the component's own.
        assert_eq!(out[0].hierarchy.sheet_path, "/ADC/");
        assert_eq!(out[0].svg_id, "U-p1");
        let mut ids: Vec<&str> = extra.iter().map(|(u, _)| u.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["U-p2", "U-p3"], "every placement still indexes back");
    }

    /// A pin count is a PAD count, across every placement of the component. Two
    /// records may share a designator inside one symbol (`TR1` on the 5BR design
    /// draws one pad as two connection points), and two placements of one ground
    /// symbol both draw pad 1 — which is why the reference publishes `DGND` on
    /// the 10KW gate-driver board as 1 pin and per-placement addition said 2.
    #[test]
    fn a_pin_count_is_the_distinct_pads_of_every_placement() {
        let mut c = comp("TR1", vec![], ComponentKind::Standard);
        c.pins = vec![upin("1", 1), upin("2", 1), upin("2", 1), upin("3", 1)];
        let (out, _) = built(&doc(vec![c]), "/", None);
        assert_eq!(out[0].classification.pin_count, 3, "three pads, four records");

        let one = |uuid: &str| {
            let mut g = comp("DGND", vec![], ComponentKind::Standard);
            g.uuid = uuid.into();
            g.pins = vec![upin("1", 1)];
            g
        };
        let (out, _) = built(&doc(vec![one("a"), one("b")]), "/", None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].classification.pin_count, 1, "two placements, one pad");
    }

    /// A document placed twice is two channels, and each channel's parts carry
    /// their own designator — else the BOM loses a whole channel.
    #[test]
    fn channel_placements_get_their_own_designators() {
        let ch = |i| Channel {
            index: i,
            name: format!("Gate_Drv_{i}"),
            format: "$Component_$ChannelIndex".into(),
        };
        let d = doc(vec![comp("C31", vec![], ComponentKind::Standard)]);
        let (a, _) = built(&d, "/A/", Some(&ch(1)));
        let (b, _) = built(&d, "/B/", Some(&ch(2)));
        assert_eq!(a[0].designator, "C31_1");
        assert_eq!(b[0].designator, "C31_2");
        assert_eq!(a[0].hierarchy.base_designator, "C31");
        assert_eq!(a[0].hierarchy.channel_index, Some(1));
        assert_eq!(ch(1).net_suffix(), "_1");
    }

    /// A symbol drawn in several display modes stores every mode's pins; only
    /// the active mode's are on the sheet.
    #[test]
    fn alternate_display_modes_do_not_double_count_pins() {
        let pin = |n: &str, mode: i64| sch::Pin {
            number: n.into(),
            name: n.into(),
            electrical: 4,
            conglomerate: 32,
            length: 10,
            part_id: 1,
            display_mode: mode,
            uuid: format!("p{n}m{mode}"),
            ..Default::default()
        };
        let mut c = comp("R1", vec![], ComponentKind::Standard);
        c.pins = vec![pin("1", 0), pin("2", 0), pin("1", 1), pin("2", 1), pin("1", 2), pin("2", 2)];
        let (out, _) = built(&doc(vec![c]), "/", None);
        assert_eq!(out[0].classification.pin_count, 2);
    }
}
