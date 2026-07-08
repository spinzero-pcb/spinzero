//! Structured PCB geometry IR — the board as *data*, not as rendered SVG.
//!
//! The per-layer SVGs the viewer historically mounted as DOM islands do not scale
//! to high layer counts (a 10-copper board is ~1.2 M DOM elements, and every zoom
//! re-rasters the whole visible vector stack). This module emits the same geometry
//! as a compact, columnar JSON document the GPU renderer uploads to buffers once —
//! see `docs/geometry-ir.md`.
//!
//! Everything is in **board coordinates, millimetres, Y-down**, one shared space.
//! Footprint-local pads/graphics/text are baked to board coordinates here (exactly
//! like the SVG path's `place_fp`), so the renderer needs no per-footprint
//! transform. Primitives reference three index tables (`layers`, `nets`,
//! `components`) instead of repeating strings, which keeps the file small and makes
//! highlight a cheap integer compare. The layer set, roles, colours, net-name
//! resolution and layer-membership rules are shared verbatim with the SVG emitter
//! (`crate::pcb`) so the two outputs never diverge.

use std::collections::HashMap;

use eda_parse_kicad::pcb::{Pcb, PcbShape, Via};
use eda_parse_kicad::schematic::Pt;
use serde::Serialize;

use crate::pcb::{
    board_viewbox, layer_has_content, layer_role, net_name_of, pad_on_layer, place_fp,
    user_layer_color, via_on_layer,
};

/// Schema id stamped into `pcb/geometry.json`.
pub const GEOMETRY_SCHEMA: &str = "extract.pcb.geometry.a0";

/// Round to 0.1 µm — far below fab tolerance, keeps the JSON small and
/// byte-deterministic (avoids long f64 tails).
fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Rounded glyph width, emitted only when it differs from the (rounded) height so
/// square-cell text keeps its compact IR. Compared after `r4` so a hair-width
/// rounding difference doesn't spuriously emit a `width`.
fn text_width_opt(width: f64, size: f64) -> Option<f64> {
    let w = r4(width);
    (w != r4(size)).then_some(w)
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// The whole board, ready for the GPU renderer.
#[derive(Serialize)]
pub struct Geometry {
    pub schema: &'static str,
    pub units: &'static str,
    /// Content extent `[x, y, w, h]` (board + off-board notes + page), board mm.
    pub bbox: [f64; 4],
    /// Paper size `[w, h]` (origin 0,0) when the board declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<[f64; 2]>,
    /// Title-block fields, for the page context the renderer draws.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
    /// Layer table (board stackup order); a primitive's `layer` indexes here.
    pub layers: Vec<LayerDef>,
    /// Net table; index 0 is the empty/no-net sentinel. A primitive's `net` indexes here.
    pub nets: Vec<String>,
    /// Component table; a primitive's `comp` indexes here (-1 = none).
    pub components: Vec<CompDef>,
    pub tracks: Tracks,
    pub vias: Vec<ViaDef>,
    pub pads: Vec<PadDef>,
    pub zones: Vec<ZoneDef>,
    pub graphics: Vec<GraphicDef>,
    pub texts: Vec<TextDef>,
}

#[derive(Serialize)]
pub struct Frame {
    pub title: String,
    pub company: String,
    pub rev: String,
    pub date: String,
    pub version: String,
    pub comments: Vec<String>,
    pub paper: String,
    /// Board file name (e.g. `board.kicad_pcb`), for the title block's "File" cell.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file: String,
}

#[derive(Serialize)]
pub struct LayerDef {
    pub name: String,
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<&'static str>,
    pub ord: i64,
    /// Resolved #RRGGBB for a non-standard ("user") layer from the KiCad theme;
    /// standard fabrication layers omit it and theme via CSS vars on the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Serialize)]
pub struct CompDef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub fp: String,
    /// Layer index of the footprint's mount side (F.Cu/B.Cu); -1 if unknown.
    pub layer: i32,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    #[serde(skip_serializing_if = "is_false")]
    pub dnp: bool,
    /// Placed courtyard/graphic extent `[x, y, w, h]`, board mm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

/// Copper tracks — the bulk of a board — kept columnar so the renderer uploads them
/// straight into GPU buffers with no per-object overhead.
#[derive(Serialize, Default)]
pub struct Tracks {
    pub seg: SegCol,
    pub arc: ArcCol,
}

/// Straight track segments: `xy` is `[x1,y1,x2,y2]` per segment; the parallel
/// `w`/`layer`/`net` arrays carry one entry per segment.
#[derive(Serialize, Default)]
pub struct SegCol {
    pub xy: Vec<f64>,
    pub w: Vec<f64>,
    pub layer: Vec<u16>,
    pub net: Vec<u32>,
}

/// Arc tracks: `xy` is `[sx,sy,mx,my,ex,ey]` (start, mid, end) per arc.
#[derive(Serialize, Default)]
pub struct ArcCol {
    pub xy: Vec<f64>,
    pub w: Vec<f64>,
    pub layer: Vec<u16>,
    pub net: Vec<u32>,
}

#[derive(Serialize)]
pub struct ViaDef {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub drill: f64,
    pub net: u32,
    /// Copper layer indices the via spans (a through via lists every copper layer). The
    /// via barrel + hole wall paint on every spanned layer.
    pub layers: Vec<u16>,
    /// Subset of `layers` where the via keeps a full copper annular ring — i.e. it actually
    /// connects there (KiCad "remove unused layers"). On the spanned layers NOT listed here
    /// the renderer draws only the barrel + hole wall, no ring. Omitted when every spanned
    /// layer keeps its ring (the common through via).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ring: Option<Vec<u16>>,
}

#[derive(Serialize)]
pub struct PadDef {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub angle: f64,
    /// 0 circle | 1 rect | 2 roundrect | 3 oval | 4 trapezoid | 5 custom.
    pub shape: u8,
    #[serde(skip_serializing_if = "is_zero")]
    pub rratio: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub drill: f64,
    /// Oval/slot drill: the second dimension H (mm); `drill` holds W. Absent for a round
    /// drill. The viewer paints the hole as a stadium of `drill`×`drillh` at `angle`.
    #[serde(skip_serializing_if = "is_zero")]
    pub drillh: f64,
    pub net: u32,
    pub comp: i32,
    pub num: String,
    /// Layer indices the pad occupies (copper + mask + paste).
    pub layers: Vec<u16>,
    /// Per-pad solder-mask expansion (mm) when overriding the board default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<f64>,
    /// Non-plated through hole (KiCad `np_thru_hole`): a bare drilled hole with no
    /// copper — the viewer paints it as a hole, not a pad.
    #[serde(skip_serializing_if = "is_false")]
    pub npth: bool,
}

#[derive(Serialize)]
pub struct ZoneDef {
    pub layer: u16,
    pub net: u32,
    /// True for a solid `filled_polygon`; false for a keepout / unfilled outline.
    pub filled: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub keepout: bool,
    /// Polygon ring `[x, y, …]` (board mm). A single ring per KiCad filled polygon.
    pub pts: Vec<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GKind {
    Seg,
    Arc,
    Circle,
    Poly,
}

/// Board (`gr_*`) and footprint (`fp_*`) graphics, unified and placed to board space.
/// `data` carries the numbers for `kind`: seg `[x1,y1,x2,y2]`, arc
/// `[sx,sy,mx,my,ex,ey]`, circle `[cx,cy,r]`, poly `[x,y,…]`. A rectangle is emitted
/// as a 4-corner poly so a rotated footprint rect stays correct.
#[derive(Serialize)]
pub struct GraphicDef {
    pub layer: u16,
    pub width: f64,
    pub kind: GKind,
    pub data: Vec<f64>,
    #[serde(skip_serializing_if = "is_false")]
    pub filled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comp: Option<u32>,
}

#[derive(Serialize)]
pub struct TextDef {
    pub layer: u16,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    /// Glyph height (mm).
    pub size: f64,
    /// Glyph width (mm), emitted only for condensed/expanded text (`width != size`);
    /// absent ⇒ the renderer treats the glyph cell as square.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Stroke (pen) thickness in mm from KiCad `(font (thickness t))`; absent ⇒ use the
    /// font's default weight. The viewer thickens the glyph strokes to hit this pen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness: Option<f64>,
    /// `[h, v]` justification: -1 left/top, 0 centre, +1 right/bottom.
    pub justify: [i8; 2],
    #[serde(skip_serializing_if = "is_false")]
    pub mirror: bool,
    /// KiCad bold text — the renderer draws with KiCad's bold pen (width/5).
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    /// KiCad italic text — the renderer shears glyphs by KiCad's 1/8 tilt.
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    /// KiCad knockout (inverted) text — filled layer-colour background, glyphs cut out.
    #[serde(skip_serializing_if = "is_false")]
    pub knockout: bool,
    /// Footprint reference/value text is kept upright (like KiCad); board `gr_text`
    /// is drawn at its literal angle.
    #[serde(skip_serializing_if = "is_false")]
    pub upright: bool,
    /// Custom outline-font family (e.g. Calibri) from `(font (face …))`; absent ⇒ the
    /// KiCad stroke font. The viewer fills glyphs with this family instead of stroking
    /// Newstroke so a board authored with a real TTF font renders in that font.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comp: Option<u32>,
    /// `reference` / `value` / `user` for footprint text; empty for board text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
}

/// Board side of a layer, derived from its name (front/back/inner copper, else none).
fn layer_side(name: &str) -> Option<&'static str> {
    if name.starts_with("In") && name.ends_with(".Cu") {
        Some("inner")
    } else if name.starts_with("F.") {
        Some("front")
    } else if name.starts_with("B.") {
        Some("back")
    } else {
        None
    }
}

/// KiCad pad-shape token -> compact code.
fn shape_code(s: &str) -> u8 {
    match s {
        "circle" => 0,
        "rect" => 1,
        "roundrect" => 2,
        "oval" => 3,
        "trapezoid" => 4,
        _ => 5,
    }
}

/// Intern a net name into the table, returning its index (0 = empty/no-net).
fn intern_net(name: &str, nets: &mut Vec<String>, idx: &mut HashMap<String, u32>) -> u32 {
    if name.is_empty() {
        return 0;
    }
    if let Some(&i) = idx.get(name) {
        return i;
    }
    let i = nets.len() as u32;
    nets.push(name.to_string());
    idx.insert(name.to_string(), i);
    i
}

/// Append one graphic shape, mapping points through `tf` (identity for board
/// graphics, `place_fp` for footprint graphics) into board coordinates.
fn push_graphic(
    out: &mut Vec<GraphicDef>,
    shape: &PcbShape,
    width: f64,
    layer: u16,
    comp: Option<u32>,
    tf: &dyn Fn(Pt) -> Pt,
) {
    let (kind, data, filled) = match shape {
        PcbShape::Seg { a, b } => {
            let (a, b) = (tf(*a), tf(*b));
            (GKind::Seg, vec![r4(a.x), r4(a.y), r4(b.x), r4(b.y)], false)
        }
        PcbShape::Arc { start, mid, end } => {
            let (s, m, e) = (tf(*start), tf(*mid), tf(*end));
            (
                GKind::Arc,
                vec![r4(s.x), r4(s.y), r4(m.x), r4(m.y), r4(e.x), r4(e.y)],
                false,
            )
        }
        PcbShape::Rect { a, b, filled } => {
            // 4 corners in the shape's own frame, then transformed — stays correct
            // under footprint rotation (the SVG path axis-aligns these; this is closer).
            let corners = [*a, Pt { x: b.x, y: a.y }, *b, Pt { x: a.x, y: b.y }];
            let mut d = Vec::with_capacity(8);
            for cn in corners {
                let q = tf(cn);
                d.push(r4(q.x));
                d.push(r4(q.y));
            }
            (GKind::Poly, d, *filled)
        }
        PcbShape::Circle { center, radius, filled } => {
            let c = tf(*center);
            // Placement is rotation + translation only (no scale), so the radius is preserved.
            (GKind::Circle, vec![r4(c.x), r4(c.y), r4(*radius)], *filled)
        }
        PcbShape::Poly { pts, filled } => {
            let mut d = Vec::with_capacity(pts.len() * 2);
            for p in pts {
                let q = tf(*p);
                d.push(r4(q.x));
                d.push(r4(q.y));
            }
            (GKind::Poly, d, *filled)
        }
    };
    out.push(GraphicDef { layer, width: r4(width), kind, data, filled, comp });
}

/// Even-odd point-in-polygon for a closed ring of points (board mm).
fn point_in_ring(pts: &[Pt], x: f64, y: f64) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (pts[i].x, pts[i].y);
        let (xj, yj) = (pts[j].x, pts[j].y);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Does via `v` connect to copper on `layer`? True when a track on that layer ends at the
/// via (within its radius) or a same-net filled zone on the layer covers its centre — the
/// test KiCad applies to keep an annular ring under "remove unused layers". Track endpoints
/// at a via are same-net by construction (clearance keeps others away), so they need no net
/// check; zones do (a GND pour covers signal vias it isn't connected to).
fn via_connects_layer(pcb: &Pcb, v: &Via, layer: &str) -> bool {
    let r = v.size / 2.0 + 1e-4;
    let near = |p: &Pt| (p.x - v.at.x).hypot(p.y - v.at.y) <= r;
    if pcb
        .tracks
        .iter()
        .any(|t| t.layer.as_str() == layer && (near(&t.start) || near(&t.end)))
    {
        return true;
    }
    let vnet = net_name_of(pcb, v.net, &v.net_name);
    pcb.zones.iter().any(|z| {
        z.filled
            && z.layer.as_str() == layer
            && net_name_of(pcb, z.net, &z.net_name) == vnet
            && point_in_ring(&z.pts, v.at.x, v.at.y)
    })
}

/// Build the geometry IR from a parsed board. `theme` resolves user-layer colours
/// (same as the SVG path); pass `Theme::default()` when none is available. `source` is
/// the board file name for the title block's "File" cell (may be empty).
pub fn build(pcb: &Pcb, theme: &crate::theme::Theme, source: &str) -> Geometry {
    // ---- layer table (same selection as the SVG path: stackup order, content-bearing) ----
    let mut raw: Vec<(String, &'static str, Option<String>, i64)> = pcb
        .layers
        .iter()
        .map(|l| (l.name.clone(), layer_role(&l.name), l.user_name.clone(), l.ordinal))
        .collect();
    if raw.is_empty() {
        for (i, (n, r)) in [
            ("F.Cu", "copper"),
            ("B.Cu", "copper"),
            ("F.SilkS", "silkscreen"),
            ("B.SilkS", "silkscreen"),
            ("Edge.Cuts", "edge"),
        ]
        .into_iter()
        .enumerate()
        {
            raw.push((n.to_string(), r, None, i as i64));
        }
    }
    let mut layers: Vec<LayerDef> = Vec::new();
    let mut layer_idx: HashMap<String, u16> = HashMap::new();
    for (name, role, user_name, ord) in &raw {
        // Keep an empty user layer out of the table (matches the SVG path), but
        // standard fabrication layers always stay so their indices are stable.
        if *role == "user" && !layer_has_content(pcb, name) {
            continue;
        }
        let _ = user_name; // display name lives in the manifest; the renderer reads it there
        let idx = layers.len() as u16;
        layer_idx.insert(name.clone(), idx);
        layers.push(LayerDef {
            name: name.clone(),
            role,
            side: layer_side(name),
            ord: *ord,
            color: if *role == "user" { user_layer_color(theme, name) } else { None },
        });
    }
    // Copper layer (index, name) pairs, for via/net layer membership.
    let copper: Vec<(u16, String)> = layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.role == "copper")
        .map(|(i, l)| (i as u16, l.name.clone()))
        .collect();

    let mut nets: Vec<String> = vec![String::new()];
    let mut net_idx: HashMap<String, u32> = HashMap::new();

    // ---- components ----
    let mut components: Vec<CompDef> = Vec::with_capacity(pcb.footprints.len());
    for fp in &pcb.footprints {
        let layer = layer_idx.get(&fp.layer).map(|&i| i as i32).unwrap_or(-1);
        let bbox = fp
            .bbox
            .map(|(mn, mx)| [r4(mn.x), r4(mn.y), r4(mx.x - mn.x), r4(mx.y - mn.y)]);
        components.push(CompDef {
            reference: fp.reference.clone(),
            fp: fp.lib_id.clone(),
            layer,
            x: r4(fp.at.x),
            y: r4(fp.at.y),
            angle: r4(fp.at.angle),
            dnp: fp.dnp,
            bbox,
        });
    }

    // ---- tracks (columnar) ----
    let mut tracks = Tracks::default();
    for t in &pcb.tracks {
        let Some(&layer) = layer_idx.get(&t.layer) else { continue };
        let net = intern_net(net_name_of(pcb, t.net, &t.net_name), &mut nets, &mut net_idx);
        match t.mid {
            Some(m) => {
                tracks.arc.xy.extend([
                    r4(t.start.x), r4(t.start.y), r4(m.x), r4(m.y), r4(t.end.x), r4(t.end.y),
                ]);
                tracks.arc.w.push(r4(t.width));
                tracks.arc.layer.push(layer);
                tracks.arc.net.push(net);
            }
            None => {
                tracks
                    .seg
                    .xy
                    .extend([r4(t.start.x), r4(t.start.y), r4(t.end.x), r4(t.end.y)]);
                tracks.seg.w.push(r4(t.width));
                tracks.seg.layer.push(layer);
                tracks.seg.net.push(net);
            }
        }
    }

    // ---- vias ----
    let mut vias: Vec<ViaDef> = Vec::with_capacity(pcb.vias.len());
    for v in &pcb.vias {
        let net = intern_net(net_name_of(pcb, v.net, &v.net_name), &mut nets, &mut net_idx);
        // Every spanned copper layer (the via barrel + hole wall paint on all of them).
        let spanned: Vec<u16> =
            copper.iter().filter(|(_, n)| via_on_layer(v, n)).map(|(i, _)| *i).collect();
        // Ring layers: with KiCad's "remove unused layers", only the copper layers the via
        // actually connects to keep their annular ring; the others show the bare barrel +
        // wall (matching pcbnew). `None` = every spanned layer keeps its ring (through via).
        // A connectivity result that resolves to nothing or to every layer collapses to
        // `None`, so a via we can't classify never loses every ring.
        let ring: Option<Vec<u16>> = if v.remove_unused_layers {
            let connected: Vec<u16> = copper
                .iter()
                .filter(|(_, n)| via_on_layer(v, n) && via_connects_layer(pcb, v, n))
                .map(|(i, _)| *i)
                .collect();
            if connected.is_empty() || connected.len() == spanned.len() {
                None
            } else {
                Some(connected)
            }
        } else {
            None
        };
        vias.push(ViaDef {
            x: r4(v.at.x),
            y: r4(v.at.y),
            size: r4(v.size),
            drill: r4(v.drill),
            net,
            layers: spanned,
            ring,
        });
    }

    // ---- pads (placed to board space) ----
    let mut pads: Vec<PadDef> = Vec::new();
    for (ci, fp) in pcb.footprints.iter().enumerate() {
        for pad in &fp.pads {
            let ctr = place_fp(fp, pad.at.x, pad.at.y);
            let net = intern_net(net_name_of(pcb, pad.net, &pad.net_name), &mut nets, &mut net_idx);
            let players: Vec<u16> = layers
                .iter()
                .enumerate()
                .filter(|(_, l)| pad_on_layer(pad, &l.name))
                .map(|(i, _)| i as u16)
                .collect();
            pads.push(PadDef {
                x: r4(ctr.x),
                y: r4(ctr.y),
                w: r4(pad.size.0),
                h: r4(pad.size.1),
                angle: r4(pad.at.angle),
                shape: shape_code(&pad.shape),
                rratio: r4(pad.roundrect_rratio),
                drill: r4(pad.drill),
                drillh: r4(pad.drill_h),
                net,
                comp: ci as i32,
                num: pad.number.clone(),
                layers: players,
                mask: pad.mask_margin.map(r4),
                npth: pad.kind == "np_thru_hole",
            });
        }
    }

    // ---- zones ----
    let mut zones: Vec<ZoneDef> = Vec::new();
    for z in &pcb.zones {
        if z.pts.len() < 3 {
            continue;
        }
        let Some(&layer) = layer_idx.get(&z.layer) else { continue };
        let net = intern_net(net_name_of(pcb, z.net, &z.net_name), &mut nets, &mut net_idx);
        let mut pts = Vec::with_capacity(z.pts.len() * 2);
        for p in &z.pts {
            pts.push(r4(p.x));
            pts.push(r4(p.y));
        }
        zones.push(ZoneDef { layer, net, filled: z.filled, keepout: z.keepout, pts });
    }

    // ---- graphics (board, then footprint, all placed to board space) ----
    let mut graphics: Vec<GraphicDef> = Vec::new();
    for g in &pcb.graphics {
        let Some(&layer) = layer_idx.get(&g.layer) else { continue };
        push_graphic(&mut graphics, &g.shape, g.width, layer, None, &|p| p);
    }
    for (ci, fp) in pcb.footprints.iter().enumerate() {
        for g in &fp.graphics {
            let Some(&layer) = layer_idx.get(&g.layer) else { continue };
            push_graphic(&mut graphics, &g.shape, g.width, layer, Some(ci as u32), &|p| {
                place_fp(fp, p.x, p.y)
            });
        }
    }

    // ---- text (board, then footprint) ----
    let mut texts: Vec<TextDef> = Vec::new();
    for t in pcb.texts.iter().filter(|t| !t.hidden) {
        let Some(&layer) = layer_idx.get(&t.layer) else { continue };
        texts.push(TextDef {
            layer,
            text: crate::svg::display_text(&t.text),
            x: r4(t.at.x),
            y: r4(t.at.y),
            angle: r4(t.at.angle),
            size: r4(t.size),
            width: text_width_opt(t.width, t.size),
            thickness: t.thickness.map(r4),
            justify: [t.justify.h, t.justify.v],
            mirror: t.justify.mirror,
            bold: t.bold,
            italic: t.italic,
            knockout: t.knockout,
            upright: false,
            font: t.font.clone(),
            comp: None,
            role: String::new(),
        });
    }
    for (ci, fp) in pcb.footprints.iter().enumerate() {
        for t in fp.texts.iter().filter(|t| !t.hidden) {
            let Some(&layer) = layer_idx.get(&t.layer) else { continue };
            let p = place_fp(fp, t.at.x, t.at.y);
            texts.push(TextDef {
                layer,
                text: crate::svg::display_text(&t.text),
                x: r4(p.x),
                y: r4(p.y),
                angle: r4(t.at.angle),
                size: r4(t.size),
                width: text_width_opt(t.width, t.size),
                thickness: t.thickness.map(r4),
                justify: [t.justify.h, t.justify.v],
                mirror: t.justify.mirror,
                bold: t.bold,
                italic: t.italic,
                knockout: t.knockout,
                upright: true,
                font: t.font.clone(),
                comp: Some(ci as u32),
                role: t.kind.clone(),
            });
        }
    }

    // ---- header (bbox / page / frame) ----
    let (vx, vy, vw, vh) = board_viewbox(pcb);
    let page = crate::svg::page_dims(pcb.paper.as_deref(), pcb.paper_dims).map(|(w, h)| [r4(w), r4(h)]);
    let frame = page.map(|_| Frame {
        title: pcb.title.clone().unwrap_or_default(),
        company: pcb.company.clone().unwrap_or_default(),
        rev: pcb.rev.clone().unwrap_or_default(),
        date: pcb.date.clone().unwrap_or_default(),
        version: pcb.generator_version.clone().unwrap_or_default(),
        comments: pcb.comments.clone(),
        paper: pcb.paper.clone().unwrap_or_default(),
        file: source.to_string(),
    });

    Geometry {
        schema: GEOMETRY_SCHEMA,
        units: "mm",
        bbox: [r4(vx), r4(vy), r4(vw), r4(vh)],
        page,
        frame,
        layers,
        nets,
        components,
        tracks,
        vias,
        pads,
        zones,
        graphics,
        texts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r##"
    (kicad_pcb (version 20240101)
      (layers (0 "F.Cu" signal) (1 "In1.Cu" signal) (31 "B.Cu" signal)
              (37 "F.SilkS" user) (39 "F.Mask" user) (44 "Edge.Cuts" user)
              (49 "F.Fab" user) (51 "F.CrtYd" user) (53 "F.Paste" user)
              (45 "User.4" user "Mechanical_Drawing"))
      (net 0 "")
      (net 1 "GND")
      (net 2 "/SIG")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 60 90)
        (property "Reference" "R1" (at 0 1 0) (layer "F.SilkS"))
        (fp_line (start -1 -0.5) (end 1 -0.5) (layer "F.SilkS") (stroke (width 0.12)))
        (fp_rect (start -1 -0.5) (end 1 0.5) (layer "F.CrtYd"))
        (pad "1" thru_hole circle (at -0.8 0) (size 1.2 1.2) (drill 0.6)
          (layers "*.Cu" "*.Mask") (net 1 "GND") (uuid "pad1"))
        (pad "2" smd roundrect (at 0.8 0) (size 0.9 1.0) (layers "F.Cu" "F.Mask" "F.Paste")
          (roundrect_rratio 0.25) (net 2 "/SIG") (uuid "pad2")))
      (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net 2) (uuid "t1"))
      (arc (start 0 0) (mid 1 1) (end 2 0) (width 0.2) (layer "B.Cu") (net 1) (uuid "t2"))
      (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v1"))
      (zone (net 1) (net_name "GND") (layer "F.Cu") (uuid "z1")
        (filled_polygon (layer "F.Cu") (pts (xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_text "NOTE" (at 30 30 0) (layer "F.SilkS")))
    "##;

    fn geom() -> Geometry {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        build(&pcb, &crate::theme::Theme::default(), "board.kicad_pcb")
    }

    #[test]
    fn via_remove_unused_keeps_only_connected_layers() {
        // 4-copper-layer board; a through via (F.Cu..B.Cu) spans all four. With
        // remove_unused_layers it still paints the barrel on every spanned layer, but keeps a
        // ring only where it connects: F.Cu (a track ends at it) and In1.Cu (a same-net GND
        // pour covers it), dropping the ring on In2.Cu and B.Cu (content-bearing but
        // unconnected to the via). A control via keeps a ring on all four (ring = None).
        let src = r##"
        (kicad_pcb (version 20240101)
          (layers (0 "F.Cu" signal) (1 "In1.Cu" signal) (2 "In2.Cu" signal) (31 "B.Cu" signal)
                  (44 "Edge.Cuts" user))
          (net 0 "")
          (net 1 "GND")
          (net 2 "/SIG")
          (segment (start 0 0) (end 5 5) (width 0.25) (layer "F.Cu") (net 1) (uuid "t1"))
          (segment (start 20 20) (end 25 25) (width 0.25) (layer "In2.Cu") (net 2) (uuid "t3"))
          (segment (start 30 30) (end 35 35) (width 0.25) (layer "B.Cu") (net 2) (uuid "t4"))
          (zone (net 1) (net_name "GND") (layer "In1.Cu") (uuid "z1")
            (filled_polygon (layer "In1.Cu") (pts (xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10))))
          (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (remove_unused_layers yes) (net 1) (uuid "v1"))
          (via (at 8 8) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v2"))
          (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts")))
        "##;
        let pcb = Pcb::parse_str(src).unwrap();
        let g = build(&pcb, &crate::theme::Theme::default(), "board.kicad_pcb");
        let names = |ls: &[u16]| {
            let mut ns: Vec<&str> = ls.iter().map(|&i| g.layers[i as usize].name.as_str()).collect();
            ns.sort_unstable();
            ns
        };
        // Both vias span every copper layer (barrel on all four).
        assert_eq!(names(&g.vias[0].layers), vec!["B.Cu", "F.Cu", "In1.Cu", "In2.Cu"]);
        assert_eq!(names(&g.vias[1].layers), vec!["B.Cu", "F.Cu", "In1.Cu", "In2.Cu"]);
        // remove_unused via keeps a ring only where it connects; control via keeps all (None).
        assert_eq!(
            names(g.vias[0].ring.as_deref().expect("remove_unused via carries a ring subset")),
            vec!["F.Cu", "In1.Cu"],
            "ring only where the via connects",
        );
        assert!(g.vias[1].ring.is_none(), "control via keeps a ring on every spanned layer");
    }

    #[test]
    fn frame_carries_paper_title_block_and_source_file() {
        // A declared paper yields the page + title-block frame the worksheet overlay draws;
        // the board file name (source) feeds the title block's "File" cell.
        let src = r##"
        (kicad_pcb (version 20240101)
          (paper "A4")
          (title_block (title "My Board") (rev "C") (company "Acme"))
          (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
          (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts")))
        "##;
        let pcb = Pcb::parse_str(src).unwrap();
        let g = build(&pcb, &crate::theme::Theme::default(), "my_board.kicad_pcb");
        assert_eq!(g.page, Some([297.0, 210.0]), "A4 landscape page");
        let f = g.frame.expect("a board with a paper carries a title-block frame");
        assert_eq!(f.title, "My Board");
        assert_eq!(f.rev, "C");
        assert_eq!(f.company, "Acme");
        assert_eq!(f.paper, "A4");
        assert_eq!(f.file, "my_board.kicad_pcb");
    }

    #[test]
    fn tables_and_indices() {
        let g = geom();
        assert_eq!(g.schema, GEOMETRY_SCHEMA);
        // F.Cu first, copper, front side.
        let fcu = g.layers.iter().position(|l| l.name == "F.Cu").unwrap();
        assert_eq!(g.layers[fcu].role, "copper");
        assert_eq!(g.layers[fcu].side, Some("front"));
        assert_eq!(g.layers.iter().find(|l| l.name == "In1.Cu").unwrap().side, Some("inner"));
        // Net 0 is the empty sentinel; GND/SIG interned.
        assert_eq!(g.nets[0], "");
        assert!(g.nets.iter().any(|n| n == "GND"));
        assert!(g.nets.iter().any(|n| n == "/SIG"));
        // The empty user layer that carries content (User.4 has no content here) is dropped;
        // a content-bearing one (F.SilkS has the note + fp line) stays.
        assert!(g.layers.iter().any(|l| l.name == "F.SilkS"));
        assert!(!g.layers.iter().any(|l| l.name == "User.4"), "empty user layer dropped");
    }

    #[test]
    fn primitives_carry_layer_and_net() {
        let g = geom();
        // One straight segment on F.Cu, net /SIG.
        assert_eq!(g.tracks.seg.w.len(), 1);
        assert_eq!(g.tracks.seg.xy.len(), 4);
        let sig = g.nets.iter().position(|n| n == "/SIG").unwrap() as u32;
        assert_eq!(g.tracks.seg.net[0], sig);
        // One arc track on B.Cu.
        assert_eq!(g.tracks.arc.w.len(), 1);
        assert_eq!(g.tracks.arc.xy.len(), 6);
        // A through via (F.Cu..B.Cu) covers every copper layer — here F.Cu, In1.Cu, B.Cu.
        assert_eq!(g.vias.len(), 1);
        assert_eq!(g.vias[0].layers.len(), 3);
        // Pads: thru-hole pad 1 (circle, drill), smd roundrect pad 2.
        assert_eq!(g.pads.len(), 2);
        let p1 = g.pads.iter().find(|p| p.num == "1").unwrap();
        assert_eq!(p1.shape, 0); // circle
        assert!(p1.drill > 0.0);
        assert!(p1.layers.len() >= 2, "thru-hole pad on *.Cu + *.Mask");
        let p2 = g.pads.iter().find(|p| p.num == "2").unwrap();
        assert_eq!(p2.shape, 2); // roundrect
        assert!((p2.rratio - 0.25).abs() < 1e-9);
        assert_eq!(p2.comp, 0); // belongs to component R1 (index 0)
        // Zone present, filled, on F.Cu, GND.
        assert_eq!(g.zones.len(), 1);
        assert!(g.zones[0].filled);
        assert_eq!(g.zones[0].pts.len(), 8);
        // Graphics: Edge.Cuts line + footprint silk line + footprint courtyard rect (poly).
        assert!(g.graphics.iter().any(|gr| matches!(gr.kind, GKind::Seg)));
        assert!(g.graphics.iter().any(|gr| matches!(gr.kind, GKind::Poly)));
        // Text: board note + footprint reference R1 (upright, role reference).
        assert!(g.texts.iter().any(|t| t.text == "NOTE" && !t.upright));
        assert!(g.texts.iter().any(|t| t.role == "reference" && t.upright));
    }

    #[test]
    fn footprint_geometry_placed_to_board_space() {
        let g = geom();
        // R1 sits at (50,60) rotated 90°. Its courtyard rect corners must land near the
        // footprint origin in board space, not at the local (-1,-0.5)..(1,0.5).
        let rect = g
            .graphics
            .iter()
            .find(|gr| gr.comp == Some(0) && matches!(gr.kind, GKind::Poly))
            .expect("placed footprint rect");
        for chunk in rect.data.chunks(2) {
            assert!((chunk[0] - 50.0).abs() < 3.0, "x near footprint origin 50");
            assert!((chunk[1] - 60.0).abs() < 3.0, "y near footprint origin 60");
        }
    }

    #[test]
    fn serializes_deterministically() {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        let a = serde_json::to_string(&build(&pcb, &crate::theme::Theme::default(), "b.kicad_pcb")).unwrap();
        let b = serde_json::to_string(&build(&pcb, &crate::theme::Theme::default(), "b.kicad_pcb")).unwrap();
        assert_eq!(a, b, "IR JSON must be byte-identical across runs");
        // Sanity: it parses back as JSON.
        let _: serde_json::Value = serde_json::from_str(&a).unwrap();
    }
}
