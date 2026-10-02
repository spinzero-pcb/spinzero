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

use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// Altium's text `Height` is the whole line box. Its capital letters are about
/// 0.72 of it, while the viewer's `size` is the capital height, so the height is
/// scaled to match what Altium draws (measured on MB1419: Altium's glyph box was
/// about 0.72 of the one the full height gave).
const TEXT_HEIGHT_TO_SIZE: f64 = 0.72;

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
    /// A text angle in the geometry is the angle a reader SEES, counter-clockwise,
    /// like a KiCad text angle. Altium's is the same visible angle, so the Y flip
    /// leaves it alone: negating it turns 90 degree text the wrong way.
    fn text_angle(self, deg: f64) -> f64 {
        r4(deg)
    }
}

/// The rigid-flex stack, with every GUID resolved to the name the designer
/// typed. `None` for a board with a single stack, which is every ordinary board
/// — the block is absent rather than empty so a reviewer is never shown a
/// one-region "stackup" that says nothing.
fn stackup(b: &PcbDoc) -> Option<crate::altium::StackupInfo> {
    if b.substacks.is_empty() {
        return None;
    }
    let name_of = |id: &str| -> String {
        b.substacks.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default()
    };
    let layer_name = |id: u8| -> String {
        b.layers.get(&id).map(|l| l.name.clone()).unwrap_or_else(|| format!("layer {id}"))
    };
    Some(crate::altium::StackupInfo {
        substacks: b
            .substacks
            .iter()
            .map(|s| crate::altium::SubstackInfo {
                name: s.name.clone(),
                is_flex: s.is_flex,
                copper_layers: s.layers.iter().map(|id| layer_name(*id)).collect(),
                layers: s.layer_names.clone(),
            })
            .collect(),
        drill_pairs: b
            .drill_pairs
            .iter()
            .map(|d| crate::altium::DrillPairInfo {
                low: d.low.clone(),
                high: d.high.clone(),
                substacks: d.substacks.iter().map(|g| name_of(g)).filter(|n| !n.is_empty()).collect(),
            })
            .collect(),
        regions: b
            .board_regions
            .iter()
            .map(|r| crate::altium::BoardRegionInfo {
                name: r.name.clone(),
                substack: name_of(&r.substack_id),
                bends: r
                    .bends
                    .iter()
                    .map(|d| {
                        let (ax, ay) = Flip.pt(d.a.0, d.a.1);
                        let (bx, by) = Flip.pt(d.b.0, d.b.1);
                        crate::altium::BendInfo {
                            angle_deg: r4(d.angle_deg),
                            radius_mm: r4(d.radius_mm),
                            from: [ax, ay],
                            to: [bx, by],
                        }
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// Child boards placed inside this one, in the bundle's Y-down space.
fn embedded_boards(b: &PcbDoc) -> Vec<crate::altium::EmbeddedBoardInfo> {
    b.embedded_boards
        .iter()
        .map(|e| {
            let (x, y) = Flip.pt(e.x, e.y);
            crate::altium::EmbeddedBoardInfo {
                document_path: e.document_path.clone(),
                at: [x, y],
                rotation: Flip.angle(e.rotation),
                mirrored: e.mirrored,
                rows: e.rows,
                columns: e.columns,
                // A pitch is a distance, so the flip does not touch it.
                row_spacing_mm: r4(e.row_spacing),
                column_spacing_mm: r4(e.column_spacing),
                instances: e.instances(),
            }
        })
        .collect()
}

/// Outline regions naming a substack the board does not declare. Such a region
/// draws on the master stack, which on a flex ribbon is the wrong layer count,
/// so it is worth counting rather than silently absorbing.
fn regions_without_substack(b: &PcbDoc) -> usize {
    if b.substacks.is_empty() {
        return 0;
    }
    b.board_regions
        .iter()
        .filter(|r| !b.substacks.iter().any(|s| s.id == r.substack_id))
        .count()
}

/// Everything the caller needs to describe the board it just extracted.
pub struct BoardSummary {
    pub layers: usize,
    pub components: usize,
    pub tracks: usize,
    pub pads: usize,
    pub vias: usize,
    /// The board's design rules, for the design model's `source` block.
    pub rules: Vec<pcb::Rule>,
    /// Net classes from `Classes6`, for the design model's `net_name_to_classes`.
    pub net_classes: Vec<(String, Vec<String>)>,
    /// The rigid-flex layer stack, when the board declares substacks.
    pub stackup: Option<crate::altium::StackupInfo>,
    /// Outline regions naming a substack the board does not declare.
    pub regions_without_substack: usize,
    /// Child boards placed inside this one.
    pub embedded_boards: Vec<crate::altium::EmbeddedBoardInfo>,
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

/// What a board contributed to the bundle.
pub struct BoardArtifacts {
    /// Cache-relative path of `pcb/geometry.json`.
    pub geometry: String,
    /// Manifest rows for the per-layer SVGs.
    pub svgs: Vec<serde_json::Value>,
    /// Board colours for the design model's `theme` block.
    pub theme: std::collections::BTreeMap<String, String>,
    /// The board's own 3D-view colours and opacities, for the design model's
    /// `board_3d` block. Not used in 2D.
    pub board_3d: crate::design::Board3d,
    pub summary: BoardSummary,
}

/// Read a board and write `pcb/geometry.json` plus the per-layer SVGs beside
/// the design bundle.
pub fn extract_pcb(
    board_path: &Path,
    out_dir: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<BoardArtifacts, String> {
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
        rules: board.rules.clone(),
        net_classes: board.net_classes.clone(),
        stackup: stackup(&board),
        regions_without_substack: regions_without_substack(&board),
        embedded_boards: embedded_boards(&board),
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

    // The vector layers come off the same IR the GPU renderer uploads, so the
    // two cannot disagree about where a primitive is. A write failure here
    // leaves the geometry — a board a reviewer can measure is worth more than
    // one they can look at.
    let svgs = match crate::altium::pcb_svg::write_layer_svgs(g, &source, out_dir, emit) {
        Ok(v) => v,
        Err(e) => {
            emit(Msg::Progress(format!("pcb layer svgs skipped: {e}")));
            Vec::new()
        }
    };
    emit(Msg::Progress(format!("pcb: {} layer svgs", svgs.len())));
    // A 3D artifact is worth having and never worth losing the board over.
    if let Err(e) = write_models(&board, &g.components, out_dir, emit) {
        emit(Msg::Progress(format!("models skipped: {e}")));
    }
    Ok(BoardArtifacts {
        geometry: rel,
        svgs,
        theme: crate::altium::pcb_svg::board_theme(g),
        board_3d: crate::design::Board3d {
            colors: board.view_colors.clone(),
            opacity: board.view_opacity.clone(),
        },
        summary,
    })
}

/// Write `models/models.json` and every embedded 3D model beside it.
///
/// Altium carries its own MCAD geometry: each model is a zlib-deflated file in a
/// stream of its own, and `ComponentBodies6` says where each one sits. So the
/// artifact needs no library to resolve and nothing can be "unresolved" except a
/// model the board only references — `EMBED=FALSE`, the file on the designer's
/// machine — which is counted the way a linked image is.
///
/// The document is the shared `extract.models.a0`, so a 3D view reads one shape
/// whatever drew the board. Two things are Altium's own and are named as such:
/// `at` on a model, because the file states the model's placement absolutely
/// rather than as an offset from the footprint, and `standoff`, which Altium
/// keeps per body.
fn write_models(
    b: &PcbDoc,
    components: &[CompDef],
    out_dir: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<usize, String> {
    if b.bodies.is_empty() && b.models.is_empty() {
        return Ok(0);
    }
    let models_dir = out_dir.join("models");
    let files_dir = models_dir.join("files");
    std::fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;

    // id -> (file name written, format). Written once however many bodies place
    // it: the corpus board embeds one model twice under two ids and 48 streams
    // hold 33 distinct files.
    let mut written: BTreeMap<&str, (String, String)> = BTreeMap::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut not_embedded = 0usize;
    for m in &b.models {
        if m.id.is_empty() {
            continue;
        }
        if m.data.is_empty() {
            not_embedded += 1;
            continue;
        }
        let stem = if m.name.trim().is_empty() { m.id.as_str() } else { m.name.trim() };
        let mut name = slug(stem);
        let mut n = 1;
        while !used.insert(name.clone()) {
            n += 1;
            name = format!("{}_{n}", slug(stem));
        }
        let format = std::path::Path::new(stem)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        std::fs::write(files_dir.join(&name), &m.data).map_err(|e| e.to_string())?;
        written.insert(m.id.as_str(), (format!("files/{name}"), format));
    }

    let by_id: BTreeMap<&str, &pcb::Model> = b.models.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut per_component: BTreeMap<u16, Vec<serde_json::Value>> = BTreeMap::new();
    let mut refs = 0usize;
    for body in &b.bodies {
        let id = body.params.s("MODELID");
        if id.is_empty() {
            continue;
        }
        let Some(owner) = body.c.component else { continue };
        let Some(model) = by_id.get(id) else { continue };
        refs += 1;
        let mil = |k: &str| body.params.f(k).map(eda_parse_altium::units::mil_mm);
        let (mx, my) = (mil("MODEL.2D.X").unwrap_or(0.0), mil("MODEL.2D.Y").unwrap_or(0.0));
        let (x, y) = Flip.pt(mx, my);
        let origin = b
            .components
            .get(usize::from(owner))
            .map(|c| Flip.pt(c.x, c.y))
            .unwrap_or((0.0, 0.0));
        // A model may carry no name of its own — the motherboard embeds one that
        // way — and the file is still real, so its id names it.
        let path = if model.name.trim().is_empty() { id } else { model.name.trim() };
        let mut entry = serde_json::json!({
            "path": path,
            "id": id,
            // Where the board puts this model, in the bundle's own space.
            "at": { "x": r4(x), "y": r4(y), "angle": r4(Flip.angle(body.params.f("MODEL.2D.ROTATION").unwrap_or(0.0))) },
            // And the same thing as the offset from the footprint the KiCad path
            // writes, so one reader serves both. It is taken from the placement
            // ORIGIN, not from `CompDef.x/y`: that is the pad-anchor centroid
            // (D2.4), which is the right answer for pick-and-place and the wrong
            // one for a model whose own origin the library chose.
            "offset": {
                "x": r4(x - origin.0),
                "y": r4(y - origin.1),
                "z": r4(mil("MODEL.3D.DZ").unwrap_or(model.dz)),
            },
            "scale": { "x": 1.0, "y": 1.0, "z": 1.0 },
            "rotate": {
                "x": r4(body.params.f("MODEL.3D.ROTX").unwrap_or(model.rot.0)),
                "y": r4(body.params.f("MODEL.3D.ROTY").unwrap_or(model.rot.1)),
                "z": r4(body.params.f("MODEL.3D.ROTZ").unwrap_or(model.rot.2)),
            },
            "standoff": r4(mil("STANDOFFHEIGHT").unwrap_or(0.0)),
            "height": r4(mil("OVERALLHEIGHT").unwrap_or(0.0)),
        });
        match written.get(id) {
            Some((file, format)) => {
                entry["file"] = serde_json::json!(file);
                entry["format"] = serde_json::json!(format);
            }
            // The board names a model it does not carry. A 3D view that cannot
            // show a part should say so, the way a linked image does.
            None => entry["embedded"] = serde_json::json!(false),
        }
        per_component.entry(owner).or_default().push(entry);
    }

    let mut entries = Vec::new();
    for (owner, models) in per_component {
        let Some(c) = components.get(usize::from(owner)) else { continue };
        entries.push(serde_json::json!({
            "reference": c.reference,
            "footprint": c.fp,
            "layer": c.layer,
            "uuid": c.uuid,
            "at": { "x": c.x, "y": c.y, "angle": c.angle },
            "models": models,
        }));
    }

    let doc = serde_json::json!({
        "schema": "extract.models.a0",
        "count": entries.len(),
        "refs": refs,
        "files": written.len(),
        "unresolved": not_embedded,
        "models": entries,
    });
    std::fs::write(
        models_dir.join("models.json"),
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    emit(Msg::Artifact("models/models.json".to_string()));
    emit(Msg::Progress(format!(
        "models: {refs} refs on {} footprints, {} files embedded, {not_embedded} referenced only",
        entries.len(),
        written.len()
    )));
    Ok(entries.len())
}

/// A model file name safe to write beside the bundle.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    s.trim_matches('_').to_string()
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
            // Set below by `paint_layers`, once the whole stack is known.
            color: None,
        });
    }
    crate::altium::pcb_svg::paint_layers(&mut layer_defs);
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
            // Set only where the pad overrides the board rule, which is what
            // Altium's expansion MODE says. Most pads defer to the rule, and 60
            // of the eval board's 604 do not.
            mask: p.solder_mask.map(r4),
            paste: p.paste_mask.map(r4),
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
        // A region that belongs to a polygon on copper is that pour's filled
        // copper (a zone, which the viewer may draw translucent). The same on an
        // overlay, mask or drawing layer is plain artwork, such as a logo. A
        // keep-out is an unfilled restriction. Everything else — a shape-based
        // pad's paste aperture, a footprint's own copper region — is a filled
        // graphic on its layer.
        let on_copper = layer_defs[usize::from(layer)].role == "copper";
        if (r.c.polygon.is_some() && on_copper) || r.is_keepout() {
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
        // The component's NAMEON / COMMENTON flags say whether Altium draws its
        // designator and comment. The text stays in the file; it is not shown.
        if owner.is_some_and(|c| {
            (t.is_designator && c.name_hidden) || (t.is_comment && c.comment_hidden)
        }) {
            continue;
        }
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
            angle: f.text_angle(t.rotation),
            size: r4(t.height * TEXT_HEIGHT_TO_SIZE),
            width: None,
            thickness: (t.width > 0.0).then(|| r4(t.width)),
            // Altium anchors a string at the left of its baseline, where KiCad
            // centres it; saying so is what keeps silkscreen legends in place.
            justify: [-1, 1],
            mirror: t.mirror,
            bold: t.bold,
            italic: t.italic,
            knockout: false,
            upright: false,
            // Only a TrueType text names a font. The stroke font ("Default")
            // keeps the viewer's own stroke engine, whatever name the record
            // carries.
            font: (t.truetype && !t.font.is_empty()).then(|| t.font.clone()),
            comp: comp_of(t.c.component),
            role,
        });
    }

    paint_order(&layer_defs, &mut tracks, &mut zones, &mut graphics);
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
    // A special string is sometimes stored in single quotes (`'.Designator'`).
    let trimmed = text.trim();
    let trimmed = trimmed
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .filter(|t| t.starts_with('.'))
        .unwrap_or(trimmed);
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
        // The layer's own name: a mechanical layer shows as `M14-Top Assembly`
        // in the stack, and the special string gives `Top Assembly`.
        ".LAYER_NAME" => owned(strip_mech_prefix(layer_name).to_string(), ""),
        // Altium prints this for a board with no assembly variant.
        ".VARIANTNAME" => owned("[No Variations]".to_string(), ""),
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

/// The viewer paints each object class in array order, so a later primitive
/// covers an earlier one. The order is the one a viewer sees from the top, so
/// the layers do not cover each other in file order. Each group keeps its own
/// file order.
fn paint_order(
    layers: &[LayerDef],
    tracks: &mut Tracks,
    zones: &mut Vec<ZoneDef>,
    graphics: &mut Vec<GraphicDef>,
) {
    let copper: Vec<u16> = layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.role == "copper")
        .map(|(i, _)| i as u16)
        .collect();
    // Seen from the top: bottom-side layers, then the copper from the bottom up,
    // then top-side layers, then layers with no side (drill, board shape).
    // A mechanical layer has no `side`, so its name says which side it is on.
    let rank = |layer: u16| -> u32 {
        if let Some(p) = copper.iter().position(|&c| c == layer) {
            return 10 + (copper.len() - p) as u32;
        }
        let l = &layers[usize::from(layer)];
        let name = l.name.to_ascii_lowercase();
        match l.side {
            Some("back") => 0,
            Some("front") => 100,
            _ if name.contains("bottom") => 0,
            _ if name.contains("top") => 100,
            _ => 200,
        }
    };
    let order = |layers_of: &[u16]| -> Vec<usize> {
        let mut idx: Vec<usize> = (0..layers_of.len()).collect();
        idx.sort_by_key(|&i| rank(layers_of[i]));
        idx
    };
    let seg = order(&tracks.seg.layer);
    tracks.seg.xy = seg.iter().flat_map(|&i| tracks.seg.xy[4 * i..4 * i + 4].to_vec()).collect();
    tracks.seg.w = seg.iter().map(|&i| tracks.seg.w[i]).collect();
    tracks.seg.net = seg.iter().map(|&i| tracks.seg.net[i]).collect();
    tracks.seg.layer = seg.iter().map(|&i| tracks.seg.layer[i]).collect();
    let arc = order(&tracks.arc.layer);
    tracks.arc.xy = arc.iter().flat_map(|&i| tracks.arc.xy[6 * i..6 * i + 6].to_vec()).collect();
    tracks.arc.w = arc.iter().map(|&i| tracks.arc.w[i]).collect();
    tracks.arc.net = arc.iter().map(|&i| tracks.arc.net[i]).collect();
    tracks.arc.layer = arc.iter().map(|&i| tracks.arc.layer[i]).collect();
    zones.sort_by_key(|z| rank(z.layer));
    graphics.sort_by_key(|g| rank(g.layer));
}

/// `M14-Top Assembly` -> `Top Assembly`; any other name comes back unchanged.
fn strip_mech_prefix(name: &str) -> &str {
    let rest = name.strip_prefix('M').unwrap_or(name);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && rest[digits..].starts_with('-') && rest.len() > digits + 1 {
        &rest[digits + 1..]
    } else {
        name
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
            solder_mask: None,
            paste_mask: None,
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
            solder_mask: None,
            paste_mask: None,
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

    /// A component's NAMEON / COMMENTON flags decide whether its designator and
    /// comment texts are drawn at all.
    #[test]
    fn hidden_component_texts_are_not_emitted() {
        let mut b = board();
        b.components.push(eda_parse_altium::pcb::Component {
            name_hidden: false,
            comment_hidden: true,
            ..eda_parse_altium::pcb::Component::default()
        });
        let text = |s: &str, des: bool, com: bool| eda_parse_altium::pcb::Text {
            c: Common { layer: 33, net: None, polygon: None, component: Some(0) },
            x: 1.0,
            y: 1.0,
            height: 1.0,
            width: 0.1,
            rotation: 0.0,
            mirror: false,
            font: String::new(),
            text: s.into(),
            is_designator: des,
            is_comment: com,
            truetype: false,
            bold: false,
            italic: false,
        };
        b.texts.push(text("R1", true, false));
        b.texts.push(text("10k", false, true));
        let g = build(&b, "b").geometry;
        assert_eq!(g.texts.len(), 1, "only the shown designator");
        assert_eq!(g.texts[0].text, "R1");
        b.components[0].name_hidden = true;
        assert!(build(&b, "b").geometry.texts.is_empty());
    }

    fn plain_text(layer: u8, rotation: f64) -> eda_parse_altium::pcb::Text {
        eda_parse_altium::pcb::Text {
            c: Common { layer, net: None, polygon: None, component: None },
            x: 5.0,
            y: 5.0,
            height: 1.0,
            width: 0.1,
            rotation,
            mirror: false,
            font: "Arial".into(),
            text: "TP1".into(),
            is_designator: false,
            is_comment: false,
            truetype: false,
            bold: false,
            italic: false,
        }
    }

    /// 90 degree text keeps its visible angle, the height maps to a smaller
    /// glyph size, and a stroke text names no font.
    #[test]
    fn text_keeps_its_angle_and_scales_its_height() {
        let mut b = board();
        b.texts.push(plain_text(33, 90.0));
        let mut tt = plain_text(33, 0.0);
        tt.truetype = true;
        b.texts.push(tt);
        let g = build(&b, "b").geometry;
        assert_eq!(g.texts[0].angle, 90.0, "not negated by the Y flip");
        assert_eq!(g.texts[0].size, 0.72);
        assert_eq!(g.texts[0].font, None, "a stroke text has no font");
        assert_eq!(g.texts[1].font.as_deref(), Some("Arial"));
    }

    #[test]
    fn quoted_special_strings_resolve() {
        let d = vec!["U1".to_string()];
        let r = |t: &str, layer: &str| resolve_special(t, None, Some(0), &d, "b", layer, false, false);
        assert_eq!(r("'.Designator'", "L").text, "U1");
        assert_eq!(r("'.Layer_Name'", "M14-Top Assembly").text, "Top Assembly");
        assert_eq!(r("'.VariantName'", "L").text, "[No Variations]");
        assert!(r(".GM14", "L").unresolved, "a legend stays as written");
    }

    /// A polygon region on an overlay is artwork, not a pour.
    #[test]
    fn a_polygon_region_off_copper_is_a_graphic() {
        let mut b = board();
        let ring = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)];
        for layer in [TOP, 33u8] {
            b.regions.push(eda_parse_altium::pcb::Region {
                c: Common { layer, net: None, polygon: Some(0), component: None },
                params: eda_parse_altium::record::TextRecord::default(),
                outline: ring.clone(),
            });
        }
        let g = build(&b, "b").geometry;
        assert_eq!(g.zones.len(), 1, "only the copper one is a zone");
        assert_eq!(g.graphics.len(), 1);
    }

    /// The bottom copper paints first and the top copper last.
    #[test]
    fn copper_paints_from_the_bottom_up() {
        let mut b = board();
        for layer in [TOP, 2u8, BOTTOM] {
            b.tracks.push(Track { c: common(layer, None), x1: 0.0, y1: 0.0, x2: 1.0, y2: 0.0, width: 0.2 });
        }
        let g = build(&b, "b").geometry;
        let name = |i: usize| g.layers[usize::from(g.tracks.seg.layer[i])].name.clone();
        assert_eq!(name(0), "Bottom Layer");
        assert_eq!(name(2), "Top Layer");
    }

    /// Seen from the top: bottom-side artwork, then copper bottom up, then
    /// top-side artwork, then side-neutral layers.
    #[test]
    fn layers_paint_by_side_as_seen_from_the_top() {
        let def = |name: &str, role: &'static str, side: Option<&'static str>| LayerDef {
            name: name.into(),
            role,
            side,
            ord: 0,
            color: None,
        };
        let layers = vec![
            def("Top Layer", "copper", Some("front")),
            def("Signal Layer 1", "copper", Some("inner")),
            def("Bottom Layer", "copper", Some("back")),
            def("Top Overlay", "silkscreen", Some("front")),
            def("Bottom Overlay", "silkscreen", Some("back")),
            def("M15-Bottom Assembly", "user", None),
            def("M14-Top Assembly", "user", None),
            def("Keep-Out Layer", "edge", None),
        ];
        let mut tracks = Tracks { seg: SegCol::default(), arc: ArcCol::default() };
        let mut graphics = Vec::new();
        let mut zones: Vec<ZoneDef> = (0..layers.len() as u16)
            .rev()
            .map(|layer| ZoneDef { layer, net: 0, filled: true, keepout: false, pts: vec![] })
            .collect();
        paint_order(&layers, &mut tracks, &mut zones, &mut graphics);
        let names: Vec<&str> = zones.iter().map(|z| layers[usize::from(z.layer)].name.as_str()).collect();
        assert_eq!(
            names,
            [
                "M15-Bottom Assembly",
                "Bottom Overlay",
                "Bottom Layer",
                "Signal Layer 1",
                "Top Layer",
                "M14-Top Assembly",
                "Top Overlay",
                "Keep-Out Layer"
            ]
        );
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
