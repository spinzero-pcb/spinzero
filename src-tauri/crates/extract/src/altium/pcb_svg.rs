//! Altium board layer SVGs, rendered from the geometry IR.
//!
//! The KiCad path renders its layer SVGs straight from the parsed board
//! (`crate::pcb`). This one renders from `pcb/geometry.json` instead — the same
//! document M2 already builds and the GPU renderer already uploads — so the
//! vector layers and the accelerated canvas cannot disagree about where a pad
//! is. Everything the IR resolved once (layer roles, the Y flip, the pad shape
//! override, net and component interning) is therefore resolved once, not twice.
//!
//! The output contract is the KiCad renderer's, verbatim: a 0-origin viewBox, a
//! `data-review-layer` / `data-layer-role` root, and per-primitive
//! `data-primitive` / `data-net` / `data-component` addressing.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::json;

use crate::ir::{GKind, Geometry, PadDef, TextDef};
use crate::pipeline::Msg;

/// Schema id the frontend's enrichment pass looks for, shared with the KiCad
/// layer SVGs so one reader handles both.
const ENRICHMENT_SCHEMA: &str = "spinzero.pcb.svg.enrichment.a0";

/// Neutral ink. Every layer SVG is monochrome and the app paints it by layer;
/// this is only what a standalone viewer or a report shows.
const INK: &str = "#B8B8B8";

/// Drill holes read as holes, not as copper, in the standalone render.
const HOLE: &str = "#2563EB";

// There is no drawing-sheet row here. A KiCad board declares its own paper size
// and the extractor renders a frame and title block onto a synthetic layer;
// Altium keeps the page frame in the SCHEMATIC and the board file carries none,
// so a worksheet row would be a layer with nothing on it.

fn c(v: f64) -> String {
    let mut s = format!("{:.3}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Write one SVG per layer beside the geometry, and return the manifest rows.
///
/// A layer with nothing on it is still emitted when it is copper or the board
/// edge — a reviewer expects an empty inner plane to exist and to be selectable
/// — and skipped otherwise, so the layer list is not padded with blank islands.
pub fn write_layer_svgs(
    g: &Geometry,
    source: &str,
    out_dir: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<Vec<serde_json::Value>, String> {
    let pcb_dir = out_dir.join("pcb");
    std::fs::create_dir_all(&pcb_dir).map_err(|e| e.to_string())?;

    let used = layers_with_content(g);
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for (i, layer) in g.layers.iter().enumerate() {
        let keep = layer.role == "copper" || layer.role == "edge" || used[i];
        if !keep {
            skipped += 1;
            continue;
        }
        let svg = render_layer(g, source, i as u16);
        let file_name = format!("{}.svg", slug(&layer.name));
        std::fs::write(pcb_dir.join(&file_name), svg).map_err(|e| e.to_string())?;
        let rel = format!("pcb/{file_name}");
        emit(Msg::Artifact(rel.clone()));
        let mut entry = json!({ "layer": layer.name, "role": layer.role, "file": rel });
        if let Some(side) = layer.side {
            entry["side"] = json!(side);
        }
        if let Some(color) = &layer.color {
            entry["color"] = json!(color);
        }
        out.push(entry);
    }
    if skipped > 0 {
        emit(Msg::Progress(format!("pcb: skipped {skipped} empty layer(s)")));
    }
    Ok(out)
}

/// Altium layer names carry spaces, brackets and dots (`M15 (CMP_Courtyard_Top)`);
/// the file name has to survive every filesystem the bundle is opened on, while
/// the manifest keeps the designer's own name.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' { ch } else { '_' })
        .collect();
    let t = s.trim_matches('_').to_string();
    if t.is_empty() {
        "layer".into()
    } else {
        t
    }
}

/// Which layers carry at least one primitive.
fn layers_with_content(g: &Geometry) -> Vec<bool> {
    let mut used = vec![false; g.layers.len()];
    let mark = |i: u16, used: &mut Vec<bool>| {
        if let Some(slot) = used.get_mut(i as usize) {
            *slot = true;
        }
    };
    for &l in g.tracks.seg.layer.iter().chain(&g.tracks.arc.layer) {
        mark(l, &mut used);
    }
    for v in &g.vias {
        for &l in &v.layers {
            mark(l, &mut used);
        }
    }
    for p in &g.pads {
        for &l in &p.layers {
            mark(l, &mut used);
        }
    }
    for z in &g.zones {
        mark(z.layer, &mut used);
    }
    for gr in &g.graphics {
        mark(gr.layer, &mut used);
    }
    for t in &g.texts {
        mark(t.layer, &mut used);
    }
    used
}

/// One layer, in the same 0-origin space every other layer uses so the viewer
/// can stack them without registering each one.
fn render_layer(g: &Geometry, source: &str, layer: u16) -> String {
    let def = &g.layers[layer as usize];
    let [vx, vy, vw, vh] = g.bbox;
    let is_back = def.side == Some("back");
    let is_copper = def.role == "copper";

    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="0 0 {} {}" data-enrichment-schema="{ENRICHMENT_SCHEMA}" data-source="{}" data-review-layer="{}" data-layer-role="{}" data-mirror-x="{is_back}" fill="none" stroke="{INK}" stroke-width="0.12" font-family="{}">"##,
        c(vw), c(vh), c(vw), c(vh),
        esc(source), esc(&def.name), def.role, crate::svg::FONT_FAMILY
    );
    // One wrapper translate keeps every layer in the same 0-origin space: the
    // viewer's net-label overlay mixes getCTM() with a (coord - viewBox.x)
    // subtraction, so a non-zero origin would shift every label off screen.
    let _ = write!(s, r##"<g transform="translate({} {})">"##, c(-vx), c(-vy));

    // The board profile rides along on every layer so a single layer still
    // reads as this board. An Altium board has MORE THAN ONE edge-role layer —
    // the keep-out layer and a mechanical layer named for the board shape both
    // qualify (M2, D2.6) — and the profile is usually on the second, so taking
    // the first one draws a keep-out boundary and calls it the board.
    let edges: Vec<usize> = g
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.role == "edge")
        .map(|(i, _)| i)
        .collect();
    for &edge in &edges {
        if !g.graphics.iter().any(|gr| gr.layer as usize == edge) {
            continue;
        }
        let _ = write!(
            s,
            r##"<g data-primitive="graphic" data-layer-name="{}" data-layer-role="board-outline">"##,
            esc(&g.layers[edge].name)
        );
        for gr in g.graphics.iter().filter(|gr| gr.layer as usize == edge) {
            emit_graphic(&mut s, &gr.kind, &gr.data, 0.15, gr.filled, "#000000");
        }
        s.push_str("</g>");
    }

    // Zones sit at the bottom of the layer so routing and pad openings read on
    // top of a pour.
    for z in g.zones.iter().filter(|z| z.layer == layer && z.pts.len() >= 6) {
        let _ = write!(s, r##"<g data-primitive="zone""##);
        net_attr(&mut s, g, z.net);
        if !z.filled {
            let _ = write!(
                s,
                r##" data-zone-type="{}""##,
                if z.keepout { "keepout" } else { "outline" }
            );
        }
        let _ = write!(s, r##" data-layer-name="{}"><polygon points=""##, esc(&def.name));
        for (i, p) in z.pts.chunks(2).enumerate() {
            if i > 0 {
                s.push(' ');
            }
            let _ = write!(s, "{},{}", c(p[0]), c(p[1]));
        }
        if z.filled {
            s.push_str(r##"" fill="#2d4a2d" stroke="none"/></g>"##);
        } else {
            s.push_str(
                r##"" fill="none" stroke="#DC2626" stroke-width="0.12" stroke-dasharray="0.5 0.3"/></g>"##,
            );
        }
    }

    if is_copper {
        let seg = &g.tracks.seg;
        for i in 0..seg.w.len() {
            if seg.layer[i] != layer {
                continue;
            }
            let xy = &seg.xy[i * 4..i * 4 + 4];
            let _ = write!(s, r##"<g data-primitive="track""##);
            net_attr(&mut s, g, seg.net[i]);
            let _ = write!(
                s,
                r##" data-layer-name="{}"><polyline points="{},{} {},{}" fill="none" stroke="{INK}" stroke-width="{}" stroke-linecap="round"/></g>"##,
                esc(&def.name), c(xy[0]), c(xy[1]), c(xy[2]), c(xy[3]), c(seg.w[i])
            );
        }
        let arc = &g.tracks.arc;
        for i in 0..arc.w.len() {
            if arc.layer[i] != layer {
                continue;
            }
            let xy = &arc.xy[i * 6..i * 6 + 6];
            let _ = write!(s, r##"<g data-primitive="track""##);
            net_attr(&mut s, g, arc.net[i]);
            let d = arc_path_d((xy[0], xy[1]), (xy[2], xy[3]), (xy[4], xy[5]));
            let _ = write!(
                s,
                r##" data-layer-name="{}"><path d="{d}" fill="none" stroke="{INK}" stroke-width="{}" stroke-linecap="round"/></g>"##,
                esc(&def.name), c(arc.w[i])
            );
        }
        for (i, v) in g.vias.iter().enumerate() {
            if !v.layers.contains(&layer) {
                continue;
            }
            let _ = write!(s, r##"<g data-primitive="via""##);
            net_attr(&mut s, g, v.net);
            let _ = write!(
                s,
                r##" data-uuid="via{i}" data-layer-name="{}"><circle cx="{}" cy="{}" r="{}" fill="{INK}" stroke="none"/></g>"##,
                esc(&def.name), c(v.x), c(v.y), c(v.size / 2.0)
            );
        }
    }

    for (i, p) in g.pads.iter().enumerate() {
        if !p.layers.contains(&layer) {
            continue;
        }
        emit_pad(&mut s, g, p, i, &def.name);
    }

    // Board and footprint graphics that live on this layer (the edge layer's own
    // line work was already drawn as the outline).
    if !edges.contains(&(layer as usize)) {
        for gr in g.graphics.iter().filter(|gr| gr.layer == layer) {
            let comp = gr
                .comp
                .and_then(|i| g.components.get(i as usize))
                .map(|cd| format!(r##" data-component="{}""##, esc(&cd.reference)))
                .unwrap_or_default();
            let _ = write!(
                s,
                r##"<g data-primitive="graphic" data-layer-name="{}"{comp}>"##,
                esc(&def.name)
            );
            emit_graphic(&mut s, &gr.kind, &gr.data, gr.width, gr.filled, INK);
            s.push_str("</g>");
        }
    }

    for t in g.texts.iter().filter(|t| t.layer == layer) {
        emit_text(&mut s, g, t, &def.name);
    }

    // Drill overlays on top, so copper and mask both read the holes.
    if is_copper || def.role == "mask" {
        for (i, v) in g.vias.iter().enumerate() {
            if !v.layers.contains(&layer) || v.drill <= 0.0 {
                continue;
            }
            let _ = write!(
                s,
                r##"<g data-primitive="via-hole" data-uuid="via{i}:hole" data-layer-name="{}"><circle cx="{}" cy="{}" r="{}" fill="{HOLE}" stroke="none"/></g>"##,
                esc(&def.name), c(v.x), c(v.y), c(v.drill / 2.0)
            );
        }
        for (i, p) in g.pads.iter().enumerate() {
            if !p.layers.contains(&layer) || p.drill <= 0.0 {
                continue;
            }
            let comp = p
                .comp
                .try_into()
                .ok()
                .and_then(|i: usize| g.components.get(i))
                .map(|cd| cd.reference.clone())
                .unwrap_or_default();
            // An oval drill is a slot; a stadium of drill x drillh says so.
            let (w, h) = (p.drill, if p.drillh > 0.0 { p.drillh } else { p.drill });
            let _ = write!(
                s,
                r##"<g data-primitive="pad-hole" data-component="{}" data-pad-number="{}" data-uuid="pad{i}:hole" data-layer-name="{}"><rect x="{}" y="{}" width="{}" height="{}" rx="{}" ry="{}" fill="{HOLE}" stroke="none" transform="rotate({} {} {})"/></g>"##,
                esc(&comp), esc(&p.num), esc(&def.name),
                c(p.x - w / 2.0), c(p.y - h / 2.0), c(w), c(h),
                c(w.min(h) / 2.0), c(w.min(h) / 2.0),
                c(p.angle), c(p.x), c(p.y)
            );
        }
    }

    s.push_str("</g></svg>");
    s
}

/// `data-net` for a net index, skipping the no-net sentinel.
fn net_attr(s: &mut String, g: &Geometry, net: u32) {
    if let Some(name) = g.nets.get(net as usize).filter(|n| !n.is_empty()) {
        let _ = write!(s, r##" data-net="{}""##, esc(name));
    }
}

/// One pad. Altium's shapes reach the IR already normalised (M2, D2.2), so this
/// only has to draw what the code says.
fn emit_pad(s: &mut String, g: &Geometry, p: &PadDef, i: usize, layer: &str) {
    let comp = usize::try_from(p.comp)
        .ok()
        .and_then(|i| g.components.get(i))
        .map(|cd| cd.reference.clone())
        .unwrap_or_default();
    let _ = write!(s, r##"<g data-primitive="pad""##);
    net_attr(s, g, p.net);
    let _ = write!(
        s,
        r##" data-component="{}" data-pad-number="{}" data-pad-shape="{}" data-uuid="pad{i}" data-layer-name="{}">"##,
        esc(&comp), esc(&p.num), p.shape, esc(layer)
    );
    let (x, y, w, h) = (p.x, p.y, p.w.max(0.001), p.h.max(0.001));
    let rot = format!(r##" transform="rotate({} {} {})""##, c(p.angle), c(x), c(y));
    match p.shape {
        0 => {
            let _ = write!(
                s,
                r##"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="{INK}" stroke="none"/>"##,
                c(x), c(y), c(w / 2.0), c(h / 2.0)
            );
        }
        3 => {
            // Oval: a stadium, so the rounding follows the short side.
            let r = w.min(h) / 2.0;
            let _ = write!(
                s,
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" ry="{}" fill="{INK}" stroke="none"{rot}/>"##,
                c(x - w / 2.0), c(y - h / 2.0), c(w), c(h), c(r), c(r)
            );
        }
        2 => {
            // Rounded rectangle: `rratio` is a fraction of the short side.
            let r = w.min(h) * p.rratio;
            let _ = write!(
                s,
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" ry="{}" fill="{INK}" stroke="none"{rot}/>"##,
                c(x - w / 2.0), c(y - h / 2.0), c(w), c(h), c(r), c(r)
            );
        }
        _ => {
            let _ = write!(
                s,
                r##"<rect x="{}" y="{}" width="{}" height="{}" fill="{INK}" stroke="none"{rot}/>"##,
                c(x - w / 2.0), c(y - h / 2.0), c(w), c(h)
            );
        }
    }
    s.push_str("</g>");
}

/// A graphic primitive from the IR's `kind` + `data` pair.
fn emit_graphic(s: &mut String, kind: &GKind, data: &[f64], width: f64, filled: bool, ink: &str) {
    let w = if width > 0.0 { width } else { 0.1 };
    let fill = if filled { ink } else { "none" };
    match kind {
        GKind::Seg if data.len() >= 4 => {
            let _ = write!(
                s,
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{ink}" stroke-width="{}" stroke-linecap="round"/>"##,
                c(data[0]), c(data[1]), c(data[2]), c(data[3]), c(w)
            );
        }
        GKind::Arc if data.len() >= 6 => {
            let d = arc_path_d((data[0], data[1]), (data[2], data[3]), (data[4], data[5]));
            let _ = write!(
                s,
                r##"<path d="{d}" fill="none" stroke="{ink}" stroke-width="{}"/>"##,
                c(w)
            );
        }
        GKind::Circle if data.len() >= 3 => {
            let _ = write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{fill}" stroke="{ink}" stroke-width="{}"/>"##,
                c(data[0]), c(data[1]), c(data[2]), c(w)
            );
        }
        GKind::Poly if data.len() >= 6 => {
            let pts: Vec<String> = data.chunks(2).map(|p| format!("{},{}", c(p[0]), c(p[1]))).collect();
            let _ = write!(
                s,
                r##"<polygon points="{}" fill="{fill}" stroke="{ink}" stroke-width="{}"/>"##,
                pts.join(" "), c(w)
            );
        }
        _ => {}
    }
}

/// Board text. The IR's justification is `[h, v]` with -1 low, 0 centre,
/// +1 high, which is the pair the SVG anchors need.
fn emit_text(s: &mut String, g: &Geometry, t: &TextDef, layer: &str) {
    if t.text.is_empty() {
        return;
    }
    let comp = t
        .comp
        .and_then(|i| g.components.get(i as usize))
        .map(|cd| format!(r##" data-component="{}""##, esc(&cd.reference)))
        .unwrap_or_default();
    let role = if t.role.is_empty() {
        String::new()
    } else {
        format!(r##" data-footprint-text-role="{}""##, esc(&t.role))
    };
    let h = match t.justify[0] {
        x if x < 0 => "start",
        0 => "middle",
        _ => "end",
    };
    let v = match t.justify[1] {
        x if x < 0 => "hanging",
        0 => "central",
        _ => "alphabetic",
    };
    let weight = if t.bold { r##" font-weight="bold""## } else { "" };
    let style = if t.italic { r##" font-style="italic""## } else { "" };
    let family = t.font.clone().unwrap_or_else(|| crate::svg::FONT_FAMILY.to_string());
    let _ = write!(
        s,
        r##"<g data-primitive="text" data-layer-name="{}"{comp}{role}><text x="{}" y="{}" font-family="{}" font-size="{}" text-anchor="{h}" dominant-baseline="{v}" fill="{INK}" stroke="none"{weight}{style} transform="rotate({} {} {})">{}</text></g>"##,
        esc(layer), c(t.x), c(t.y), esc(&family), c(t.size),
        c(-t.angle), c(t.x), c(t.y), esc(&t.text)
    );
}

/// SVG path for a three-point arc (start, a point on it, end). Collinear points
/// degenerate to a straight line rather than to an invisible path.
fn arc_path_d(a: (f64, f64), m: (f64, f64), e: (f64, f64)) -> String {
    let d = 2.0 * (a.0 * (m.1 - e.1) + m.0 * (e.1 - a.1) + e.0 * (a.1 - m.1));
    if d.abs() < 1e-12 {
        return format!("M {} {} L {} {}", c(a.0), c(a.1), c(e.0), c(e.1));
    }
    let sq = |p: (f64, f64)| p.0 * p.0 + p.1 * p.1;
    let ux = (sq(a) * (m.1 - e.1) + sq(m) * (e.1 - a.1) + sq(e) * (a.1 - m.1)) / d;
    let uy = (sq(a) * (e.0 - m.0) + sq(m) * (a.0 - e.0) + sq(e) * (m.0 - a.0)) / d;
    let r = ((a.0 - ux).powi(2) + (a.1 - uy).powi(2)).sqrt();
    // The cross product of (m-a) and (e-a) says which way round the mid point
    // sits, which is the sweep; the mid point being outside the half turn is
    // what makes it a large arc.
    let cross = (m.0 - a.0) * (e.1 - a.1) - (m.1 - a.1) * (e.0 - a.0);
    let sweep = if cross < 0.0 { 1 } else { 0 };
    let ang = |p: (f64, f64)| (p.1 - uy).atan2(p.0 - ux);
    let mut span = ang(e) - ang(a);
    if sweep == 1 {
        while span > 0.0 {
            span -= std::f64::consts::TAU;
        }
    } else {
        while span < 0.0 {
            span += std::f64::consts::TAU;
        }
    }
    let large = if span.abs() > std::f64::consts::PI { 1 } else { 0 };
    format!(
        "M {} {} A {} {} 0 {large} {sweep} {} {}",
        c(a.0), c(a.1), c(r), c(r), c(e.0), c(e.1)
    )
}

/// The board colours a viewer paints an Altium board with.
///
/// `Board6` carries no palette (M2, D2.6) — Altium keeps it in the user's view
/// configuration, not in the document — so these are Altium Designer's own
/// factory defaults, stated here rather than invented per layer. The keys are
/// the ones the frontend's theme map already reads, so an Altium board themes
/// through exactly the path a KiCad board does.
pub fn board_theme(g: &Geometry) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inner = 0;
    for l in &g.layers {
        let key = match (l.role, l.side) {
            ("copper", Some("front")) => "copper.f".to_string(),
            ("copper", Some("back")) => "copper.b".to_string(),
            ("copper", _) => {
                inner += 1;
                format!("copper.in{inner}")
            }
            ("silkscreen", Some("front")) => "f_silks".into(),
            ("silkscreen", Some("back")) => "b_silks".into(),
            ("mask", Some("front")) => "f_mask".into(),
            ("mask", Some("back")) => "b_mask".into(),
            ("paste", Some("front")) => "f_paste".into(),
            ("paste", Some("back")) => "b_paste".into(),
            ("edge", _) => "edge_cuts".into(),
            _ => continue,
        };
        // Altium's factory palette: top copper red, bottom blue, the inner
        // layers walking its default sequence, overlays yellow over grey, the
        // masks purple, the pastes grey, and the keep-out magenta.
        let hex = match key.as_str() {
            "copper.f" => "#FF0000",
            "copper.b" => "#0000FF",
            "f_silks" => "#FFFF00",
            "b_silks" => "#808080",
            "f_mask" | "b_mask" => "#800080",
            "f_paste" | "b_paste" => "#969696",
            "edge_cuts" => "#FF00FF",
            _ => INNER_COPPER[(inner.max(1) - 1) % INNER_COPPER.len()],
        };
        out.insert(key, hex.to_string());
    }
    out.insert("via_hole_walls".into(), HOLE.to_string());
    out
}

/// Altium's default sequence for the inner copper layers.
const INNER_COPPER: [&str; 8] = [
    "#808000", "#008080", "#800000", "#008000", "#804000", "#008040", "#400080", "#408000",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ArcCol, GraphicDef, LayerDef, PadDef, SegCol, Tracks, ViaDef, ZoneDef};

    fn layer(name: &str, role: &'static str, side: Option<&'static str>, ord: i64) -> LayerDef {
        LayerDef { name: name.into(), role, side, ord, color: None }
    }

    fn geometry() -> Geometry {
        Geometry {
            schema: crate::ir::GEOMETRY_SCHEMA,
            units: "mm",
            bbox: [10.0, 20.0, 100.0, 50.0],
            page: None,
            frame: None,
            layers: vec![
                layer("L1_Top", "copper", Some("front"), 0),
                layer("L2_GND", "copper", Some("inner"), 1),
                layer("Top Overlay", "silkscreen", Some("front"), 2),
                layer("Board Shape", "edge", None, 3),
                layer("M13 (3D Body)", "user", None, 4),
            ],
            nets: vec![String::new(), "GND".into()],
            components: vec![crate::ir::CompDef {
                reference: "R1".into(),
                fp: "0402".into(),
                layer: 0,
                x: 20.0,
                y: 30.0,
                angle: 0.0,
                dnp: false,
                bbox: None,
                uuid: "c1".into(),
            }],
            tracks: Tracks {
                seg: SegCol {
                    xy: vec![10.0, 20.0, 30.0, 20.0],
                    w: vec![0.2],
                    layer: vec![0],
                    net: vec![1],
                },
                arc: ArcCol::default(),
            },
            vias: vec![ViaDef {
                x: 40.0,
                y: 30.0,
                size: 0.6,
                drill: 0.3,
                net: 1,
                layers: vec![0, 1],
                ring: None,
            }],
            pads: vec![PadDef {
                paste: None,
                x: 20.0,
                y: 30.0,
                w: 1.0,
                h: 0.6,
                angle: 0.0,
                shape: 2,
                rratio: 0.25,
                drill: 0.0,
                drillh: 0.0,
                net: 1,
                comp: 0,
                num: "1".into(),
                layers: vec![0],
                mask: None,
                npth: false,
            }],
            zones: vec![ZoneDef {
                layer: 1,
                net: 1,
                filled: true,
                keepout: false,
                pts: vec![10.0, 20.0, 60.0, 20.0, 60.0, 60.0],
            }],
            graphics: vec![GraphicDef {
                layer: 3,
                width: 0.15,
                kind: GKind::Seg,
                data: vec![10.0, 20.0, 110.0, 20.0],
                filled: false,
                comp: None,
            }],
            texts: vec![],
        }
    }

    /// Every layer lands in the same 0-origin space, or the viewer's label
    /// overlay puts every net name off screen.
    #[test]
    fn every_layer_shares_one_zero_origin_view_box() {
        let g = geometry();
        let svg = render_layer(&g, "b.PcbDoc", 0);
        assert!(svg.contains(r#"viewBox="0 0 100 50""#), "{svg}");
        assert!(svg.contains(r#"<g transform="translate(-10 -20)">"#), "{svg}");
    }

    /// The addressing contract the enrichment pass and the cross-probe read.
    #[test]
    fn primitives_carry_their_layer_net_and_component() {
        let g = geometry();
        let svg = render_layer(&g, "b.PcbDoc", 0);
        assert!(svg.contains(r#"data-review-layer="L1_Top" data-layer-role="copper""#), "{svg}");
        assert!(svg.contains(r#"data-primitive="track" data-net="GND""#), "{svg}");
        assert!(svg.contains(r#"data-primitive="via" data-net="GND""#), "{svg}");
        assert!(
            svg.contains(r#"data-primitive="pad" data-net="GND" data-component="R1" data-pad-number="1""#),
            "{svg}"
        );
    }

    /// The board profile rides along on every layer, so a single copper layer
    /// still reads as this board.
    #[test]
    fn the_outline_is_drawn_on_every_layer() {
        let g = geometry();
        for i in 0..4u16 {
            let svg = render_layer(&g, "b.PcbDoc", i);
            assert!(svg.contains(r#"data-layer-role="board-outline""#), "layer {i}: {svg}");
        }
    }

    /// An Altium board has two edge-role layers and the profile is on the
    /// second one, so taking the first draws a keep-out boundary and calls it
    /// the board.
    #[test]
    fn the_profile_is_found_on_whichever_edge_layer_carries_it() {
        let mut g = geometry();
        // Insert a keep-out layer ahead of the board shape, carrying nothing.
        g.layers.insert(3, layer("Keep-Out Layer", "edge", None, 3));
        for gr in &mut g.graphics {
            gr.layer += 1;
        }
        for z in &mut g.zones {
            // The pour stays where it was.
            let _ = z;
        }
        let svg = render_layer(&g, "b", 0);
        assert!(
            svg.contains(r#"data-layer-name="Board Shape" data-layer-role="board-outline""#),
            "the profile layer is the one drawn: {svg}"
        );
        assert!(
            !svg.contains(r#"data-layer-name="Keep-Out Layer" data-layer-role="board-outline""#),
            "an edge layer with nothing on it draws no outline group"
        );
    }

    /// A back layer tells the viewer to mirror it; a front one does not.
    #[test]
    fn a_back_layer_says_so() {
        let mut g = geometry();
        g.layers[1].side = Some("back");
        assert!(render_layer(&g, "b", 0).contains(r#"data-mirror-x="false""#));
        assert!(render_layer(&g, "b", 1).contains(r#"data-mirror-x="true""#));
    }

    /// An empty user layer is not worth a row in the layer list; an empty inner
    /// copper layer is, because a reviewer expects the stack to be complete.
    #[test]
    fn empty_layers_are_kept_only_when_the_stack_needs_them() {
        let g = geometry();
        let used = layers_with_content(&g);
        assert!(!used[4], "the 3D-body layer carries nothing");
        assert!(used[1], "the plane carries its pour");
    }

    /// A designer's layer name survives into the manifest and is only slugged
    /// for the file name.
    #[test]
    fn a_layer_name_with_brackets_still_makes_a_file_name() {
        assert_eq!(slug("M15 (CMP_Courtyard_Top)"), "M15__CMP_Courtyard_Top");
        assert_eq!(slug("L1_Top"), "L1_Top");
        assert_eq!(slug("!!!"), "layer");
    }

    /// Three collinear points are a line, not an invisible arc.
    #[test]
    fn a_degenerate_arc_is_a_line() {
        let d = arc_path_d((0.0, 0.0), (1.0, 0.0), (2.0, 0.0));
        assert_eq!(d, "M 0 0 L 2 0");
    }

    /// The theme covers the stack the board actually has, under the keys the
    /// frontend already reads.
    #[test]
    fn the_board_theme_keys_the_stack_it_finds() {
        let t = board_theme(&geometry());
        assert_eq!(t.get("copper.f").map(String::as_str), Some("#FF0000"));
        assert_eq!(t.get("f_silks").map(String::as_str), Some("#FFFF00"));
        assert_eq!(t.get("edge_cuts").map(String::as_str), Some("#FF00FF"));
        assert!(t.contains_key("copper.in1"), "the inner plane gets its own colour");
        assert!(!t.contains_key("copper.b"), "this stack has no bottom layer");
    }
}
