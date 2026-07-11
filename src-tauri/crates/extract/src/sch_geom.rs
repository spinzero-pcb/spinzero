//! Per-element schematic geometry — every drawable element on each sheet as
//! `(uuid, kind, bbox, signature)`, sheet millimetres, Y-down (the same space the
//! sheet SVG uses).
//!
//! PCB geometry (`ir.rs`) gave the diff engine a per-primitive board model; the
//! schematic side had only the design.json indexes (net/component membership, no
//! positions), so a nudged power symbol / redrawn wire / moved note had no way to be
//! told apart or located — they all clubbed into one unanchored "graphical edits"
//! row. This artifact lets `diff.rs` split those into one *anchored* change per edit:
//! the frontend frames each change from its element's `data-uuid` group directly, so
//! only the uuid + a position-free content signature (to label move vs edit) is
//! needed here, not a pixel-faithful bbox.

use eda_parse_kicad::schematic::{LabelKind, Pt, Schematic, Shape, SymbolInstance};
use serde::Serialize;

use crate::design::SheetInfo;
use crate::geom;

/// Schema id stamped into `schematics/geometry.json`.
pub const SCH_GEOMETRY_SCHEMA: &str = "extract.sch.geometry.a0";

/// Round to 0.1 µm — well below any real edit, keeps the JSON small and
/// byte-deterministic (mirrors `ir::r4`).
fn r4(v: f64) -> f64 {
    let r = (v * 1e4).round() / 1e4;
    if r == 0.0 { 0.0 } else { r } // normalise -0.0 so a signature never flips on sign
}

/// The whole design's schematic geometry: one entry per unique source sheet file.
#[derive(Serialize)]
pub struct SchGeometry {
    pub schema: &'static str,
    pub units: &'static str,
    pub sheets: Vec<SheetGeom>,
}

/// Every drawable element on one `.kicad_sch` file. Repeated instances of a sheet
/// share the file (identical elements), so the file name is the dedupe key — the
/// diff maps it back to sheet numbers via `Bundle::sheet_files`.
#[derive(Serialize)]
pub struct SheetGeom {
    pub file: String,
    pub elements: Vec<SchElem>,
}

/// One drawable element: its stable uuid, a kind tag, its extent, and a
/// position-free content signature (so the diff labels a same-signature/moved-bbox
/// element "moved" and a changed-signature one "edited").
#[derive(Serialize, Clone)]
pub struct SchElem {
    pub uuid: String,
    pub kind: &'static str,
    /// `[x, y, w, h]` sheet mm — the element's extent (drives clustering; the
    /// frontend still frames from the live SVG `data-uuid` group).
    pub bbox: [f64; 4],
    pub sig: String,
}

/// Build the schematic geometry for a whole design (one `SheetGeom` per unique
/// source file). Deterministic: files sorted by name, elements by `(kind, uuid)`.
pub fn build_sch_geometry(sheets: &[(SheetInfo, &Schematic)]) -> SchGeometry {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out: Vec<SheetGeom> = Vec::new();
    for (info, sch) in sheets {
        if info.filename.is_empty() || !seen.insert(info.filename.as_str()) {
            continue;
        }
        out.push(build_sheet(&info.filename, sch));
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    SchGeometry { schema: SCH_GEOMETRY_SCHEMA, units: "mm", sheets: out }
}

/// Axis-aligned `[x, y, w, h]` extent of a point set, rounded. A single point yields
/// a zero-size box at that point (fine — the diff inflates by a grid step to cluster).
fn bbox_of(pts: &[Pt]) -> [f64; 4] {
    let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y);
    }
    if !minx.is_finite() {
        return [0.0, 0.0, 0.0, 0.0];
    }
    [r4(minx), r4(miny), r4(maxx - minx), r4(maxy - miny)]
}

/// Placed bounding box of a symbol: transform the library body-box corners through the
/// placement (mirrors `design::placed_bbox`, but kept here so power symbols — excluded
/// from the component list — get geometry too). Point box at the origin when the
/// library symbol carries no geometry.
fn symbol_bbox(sch: &Schematic, sym: &SymbolInstance) -> [f64; 4] {
    if let Some((min, max)) = sch.lib_for(sym).and_then(|l| l.bbox) {
        let corners = [(min.x, min.y), (max.x, min.y), (max.x, max.y), (min.x, max.y)];
        let pts: Vec<Pt> = corners
            .iter()
            .map(|&(px, py)| {
                let (x, y) =
                    geom::place_mm(sym.at.x, sym.at.y, sym.at.angle, sym.mirror.as_deref(), px, py);
                Pt { x, y }
            })
            .collect();
        return bbox_of(&pts);
    }
    [r4(sym.at.x), r4(sym.at.y), 0.0, 0.0]
}

/// Points describing a sheet graphic's extent.
fn shape_points(s: &Shape) -> Vec<Pt> {
    match s {
        Shape::Rect { a, b, .. } => vec![*a, *b],
        Shape::Poly { pts, .. } => pts.clone(),
        Shape::Circle { center, radius, .. } => vec![
            Pt { x: center.x - radius, y: center.y - radius },
            Pt { x: center.x + radius, y: center.y + radius },
        ],
        Shape::Arc { start, mid, end } => vec![*start, *mid, *end],
    }
}

fn shape_tag(s: &Shape) -> &'static str {
    match s {
        Shape::Rect { .. } => "rect",
        Shape::Poly { .. } => "poly",
        Shape::Circle { .. } => "circle",
        Shape::Arc { .. } => "arc",
    }
}

fn build_sheet(file: &str, sch: &Schematic) -> SheetGeom {
    let mut elems: Vec<SchElem> = Vec::new();
    let mut push = |uuid: &str, kind: &'static str, bbox: [f64; 4], sig: String| {
        if uuid.is_empty() {
            return; // no stable id → can't pair across revisions; the fallback covers it
        }
        elems.push(SchElem { uuid: uuid.to_string(), kind, bbox, sig });
    };

    for sym in &sch.symbols {
        let is_power =
            sch.lib_for(sym).map(|l| l.power).unwrap_or(false) || sym.lib_id.starts_with("power:");
        let kind = if is_power { "power" } else { "symbol" };
        // Signature excludes position (that lives in the bbox), so a pure drag reads as
        // "moved" and a rotation / library swap reads as "edited".
        let sig = format!(
            "{}|u{}|a{}|m{}",
            sym.lib_id,
            sym.unit,
            r4(sym.at.angle),
            sym.mirror.as_deref().unwrap_or("")
        );
        push(&sym.uuid, kind, symbol_bbox(sch, sym), sig);
    }

    for w in &sch.wires {
        let kind = if w.is_bus { "bus" } else { "wire" };
        let sig = format!("{kind}|w{}", r4(w.width));
        push(&w.uuid, kind, bbox_of(&[w.a, w.b]), sig);
    }

    for be in &sch.bus_entries {
        push(&be.uuid, "bus_entry", bbox_of(&[be.a, be.b]), "bus_entry".into());
    }

    for j in &sch.junctions {
        let sig = format!("junction|d{}", r4(j.diameter));
        push(&j.uuid, "junction", bbox_of(&[j.at]), sig);
    }

    for l in &sch.labels {
        let kind = match l.kind {
            LabelKind::Local => "label",
            LabelKind::Global => "global_label",
            LabelKind::Hierarchical => "hier_label",
        };
        let sig = format!("{kind}|{}|a{}|s{}", l.text, r4(l.at.angle), l.shape.as_deref().unwrap_or(""));
        push(&l.uuid, kind, bbox_of(&[l.at.into()]), sig);
    }

    for n in &sch.notes {
        let bbox = match n.box_size {
            Some((w, h)) => [r4(n.at.x), r4(n.at.y), r4(w), r4(h)],
            None => bbox_of(&[n.at.into()]),
        };
        push(&n.uuid, "text", bbox, format!("text|{}", n.text));
    }

    for g in &sch.graphics {
        let sig = format!("graphic|{}|w{}", shape_tag(&g.shape), r4(g.width));
        push(&g.uuid, "graphic", bbox_of(&shape_points(&g.shape)), sig);
    }

    for f in &sch.netclass_flags {
        let sig = format!("ncf|{}|{}", f.netclass, f.shape);
        push(&f.uuid, "netclass_flag", bbox_of(&[f.at.into()]), sig);
    }

    elems.sort_by(|a, b| (a.kind, &a.uuid).cmp(&(b.kind, &b.uuid)));
    SheetGeom { file: file.to_string(), elements: elems }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r##"
    (kicad_sch (version 20240101) (uuid "root")
      (lib_symbols
        (symbol "power:GND"
          (power)
          (symbol "GND_0_1"
            (polyline (pts (xy -1.27 0) (xy 1.27 0)) (stroke (width 0)))
            (polyline (pts (xy 0 0) (xy 0 -2.54)) (stroke (width 0)))))
        (symbol "Device:R"
          (symbol "R_0_1" (rectangle (start -1 -2.5) (end 1 2.5)))
          (symbol "R_1_1" (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1")))))
      (symbol (lib_id "power:GND") (at 100 100 0) (unit 1) (uuid "gnd1")
        (property "Reference" "#PWR01") (property "Value" "GND"))
      (symbol (lib_id "Device:R") (at 50 50 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (property "Value" "10k"))
      (wire (pts (xy 100 100) (xy 100 90)) (uuid "w1"))
      (junction (at 100 100) (uuid "j1"))
      (label "SIG" (at 40 40 0) (uuid "lbl1"))
      (text "note here" (at 20 20 0) (uuid "n1")))
    "##;

    fn geom() -> SheetGeom {
        let sch = Schematic::parse_str(SHEET).unwrap();
        build_sheet("root.kicad_sch", &sch)
    }

    #[test]
    fn every_kind_captured_with_uuid_and_bbox() {
        let g = geom();
        let by_uuid = |u: &str| g.elements.iter().find(|e| e.uuid == u).cloned();
        assert_eq!(by_uuid("gnd1").unwrap().kind, "power", "GND is a power symbol");
        assert_eq!(by_uuid("r1").unwrap().kind, "symbol");
        assert_eq!(by_uuid("w1").unwrap().kind, "wire");
        assert_eq!(by_uuid("j1").unwrap().kind, "junction");
        assert_eq!(by_uuid("lbl1").unwrap().kind, "label");
        assert_eq!(by_uuid("n1").unwrap().kind, "text");
        // The GND symbol's placed box is non-degenerate and centred near its placement.
        let gnd = by_uuid("gnd1").unwrap();
        assert!(gnd.bbox[2] > 0.0 && gnd.bbox[3] > 0.0, "placed symbol has extent: {:?}", gnd.bbox);
    }

    #[test]
    fn signature_excludes_position() {
        // The same symbol moved to a new (at …) keeps its signature (a "moved", not
        // "edited"), but its bbox changes.
        let a = geom();
        let moved_src = SHEET.replace("(at 100 100 0) (unit 1) (uuid \"gnd1\")", "(at 120 130 0) (unit 1) (uuid \"gnd1\")");
        let b = build_sheet("root.kicad_sch", &Schematic::parse_str(&moved_src).unwrap());
        let ga = a.elements.iter().find(|e| e.uuid == "gnd1").unwrap();
        let gb = b.elements.iter().find(|e| e.uuid == "gnd1").unwrap();
        assert_eq!(ga.sig, gb.sig, "a pure move keeps the signature");
        assert_ne!(ga.bbox, gb.bbox, "but the bbox moves");
    }

    #[test]
    fn serializes_deterministically() {
        let sch = Schematic::parse_str(SHEET).unwrap();
        let g = build_sch_geometry(&[(sheet_info("root.kicad_sch"), &sch)]);
        let a = serde_json::to_string(&g).unwrap();
        let b = serde_json::to_string(&build_sch_geometry(&[(sheet_info("root.kicad_sch"), &sch)])).unwrap();
        assert_eq!(a, b, "schematic geometry JSON must be byte-identical across runs");
    }

    fn sheet_info(file: &str) -> SheetInfo {
        SheetInfo {
            filename: file.into(),
            path: file.into(),
            sheet_number: 1,
            sheet_path: "/".into(),
            sheet_path_uuids: "/".into(),
            title: String::new(),
            page: String::new(),
            notes: Vec::new(),
            company: String::new(),
            rev: String::new(),
            date: String::new(),
        }
    }
}
