//! Per-element Altium schematic geometry — the same `schematics/geometry.json`
//! the KiCad path emits (`crate::sch_geom`), built from `.SchDoc` documents.
//!
//! The diff engine reads this to split a sheet's graphical edits into one
//! *anchored* row per edit instead of one clubbed "graphical edits" row. It
//! needs three things per object: a stable id that also appears as the SVG's
//! `data-uuid` (so the viewer can frame the change), an extent in the sheet's
//! own millimetres, and a **position-free** content signature that tells a move
//! apart from an edit.
//!
//! Two things differ from the KiCad builder and both come from the format.
//! Altium stores a symbol's artwork already placed in sheet coordinates, so
//! every signature that describes a component's body is taken RELATIVE to the
//! component origin — otherwise dragging a part would rewrite the coordinates
//! of all of its graphics and read as an edit. And Altium leaves some objects
//! unnamed (every junction), so ids come through [`super::oid`], which is what
//! the renderer stamps too.

use std::collections::HashSet;

use eda_parse_altium::sch::{self, GShape, Graphic, Param, Pin, Pt, SchDoc, SchText, TextKind};
use eda_parse_altium::units;

use crate::sch_geom::{r4, SchElem, SchGeometry, SheetGeom, SCH_GEOMETRY_SCHEMA};

use super::sch_svg::{graphic_origin, graphic_points};
use super::{oid, LoadedSheet};

/// An Altium coordinate in millimetres.
fn mm(v: i64) -> f64 {
    units::sch_mm(0, v)
}

/// Sheet frame: Altium is Y-up from the bottom-left, the bundle is Y-down from
/// the top-left, and the flip is about the PAGE height — the same anchor the
/// renderer uses, so geometry and SVG agree.
struct Frame {
    height: f64,
}

impl Frame {
    fn xy(&self, p: Pt) -> (f64, f64) {
        (mm(p.x), self.height - mm(p.y))
    }
}

/// Build the schematic geometry for a whole design, one entry per unique source
/// document. A sheet placed twice is the same file with the same elements, so
/// the file name is the dedupe key — the diff maps it back to sheet numbers.
pub fn build(
    sheets: &[LoadedSheet],
    project_params: &std::collections::BTreeMap<String, String>,
) -> SchGeometry {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<SheetGeom> = Vec::new();
    for s in sheets {
        if s.info.filename.is_empty() || !seen.insert(s.info.filename.as_str()) {
            continue;
        }
        out.push(build_sheet(&s.info.filename, &s.sch, project_params));
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    SchGeometry { schema: SCH_GEOMETRY_SCHEMA, units: "mm", sheets: out }
}

/// Axis-aligned `[x, y, w, h]` of a point set, already in bundle millimetres.
fn bbox_of(pts: &[(f64, f64)]) -> [f64; 4] {
    let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pts {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    if !minx.is_finite() {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [r4(minx), r4(miny), r4(maxx - minx), r4(maxy - miny)]
}

/// A point as an offset from an origin, in Altium's own integer units — exact,
/// and the reason a whole-symbol drag keeps its signature.
fn rel(p: Pt, o: Pt) -> String {
    format!("{},{}", p.x - o.x, p.y - o.y)
}

fn shape_tag(s: &GShape) -> &'static str {
    match s {
        GShape::Line { .. } => "line",
        GShape::Polyline { .. } => "polyline",
        GShape::Polygon { .. } => "polygon",
        GShape::Rect { .. } => "rect",
        GShape::Ellipse { .. } => "ellipse",
        GShape::Arc { .. } => "arc",
    }
}

/// A drawing primitive's appearance, relative to `o`. Colour is part of it:
/// Altium keeps the palette on the object, so a recoloured wire is a real
/// visible edit rather than a theme setting.
fn graphic_sig(g: &Graphic, o: Pt) -> String {
    let pts: Vec<String> = graphic_points(g).iter().map(|p| rel(*p, o)).collect();
    let extra = match &g.shape {
        GShape::Arc { start_deg, end_deg, .. } => format!("|{}|{}", r4(*start_deg), r4(*end_deg)),
        GShape::Polyline { start_shape, end_shape, shape_size, .. } => {
            format!("|{start_shape}|{end_shape}|{shape_size}")
        }
        _ => String::new(),
    };
    format!(
        "{}|{}{}|w{}|s{}|{}|{}",
        shape_tag(&g.shape),
        pts.join(" "),
        extra,
        g.width,
        g.style,
        g.color,
        g.fill.as_deref().unwrap_or(""),
    )
}

/// A parameter's presentation. Its TEXT is excluded — a Comment or MPN edit is a
/// semantic change the component row already reports, and folding it in here
/// would make one user action read as two.
fn param_sig(p: &Param, o: Pt) -> String {
    format!(
        "{}|h{}|{}|f{}|j{}|r{}|m{}|{}",
        p.name,
        p.hidden as u8,
        rel(p.at, o),
        p.font,
        p.justify,
        p.orientation,
        p.mirrored as u8,
        p.color,
    )
}

/// A pin's presentation and geometry. The electrical type is deliberately
/// included: unlike KiCad, where it lives in a shared library symbol and gets
/// its own semantic row, Altium writes it on the placement.
fn pin_sig(p: &Pin, o: Pt) -> String {
    format!(
        "{}|{}|e{}|c{}|l{}|{}|p{}|d{}|oe{}|in{}|{}|nf{}|df{}",
        p.number,
        p.name,
        p.electrical,
        p.conglomerate,
        p.length,
        rel(p.at, o),
        p.part_id,
        p.display_mode,
        p.outer_edge,
        p.inner,
        p.color,
        p.name_font,
        p.number_font,
    )
}

fn text_kind(k: TextKind) -> &'static str {
    match k {
        TextKind::Label => "label",
        TextKind::Frame => "frame",
        TextKind::Note => "note",
    }
}

fn build_sheet(
    file: &str,
    doc: &SchDoc,
    project_params: &std::collections::BTreeMap<String, String>,
) -> SheetGeom {
    let f = Frame { height: mm(doc.sheet.height).max(1.0) };
    let mut elems: Vec<SchElem> = Vec::new();
    let mut push = |uuid: String, kind: &'static str, pts: &[(f64, f64)], sig: String| {
        elems.push(SchElem { uuid, kind, bbox: bbox_of(pts), sig });
    };
    let p1 = |p: Pt| vec![f.xy(p)];

    // Placed components. Everything describing the body is relative to the
    // placement, so a drag is a "moved" and a restyle an "edited".
    for c in &doc.components {
        let o = c.at;
        let mut pts: Vec<(f64, f64)> = vec![f.xy(o)];
        if let Some((min, max)) = c.bbox {
            pts.push(f.xy(min));
            pts.push(f.xy(max));
        }
        for g in &c.graphics {
            pts.extend(graphic_points(g).iter().map(|p| f.xy(*p)));
        }
        for p in &c.pins {
            pts.push(f.xy(p.at));
            pts.push(f.xy(p.connection()));
        }
        let mut pins: Vec<String> = c.pins.iter().map(|p| pin_sig(p, o)).collect();
        pins.sort();
        let mut params: Vec<String> = c.parameters.iter().map(|p| param_sig(p, o)).collect();
        params.sort();
        let mut gs: Vec<String> = c.graphics.iter().map(|g| graphic_sig(g, o)).collect();
        gs.sort();
        let sig = format!(
            "{}|{}|{}|part{}/{}|dm{}|r{}|m{}|dsg[h{}|{}|f{}|r{}|{}]|pins[{}]|params[{}]|gfx[{}]",
            c.library_ref,
            c.footprint,
            c.kind.as_str(),
            c.current_part_id,
            c.part_count,
            c.display_mode,
            c.orientation,
            c.mirrored as u8,
            c.designator_hidden as u8,
            rel(c.designator_at, o),
            c.designator_font,
            c.designator_orientation,
            c.designator_color,
            pins.join(";"),
            params.join(";"),
            gs.join(";"),
        );
        push(oid(&c.uuid, "c", o), "symbol", &pts, sig);
    }

    // Wires and buses are polylines: the shape is relative to the first point,
    // so a dragged wire keeps its signature and a redrawn one does not.
    for (kind, list) in [("wire", &doc.wires), ("bus", &doc.buses)] {
        for w in list {
            let Some(&first) = w.pts.first() else { continue };
            let pts: Vec<(f64, f64)> = w.pts.iter().map(|p| f.xy(*p)).collect();
            let rels: Vec<String> = w.pts.iter().map(|p| rel(*p, first)).collect();
            let sig = format!("{kind}|{}|w{}|{}", rels.join(" "), w.width, w.color);
            push(oid(&w.uuid, kind, first), kind, &pts, sig);
        }
    }

    for j in &doc.junctions {
        push(oid(&j.uuid, "j", j.at), "junction", &p1(j.at), format!("junction|{}", j.color));
    }

    for n in &doc.no_ercs {
        let sig = format!("no_connect|{}|{}", n.symbol, n.color);
        push(oid(&n.uuid, "nc", n.at), "no_connect", &p1(n.at), sig);
    }

    // A label with no text draws no SVG group, so it gets no row: a row with no
    // group is a change the viewer can report and never frame.
    for l in doc.net_labels.iter().filter(|l| !l.text.trim().is_empty()) {
        let sig = format!("label|{}|f{}|r{}|{}", l.text, l.font, l.orientation, l.color);
        push(oid(&l.uuid, "nl", l.at), "label", &p1(l.at), sig);
    }

    // A power port names a net rather than being a part, which is exactly what
    // KiCad's power symbols do — so it carries the same kind and the same noun.
    for p in &doc.power_ports {
        let sig = format!(
            "power|{}|st{}|f{}|r{}|n{}|{}",
            p.text, p.style, p.font, p.orientation, p.show_net_name as u8, p.color
        );
        push(oid(&p.uuid, "pp", p.at), "power", &p1(p.at), sig);
    }

    for p in &doc.ports {
        let pts: Vec<(f64, f64)> = p.terminals().iter().map(|t| f.xy(*t)).collect();
        let sig = format!(
            "port|{}|io{}|st{}|a{}|w{}|h{}|f{}|{}|{}",
            p.name, p.io_type, p.style, p.alignment, p.width, p.height, p.font, p.color,
            p.text_color,
        );
        push(oid(&p.uuid, "p", p.at), "port", &pts, sig);
    }

    // A sheet symbol carries its entries: adding or renaming one re-wires the
    // hierarchy, so it belongs in the parent's signature rather than as its own
    // row the viewer could not frame.
    for s in &doc.sheet_symbols {
        let far = Pt { x: s.at.x + s.xsize * sch::UNIT, y: s.at.y - s.ysize * sch::UNIT };
        let mut entries: Vec<String> = s
            .entries
            .iter()
            .map(|e| {
                format!(
                    "{}|s{}|d{}|io{}|st{}|{}|{}",
                    e.name, e.side, e.distance_from_top, e.io_type, e.style, e.arrow_kind, e.color
                )
            })
            .collect();
        entries.sort();
        let sig = format!(
            "sheet_symbol|{}|{}|{}x{}|{}|{}|entries[{}]",
            s.name,
            s.filename,
            s.xsize,
            s.ysize,
            s.color,
            s.area_color,
            entries.join(";"),
        );
        push(oid(&s.uuid, "ss", s.at), "sheet_symbol", &[f.xy(s.at), f.xy(far)], sig);
    }

    // Net-class and other directives placed on a wire. The parameter TEXT is
    // the directive's payload — a class rename is the change here, not a
    // separate semantic row — so unlike a component parameter it is included.
    for ps in &doc.param_sets {
        let mut params: Vec<String> = ps
            .parameters
            .iter()
            .map(|p| format!("{}={}|h{}", p.name, p.text, p.hidden as u8))
            .collect();
        params.sort();
        let sig = format!("netclass_flag|{}|[{}]", ps.name, params.join(";"));
        push(oid(&ps.uuid, "ps", ps.at), "netclass_flag", &p1(ps.at), sig);
    }

    // Free sheet artwork. The template's frame is excluded: it is the drawing
    // sheet, not the design, and it moves with the page rather than with an edit.
    for g in &doc.graphics {
        let pts: Vec<(f64, f64)> = graphic_points(g).iter().map(|p| f.xy(*p)).collect();
        let origin = graphic_origin(g);
        push(oid(&g.uuid, "g", origin), "graphic", &pts, graphic_sig(g, origin));
    }

    // Compile masks and regions draw as outlines and are reported, not applied
    // (§7) — but adding one still changes what the sheet claims, so it diffs.
    for r in &doc.regions {
        let sig = format!("region|{}", rel(r.max, r.min));
        push(oid(&r.uuid, "rg", r.min), "graphic", &[f.xy(r.min), f.xy(r.max)], sig);
    }
    for b in &doc.blankets {
        let pts: Vec<(f64, f64)> = graphic_points(b).iter().map(|p| f.xy(*p)).collect();
        let origin = graphic_origin(b);
        push(oid(&b.uuid, "bl", origin), "graphic", &pts, graphic_sig(b, origin));
    }

    // Text with nothing in it is not drawn (§7), so it gets no row either: a
    // geometry entry the SVG has no group for is a change the viewer could
    // report and never frame. Altium keeps a lot of them — 106 blank labels on
    // one corpus sheet. A `=Special` that expands to nothing is the same case,
    // but resolving it needs the project, which the renderer has and this does
    // not; those few stay.
    // A text with nothing in it draws no SVG group, and a `=Special` that
    // RESOLVES to nothing draws none either — so neither gets a row. The second
    // needs the project's parameters, which is why they reach this far.
    for t in doc.texts.iter().filter(|t| {
        !t.template
            && !t.text.trim().is_empty()
            && !super::sch_svg::special_draws_nothing(&t.text, doc, project_params)
    }) {
        push(oid(&t.uuid, "t", t.at), "text", &text_points(&f, t), text_sig(t));
    }

    for i in doc.images.iter().filter(|i| !i.template) {
        let sig = format!("image|{}|e{}|{}", i.file_name, i.embedded as u8, rel(i.max, i.min));
        push(oid(&i.uuid, "im", i.min), "image", &[f.xy(i.min), f.xy(i.max)], sig);
    }

    elems.sort_by(|a, b| (a.kind, &a.uuid).cmp(&(b.kind, &b.uuid)));
    SheetGeom { file: file.to_string(), elements: elems }
}

fn text_points(f: &Frame, t: &SchText) -> Vec<(f64, f64)> {
    let mut pts = vec![f.xy(t.at)];
    if let Some(c) = t.corner {
        pts.push(f.xy(c));
    }
    pts
}

fn text_sig(t: &SchText) -> String {
    format!(
        "{}|{}|f{}|j{}|r{}|m{}|b{}|{}",
        text_kind(t.kind),
        t.text,
        t.font,
        t.justify,
        t.orientation,
        t.mirrored as u8,
        t.show_border as u8,
        t.color,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_altium::sch::{Component, Junction, SheetProps, Wire};

    const U: i64 = sch::UNIT;

    fn pt(x: i64, y: i64) -> Pt {
        Pt { x: x * U, y: y * U }
    }

    fn doc() -> SchDoc {
        SchDoc {
            sheet: SheetProps { height: 500 * U, width: 800 * U, ..Default::default() },
            components: vec![Component {
                library_ref: "Res".into(),
                designator: "R1".into(),
                uuid: "c1".into(),
                at: pt(100, 100),
                designator_at: pt(102, 104),
                bbox: Some((pt(98, 98), pt(102, 102))),
                graphics: vec![Graphic {
                    shape: GShape::Rect { min: pt(98, 98), max: pt(102, 102) },
                    color: "#800000".into(),
                    fill: None,
                    width: 0,
                    style: 0,
                    part_id: 1,
                    display_mode: 0,
                    uuid: "g1".into(),
                }],
                ..Default::default()
            }],
            wires: vec![Wire {
                pts: vec![pt(10, 10), pt(20, 10)],
                uuid: "w1".into(),
                color: "#000080".into(),
                width: 0,
            }],
            // Altium names no junction, which is exactly the case `oid` exists for.
            junctions: vec![Junction { at: pt(20, 10), uuid: String::new(), color: String::new() }],
            ..Default::default()
        }
    }

    fn elems(d: &SchDoc) -> Vec<SchElem> {
        build_sheet("Sheet.SchDoc", d, &std::collections::BTreeMap::new()).elements
    }

    fn find(d: &SchDoc, uuid: &str) -> SchElem {
        elems(d).into_iter().find(|e| e.uuid == uuid).expect(uuid)
    }

    #[test]
    fn every_kind_captured_with_an_id() {
        let d = doc();
        assert_eq!(find(&d, "c1").kind, "symbol");
        assert_eq!(find(&d, "w1").kind, "wire");
        // The unnamed junction still gets a handle, derived from its position.
        let j = find(&d, &format!("~j:{},{}", 20 * U, 10 * U));
        assert_eq!(j.kind, "junction");
    }

    #[test]
    fn geometry_is_y_down_from_the_page_top() {
        // The component sits 100 units up from the bottom of a 500-unit page, so
        // it lands 400 units down from the top — the renderer's own anchor.
        let d = doc();
        let c = find(&d, "c1");
        assert!((c.bbox[1] - mm(398 * U)).abs() < 1e-6, "{:?}", c.bbox);
        assert!(c.bbox[2] > 0.0 && c.bbox[3] > 0.0, "placed symbol has extent: {:?}", c.bbox);
    }

    #[test]
    fn a_symbol_drag_keeps_its_signature() {
        // Altium stores a symbol's artwork ALREADY PLACED, so a drag rewrites
        // every graphic and pin coordinate. Taking them relative to the origin is
        // what keeps this a "moved" rather than an "edited".
        let a = doc();
        let mut b = doc();
        let c = &mut b.components[0];
        let (dx, dy) = (30 * U, 40 * U);
        c.at = Pt { x: c.at.x + dx, y: c.at.y + dy };
        c.designator_at = Pt { x: c.designator_at.x + dx, y: c.designator_at.y + dy };
        c.bbox = Some((pt(128, 138), pt(132, 142)));
        c.graphics[0].shape = GShape::Rect { min: pt(128, 138), max: pt(132, 142) };
        assert_eq!(find(&a, "c1").sig, find(&b, "c1").sig);
        assert_ne!(find(&a, "c1").bbox, find(&b, "c1").bbox);
    }

    #[test]
    fn a_restyled_symbol_reads_as_an_edit() {
        let a = doc();
        let mut b = doc();
        b.components[0].graphics[0].color = "#00FF00".into();
        assert_ne!(find(&a, "c1").sig, find(&b, "c1").sig);
    }

    #[test]
    fn a_parameter_value_edit_leaves_the_signature_alone() {
        // The Comment/MPN row is the component diff's job; reporting it here too
        // would make one edit read as two.
        let a = doc();
        let mut b = doc();
        b.components[0].parameters.push(Param {
            name: "Comment".into(),
            text: "10k".into(),
            ..Default::default()
        });
        let mut c = doc();
        c.components[0].parameters.push(Param {
            name: "Comment".into(),
            text: "22k".into(),
            ..Default::default()
        });
        assert_ne!(find(&a, "c1").sig, find(&b, "c1").sig, "adding a parameter is an edit");
        assert_eq!(find(&b, "c1").sig, find(&c, "c1").sig, "its value is not");
    }

    #[test]
    fn a_dragged_wire_keeps_its_signature_and_a_redrawn_one_does_not() {
        let a = doc();
        let mut moved = doc();
        moved.wires[0].pts = vec![pt(10, 30), pt(20, 30)];
        let mut redrawn = doc();
        redrawn.wires[0].pts = vec![pt(10, 10), pt(20, 10), pt(20, 20)];
        assert_eq!(find(&a, "w1").sig, find(&moved, "w1").sig);
        assert_ne!(find(&a, "w1").sig, find(&redrawn, "w1").sig);
    }

    #[test]
    fn serializes_deterministically() {
        let sheets = |d: SchDoc| SchGeometry {
            schema: SCH_GEOMETRY_SCHEMA,
            units: "mm",
            sheets: vec![build_sheet("Sheet.SchDoc", &d, &std::collections::BTreeMap::new())],
        };
        let a = serde_json::to_string(&sheets(doc())).unwrap();
        let b = serde_json::to_string(&sheets(doc())).unwrap();
        assert_eq!(a, b, "schematic geometry JSON must be byte-identical across runs");
    }
}
