//! Altium board front-end: `.PcbDoc` -> the same [`crate::ir::Geometry`] the
//! KiCad path emits, written as `pcb/geometry.json`.
//!
//! The output contract does not change, so the GPU renderer, the net-length and
//! layer-count review moves and the diff engine all work unchanged. What this
//! module owns is the projection: Altium's Y-up millimetre space into the
//! bundle's Y-down one, its legacy layer ids into the bundle's `role`
//! vocabulary, and its stacked pad model into one pad per placed pad.
//!
//! **The flip.** Altium coordinates are absolute in a workspace whose origin is
//! bottom-left; the bundle is Y-down. Every point therefore goes through
//! [`Flip::pt`], which subtracts Y from a fixed constant — the workspace height,
//! not the content extent, so two revisions of a board land in the same space
//! and the diff compares like with like. A Y flip also reverses the sense of
//! every angle, which is why rotations are negated on the way out.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use eda_parse_altium::pcb::{self, PcbDoc, BOTTOM, KEEP_OUT, MULTI_LAYER, TOP};
use eda_parse_altium::{Doc, Kind};

use crate::ir::{
    ArcCol, CompDef, GKind, Geometry, GraphicDef, LayerDef, PadDef, SegCol, TextDef, Tracks,
    ViaDef, ZoneDef, GEOMETRY_SCHEMA,
};
use crate::pipeline::Msg;

use super::layers;

/// Altium's workspace is 100 inches square, and its origin is bottom-left. The
/// flip axis is that constant rather than the board's own extent so the mapping
/// is a property of the FILE FORMAT, not of the content: move one track and
/// every other primitive keeps its coordinates, which is what the diff engine
/// needs.
const WORKSPACE_MM: f64 = 2540.0;

/// Round to 0.1 µm, matching the KiCad path: far below fab tolerance, keeps the
/// JSON small and byte-deterministic.
fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// Altium space (Y-up, millimetres) to bundle space (Y-down, millimetres).
#[derive(Debug, Clone, Copy)]
struct Flip;

impl Flip {
    fn pt(self, x: f64, y: f64) -> (f64, f64) {
        (r4(x), r4(WORKSPACE_MM - y))
    }
    /// A Y flip reverses the sense of a rotation.
    fn angle(self, deg: f64) -> f64 {
        r4(-deg)
    }
}

/// Everything the caller needs to describe the board it just extracted.
pub struct BoardSummary {
    pub layers: usize,
    pub components: usize,
    pub tracks: usize,
    pub pads: usize,
    pub vias: usize,
    /// Net classes from `Classes6`, for the design model's `net_name_to_classes`.
    pub net_classes: Vec<(String, Vec<String>)>,
    /// Layer-stack regions detected but not modelled (plan §4.5).
    pub substacks: Vec<String>,
    /// Board streams read but not modelled, by stream and record count.
    pub skipped: BTreeMap<String, usize>,
    /// Altium special strings drawn verbatim because this build has no value
    /// for them.
    pub unresolved_specials: usize,
}

/// The `.PcbDoc` belonging to an Altium project, if one sits beside it.
///
/// A `.PrjPcb` names its documents; a loose `.SchDoc` has the board next to it
/// under the same stem, and a `.PcbDoc` opened directly is its own board.
pub fn board_beside(project: &Path) -> Option<std::path::PathBuf> {
    if Kind::of(project) == Kind::Board {
        return Some(project.to_path_buf());
    }
    let dir = project.parent()?;
    let stem = project.file_stem()?.to_str()?.to_ascii_lowercase();
    let mut boards: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| Kind::of(p) == Kind::Board)
        .collect();
    boards.sort();
    // A same-stem board is the project's own; otherwise a single board in the
    // project directory is unambiguous. Several with no stem match is not, and
    // guessing there would review the wrong board.
    boards
        .iter()
        .find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase() == stem)
                .unwrap_or(false)
        })
        .cloned()
        .or_else(|| boards.first().cloned().filter(|_| boards.len() == 1))
}

/// Read a board and write `pcb/geometry.json` beside the design bundle.
pub fn extract_pcb(
    board_path: &Path,
    out_dir: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<(String, BoardSummary), String> {
    let doc = Doc::open(board_path)?;
    let board = pcb::parse(&doc);
    let source = board_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let built = build(&board, &source);
    let g = &built.geometry;
    let summary = BoardSummary {
        layers: g.layers.len(),
        components: g.components.len(),
        tracks: g.tracks.seg.w.len() + g.tracks.arc.w.len(),
        pads: g.pads.len(),
        vias: g.vias.len(),
        net_classes: board.net_classes.clone(),
        substacks: board.substacks.clone(),
        skipped: board.skipped.clone(),
        unresolved_specials: built.unresolved_specials,
    };
    let pcb_dir = out_dir.join("pcb");
    std::fs::create_dir_all(&pcb_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string(g).map_err(|e| e.to_string())?;
    std::fs::write(pcb_dir.join("geometry.json"), json).map_err(|e| e.to_string())?;
    emit(Msg::Progress(format!(
        "pcb geometry: {} layers, {} nets, {} seg + {} arc tracks, {} vias, {} pads, {} zones, {} graphics, {} texts",
        g.layers.len(),
        g.nets.len().saturating_sub(1),
        g.tracks.seg.w.len(),
        g.tracks.arc.w.len(),
        g.vias.len(),
        g.pads.len(),
        g.zones.len(),
        g.graphics.len(),
        g.texts.len(),
    )));
    let rel = "pcb/geometry.json".to_string();
    emit(Msg::Artifact(rel.clone()));
    Ok((rel, summary))
}

/// The geometry document plus what building it could not resolve.
pub struct Built {
    pub geometry: Geometry,
    /// Altium special strings with no value in this build (a drill legend, a
    /// print date). The literal is drawn and the count is reported.
    pub unresolved_specials: usize,
}

/// Project a parsed board into the geometry IR.
pub fn build(b: &PcbDoc, source: &str) -> Built {
    let f = Flip;

    // ---- layer table -------------------------------------------------------
    let mut used: BTreeMap<u8, bool> = BTreeMap::new();
    let mut mark = |id: u8| {
        used.insert(id, true);
    };
    for t in &b.tracks {
        mark(t.c.layer);
    }
    for a in &b.arcs {
        mark(a.c.layer);
    }
    for p in &b.pads {
        mark(p.c.layer);
    }
    for v in &b.vias {
        mark(v.c.layer);
    }
    for t in &b.texts {
        mark(t.c.layer);
    }
    for x in &b.fills {
        mark(x.c.layer);
    }
    for r in &b.regions {
        mark(r.c.layer);
    }
    let order = layers::emit_order(b, &|id| used.contains_key(&id));
    let name_of = |id: u8| {
        b.layers
            .get(&id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| pcb::default_layer_name(id))
    };
    // A layer's role depends on its name as well as its id (a mechanical layer
    // can be the board profile), so it is resolved once and read back by index.
    let role_of = |id: u8| layers::role(id, &name_of(id));
    let mut layer_defs: Vec<LayerDef> = Vec::new();
    let mut layer_idx: HashMap<u8, u16> = HashMap::new();
    for (ord, &id) in order.iter().enumerate() {
        layer_idx.insert(id, layer_defs.len() as u16);
        layer_defs.push(LayerDef {
            name: name_of(id),
            role: role_of(id),
            side: layers::side(id),
            ord: ord as i64,
            // Colour is Altium's own only for the layers the bundle does not
            // already theme by role; the fabrication layers keep the viewer's
            // CSS variables, exactly as the KiCad path leaves them.
            color: None,
        });
    }
    let li = |id: u8| layer_idx.get(&id).copied();

    // ---- nets --------------------------------------------------------------
    // `Nets6` order IS the id every primitive references, so the table is
    // reproduced verbatim with index 0 kept as the schema's no-net sentinel.
    let mut nets: Vec<String> = vec![String::new()];
    nets.extend(b.nets.iter().map(|n| n.name.clone()));
    let net_of = |i: Option<u16>| i.map(|i| u32::from(i) + 1).unwrap_or(0);

    // ---- components --------------------------------------------------------
    let designators = pcb::designators(b);
    // Corner case 23: Altium places a part at the centre of the bounding box of
    // its own pad anchors, NOT at the footprint origin. Emitting the origin puts
    // every part slightly out and an off-body origin badly out.
    let mut pad_bbox: HashMap<u16, [f64; 4]> = HashMap::new();
    for p in &b.pads {
        let Some(c) = p.c.component else { continue };
        let e = pad_bbox.entry(c).or_insert([f64::MAX, f64::MAX, f64::MIN, f64::MIN]);
        e[0] = e[0].min(p.x);
        e[1] = e[1].min(p.y);
        e[2] = e[2].max(p.x);
        e[3] = e[3].max(p.y);
    }
    let mut components: Vec<CompDef> = Vec::with_capacity(b.components.len());
    for (i, c) in b.components.iter().enumerate() {
        let bb = pad_bbox.get(&(i as u16));
        let (cx, cy) = match bb {
            Some(e) => ((e[0] + e[2]) / 2.0, (e[1] + e[3]) / 2.0),
            None => (c.x, c.y),
        };
        let (x, y) = f.pt(cx, cy);
        components.push(CompDef {
            reference: designators.get(i).cloned().unwrap_or_default(),
            fp: c.pattern.clone(),
            layer: li(c.layer).map(i32::from).unwrap_or(-1),
            x,
            y,
            angle: f.angle(c.rotation),
            // Altium records "not fitted" in a variant, not on the placement, so
            // the board says nothing about DNP; the BOM path owns that.
            dnp: false,
            bbox: bb.map(|e| {
                let (x0, y0) = f.pt(e[0], e[3]);
                [x0, y0, r4(e[2] - e[0]), r4(e[3] - e[1])]
            }),
            // `SOURCEUNIQUEID` is the schematic instance, which is what the diff
            // engine pairs on across revisions; `UNIQUEID` is per-file and
            // changes when the board is re-annotated.
            uuid: if c.source_unique_id.is_empty() {
                c.unique_id.clone()
            } else {
                c.source_unique_id.clone()
            },
        });
    }
    let comp_of = |i: Option<u16>| i.map(u32::from);

    // ---- line work ---------------------------------------------------------
    let mut tracks = Tracks { seg: SegCol::default(), arc: ArcCol::default() };
    let mut graphics: Vec<GraphicDef> = Vec::new();
    for t in &b.tracks {
        let Some(layer) = li(t.c.layer) else { continue };
        let (x1, y1) = f.pt(t.x1, t.y1);
        let (x2, y2) = f.pt(t.x2, t.y2);
        if role_of(t.c.layer) == "copper" {
            tracks.seg.xy.extend([x1, y1, x2, y2]);
            tracks.seg.w.push(r4(t.width));
            tracks.seg.layer.push(layer);
            tracks.seg.net.push(net_of(t.c.net));
        } else {
            graphics.push(GraphicDef {
                layer,
                width: r4(t.width),
                kind: GKind::Seg,
                data: vec![x1, y1, x2, y2],
                filled: false,
                comp: comp_of(t.c.component),
            });
        }
    }
    for a in &b.arcs {
        let Some(layer) = li(a.c.layer) else { continue };
        let sweep = normalise_sweep(a.start_angle, a.end_angle);
        // A closed arc has no meaningful three-point form — start and end sit on
        // top of each other and the renderer would draw nothing.
        if sweep >= 359.9 {
            let (cx, cy) = f.pt(a.cx, a.cy);
            graphics.push(GraphicDef {
                layer,
                width: r4(a.width),
                kind: GKind::Circle,
                data: vec![cx, cy, r4(a.radius)],
                filled: false,
                comp: comp_of(a.c.component),
            });
            continue;
        }
        let at = |deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            f.pt(a.cx + a.radius * c, a.cy + a.radius * s)
        };
        let (sx, sy) = at(a.start_angle);
        let (mx, my) = at(a.start_angle + sweep / 2.0);
        let (ex, ey) = at(a.start_angle + sweep);
        if role_of(a.c.layer) == "copper" {
            tracks.arc.xy.extend([sx, sy, mx, my, ex, ey]);
            tracks.arc.w.push(r4(a.width));
            tracks.arc.layer.push(layer);
            tracks.arc.net.push(net_of(a.c.net));
        } else {
            graphics.push(GraphicDef {
                layer,
                width: r4(a.width),
                kind: GKind::Arc,
                data: vec![sx, sy, mx, my, ex, ey],
                filled: false,
                comp: comp_of(a.c.component),
            });
        }
    }

    // ---- vias --------------------------------------------------------------
    let mut vias: Vec<ViaDef> = Vec::new();
    for v in &b.vias {
        let span = layers::span(&b.stack, v.from_layer, v.to_layer);
        let idx: Vec<u16> = span.iter().filter_map(|&id| li(id)).collect();
        if idx.is_empty() {
            continue;
        }
        let (x, y) = f.pt(v.x, v.y);
        vias.push(ViaDef {
            x,
            y,
            size: r4(v.diameter),
            drill: r4(v.hole),
            net: net_of(v.c.net),
            layers: idx,
            // Altium keeps the ring on every layer a via spans; there is no
            // "remove unused layers" equivalent to narrow it.
            ring: None,
        });
    }

    // ---- pads --------------------------------------------------------------
    let mut pads: Vec<PadDef> = Vec::new();
    for p in &b.pads {
        let occupied = layers::occupies(&b.stack, p.c.layer);
        let mut idx: Vec<u16> = occupied.iter().filter_map(|&id| li(id)).collect();
        // A pad's mask and paste apertures live on the fabrication layers, which
        // is where the mask/paste review moves look for them.
        for id in mask_and_paste(p.c.layer) {
            idx.extend(li(id));
        }
        if idx.is_empty() {
            continue;
        }
        let (x, y) = f.pt(p.x, p.y);
        let (shape, rratio) = shape_code(p.shape, p.w, p.h, p.corner_ratio);
        pads.push(PadDef {
            x,
            y,
            w: r4(p.w),
            h: r4(p.h),
            angle: f.angle(p.rotation),
            shape,
            rratio,
            drill: r4(p.hole),
            drillh: 0.0,
            net: net_of(p.c.net),
            comp: p.c.component.map(i32::from).unwrap_or(-1),
            num: p.name.clone(),
            layers: idx,
            mask: None,
            // A drilled hole with no plating is a bare hole — a mounting hole,
            // not a pad — and the viewer paints it as one.
            npth: p.hole > 0.0 && !p.plated,
        });
    }

    // ---- regions, fills and the board shape --------------------------------
    let mut zones: Vec<ZoneDef> = Vec::new();
    for r in &b.regions {
        let Some(layer) = li(r.c.layer) else { continue };
        if r.outline.len() < 3 {
            continue;
        }
        let pts: Vec<f64> = r
            .outline
            .iter()
            .flat_map(|&(x, y)| {
                let (x, y) = f.pt(x, y);
                [x, y]
            })
            .collect();
        // A region that belongs to a polygon is that pour's filled copper; a
        // keep-out is an unfilled restriction. Everything else — a shape-based
        // pad's paste aperture, a footprint's own copper region — is a filled
        // graphic on its layer.
        if r.c.polygon.is_some() || r.is_keepout() {
            zones.push(ZoneDef {
                layer,
                net: net_of(r.c.net),
                filled: r.c.polygon.is_some() && !r.is_keepout(),
                keepout: r.is_keepout(),
                pts,
            });
        } else {
            graphics.push(GraphicDef {
                layer,
                width: 0.0,
                kind: GKind::Poly,
                data: pts,
                filled: true,
                comp: comp_of(r.c.component),
            });
        }
    }
    for x in &b.fills {
        let Some(layer) = li(x.c.layer) else { continue };
        // A fill is an axis-aligned rectangle rotated about its own centre;
        // emitting the four placed corners keeps a rotated one correct.
        let (cx, cy) = ((x.x1 + x.x2) / 2.0, (x.y1 + x.y2) / 2.0);
        let (s, c) = x.rotation.to_radians().sin_cos();
        let corner = |px: f64, py: f64| {
            let (dx, dy) = (px - cx, py - cy);
            f.pt(cx + dx * c - dy * s, cy + dx * s + dy * c)
        };
        let data: Vec<f64> = [(x.x1, x.y1), (x.x2, x.y1), (x.x2, x.y2), (x.x1, x.y2)]
            .into_iter()
            .flat_map(|(px, py)| {
                let (px, py) = corner(px, py);
                [px, py]
            })
            .collect();
        graphics.push(GraphicDef {
            layer,
            width: 0.0,
            kind: GKind::Poly,
            data,
            filled: true,
            comp: comp_of(x.c.component),
        });
    }
    // The board shape, on the `edge` layer, so the existing "board outline =
    // graphics on the edge layer" review move keeps working unchanged.
    if let Some(edge) = li(KEEP_OUT) {
        for ring in &b.outline {
            if ring.len() < 3 {
                continue;
            }
            graphics.push(GraphicDef {
                layer: edge,
                width: 0.0,
                kind: GKind::Poly,
                data: ring
                    .iter()
                    .flat_map(|&(x, y)| {
                        let (x, y) = f.pt(x, y);
                        [x, y]
                    })
                    .collect(),
                filled: false,
                comp: None,
            });
        }
    }

    // ---- text --------------------------------------------------------------
    let mut texts: Vec<TextDef> = Vec::new();
    let mut unresolved_specials = 0usize;
    for t in &b.texts {
        let Some(layer) = li(t.c.layer) else { continue };
        let owner = t.c.component.and_then(|i| b.components.get(usize::from(i)));
        let r = resolve_special(
            &t.text,
            owner,
            t.c.component,
            &designators,
            source,
            &name_of(t.c.layer),
            t.is_designator,
            t.is_comment,
        );
        let (text, role) = (r.text, r.role);
        if r.unresolved {
            unresolved_specials += 1;
        }
        if text.is_empty() {
            continue;
        }
        let (x, y) = f.pt(t.x, t.y);
        texts.push(TextDef {
            layer,
            text,
            x,
            y,
            angle: f.angle(t.rotation),
            size: r4(t.height),
            width: None,
            thickness: (t.width > 0.0).then(|| r4(t.width)),
            // Altium anchors a string at the left of its baseline, where KiCad
            // centres it; saying so is what keeps silkscreen legends in place.
            justify: [-1, 1],
            mirror: t.mirror,
            bold: false,
            italic: false,
            knockout: false,
            upright: false,
            font: (!t.font.is_empty()).then(|| t.font.clone()),
            comp: comp_of(t.c.component),
            role,
        });
    }

    let bbox = bbox_of(&components, &tracks, &pads, &vias, &graphics, &zones);
    let geometry = Geometry {
        schema: GEOMETRY_SCHEMA,
        units: "mm",
        bbox,
        // Altium's sheet is a drawing frame around the board, not a page the
        // bundle draws; the title block it would fill is not modelled yet.
        page: None,
        frame: None,
        layers: layer_defs,
        nets,
        components,
        tracks,
        vias,
        pads,
        zones,
        graphics,
        texts,
    };
    Built { geometry, unresolved_specials }
}

/// The mask and paste layers a pad on `layer` also opens.
fn mask_and_paste(layer: u8) -> Vec<u8> {
    match layer {
        TOP => vec![35, 37],
        BOTTOM => vec![36, 38],
        // A through-hole pad opens both sides.
        MULTI_LAYER => vec![35, 36, 37, 38],
        _ => Vec::new(),
    }
}

/// Altium's pad shape to the bundle's compact code, with the corner ratio the
/// renderer needs for a rounded rectangle.
fn shape_code(shape: u8, w: f64, h: f64, corner_ratio: f64) -> (u8, f64) {
    match shape {
        // A round pad whose sides differ is an oval; equal, a circle.
        pcb::SHAPE_ROUND if (w - h).abs() < 1e-6 => (0, 0.0),
        pcb::SHAPE_ROUND => (3, 0.0),
        pcb::SHAPE_RECT => (1, 0.0),
        // The bundle has no octagon. A rounded rectangle is the closest shape
        // the renderer can draw and is what Altium's own octagons approximate.
        pcb::SHAPE_OCTAGONAL => (2, 0.2),
        pcb::SHAPE_ROUNDED_RECT => (2, r4(corner_ratio)),
        _ => (5, 0.0),
    }
}

/// One resolved text: what to draw, its `role`, and whether a special string
/// was left standing.
struct Resolved {
    text: String,
    role: String,
    /// True when the string is one of Altium's specials and this build has no
    /// value for it — a drill legend, a print date. The literal is kept (an
    /// empty silkscreen legend is a worse lie than a visible placeholder) and
    /// counted into the bundle's `unresolved` block.
    unresolved: bool,
}

/// Altium's special strings, resolved against the component or the document
/// that gives them a value.
///
/// A silkscreen designator is stored as the literal `.DESIGNATOR`, and emitting
/// that verbatim is what puts `.DESIGNATOR` on the rendered board instead of
/// `U6`.
#[allow(clippy::too_many_arguments)]
fn resolve_special(
    text: &str,
    owner: Option<&pcb::Component>,
    owner_index: Option<u16>,
    designators: &[String],
    source: &str,
    layer_name: &str,
    is_designator: bool,
    is_comment: bool,
) -> Resolved {
    let trimmed = text.trim();
    let key = trimmed.to_ascii_uppercase();
    let designator = owner_index
        .and_then(|i| designators.get(usize::from(i)))
        .cloned()
        .unwrap_or_default();
    let owned = |text: String, role: &str| Resolved {
        text,
        role: role.to_string(),
        unresolved: false,
    };
    match key.as_str() {
        ".DESIGNATOR" | ".NAME" => owned(designator, "reference"),
        ".COMMENT" | ".VALUE" => {
            owned(owner.map(|c| c.comment.clone()).unwrap_or_default(), "value")
        }
        ".PCB_FILE_NAME" | ".PCB_FILE_NAME_NO_PATH" => owned(source.to_string(), ""),
        ".LAYER_NAME" => owned(layer_name.to_string(), ""),
        // The file marks which text object IS the designator and which is the
        // comment, so the `role` never has to be guessed from the string.
        _ if is_designator => owned(text.to_string(), "reference"),
        _ if is_comment => owned(text.to_string(), "value"),
        _ if is_special(trimmed) => Resolved {
            text: text.to_string(),
            role: String::new(),
            unresolved: true,
        },
        _ => owned(
            text.to_string(),
            if owner.is_some() { "user" } else { "" },
        ),
    }
}

/// True for Altium's own spelling of a special string: a leading dot then an
/// identifier, with no spaces. `.PRJ_Copyright` is one; `.5 mm` is not.
fn is_special(text: &str) -> bool {
    let mut cs = text.chars();
    cs.next() == Some('.')
        && matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && text[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An Altium arc's sweep, counter-clockwise from `start` to `end`, in degrees.
/// A start equal to the end means the full circle, which is how Altium writes
/// one (`0` to `360`).
fn normalise_sweep(start: f64, end: f64) -> f64 {
    let mut sweep = end - start;
    while sweep < 0.0 {
        sweep += 360.0;
    }
    if sweep == 0.0 {
        360.0
    } else {
        sweep.min(360.0)
    }
}

/// Content extent `[x, y, w, h]`. Everything the renderer draws is included, so
/// off-board notes do not fall outside the view.
fn bbox_of(
    components: &[CompDef],
    tracks: &Tracks,
    pads: &[PadDef],
    vias: &[ViaDef],
    graphics: &[GraphicDef],
    zones: &[ZoneDef],
) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut add = |x: f64, y: f64| {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    };
    for c in tracks.seg.xy.chunks_exact(2).chain(tracks.arc.xy.chunks_exact(2)) {
        add(c[0], c[1]);
    }
    for p in pads {
        let r = p.w.max(p.h) / 2.0;
        add(p.x - r, p.y - r);
        add(p.x + r, p.y + r);
    }
    for v in vias {
        add(v.x - v.size / 2.0, v.y - v.size / 2.0);
        add(v.x + v.size / 2.0, v.y + v.size / 2.0);
    }
    for g in graphics {
        match g.kind {
            GKind::Circle => {
                if let [cx, cy, r] = g.data[..] {
                    add(cx - r, cy - r);
                    add(cx + r, cy + r);
                }
            }
            _ => {
                for c in g.data.chunks_exact(2) {
                    add(c[0], c[1]);
                }
            }
        }
    }
    for z in zones {
        for c in z.pts.chunks_exact(2) {
            add(c[0], c[1]);
        }
    }
    for c in components {
        add(c.x, c.y);
    }
    if x0 > x1 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    const PAD: f64 = 1.0;
    [r4(x0 - PAD), r4(y0 - PAD), r4(x1 - x0 + 2.0 * PAD), r4(y1 - y0 + 2.0 * PAD)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_altium::pcb::{Common, Layer, Pad, Track, Via};

    fn board() -> PcbDoc {
        let mut b = PcbDoc::default();
        for id in [TOP, 2u8, BOTTOM, 33, 35, 37, 36, 38, KEEP_OUT] {
            b.layers.insert(id, Layer { id, name: pcb::default_layer_name(id), ..Layer::default() });
        }
        b.stack = vec![TOP, 2, BOTTOM];
        b.nets.push(eda_parse_altium::pcb::Net { name: "GND".into(), color: "#000000".into() });
        b
    }

    fn common(layer: u8, net: Option<u16>) -> Common {
        Common { layer, net, polygon: None, component: None }
    }

    /// The flip is a property of the format, not of the content: a board's
    /// coordinates do not move because another primitive did.
    #[test]
    fn the_y_flip_is_anchored_to_the_workspace_not_the_content() {
        let mut b = board();
        b.tracks.push(Track {
            c: common(TOP, Some(0)),
            x1: 10.0,
            y1: 20.0,
            x2: 30.0,
            y2: 20.0,
            width: 0.2,
        });
        let g = build(&b, "b.PcbDoc").geometry;
        assert_eq!(g.tracks.seg.xy[0], 10.0);
        assert_eq!(g.tracks.seg.xy[1], WORKSPACE_MM - 20.0);
        b.tracks.push(Track { c: common(TOP, None), x1: 0.0, y1: 900.0, x2: 1.0, y2: 900.0, width: 0.2 });
        let g2 = build(&b, "b.PcbDoc").geometry;
        assert_eq!(&g2.tracks.seg.xy[..4], &g.tracks.seg.xy[..4], "the first track has not moved");
    }

    /// `Nets6` order is the id primitives carry, and index 0 stays the schema's
    /// no-net sentinel.
    #[test]
    fn net_indices_follow_the_nets6_table_with_a_sentinel_at_zero() {
        let mut b = board();
        b.nets.push(eda_parse_altium::pcb::Net { name: "VCC".into(), color: String::new() });
        b.tracks.push(Track { c: common(TOP, Some(1)), x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.0, width: 0.2 });
        b.tracks.push(Track { c: common(TOP, None), x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.0, width: 0.2 });
        let g = build(&b, "b").geometry;
        assert_eq!(g.nets, vec!["", "GND", "VCC"]);
        assert_eq!(g.tracks.seg.net, vec![2, 0]);
    }

    /// Corner case 23: Altium's own placement is the centre of the pad-anchor
    /// bounding box, not the footprint origin.
    #[test]
    fn a_component_is_placed_at_its_pad_bounding_box_centre() {
        let mut b = board();
        b.components.push(eda_parse_altium::pcb::Component {
            designator: "U1".into(),
            pattern: "QFN".into(),
            x: 100.0,
            y: 100.0,
            rotation: 90.0,
            layer: TOP,
            ..eda_parse_altium::pcb::Component::default()
        });
        let pad = |x: f64, y: f64| Pad {
            c: Common { layer: TOP, net: None, polygon: None, component: Some(0) },
            name: "1".into(),
            x,
            y,
            w: 0.5,
            h: 0.5,
            shape: pcb::SHAPE_RECT,
            corner_ratio: 0.0,
            hole: 0.0,
            rotation: 0.0,
            plated: true,
        };
        b.pads.push(pad(10.0, 10.0));
        b.pads.push(pad(20.0, 30.0));
        let g = build(&b, "b").geometry;
        assert_eq!(g.components[0].x, 15.0, "not the footprint origin at 100");
        assert_eq!(g.components[0].y, WORKSPACE_MM - 20.0);
        assert_eq!(g.components[0].angle, -90.0, "a Y flip reverses the rotation");
    }

    /// A through-hole pad opens copper on every layer of the stack plus both
    /// masks and pastes; an unplated one is a hole, not a pad.
    #[test]
    fn a_through_hole_pad_spans_the_stack_and_reports_plating() {
        let mut b = board();
        b.pads.push(Pad {
            c: common(MULTI_LAYER, None),
            name: "MH".into(),
            x: 5.0,
            y: 5.0,
            w: 3.0,
            h: 3.0,
            shape: pcb::SHAPE_ROUND,
            corner_ratio: 0.0,
            hole: 2.0,
            rotation: 0.0,
            plated: false,
        });
        let g = build(&b, "b").geometry;
        let p = &g.pads[0];
        assert!(p.npth, "a drilled hole with no plating is a mounting hole");
        assert_eq!(p.shape, 0, "equal sides make it a circle, not an oval");
        // top, mid, bottom copper + two pastes + two masks
        assert_eq!(p.layers.len(), 7);
    }

    /// A blind via spans only the copper between its own two layers.
    #[test]
    fn a_via_spans_only_its_own_layers() {
        let mut b = board();
        b.vias.push(Via {
            c: common(MULTI_LAYER, Some(0)),
            x: 1.0,
            y: 1.0,
            diameter: 0.6,
            hole: 0.3,
            from_layer: TOP,
            to_layer: 2,
        });
        let g = build(&b, "b").geometry;
        assert_eq!(g.vias[0].layers.len(), 2);
        assert_eq!(g.vias[0].net, 1);
    }

    /// Altium writes a closed arc as 0 -> 360, which has no three-point form.
    #[test]
    fn a_full_circle_arc_becomes_a_circle_not_a_degenerate_arc() {
        let mut b = board();
        b.arcs.push(eda_parse_altium::pcb::Arc {
            c: common(33, None),
            cx: 10.0,
            cy: 10.0,
            radius: 2.0,
            start_angle: 0.0,
            end_angle: 360.0,
            width: 0.1,
        });
        let g = build(&b, "b").geometry;
        assert!(matches!(g.graphics[0].kind, GKind::Circle));
        assert_eq!(g.graphics[0].data, vec![10.0, WORKSPACE_MM - 10.0, 2.0]);
        assert!(g.tracks.arc.w.is_empty(), "a silkscreen arc is not a copper track");
    }

    /// The literal `.DESIGNATOR` is a special string, not silkscreen text.
    #[test]
    fn special_strings_resolve_against_their_component() {
        let d = vec!["U6_CH1".to_string()];
        let c = eda_parse_altium::pcb::Component {
            designator: "U6".into(),
            comment: "STM32".into(),
            ..eda_parse_altium::pcb::Component::default()
        };
        let r = |t: &str, own: bool| {
            resolve_special(
                t, own.then_some(&c), own.then_some(0), &d, "b.PcbDoc", "Top Overlay", false, false,
            )
        };
        assert_eq!(r(".Designator", true).text, "U6_CH1");
        assert_eq!(
            resolve_special("RD1", Some(&c), Some(0), &d, "b", "L", true, false).role,
            "reference",
            "the role comes from the file's own flag, not from matching the string"
        );
        assert_eq!(r(".Comment", true).text, "STM32");
        assert_eq!(r(".Comment", true).role, "value");
        assert_eq!(r("PIN 1", true).role, "user");
        assert_eq!(r("KEEP CLEAR", false).role, "");
        assert_eq!(r(".PCB_FILE_NAME_NO_PATH", false).text, "b.PcbDoc");
        assert_eq!(r(".LAYER_NAME", false).text, "Top Overlay");
        // A special string with no value is drawn as it stands and counted: an
        // empty silkscreen legend is a worse lie than a visible placeholder.
        let legend = r(".LEGEND", false);
        assert!(legend.unresolved);
        assert_eq!(legend.text, ".LEGEND");
        assert!(r(".PRJ_Copyright", false).unresolved, "specials are not all upper case");
        assert!(!r("1.5mm", false).unresolved, "not every dot is a special string");
        assert!(!r(".5 mm clearance", false).unresolved, "nor every leading one");
    }

    #[test]
    fn the_board_shape_lands_on_the_edge_layer() {
        let mut b = board();
        b.outline.push(vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)]);
        let g = build(&b, "b").geometry;
        let edge = g.layers.iter().position(|l| l.role == "edge").expect("an edge layer");
        assert!(g
            .graphics
            .iter()
            .any(|x| usize::from(x.layer) == edge && matches!(x.kind, GKind::Poly)));
    }
}
