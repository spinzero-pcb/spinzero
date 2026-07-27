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

use eda_parse_kicad::schematic::{
    LabelKind, LibPin, LibSymbol, LibText, Pt, Schematic, Shape, SymbolInstance,
};
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
    if let Some((min, max)) = sch.lib_for(sym).and_then(|l| l.bbox_for_unit(sym.unit)) {
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

/// Presentation signature of a symbol's *visible* property fields — per field: its key,
/// font size, bold/italic, and position RELATIVE to the symbol placement. Field VALUES
/// are excluded (a value/MPN edit is a semantic change reported on its own row); a hidden
/// or blank field contributes only its (in)visibility, so moving or resizing text that
/// isn't drawn never reads as an edit. Relative positions keep a whole-symbol drag a
/// "moved" (the fields travel with the body), while dragging one field or changing a
/// field's font size flips the signature to an "edited".
fn fields_sig(sym: &SymbolInstance) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(sym.properties.len());
    for p in &sym.properties {
        if p.effects.hidden || p.value.trim().is_empty() {
            parts.push(format!("{}:-", p.key)); // not drawn: only its (in)visibility
            continue;
        }
        // Field position relative to the symbol origin ("~" when KiCad auto-places it),
        // so a pure body drag — which carries the fields along — leaves it unchanged.
        let rel = match p.at {
            Some(at) => format!("{},{},{}", r4(at.x - sym.at.x), r4(at.y - sym.at.y), r4(at.angle)),
            None => "~".to_string(),
        };
        parts.push(format!(
            "{}:z{}:{}{}:{}",
            p.key, r4(p.effects.size), p.effects.bold as u8, p.effects.italic as u8, rel,
        ));
    }
    parts.join(";")
}

/// Presentation signature of the *library body* the placed unit draws — the half of a
/// symbol's appearance that lives in `lib_symbols` rather than on the instance.
///
/// Restyling a symbol's pin text (or nudging a pin, hiding one, switching its graphic
/// style) leaves the instance untouched and often leaves the placed bbox untouched too,
/// so without this the edit was invisible to the diff. Scoped to the pins/graphics/text
/// the unit actually draws (`unit == 0 || unit == placed unit`, mirroring the renderer
/// and [`LibSymbol::bbox_for_unit`]) so an edit confined to U12.A doesn't flag U12.B/.C.
///
/// Pin ELECTRICAL type is deliberately excluded: an input→output flip is a semantic
/// change the changeset reports on its own component row, and folding it in here would
/// make one user action read as two.
fn lib_body_sig(lib: &LibSymbol, unit: u32) -> String {
    let on_unit = |u: u32| u == 0 || u == unit;
    // Sorted by (number, name, unit) so a library re-emitted in a different element order
    // — KiCad rewrites the cached block wholesale — doesn't read as an edit.
    let mut pins: Vec<&LibPin> = lib.pins.iter().filter(|p| on_unit(p.unit)).collect();
    pins.sort_by(|x, y| {
        (&x.number, &x.name, x.unit).cmp(&(&y.number, &y.name, y.unit))
    });
    let mut parts: Vec<String> = Vec::with_capacity(pins.len() + lib.texts.len() + 1);
    parts.push(format!(
        "hdr|pn{}|nm{}|off{}",
        lib.pin_numbers_hidden as u8, lib.pin_names_hidden as u8, r4(lib.pin_name_offset),
    ));
    for p in pins {
        parts.push(format!(
            "p{}|{}|{}|{},{},{}|l{}|h{}|zn{}|zb{}",
            p.number,
            p.name,
            p.shape,
            r4(p.at.x),
            r4(p.at.y),
            r4(p.at.angle),
            r4(p.length),
            p.hidden as u8,
            r4(p.name_size),
            r4(p.number_size),
        ));
    }
    let mut texts: Vec<&LibText> = lib.texts.iter().filter(|t| on_unit(t.unit)).collect();
    texts.sort_by(|x, y| x.text.cmp(&y.text));
    for t in texts {
        parts.push(format!(
            "t{}|{},{},{}|z{}",
            t.text,
            r4(t.at.x),
            r4(t.at.y),
            r4(t.at.angle),
            r4(t.effects.size),
        ));
    }
    parts.join(";")
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

    // (cache entry, unit) -> library-body signature. A 100-pin MCU placed as three units
    // on one sheet would otherwise re-walk its pin list per instance.
    let mut lib_sigs: std::collections::HashMap<(&str, u32), String> =
        std::collections::HashMap::new();

    for sym in &sch.symbols {
        let lib = sch.lib_for(sym);
        let is_power = lib.map(|l| l.power).unwrap_or(false) || sym.lib_id.starts_with("power:");
        let kind = if is_power { "power" } else { "symbol" };
        let body = match lib {
            Some(l) => lib_sigs
                .entry((l.lib_id.as_str(), sym.unit))
                .or_insert_with(|| lib_body_sig(l, sym.unit))
                .clone(),
            None => String::new(),
        };
        // Signature excludes the symbol's own position (that lives in the bbox), so a pure
        // drag reads as "moved" and a rotation / library swap / field edit reads as
        // "edited". `fields_sig` folds in the visible property fields' presentation (font
        // size, relative position, weight, visibility) so restyling or repositioning a
        // reference/value label — invisible in the body bbox — is caught too, and
        // `lib_body_sig` does the same for the library-side drawing (pin text sizes, pin
        // geometry and style, body text) that the instance doesn't carry.
        let sig = format!(
            "{}|u{}|a{}|m{}|{}|{}",
            sym.lib_id,
            sym.unit,
            r4(sym.at.angle),
            sym.mirror.as_deref().unwrap_or(""),
            fields_sig(sym),
            body,
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

    /// A resistor whose Reference/Value fields carry explicit positions + font sizes, so
    /// the field-presentation signature has something to react to.
    const FIELDS_SHEET: &str = r##"
    (kicad_sch (version 20240101) (uuid "root")
      (lib_symbols
        (symbol "Device:R"
          (symbol "R_0_1" (rectangle (start -1 -2.5) (end 1 2.5)))
          (symbol "R_1_1" (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1")))))
      (symbol (lib_id "Device:R") (at 50 50 0) (unit 1) (uuid "r1")
        (property "Reference" "R1" (at 52 48 0) (effects (font (size 1.27 1.27))))
        (property "Value" "10k" (at 52 52 0) (effects (font (size 1.27 1.27))))))
    "##;

    fn field_sig(src: &str) -> String {
        let sch = Schematic::parse_str(src).unwrap();
        build_sheet("root.kicad_sch", &sch)
            .elements
            .into_iter()
            .find(|e| e.uuid == "r1")
            .unwrap()
            .sig
    }

    #[test]
    fn font_size_change_flips_signature() {
        // Only a field's font size grows: the body bbox is untouched, so this must show up
        // in the signature (an "edited") — the C68 case the plain body diff missed.
        let a = field_sig(FIELDS_SHEET);
        let bigger = FIELDS_SHEET.replacen("(size 1.27 1.27)", "(size 2.54 2.54)", 1);
        assert_ne!(a, field_sig(&bigger), "a field font-size change changes the signature");
    }

    /// A two-unit symbol whose pins carry explicit name/number text sizes, so the
    /// library-body signature has something to react to — and so a unit-scoped edit can
    /// be shown NOT to disturb the other unit.
    const LIB_SHEET: &str = r##"
    (kicad_sch (version 20240101) (uuid "root")
      (lib_symbols
        (symbol "MCU:U12"
          (symbol "U12_1_1"
            (rectangle (start -5 -5) (end 5 5))
            (pin input line (at -7.62 2.54 0) (length 2.54)
              (name "GPIO12" (effects (font (size 1.27 1.27))))
              (number "48" (effects (font (size 1.27 1.27))))))
          (symbol "U12_2_1"
            (rectangle (start -5 -5) (end 5 5))
            (pin passive line (at -7.62 0 0) (length 2.54)
              (name "VDD" (effects (font (size 1.27 1.27))))
              (number "50" (effects (font (size 1.27 1.27))))))))
      (symbol (lib_id "MCU:U12") (at 50 50 0) (unit 1) (uuid "u12a")
        (property "Reference" "U12" (at 52 48 0) (effects (font (size 1.27 1.27)))))
      (symbol (lib_id "MCU:U12") (at 80 50 0) (unit 2) (uuid "u12b")
        (property "Reference" "U12" (at 82 48 0) (effects (font (size 1.27 1.27))))))
    "##;

    fn sig_of(src: &str, uuid: &str) -> String {
        let sch = Schematic::parse_str(src).unwrap();
        build_sheet("root.kicad_sch", &sch)
            .elements
            .into_iter()
            .find(|e| e.uuid == uuid)
            .unwrap()
            .sig
    }

    #[test]
    fn pin_text_font_size_change_flips_signature() {
        // The pin NAME's font size grows in the library body. Nothing on the instance
        // changed and the placed bbox is untouched (pin anchors are where they were), so
        // this only shows up if the library body is part of the signature.
        let bigger = LIB_SHEET.replacen(
            r#"(name "GPIO12" (effects (font (size 1.27 1.27))))"#,
            r#"(name "GPIO12" (effects (font (size 2.54 2.54))))"#,
            1,
        );
        assert_ne!(
            sig_of(LIB_SHEET, "u12a"),
            sig_of(&bigger, "u12a"),
            "a pin-text font-size change changes the signature"
        );
    }

    #[test]
    fn pin_number_font_size_change_flips_signature() {
        let bigger = LIB_SHEET.replacen(
            r#"(number "48" (effects (font (size 1.27 1.27))))"#,
            r#"(number "48" (effects (font (size 2.54 2.54))))"#,
            1,
        );
        assert_ne!(sig_of(LIB_SHEET, "u12a"), sig_of(&bigger, "u12a"));
    }

    #[test]
    fn lib_edit_is_scoped_to_the_placed_unit() {
        // Restyling unit A's pin text must leave unit B's signature alone — U12.B was not
        // touched, so it must not read as edited.
        let bigger = LIB_SHEET.replacen(
            r#"(name "GPIO12" (effects (font (size 1.27 1.27))))"#,
            r#"(name "GPIO12" (effects (font (size 2.54 2.54))))"#,
            1,
        );
        assert_eq!(
            sig_of(LIB_SHEET, "u12b"),
            sig_of(&bigger, "u12b"),
            "the untouched unit keeps its signature"
        );
    }

    #[test]
    fn pin_electrical_type_is_not_in_the_signature() {
        // input → output is a semantic change reported on its own component row by the
        // diff engine's pin pass; folding it in here would make one edit read as two.
        let retyped = LIB_SHEET.replacen("(pin input line (at -7.62 2.54 0)", "(pin output line (at -7.62 2.54 0)", 1);
        assert_eq!(
            sig_of(LIB_SHEET, "u12a"),
            sig_of(&retyped, "u12a"),
            "electrical type stays out of the presentation signature"
        );
    }

    #[test]
    fn pin_move_flips_signature() {
        let moved = LIB_SHEET.replacen("(at -7.62 2.54 0) (length 2.54)", "(at -7.62 3.81 0) (length 2.54)", 1);
        assert_ne!(sig_of(LIB_SHEET, "u12a"), sig_of(&moved, "u12a"));
    }

    #[test]
    fn field_reposition_flips_signature() {
        // Dragging just the Reference label (symbol body unmoved) changes the signature.
        let a = field_sig(FIELDS_SHEET);
        let moved = FIELDS_SHEET.replace(r#""Reference" "R1" (at 52 48 0)"#, r#""Reference" "R1" (at 56 48 0)"#);
        assert_ne!(a, field_sig(&moved), "moving one field changes the signature");
    }

    #[test]
    fn whole_symbol_drag_keeps_signature_with_positioned_fields() {
        // Moving the WHOLE symbol shifts its `at` and every field's absolute `at` by the
        // same delta: the field positions are stored relative to the body, so the signature
        // is stable (a "moved", not an "edited") even though fields carry explicit `at`.
        let a = field_sig(FIELDS_SHEET);
        let dragged = FIELDS_SHEET
            .replace("(at 50 50 0) (unit 1) (uuid \"r1\")", "(at 70 80 0) (unit 1) (uuid \"r1\")")
            .replace("(at 52 48 0)", "(at 72 78 0)")
            .replace("(at 52 52 0)", "(at 72 82 0)");
        assert_eq!(a, field_sig(&dragged), "a whole-symbol drag preserves the signature");
    }

    #[test]
    fn field_value_edit_alone_keeps_signature() {
        // Retyping a field's VALUE (a semantic change reported on its own row) must NOT
        // move the presentation signature — otherwise it would double-report.
        let a = field_sig(FIELDS_SHEET);
        let revalued = FIELDS_SHEET.replace(r#""Value" "10k""#, r#""Value" "22k""#);
        assert_eq!(a, field_sig(&revalued), "a value edit leaves the presentation signature alone");
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
