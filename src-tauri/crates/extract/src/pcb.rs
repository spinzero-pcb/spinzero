//! Board-side extraction: 3D model references and per-layer board SVGs.
//!
//! The full board STEP solid assembly is deferred (no mature pure-Rust B-rep
//! kernel), so 3D export here records each footprint's model references and
//! transforms into `models/models.json`, copying the model files when their path
//! resolves on disk.
//!
//! The PCB SVGs mirror the schematic ones and match the schema the viewer's PCB
//! canvas already consumes (schema `spinzero.pcb.svg.enrichment.a0`):
//! one lean, monochrome SVG per layer, every element wrapped in a
//! `<g data-primitive=…>` group carrying the `data-net` / `data-component` /
//! `data-pad-number` / `data-uuid` / `data-layer-name` the canvas themes by CSS
//! and hit-tests for cross-probe. All layers share one board coordinate space
//! (millimetres, Y-down) so the stacked layer islands register exactly.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use eda_parse_kicad::pcb::{Footprint, Pad, Pcb, PcbShape, PcbText};
use eda_parse_kicad::schematic::Pt;
use serde_json::json;

use crate::pipeline::Msg;

/// Padding around the board outline, in mm.
const PAD: f64 = 1.0;

/// Board artifacts written next to the design bundle.
#[derive(Default)]
pub struct PcbArtifacts {
    /// Manifest `pcb_svgs` entries (one per reviewable layer + the worksheet).
    pub svgs: Vec<serde_json::Value>,
    /// Cache-relative path of the structured geometry IR (`pcb/geometry.json`), the
    /// GPU renderer's input; `None` if the board has no geometry / the build failed.
    pub geometry: Option<String>,
}

/// Extract board artifacts next to the design bundle: 3D model refs, the structured
/// geometry IR (`pcb/geometry.json`), and the per-layer SVGs. A missing/unreadable
/// board is not fatal — it just yields nothing (schematic-only projects are common).
pub fn extract_pcb(
    pcb_path: &Path,
    out_dir: &Path,
    theme: &crate::theme::Theme,
    emit: &mut dyn FnMut(Msg),
) -> Result<PcbArtifacts, String> {
    let Ok(src) = std::fs::read_to_string(pcb_path) else {
        return Ok(PcbArtifacts::default());
    };
    let pcb = Pcb::parse_str(&src).map_err(|e| format!("{}: {e}", pcb_path.display()))?;

    write_models(&pcb, pcb_path, out_dir, emit)?;
    let source = pcb_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let svgs = write_layer_svgs(&pcb, &source, out_dir, theme, emit)?;
    // The structured geometry IR (the GPU renderer's input) is additive — a failure
    // here must not sink the SVG path the current renderer still uses.
    let geometry = match write_geometry(&pcb, &source, out_dir, theme, emit) {
        Ok(rel) => Some(rel),
        Err(e) => {
            emit(Msg::Progress(format!("pcb geometry skipped: {e}")));
            None
        }
    };
    emit(Msg::Progress(format!(
        "pcb: {} footprints, {} tracks, {} vias, {} zones, {} layers",
        pcb.footprints.len(),
        pcb.tracks.len(),
        pcb.vias.len(),
        pcb.zones.len(),
        svgs.len()
    )));
    Ok(PcbArtifacts { svgs, geometry })
}

/// Build the geometry IR and write it as compact JSON to `pcb/geometry.json`.
fn write_geometry(
    pcb: &Pcb,
    source: &str,
    out_dir: &Path,
    theme: &crate::theme::Theme,
    emit: &mut dyn FnMut(Msg),
) -> Result<String, String> {
    let g = crate::ir::build(pcb, theme, source);
    let pcb_dir = out_dir.join("pcb");
    std::fs::create_dir_all(&pcb_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string(&g).map_err(|e| e.to_string())?;
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
    Ok(rel)
}

/// Write `models/models.json` and copy every resolvable 3D model file alongside it.
///
/// Records each footprint's model references with their per-model transform and the
/// footprint placement, so a later 3D view can assemble the board and cross-probe
/// 3D↔2D by reference. Files are copied (deduped by source) into `models/files/`,
/// resolving KiCad's `${KICAD*_3DMODEL_DIR}` / `${KIPRJMOD}` / `${KISYS3DMOD}` vars
/// (with install-dir fallbacks) and relative paths against the board. When a ref is a
/// `.wrl`/`.wrz` (render mesh) the `.step`/`.stp` sibling — the solid MCAD model — is
/// copied too, since that is what 3D conversion (e.g. geometer STEP→GLB) wants.
fn write_models(
    pcb: &Pcb,
    pcb_path: &Path,
    out_dir: &Path,
    emit: &mut dyn FnMut(Msg),
) -> Result<(), String> {
    let models_dir = out_dir.join("models");
    std::fs::create_dir_all(&models_dir).map_err(|e| e.to_string())?;
    let files_dir = models_dir.join("files");
    let board_dir = pcb_path.parent().unwrap_or(Path::new("."));

    // Dedupe copies by source path; disambiguate basename collisions.
    let mut by_src: HashMap<PathBuf, String> = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    let mut refs = 0usize;
    let mut unresolved = 0usize;

    let mut entries = Vec::new();
    for fp in &pcb.footprints {
        if fp.models.is_empty() {
            continue;
        }
        let mut models = Vec::new();
        for m in &fp.models {
            refs += 1;
            let mut entry = json!({
                "path": m.path,
                "offset": { "x": m.offset.0, "y": m.offset.1, "z": m.offset.2 },
                "scale": { "x": m.scale.0, "y": m.scale.1, "z": m.scale.2 },
                "rotate": { "x": m.rotate.0, "y": m.rotate.1, "z": m.rotate.2 },
            });
            match resolve_candidates(&m.path, board_dir).into_iter().find(|p| p.is_file()) {
                Some(resolved) => {
                    if let Some(rel) = copy_model(&resolved, &files_dir, &mut by_src, &mut used_names)
                    {
                        if let Some(ext) = file_ext_lower(&resolved) {
                            entry["format"] = json!(ext);
                        }
                        entry["file"] = json!(rel);
                    }
                    // Pull in the solid STEP sibling of a render-only .wrl/.wrz.
                    if let Some(step) = step_sibling(&resolved) {
                        if let Some(rel) =
                            copy_model(&step, &files_dir, &mut by_src, &mut used_names)
                        {
                            entry["step"] = json!(rel);
                        }
                    }
                }
                None => unresolved += 1,
            }
            models.push(entry);
        }
        entries.push(json!({
            "reference": fp.reference,
            "footprint": fp.lib_id,
            "layer": fp.layer,
            "uuid": fp.uuid,
            "at": { "x": fp.at.x, "y": fp.at.y, "angle": fp.at.angle },
            "models": models,
        }));
    }

    let doc = json!({
        "schema": "extract.models.a0",
        "count": entries.len(),
        "refs": refs,
        "files": by_src.len(),
        "unresolved": unresolved,
        "models": entries,
    });
    std::fs::write(
        models_dir.join("models.json"),
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    emit(Msg::Artifact("models/models.json".to_string()));
    emit(Msg::Progress(format!(
        "models: {refs} refs on {} footprints, {} files copied, {unresolved} unresolved",
        entries.len(),
        by_src.len()
    )));
    Ok(())
}

/// Copy a model file into `files_dir`, deduped by source. Returns its
/// `models/files/<name>` relative path (reusing a prior copy of the same source).
fn copy_model(
    src: &Path,
    files_dir: &Path,
    by_src: &mut HashMap<PathBuf, String>,
    used_names: &mut HashSet<String>,
) -> Option<String> {
    if let Some(rel) = by_src.get(src) {
        return Some(rel.clone());
    }
    let base = src.file_name()?.to_string_lossy().into_owned();
    // Disambiguate two different sources that share a basename.
    let mut name = base.clone();
    let mut i = 1;
    while used_names.contains(&name) {
        name = format!("{i}_{base}");
        i += 1;
    }
    std::fs::create_dir_all(files_dir).ok()?;
    std::fs::copy(src, files_dir.join(&name)).ok()?;
    used_names.insert(name.clone());
    let rel = format!("models/files/{name}");
    by_src.insert(src.to_path_buf(), rel.clone());
    Some(rel)
}

/// Lowercased file extension (no dot), if any.
fn file_ext_lower(p: &Path) -> Option<String> {
    p.extension().map(|e| e.to_string_lossy().to_lowercase())
}

/// The solid `.step`/`.stp` sibling of a render-only `.wrl`/`.wrz`, if present.
fn step_sibling(p: &Path) -> Option<PathBuf> {
    match file_ext_lower(p)?.as_str() {
        "wrl" | "wrz" => {}
        _ => return None,
    }
    ["step", "stp", "STEP", "STP", "Step", "Stp"]
        .into_iter()
        .map(|ext| p.with_extension(ext))
        .find(|c| c.is_file())
}

/// Common KiCad 3D-model library roots, tried when `${KICAD*_3DMODEL_DIR}` is unset
/// (a machine that has KiCad installed at the default path but no env var exported).
fn model_dir_fallbacks() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for pf in ["C:/Program Files/KiCad", "C:/Program Files (x86)/KiCad"] {
        for ver in ["9.0", "8.0", "7.0", "6.0"] {
            v.push(PathBuf::from(format!("{pf}/{ver}/share/kicad/3dmodels")));
        }
    }
    // Linux/macOS defaults, harmless on Windows (they just won't exist).
    v.push(PathBuf::from("/usr/share/kicad/3dmodels"));
    v.push(PathBuf::from("/Applications/KiCad/KiCad.app/Contents/SharedSupport/3dmodels"));
    v
}

/// Candidate base directories for a KiCad path variable.
fn var_bases(var: &str, board_dir: &Path) -> Vec<PathBuf> {
    if var == "KIPRJMOD" {
        return vec![board_dir.to_path_buf()];
    }
    if var.contains("3DMODEL") || var == "KISYS3DMOD" {
        if let Ok(p) = std::env::var(var) {
            if !p.is_empty() {
                return vec![PathBuf::from(p)];
            }
        }
        return model_dir_fallbacks();
    }
    // Any other variable: honour the environment, else give up on this ref.
    match std::env::var(var) {
        Ok(p) if !p.is_empty() => vec![PathBuf::from(p)],
        _ => Vec::new(),
    }
}

/// Resolve a KiCad `(model …)` path to candidate absolute paths, in priority order.
/// Handles the common leading-`${VAR}` form, mid-string vars (env-only), absolute
/// paths, and relative paths (resolved against the board then the model-dir
/// fallbacks). The caller picks the first candidate that exists on disk.
fn resolve_candidates(path: &str, board_dir: &Path) -> Vec<PathBuf> {
    // Leading `${VAR}/rest` — the overwhelmingly common KiCad form.
    if path.starts_with("${") {
        if let Some(end) = path.find('}') {
            let var = &path[2..end];
            let suffix = path[end + 1..].trim_start_matches(['/', '\\']);
            return var_bases(var, board_dir).into_iter().map(|b| b.join(suffix)).collect();
        }
    }
    // A `${VAR}` elsewhere in the string — expand from the environment, single shot.
    if path.contains("${") {
        return match expand_env_only(path) {
            Some(s) => vec![PathBuf::from(s)],
            None => Vec::new(),
        };
    }
    // Literal path: absolute as-is, else relative to the board then the fallbacks.
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return vec![p];
    }
    let mut v = vec![board_dir.join(&p)];
    for base in model_dir_fallbacks() {
        v.push(base.join(&p));
    }
    v
}

/// Expand every `${VAR}` from the environment; `None` if any is unset.
fn expand_env_only(path: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}')? + start;
        out.push_str(&std::env::var(&rest[start + 2..end]).ok()?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

// ---------------------------------------------------------------- layer set

/// The review role of a board layer. The well-known fabrication layers get a
/// specific role; every other layer (user drawings, comments, fab notes,
/// margins, adhesive, renamed `User.N` layers …) is `"user"` — a generic
/// graphic layer the renderer still emits, *provided it carries content*
/// (see `layer_has_content`). Nothing is dropped outright.
pub(crate) fn layer_role(name: &str) -> &'static str {
    if name.ends_with(".Cu") {
        "copper"
    } else if name.ends_with(".SilkS") {
        "silkscreen"
    } else if name.ends_with(".Mask") {
        "mask"
    } else if name.ends_with(".Fab") {
        "fab"
    } else if name.ends_with(".CrtYd") {
        "courtyard"
    } else if name.ends_with(".Paste") {
        "paste"
    } else if name == "Edge.Cuts" {
        "edge"
    } else {
        "user"
    }
}

/// Whether a non-standard ("user") layer carries anything the renderer would
/// draw — board graphics/text, footprint graphics/text, or a zone on that layer
/// (e.g. a conformal-coating mask zone on a renamed user layer). Empty user layers
/// (defined in the stackup but never drawn on) are skipped so they don't clutter the
/// layer list with blank islands. The shared `Edge.Cuts` outline drawn on every layer
/// does NOT count as content.
pub(crate) fn layer_has_content(pcb: &Pcb, layer: &str) -> bool {
    pcb.graphics.iter().any(|g| g.layer == layer)
        || pcb.texts.iter().any(|t| t.layer == layer && !t.hidden)
        || pcb.zones.iter().any(|z| z.layer == layer && z.pts.len() >= 3)
        || pcb.footprints.iter().any(|fp| {
            fp.graphics.iter().any(|g| g.layer == layer)
                || fp.texts.iter().any(|t| t.layer == layer && !t.hidden)
        })
}

/// Resolve a user/graphic layer's colour from the active KiCad theme. The board
/// theme flattens non-copper layers as `lowercase(name).replace('.', '_')`
/// (`User.1`→`user_1`, `Dwgs.User`→`dwgs_user`, `F.Adhes`→`f_adhes`), so the
/// frontend can paint each user layer in its real KiCad colour. `None` when no
/// theme is reachable (the viewer then falls back to its neutral default).
pub(crate) fn user_layer_color(theme: &crate::theme::Theme, name: &str) -> Option<String> {
    let key = name.to_lowercase().replace('.', "_");
    theme.board(&key).map(str::to_string)
}

/// Render one lean SVG per reviewable layer, in board coordinates shared across
/// all layers so the stacked islands register.
fn write_layer_svgs(
    pcb: &Pcb,
    source: &str,
    out_dir: &Path,
    theme: &crate::theme::Theme,
    emit: &mut dyn FnMut(Msg),
) -> Result<Vec<serde_json::Value>, String> {
    let pcb_dir = out_dir.join("pcb");
    std::fs::create_dir_all(&pcb_dir).map_err(|e| e.to_string())?;

    let vb = board_viewbox(pcb);

    // Layers come from the stackup (so the order is the board's), each tagged with
    // its review role and the designer's display name (`User.3` → "Mechanical
    // Drawing"). Fall back to the standard set when a board omits the table.
    let mut layers: Vec<(String, &'static str, Option<String>)> = pcb
        .layers
        .iter()
        .map(|l| (l.name.clone(), layer_role(&l.name), l.user_name.clone()))
        .collect();
    if layers.is_empty() {
        for (n, r) in [
            ("F.Cu", "copper"),
            ("B.Cu", "copper"),
            ("F.SilkS", "silkscreen"),
            ("B.SilkS", "silkscreen"),
            ("Edge.Cuts", "edge"),
        ] {
            layers.push((n.to_string(), r, None));
        }
    }

    let mut out = Vec::new();
    let mut skipped = 0usize;
    for (name, role, user_name) in &layers {
        // Non-standard ("user") layers are kept only when they actually carry
        // content — an empty user layer defined in the stackup is skipped so the
        // viewer's layer list isn't padded with blank islands.
        if *role == "user" && !layer_has_content(pcb, name) {
            skipped += 1;
            continue;
        }
        let svg = render_layer(pcb, source, name, role, vb);
        let file_name = format!("{}.svg", name.replace('.', "_"));
        std::fs::write(pcb_dir.join(&file_name), svg).map_err(|e| e.to_string())?;
        let rel = format!("pcb/{file_name}");
        emit(Msg::Artifact(rel.clone()));
        let mut entry = json!({ "layer": name, "role": role, "file": rel });
        // Carry the designer's display name so the viewer can label "FabNotes"
        // instead of the canonical "User.2".
        if let Some(disp) = user_name {
            entry["user_name"] = json!(disp);
        }
        // Resolve the user layer's KiCad colour so the frontend paints it in the
        // real theme colour rather than a single neutral grey for every user layer.
        if *role == "user" {
            if let Some(hex) = user_layer_color(theme, name) {
                entry["color"] = json!(hex);
            }
        }
        out.push(entry);
    }
    if skipped > 0 {
        emit(Msg::Progress(format!("pcb: skipped {skipped} empty user layer(s)")));
    }

    // Drawing-sheet frame + title block on its own layer (like the schematic's
    // worksheet), so the board reads in its A3/A4 page context — drawn only when the
    // board declares a known paper size. Shown by default; the frontend paints it in
    // the worksheet colour and stacks it at the bottom.
    if let Some((pw, ph)) = crate::svg::page_dims(pcb.paper.as_deref(), pcb.paper_dims) {
        let wcolor = theme.sch("worksheet").unwrap_or(crate::svg::WORKSHEET_RED);
        let svg = render_worksheet(pcb, source, vb, pw, ph, wcolor);
        std::fs::write(pcb_dir.join("worksheet.svg"), svg).map_err(|e| e.to_string())?;
        let rel = "pcb/worksheet.svg".to_string();
        emit(Msg::Artifact(rel.clone()));
        out.push(json!({ "layer": WORKSHEET_LAYER, "role": "worksheet", "file": rel, "color": wcolor }));
    }
    Ok(out)
}

/// Display name of the synthetic drawing-sheet layer. Kept in sync with the
/// frontend's `layerRank` so the sheet stacks behind the board.
const WORKSHEET_LAYER: &str = "Drawing Sheet";

/// Render the drawing-sheet frame + title block into a layer SVG that shares the
/// board viewBox (KiCad page coordinates, so the frame at `(0,0)..(pw,ph)` lines up
/// with the board). The frame colour is baked for standalone/report use; in the app
/// the per-layer CSS recolours it to the same worksheet colour.
fn render_worksheet(
    pcb: &Pcb,
    source: &str,
    vb: (f64, f64, f64, f64),
    pw: f64,
    ph: f64,
    color: &str,
) -> String {
    let (vx, vy, vw, vh) = vb;
    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="0 0 {} {}" data-enrichment-schema="spinzero.pcb.svg.enrichment.a0" data-source="{}" data-review-layer="{}" data-layer-role="worksheet" data-mirror-x="false" fill="none" font-family="{}">"##,
        c(vw), c(vh), c(vw), c(vh), esc(source), esc(WORKSHEET_LAYER), crate::svg::FONT_FAMILY
    );
    let _ = write!(s, r##"<g transform="translate({} {})">"##, c(-vx), c(-vy));
    let fd = crate::svg::FrameData {
        title: pcb.title.clone().unwrap_or_default(),
        company: pcb.company.clone().unwrap_or_default(),
        rev: pcb.rev.clone().unwrap_or_default(),
        date: pcb.date.clone().unwrap_or_default(),
        version: pcb.generator_version.clone().unwrap_or_default(),
        comments: pcb.comments.clone(),
        paper: pcb.paper.clone().unwrap_or_default(),
        sheet_path: String::new(),
        file: source.to_string(),
        id: String::new(),
    };
    crate::svg::render_frame(&mut s, pw, ph, &fd, color);
    s.push_str("</g></svg>");
    s
}

/// `(minx, miny, w, h)` board bounding box (padded), shared by every layer SVG.
pub(crate) fn board_viewbox(pcb: &Pcb) -> (f64, f64, f64, f64) {
    let mut min = Pt { x: f64::MAX, y: f64::MAX };
    let mut max = Pt { x: f64::MIN, y: f64::MIN };
    let mut add = |p: Pt| {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    };
    for (a, b) in &pcb.edges {
        add(*a);
        add(*b);
    }
    for t in &pcb.tracks {
        add(t.start);
        add(t.end);
    }
    for v in &pcb.vias {
        add(Pt { x: v.at.x - v.size, y: v.at.y - v.size });
        add(Pt { x: v.at.x + v.size, y: v.at.y + v.size });
    }
    for z in &pcb.zones {
        for p in &z.pts {
            add(*p);
        }
    }
    for fp in &pcb.footprints {
        if let Some((mn, mx)) = fp.bbox {
            add(mn);
            add(mx);
        }
        for pad in &fp.pads {
            add(place_fp(fp, pad.at.x, pad.at.y));
        }
    }
    // Board-level graphics and text can sit OFF the board outline (fab-note blocks
    // and drawing-sheet annotations on user/comment layers). Every layer SVG shares
    // this one viewBox, so their extent must be included or the SVG clips them — the
    // "FabNotes text not visible" report. (Footprint graphics/text are already
    // covered by the footprint bbox above.)
    for g in &pcb.graphics {
        for p in shape_extent_points(&g.shape) {
            add(p);
        }
    }
    for t in pcb.texts.iter().filter(|t| !t.hidden) {
        let (mn, mx) = text_block_extent(t);
        add(mn);
        add(mx);
    }
    // The drawing sheet (paper size) frames the whole design. Include the full page
    // so the board reads in its sheet context and the worksheet layer registers — KiCad
    // places everything in page coordinates, so the page origin is (0,0).
    if let Some((pw, ph)) = crate::svg::page_dims(pcb.paper.as_deref(), pcb.paper_dims) {
        add(Pt { x: 0.0, y: 0.0 });
        add(Pt { x: pw, y: ph });
    }
    if min.x > max.x {
        // No geometry at all — a safe default page.
        return (0.0, 0.0, 100.0, 100.0);
    }
    (min.x - PAD, min.y - PAD, (max.x - min.x) + 2.0 * PAD, (max.y - min.y) + 2.0 * PAD)
}

/// Extreme points of a board graphic shape, for the shared board viewBox.
fn shape_extent_points(shape: &PcbShape) -> Vec<Pt> {
    match shape {
        PcbShape::Seg { a, b } => vec![*a, *b],
        PcbShape::Arc { start, mid, end } => vec![*start, *mid, *end],
        PcbShape::Rect { a, b, .. } => vec![*a, *b],
        PcbShape::Circle { center, radius, .. } => vec![
            Pt { x: center.x - radius, y: center.y - radius },
            Pt { x: center.x + radius, y: center.y + radius },
        ],
        PcbShape::Poly { pts, .. } => pts.clone(),
    }
}

/// Conservative board-frame bounding box of a (possibly multi-line) text block,
/// derived from its anchor, size and justify — enough to keep an off-board note
/// inside the shared viewBox. Rotation is ignored (off-board notes are virtually
/// always unrotated) and the advance estimate is generous so nothing clips.
fn text_block_extent(t: &PcbText) -> (Pt, Pt) {
    let mut max_chars = 0usize;
    let mut n = 0usize;
    for line in t.text.split('\n') {
        max_chars = max_chars.max(line.chars().count());
        n += 1;
    }
    let font = t.size.max(0.5) * 4.0 / 3.0;
    let w = max_chars as f64 * font * 0.65; // ~average glyph advance
    let h = n.max(1) as f64 * font * 1.2;
    let (x0, x1) = match t.justify.h {
        j if j < 0 => (t.at.x, t.at.x + w),
        j if j > 0 => (t.at.x - w, t.at.x),
        _ => (t.at.x - w / 2.0, t.at.x + w / 2.0),
    };
    let (y0, y1) = match t.justify.v {
        j if j < 0 => (t.at.y, t.at.y + h),
        j if j > 0 => (t.at.y - h, t.at.y),
        _ => (t.at.y - h / 2.0, t.at.y + h / 2.0),
    };
    (Pt { x: x0, y: y0 }, Pt { x: x1, y: y1 })
}

// ---------------------------------------------------------------- rendering

fn c(v: f64) -> String {
    let mut s = format!("{:.4}", v);
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
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Footprint-local point -> board coordinates (placement rotation + translation).
pub(crate) fn place_fp(fp: &Footprint, lx: f64, ly: f64) -> Pt {
    let (s, co) = fp.at.angle.to_radians().sin_cos();
    Pt { x: fp.at.x + lx * co + ly * s, y: fp.at.y - lx * s + ly * co }
}

fn render_layer(
    pcb: &Pcb,
    source: &str,
    layer: &str,
    role: &str,
    vb: (f64, f64, f64, f64),
) -> String {
    let (vx, vy, vw, vh) = vb;
    let is_copper = role == "copper";
    let is_back = layer.starts_with('B');

    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="0 0 {} {}" data-enrichment-schema="spinzero.pcb.svg.enrichment.a0" data-source="{}" data-review-layer="{}" data-layer-role="{}" data-mirror-x="{}" fill="none" stroke="#B8B8B8" stroke-width="0.12" font-family="{}">"##,
        c(vw), c(vh), c(vw), c(vh), esc(source), esc(layer), role, is_back, crate::svg::FONT_FAMILY
    );
    // Normalize to a 0-origin viewBox: the viewer's net-label
    // overlay mixes getCTM() with a (coord - viewBox.x) subtraction, so a non-zero
    // viewBox origin would shift every label off-screen. One wrapper translate keeps
    // all 15 layers in the same 0-origin space and registers them.
    let _ = write!(s, r##"<g transform="translate({} {})">"##, c(-vx), c(-vy));

    // Board outline on every layer, drawn from the real Edge.Cuts shapes (arcs and
    // circles kept curved, not chord-flattened) so a filleted/rounded board reads
    // true. Edge.Cuts copper segments, if any, are drawn as straight outline too.
    let _ = write!(s, r##"<g data-primitive="graphic" data-layer-name="Edge.Cuts" data-layer-role="board-outline">"##);
    for g in pcb.graphics.iter().filter(|g| g.layer == "Edge.Cuts") {
        emit_shape(&mut s, &g.shape, 0.15, "#000000", |p| p);
    }
    for t in pcb.tracks.iter().filter(|t| t.layer == "Edge.Cuts") {
        let shape = match t.mid {
            Some(m) => PcbShape::Arc { start: t.start, mid: m, end: t.end },
            None => PcbShape::Seg { a: t.start, b: t.end },
        };
        emit_shape(&mut s, &shape, 0.15, "#000000", |p| p);
    }
    s.push_str("</g>");

    // Board graphics that live on this layer (skip Edge.Cuts — drawn above).
    if layer != "Edge.Cuts" {
        for g in pcb.graphics.iter().filter(|g| g.layer == layer) {
            let _ = write!(
                s,
                r##"<g data-primitive="graphic" data-layer-name="{}"><g>"##,
                esc(layer)
            );
            emit_shape(&mut s, &g.shape, g.width, "#B8B8B8", |p| p);
            s.push_str("</g></g>");
        }
        for t in pcb.texts.iter().filter(|t| t.layer == layer && !t.hidden) {
            emit_text(&mut s, t, "text", "", "", false, |p| p);
        }
    }

    // Zones render on whatever layer they belong to — copper pours AND non-copper
    // zones such as the solder-mask regions KiCad lets you draw on F/B.Mask (and
    // conformal-coating zones on user layers). Drawn at the bottom of the layer, below
    // tracks/pads/apertures, so routing and pad openings read on top. The app recolours
    // the baked fill to the layer's own colour via `--lc`, so a mask zone reads in the
    // mask colour, not copper green.
    for z in pcb.zones.iter().filter(|z| z.layer == layer && z.pts.len() >= 3) {
        let _ = write!(s, r##"<g data-primitive="zone""##);
        net_attr(&mut s, &z.net_name);
        if !z.filled {
            let _ = write!(s, r##" data-zone-type="{}""##, if z.keepout { "keepout" } else { "outline" });
        }
        let _ = write!(s, r##" data-layer-name="{}"><polygon points=""##, esc(layer));
        for (i, p) in z.pts.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            let _ = write!(s, "{},{}", c(p.x), c(p.y));
        }
        if z.filled {
            s.push_str(r##"" fill="#2d4a2d" stroke="none"/></g>"##);
        } else {
            // Keepout / unfilled: draw only the boundary so it cannot be mistaken
            // for poured copper.
            s.push_str(r##"" fill="none" stroke="#DC2626" stroke-width="0.12" stroke-dasharray="0.5 0.3"/></g>"##);
        }
    }

    // Copper-only geometry: tracks, then vias.
    if is_copper {
        for t in pcb.tracks.iter().filter(|t| t.layer == layer) {
            let pts = match t.mid {
                Some(m) => format!("{},{} {},{} {},{}", c(t.start.x), c(t.start.y), c(m.x), c(m.y), c(t.end.x), c(t.end.y)),
                None => format!("{},{} {},{}", c(t.start.x), c(t.start.y), c(t.end.x), c(t.end.y)),
            };
            let _ = write!(s, r##"<g data-primitive="track""##);
            net_attr(&mut s, net_name_of(pcb, t.net, &t.net_name));
            let _ = write!(
                s,
                r##" data-layer-name="{}" data-uuid="{}"><polyline points="{}" fill="none" stroke="#B8B8B8" stroke-width="{}" style="stroke-width:{}" stroke-linecap="round" stroke-linejoin="round"/></g>"##,
                esc(layer), esc(&t.uuid), pts, c(t.width), c(t.width)
            );
        }
        for v in pcb.vias.iter().filter(|v| via_on_layer(v, layer)) {
            let _ = write!(s, r##"<g data-primitive="via""##);
            net_attr(&mut s, net_name_of(pcb, v.net, &v.net_name));
            let _ = write!(
                s,
                r##" data-uuid="{}" data-layer-name="{}"><circle cx="{}" cy="{}" r="{}" fill="#B8B8B8" stroke="none"/></g>"##,
                esc(&v.uuid), esc(layer), c(v.at.x), c(v.at.y), c(v.size / 2.0)
            );
        }
    }

    // Footprints: per-layer children (pads on copper/mask/paste; graphics+text on
    // silk/fab/courtyard). One group per footprint that has content here.
    for fp in &pcb.footprints {
        render_footprint_layer(&mut s, fp, layer, role);
    }

    // Drill overlays on top (copper + mask read the holes).
    if is_copper || role == "mask" {
        for v in pcb.vias.iter().filter(|v| via_on_layer(v, layer)) {
            let _ = write!(
                s,
                r##"<g data-primitive="via-hole" data-uuid="{}:hole" data-layer-name="{}"><circle cx="{}" cy="{}" r="{}" fill="#2563EB" stroke="none"/></g>"##,
                esc(&v.uuid), esc(layer), c(v.at.x), c(v.at.y), c(v.drill / 2.0)
            );
        }
        for fp in &pcb.footprints {
            for pad in fp.pads.iter().filter(|p| p.drill > 0.0 && pad_on_layer(p, layer)) {
                let ctr = place_fp(fp, pad.at.x, pad.at.y);
                let _ = write!(
                    s,
                    r##"<g data-primitive="pad-hole" data-component="{}" data-pad-number="{}" data-uuid="{}:hole" data-layer-name="{}"><circle cx="{}" cy="{}" r="{}" fill="#2563EB" stroke="none"/></g>"##,
                    esc(&fp.reference), esc(&pad.number), esc(&pad.uuid), esc(layer),
                    c(ctr.x), c(ctr.y), c(pad.drill / 2.0)
                );
            }
        }
    }

    s.push_str("</g></svg>");
    s
}

/// Emit the footprint group for one layer: copper/mask/paste get pads, the
/// graphic layers get footprint graphics and text. Skipped when the footprint
/// has nothing on this layer.
fn render_footprint_layer(s: &mut String, fp: &Footprint, layer: &str, role: &str) {
    let pads: Vec<&Pad> = match role {
        "copper" | "mask" | "paste" => fp.pads.iter().filter(|p| pad_on_layer(p, layer)).collect(),
        _ => Vec::new(),
    };
    let graphics: Vec<_> = match role {
        "silkscreen" | "fab" | "courtyard" | "user" => {
            fp.graphics.iter().filter(|g| g.layer == layer).collect()
        }
        _ => Vec::new(),
    };
    let texts: Vec<&PcbText> = match role {
        "silkscreen" | "fab" | "user" => {
            fp.texts.iter().filter(|t| t.layer == layer && !t.hidden).collect()
        }
        _ => Vec::new(),
    };
    // KiCad marks "do not populate" parts with an X on the fab layer.
    let dnp_cross = role == "fab" && fp.dnp && fp.bbox.is_some();
    if pads.is_empty() && graphics.is_empty() && texts.is_empty() && !dnp_cross {
        return;
    }

    let _ = write!(
        s,
        r##"<g data-primitive="footprint" data-component="{}" data-uuid="{}" data-footprint="{}" data-layer-name="{}">"##,
        esc(&fp.reference), esc(&fp.uuid), esc(&fp.lib_id), esc(layer)
    );
    for pad in &pads {
        emit_pad(s, fp, pad, layer, role);
    }
    for g in &graphics {
        let _ = write!(s, r##"<g data-primitive="footprint-graphic"><g>"##);
        emit_shape(s, &g.shape, g.width, "#B8B8B8", |p| place_fp(fp, p.x, p.y));
        s.push_str("</g></g>");
    }
    for t in &texts {
        emit_text(s, t, "footprint-text", &fp.reference, &t.kind, true, |p| place_fp(fp, p.x, p.y));
    }
    if dnp_cross {
        let (mn, mx) = fp.bbox.unwrap();
        let _ = write!(
            s,
            r##"<g data-primitive="footprint-graphic" data-dnp="1"><polyline points="{},{} {},{}" fill="none" stroke="#B8B8B8" stroke-width="0.1" style="stroke-width:0.1"/><polyline points="{},{} {},{}" fill="none" stroke="#B8B8B8" stroke-width="0.1" style="stroke-width:0.1"/></g>"##,
            c(mn.x), c(mn.y), c(mx.x), c(mx.y), c(mn.x), c(mx.y), c(mx.x), c(mn.y)
        );
    }
    s.push_str("</g>");
}

/// Emit one pad as a `<g data-primitive="pad">` with its copper/aperture shape.
/// On a `.Mask` layer the shape is grown by the pad's solder-mask margin so the
/// aperture matches what KiCad opens (board default is 0 here).
fn emit_pad(s: &mut String, fp: &Footprint, pad: &Pad, layer: &str, role: &str) {
    let ctr = place_fp(fp, pad.at.x, pad.at.y);
    let _ = write!(s, r##"<g data-primitive="pad" data-component="{}" data-pad-number="{}""##, esc(&fp.reference), esc(&pad.number));
    net_attr(s, &pad.net_name);
    let _ = write!(
        s,
        r##" data-pad-type="{}" data-pad-shape="{}" data-uuid="{}" data-layer-name="{}">"##,
        esc(&pad.kind), esc(&pad.shape), esc(&pad.uuid), esc(layer)
    );
    // Mask aperture = copper grown by the per-pad margin (each side); other layers
    // use the copper size unchanged.
    let grow = if role == "mask" { 2.0 * pad.mask_margin.unwrap_or(0.0) } else { 0.0 };
    let (cw, ch) = (pad.size.0, pad.size.1);
    let (sx, sy) = ((cw + grow).max(0.05), (ch + grow).max(0.05));
    if pad.shape == "circle" || (pad.shape == "oval" && (sx - sy).abs() < 1e-9) {
        let _ = write!(
            s,
            r##"<circle cx="{}" cy="{}" r="{}" fill="#000000" stroke="none"/>"##,
            c(ctr.x), c(ctr.y), c(sx / 2.0)
        );
    } else {
        // rect / roundrect / oval / trapezoid / custom: an oriented (rounded) rect.
        // A `rotate(-angle …)` matches place_fp's handedness (module docs); the
        // corner radius reproduces KiCad's roundrect ratio and oval stadium.
        let radius = match pad.shape.as_str() {
            "roundrect" => pad.roundrect_rratio * cw.min(ch) + grow / 2.0,
            "oval" => sx.min(sy) / 2.0,
            _ => 0.0,
        };
        let _ = write!(s, r##"<rect x="{}" y="{}" width="{}" height="{}""##,
            c(ctr.x - sx / 2.0), c(ctr.y - sy / 2.0), c(sx), c(sy));
        if radius > 0.0 {
            let _ = write!(s, r##" rx="{}""##, c(radius));
        }
        if pad.at.angle.abs() > 1e-9 {
            let _ = write!(s, r##" transform="rotate({} {} {})""##, c(-pad.at.angle), c(ctr.x), c(ctr.y));
        }
        s.push_str(r##" fill="#000000" stroke="none"/>"##);
    }
    s.push_str("</g>");
}

/// Emit a graphic shape's inner element(s) in `color`, with point coordinates
/// mapped through `tf` (identity for board graphics, `place_fp` for footprint
/// graphics). Arcs are emitted as true circular-arc paths, not chord polylines.
fn emit_shape(s: &mut String, shape: &PcbShape, width: f64, color: &str, tf: impl Fn(Pt) -> Pt) {
    let stroke = format!(
        r##"fill="none" stroke="{}" stroke-width="{}" style="stroke-width:{}" stroke-linecap="round" stroke-linejoin="round""##,
        color, c(width), c(width)
    );
    match shape {
        PcbShape::Seg { a, b } => {
            let (a, b) = (tf(*a), tf(*b));
            let _ = write!(s, r##"<polyline points="{},{} {},{}" {}/>"##, c(a.x), c(a.y), c(b.x), c(b.y), stroke);
        }
        PcbShape::Arc { start, mid, end } => {
            let (a, m, e) = (tf(*start), tf(*mid), tf(*end));
            match arc_path_d(a, m, e) {
                Some(d) => {
                    let _ = write!(s, r##"<path d="{}" {}/>"##, d, stroke);
                }
                // Degenerate (collinear) arc: fall back to the chord polyline.
                None => {
                    let _ = write!(s, r##"<polyline points="{},{} {},{} {},{}" {}/>"##, c(a.x), c(a.y), c(m.x), c(m.y), c(e.x), c(e.y), stroke);
                }
            }
        }
        PcbShape::Rect { a, b, filled } => {
            let (a, b) = (tf(*a), tf(*b));
            let pts = [a, Pt { x: b.x, y: a.y }, b, Pt { x: a.x, y: b.y }];
            emit_poly(s, &pts, *filled, color, &stroke);
        }
        PcbShape::Circle { center, radius, filled } => {
            let ctr = tf(*center);
            if *filled {
                let _ = write!(s, r##"<circle cx="{}" cy="{}" r="{}" fill="{}" stroke="none"/>"##, c(ctr.x), c(ctr.y), c(*radius), color);
            } else {
                let _ = write!(s, r##"<circle cx="{}" cy="{}" r="{}" {}/>"##, c(ctr.x), c(ctr.y), c(*radius), stroke);
            }
        }
        PcbShape::Poly { pts, filled } => {
            let mapped: Vec<Pt> = pts.iter().map(|p| tf(*p)).collect();
            emit_poly(s, &mapped, *filled, color, &stroke);
        }
    }
}

/// SVG path `d` for the circular arc through three points (KiCad's start/mid/end).
/// `None` when the points are collinear (no finite circle). Coordinates are in the
/// same Y-down board space the SVG renders in, so the sweep flag is read directly.
fn arc_path_d(a: Pt, m: Pt, e: Pt) -> Option<String> {
    use std::f64::consts::{PI, TAU};
    let det = 2.0 * (a.x * (m.y - e.y) + m.x * (e.y - a.y) + e.x * (a.y - m.y));
    if det.abs() < 1e-9 {
        return None;
    }
    let (a2, m2, e2) = (a.x * a.x + a.y * a.y, m.x * m.x + m.y * m.y, e.x * e.x + e.y * e.y);
    let cx = (a2 * (m.y - e.y) + m2 * (e.y - a.y) + e2 * (a.y - m.y)) / det;
    let cy = (a2 * (e.x - m.x) + m2 * (a.x - e.x) + e2 * (m.x - a.x)) / det;
    let r = ((a.x - cx).powi(2) + (a.y - cy).powi(2)).sqrt();
    let ang = |p: Pt| (p.y - cy).atan2(p.x - cx);
    let norm = |x: f64| {
        let t = x % TAU;
        if t < 0.0 { t + TAU } else { t }
    };
    let (s0, m0, e0) = (ang(a), ang(m), ang(e));
    let arc_pos = norm(e0 - s0); // span start->end in the +angle (sweep=1) direction
    let mid_pos = norm(m0 - s0);
    // The mid point picks the direction: if it lies on the +angle arc, sweep=1.
    let (sweep, swept) = if mid_pos <= arc_pos { (1, arc_pos) } else { (0, TAU - arc_pos) };
    let large = if swept > PI { 1 } else { 0 };
    Some(format!(
        "M {},{} A {},{} 0 {} {} {},{}",
        c(a.x), c(a.y), c(r), c(r), large, sweep, c(e.x), c(e.y)
    ))
}

fn emit_poly(s: &mut String, pts: &[Pt], filled: bool, color: &str, stroke: &str) {
    let tag = if filled { "polygon" } else { "polyline" };
    let _ = write!(s, "<{tag} points=\"");
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{},{}", c(p.x), c(p.y));
    }
    if filled {
        let _ = write!(s, r##"" fill="{color}" stroke="none"/>"##);
    } else {
        let _ = write!(s, r##"" {stroke}/>"##);
    }
}

fn emit_text(
    s: &mut String,
    t: &PcbText,
    primitive: &str,
    component: &str,
    role: &str,
    keep_upright: bool,
    tf: impl Fn(Pt) -> Pt,
) {
    let p = tf(Pt { x: t.at.x, y: t.at.y });
    let _ = write!(s, r##"<g data-primitive="{}" data-layer-name="{}""##, primitive, esc(&t.layer));
    if !component.is_empty() {
        let _ = write!(s, r##" data-component="{}" data-footprint-text-role="{}""##, esc(component), esc(role));
    }
    s.push('>');

    // Font height: KiCad's text "size" is the glyph height; an SVG font renders
    // its cap height at ~3/4 of the em, so scale by 4/3 to match KiCad (same factor
    // the schematic uses). `stroke="none"` is essential — without it the glyphs
    // inherit the SVG root stroke and render as a fill+outline, far heavier than
    // KiCad's thin stroke-font text (the "very thick"/"much bigger" report). The
    // fill stays #000000 so the app's per-layer CSS recolours it (silk → yellow …).
    // No minimum-size floor: KiCad fab/comment text is routinely 0.2–0.3 mm and must
    // stay that small (a 0.5 mm floor rendered R104's 0.26 mm fab ref and 0.2 mm net
    // comments at 0.67 mm, overflowing the fab box — the "much bigger" report). The
    // tiny guard only keeps a degenerate `(size 0)` from a zero/negative font-size;
    // `text_size` already defaults a missing size to KiCad's 1 mm.
    let font = t.size.max(0.01) * 4.0 / 3.0;
    let anchor = match t.justify.h {
        h if h < 0 => "start",
        h if h > 0 => "end",
        _ => "middle",
    };
    let baseline = match t.justify.v {
        v if v < 0 => "hanging", // top-anchored
        v if v > 0 => "auto",    // bottom-anchored
        _ => "central",          // centred
    };
    // Split into lines the way KiCad's wxStringSplit does: a SINGLE trailing newline is
    // just the line terminator and is dropped, but any further blank rows are real lines
    // KiCad reserves height for. So "CAN_H\n\n" is a 2-line block, and bottom-justified
    // that lifts "CAN_H" one interline above its anchor — popping *all* trailing blanks
    // (as before) dropped that row and sat the label too low (the misplaced silk text).
    let mut lines: Vec<&str> = t.text.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    let n = lines.len() as f64;
    let line_h = font * 1.2;
    // Vertical offset of the first line so the whole block honours the anchor side.
    let first_dy = match t.justify.v {
        v if v < 0 => 0.0,                 // top: first line at the anchor
        v if v > 0 => -(n - 1.0) * line_h, // bottom: last line at the anchor
        _ => -(n - 1.0) / 2.0 * line_h,    // centre: block straddles the anchor
    };
    // Rotate the glyphs about the anchor. Footprint text (reference/value) is kept
    // upright like the schematic — KiCad normalises it so it never reads upside-down —
    // but board `gr_text` is NOT: KiCad plots it at its literal angle, so the upright
    // clamp would flip a 270°/-90° silk label 180°. Footprint text stores an absolute angle (already includes the
    // placement rotation), so `t.at.angle` is used directly either way.
    let transform = if keep_upright {
        crate::svg::text_transform(t.at.angle, false, p.x, p.y)
    } else {
        // Literal angle, normalised to (-180, 180]; KiCad CCW -> SVG CW = rotate(-a).
        let mut a = t.at.angle % 360.0;
        if a > 180.0 {
            a -= 360.0;
        } else if a <= -180.0 {
            a += 360.0;
        }
        if a.abs() > 1e-9 {
            format!(r#" transform="rotate({} {} {})""#, c(-a), c(p.x), c(p.y))
        } else {
            String::new()
        }
    };
    let _ = write!(
        s,
        r##"<text x="{}" y="{}" font-size="{}" text-anchor="{}" dominant-baseline="{}" stroke="none" fill="#000000"{}>"##,
        c(p.x), c(p.y), c(font), anchor, baseline, transform
    );
    // A blank line carries its height onto the NEXT glyph-bearing line. An empty
    // `<tspan dy=…>` has no glyph for the browser to hang the dy on, so its row would
    // collapse — the missing gaps between numbered fab notes. Folding the advance into
    // the following tspan keeps the blank rows without emitting an empty element.
    let mut pending = 0.0;
    for (i, line) in lines.iter().enumerate() {
        let step = if i == 0 { first_dy } else { line_h };
        if line.is_empty() {
            pending += step;
            continue;
        }
        let _ = write!(
            s,
            r##"<tspan x="{}" dy="{}">{}</tspan>"##,
            c(p.x), c(step + pending), crate::svg::render_markup(line)
        );
        pending = 0.0;
    }
    s.push_str("</text></g>");
}

/// Net name for a track/via, resolving the two KiCad encodings. KiCad 10 carries the
/// name on the object itself (`obj_name`); KiCad ≤9 segments/vias store only a numeric
/// `code`, whose name lives in the board net table — so prefer the object's name and
/// fall back to the table lookup.
pub(crate) fn net_name_of<'a>(pcb: &'a eda_parse_kicad::pcb::Pcb, code: i64, obj_name: &'a str) -> &'a str {
    if obj_name.is_empty() {
        pcb.net_name(code)
    } else {
        obj_name
    }
}

/// Net-name attribute, emitted only for a real (named) net.
fn net_attr(s: &mut String, name: &str) {
    if !name.is_empty() {
        let _ = write!(s, r##" data-net="{}""##, esc(name));
    }
}

/// Whether a via's copper span reaches `layer` (through vias cover every copper).
pub(crate) fn via_on_layer(v: &eda_parse_kicad::pcb::Via, layer: &str) -> bool {
    let through = v.layers.iter().any(|l| l == "F.Cu") && v.layers.iter().any(|l| l == "B.Cu");
    through || v.layers.iter().any(|l| l == layer)
}

/// Whether a pad's layer set includes `layer`, honouring KiCad wildcards
/// (`*.Cu`, `F&B.Cu`, `*.Mask`, `*.Paste`).
pub(crate) fn pad_on_layer(pad: &Pad, layer: &str) -> bool {
    pad.layers.iter().any(|spec| layer_glob_matches(spec, layer))
}

fn layer_glob_matches(spec: &str, layer: &str) -> bool {
    if spec == layer {
        return true;
    }
    // Split "F.Cu" into ("F", "Cu"); wildcards apply to the side prefix.
    let (l_side, l_type) = layer.split_once('.').unwrap_or((layer, ""));
    let (s_side, s_type) = spec.split_once('.').unwrap_or((spec, ""));
    if s_type != l_type {
        return false;
    }
    match s_side {
        "*" => true,
        "F&B" => l_side == "F" || l_side == "B",
        other => other == l_side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r##"
    (kicad_pcb (version 20240101)
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (35 "B.SilkS" user)
              (37 "F.SilkS" user) (39 "F.Mask" user) (44 "Edge.Cuts" user)
              (49 "F.Fab" user) (51 "F.CrtYd" user) (53 "F.Paste" user))
      (net 0 "")
      (net 1 "GND")
      (net 2 "/SIG")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 60 0)
        (property "Reference" "R1" (at 0 1 0) (layer "F.SilkS"))
        (fp_line (start -1 -0.5) (end 1 -0.5) (layer "F.SilkS") (stroke (width 0.12)))
        (pad "1" thru_hole circle (at -0.8 0) (size 1.2 1.2) (drill 0.6)
          (layers "*.Cu" "*.Mask") (net 1 "GND") (uuid "pad1"))
        (pad "2" smd roundrect (at 0.8 0) (size 0.9 1.0) (layers "F.Cu" "F.Mask" "F.Paste")
          (net 2 "/SIG") (uuid "pad2")))
      (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net 2) (uuid "t1"))
      (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v1"))
      (zone (net 1) (net_name "GND") (layer "F.Cu") (uuid "z1")
        (filled_polygon (layer "F.Cu") (pts (xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20))))
      (zone (net 0) (net_name "") (layer "F.Mask") (uuid "zmask")
        (filled_polygon (layer "F.Mask") (island) (pts (xy 2 2) (xy 8 2) (xy 8 8) (xy 2 8))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_line (start 100 0) (end 100 80) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 80) (layer "Edge.Cuts"))
      (gr_line (start 0 80) (end 100 80) (layer "Edge.Cuts")))
    "##;

    fn render(layer: &str, role: &str) -> String {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        let vb = board_viewbox(&pcb);
        render_layer(&pcb, "board.kicad_pcb", layer, role, vb)
    }

    #[test]
    fn renders_pcb_artifacts() {
        let dir = std::env::temp_dir().join(format!("extract_pcb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pcb_path = dir.join("board.kicad_pcb");
        std::fs::write(&pcb_path, BOARD).unwrap();

        let mut emit = |_: Msg| {};
        let theme = crate::theme::Theme::default();
        let art = extract_pcb(&pcb_path, &dir, &theme, &mut emit).unwrap();
        let svgs = &art.svgs;
        // One SVG per reviewable layer; copper first.
        assert!(svgs.iter().any(|v| v["layer"] == "F.Cu" && v["role"] == "copper"));
        assert!(svgs.iter().any(|v| v["layer"] == "Edge.Cuts" && v["role"] == "edge"));
        assert!(svgs.iter().any(|v| v["layer"] == "F.SilkS" && v["role"] == "silkscreen"));

        // The structured geometry IR is written and is valid JSON with the right schema.
        assert_eq!(art.geometry.as_deref(), Some("pcb/geometry.json"));
        let geom: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("pcb/geometry.json")).unwrap()).unwrap();
        assert_eq!(geom["schema"], "extract.pcb.geometry.a0");

        let models: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("models/models.json")).unwrap()).unwrap();
        assert_eq!(models["schema"], "extract.models.a0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copper_carries_tracks_pads_vias_zones_nets() {
        let fcu = render("F.Cu", "copper");
        assert!(fcu.contains(r##"data-primitive="track""##));
        assert!(fcu.contains(r##"data-net="/SIG""##));
        assert!(fcu.contains(r##"data-primitive="zone""##));
        assert!(fcu.contains(r##"data-primitive="via""##));
        assert!(fcu.contains(r##"data-primitive="via-hole""##));
        // Tracks carry a style stroke-width (the frontend mines length/width from it).
        assert!(fcu.contains("style=\"stroke-width:0.25\""));
        // Pad #1 is a thru-hole, so it appears on F.Cu with a drill overlay.
        assert!(fcu.contains(r##"data-primitive="pad""##));
        assert!(fcu.contains(r##"data-primitive="pad-hole""##));
        assert!(fcu.contains(r##"data-component="R1""##));
        assert!(fcu.contains(r##"data-pad-number="1""##));
        // The board outline is present on the copper layer too.
        assert!(fcu.contains(r##"data-layer-name="Edge.Cuts""##));
    }

    // KiCad 10 writes the net NAME directly on each pad/track/via/zone (`(net "X")`)
    // and has no numeric net table. The emitter must still produce `data-net` on every
    // copper object — without it the app shows no net labels and net highlight /
    // selection / search are all dead (the jetson-agx-thor-baseboard report).
    const BOARD_K10: &str = r##"
    (kicad_pcb (version 20260206) (generator_version "10.0")
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 60 0)
        (property "Reference" "R1" (at 0 1 0) (layer "F.SilkS"))
        (pad "1" smd roundrect (at -0.8 0) (size 0.9 1.0) (layers "F.Cu") (net "GND")
          (uuid "pad1")))
      (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net "/SIG") (uuid "t1"))
      (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "GND") (uuid "v1"))
      (zone (net "GND") (layer "F.Cu") (uuid "z1")
        (filled_polygon (layer "F.Cu") (pts (xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_line (start 100 0) (end 100 80) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 80) (layer "Edge.Cuts"))
      (gr_line (start 0 80) (end 100 80) (layer "Edge.Cuts")))
    "##;

    #[test]
    fn kicad10_copper_emits_data_net() {
        let pcb = Pcb::parse_str(BOARD_K10).unwrap();
        let vb = board_viewbox(&pcb);
        let fcu = render_layer(&pcb, "board.kicad_pcb", "F.Cu", "copper", vb);
        // Track, via and pad all carry their KiCad-10 net name.
        assert!(fcu.contains(r##"data-primitive="track""##));
        assert!(fcu.contains(r##"data-net="/SIG""##));
        assert!(fcu.contains(r##"data-primitive="via""##));
        assert!(fcu.contains(r##"data-primitive="pad""##));
        // GND is shared by the via, pad and zone — at least one data-net="GND" appears.
        assert!(fcu.contains(r##"data-net="GND""##));
    }

    #[test]
    fn thruhole_pad_reaches_back_copper_smd_does_not() {
        let bcu = render("B.Cu", "copper");
        // Through-hole pad 1 ("*.Cu") reaches B.Cu; SMD pad 2 ("F.Cu") does not.
        assert!(bcu.contains(r##"data-pad-number="1""##));
        assert!(!bcu.contains(r##"data-pad-number="2""##));
        // The through via spans to B.Cu.
        assert!(bcu.contains(r##"data-primitive="via""##));
    }

    #[test]
    fn silkscreen_carries_graphics_and_text_not_copper() {
        let silk = render("F.SilkS", "silkscreen");
        assert!(silk.contains(r##"data-primitive="footprint-graphic""##));
        assert!(silk.contains(r##"data-primitive="footprint-text""##));
        assert!(silk.contains("R1"));
        assert!(!silk.contains(r##"data-primitive="track""##));
    }

    #[test]
    fn mask_zone_renders_on_its_layer_not_copper() {
        // KiCad lets you draw a non-copper zone on F/B.Mask — a solder-mask region.
        // It must render on the mask layer (not just copper pours), as a solid fill.
        let fmask = render("F.Mask", "mask");
        assert!(fmask.contains(r##"data-primitive="zone""##), "mask zone should render on F.Mask");
        assert!(fmask.contains(r##"data-layer-name="F.Mask""##));
        // A filled mask zone is solid copper-pour-style, not a dashed keepout/outline.
        assert!(!fmask.contains("data-zone-type="), "a filled mask zone is not a keepout/outline");
        // The netless mask zone must not leak onto the copper layer (F.Cu keeps only
        // its own GND pour); copper still carries its zone.
        let fcu = render("F.Cu", "copper");
        assert!(!fcu.contains(r##"data-layer-name="F.Mask""##), "mask zone must not appear on copper");
        assert!(fcu.contains(r##"data-primitive="zone""##), "copper pour still renders on F.Cu");
    }

    const BOARD_USER_LAYERS: &str = r##"
    (kicad_pcb (version 20241229)
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user)
              (44 "Edge.Cuts" user)
              (43 "User.3" user "Mechanical_Drawing")
              (53 "User.8" user "PowerBoard_EdgeCut"))
      (net 0 "")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 10 10 0)
        (property "Reference" "R1" (at 0 -1 0) (layer "F.SilkS"))
        (fp_text user "ASSY" (at 0 2 0) (layer "User.3"))
        (pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu") (net 0) (uuid "p1")))
      (gr_line (start 0 0) (end 20 0) (layer "User.3") (stroke (width 0.2)))
      (gr_text "Notes" (at 5 5) (layer "User.3"))
      (gr_line (start 0 0) (end 20 0) (layer "Edge.Cuts")))
    "##;

    #[test]
    fn user_layers_export_only_when_they_have_content() {
        let dir = std::env::temp_dir().join(format!("extract_user_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pcb_path = dir.join("board.kicad_pcb");
        std::fs::write(&pcb_path, BOARD_USER_LAYERS).unwrap();

        let mut emit = |_: Msg| {};
        // A theme that knows User.3's colour (board key `user_3`).
        let mut theme = crate::theme::Theme::default();
        theme.board.insert("user_3".to_string(), "#C2C2C2".to_string());
        let svgs = extract_pcb(&pcb_path, &dir, &theme, &mut emit).unwrap().svgs;

        // User.3 has board + footprint content → exported, role "user", display
        // name and resolved colour carried through.
        let u3 = svgs.iter().find(|v| v["layer"] == "User.3").expect("User.3 exported");
        assert_eq!(u3["role"], "user");
        assert_eq!(u3["user_name"], "Mechanical_Drawing");
        assert_eq!(u3["color"], "#C2C2C2");
        // User.8 is defined in the stackup but never drawn on → skipped entirely.
        assert!(!svgs.iter().any(|v| v["layer"] == "User.8"), "empty user layer must be skipped");
        // The user layer's SVG actually carries its board graphics + footprint text.
        let svg = std::fs::read_to_string(dir.join("pcb/User_3.svg")).unwrap();
        assert!(svg.contains(">Notes</tspan>"), "board gr_text rendered on the user layer");
        assert!(svg.contains(">ASSY</tspan>"), "footprint text rendered on the user layer");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_layer_with_only_a_zone_counts_as_content() {
        // A renamed user layer whose sole content is a zone (e.g. a conformal-coating
        // mask region) must still export — otherwise its zone has no SVG to render
        // into. A user layer drawn on by nothing stays skipped.
        let board = r##"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user)
                  (61 "User.12" user "Conformal_Mask_Bot")
                  (62 "User.13" user "Empty"))
          (net 0 "")
          (gr_line (start 0 0) (end 20 0) (layer "Edge.Cuts"))
          (zone (net 0) (net_name "") (layer "User.12") (uuid "zc")
            (filled_polygon (layer "User.12") (pts (xy 0 0) (xy 10 0) (xy 10 10) (xy 0 10)))))
        "##;
        let pcb = Pcb::parse_str(board).unwrap();
        assert!(layer_has_content(&pcb, "User.12"), "a user layer carrying only a zone has content");
        assert!(!layer_has_content(&pcb, "User.13"), "a user layer drawn on by nothing is skipped");
    }

    #[test]
    fn layer_glob_matching() {
        assert!(layer_glob_matches("*.Cu", "F.Cu"));
        assert!(layer_glob_matches("*.Cu", "In1.Cu"));
        assert!(layer_glob_matches("F&B.Cu", "B.Cu"));
        assert!(!layer_glob_matches("F.Cu", "B.Cu"));
        assert!(!layer_glob_matches("*.Mask", "F.Cu"));
    }

    const BOARD2: &str = r##"
    (kicad_pcb (version 20241229)
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (37 "F.SilkS" user)
              (39 "F.Mask" user) (44 "Edge.Cuts" user) (49 "F.Fab" user))
      (net 0 "")
      (net 1 "GND")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 50 0)
        (attr smd dnp)
        (property "Reference" "R9" (at 0 -2 0) (layer "F.SilkS") (hide yes))
        (property "Value" "1k" (at 0 2 0) (layer "F.Fab"))
        (fp_text user "${REFERENCE}" (at 0 0 0) (layer "F.SilkS"))
        (fp_rect (start -2 -1) (end 2 1) (layer "F.CrtYd"))
        (pad "1" smd roundrect (at -1 0) (size 1.0 1.4) (layers "F.Cu" "F.Mask")
          (roundrect_rratio 0.25) (solder_mask_margin 0.1) (net 1 "GND") (uuid "pad1"))
        (pad "2" smd oval (at 1 0) (size 1.0 2.0) (layers "F.Cu" "F.Mask")
          (net 1 "GND") (uuid "pad2")))
      (gr_arc (start 40 60) (mid 50 58) (end 60 60) (layer "F.SilkS")
        (stroke (width 0.12)))
      (zone (net 0) (net_name "") (layer "F.Cu") (uuid "z2")
        (keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed)
                 (copperpour not_allowed))
        (polygon (pts (xy 45 45) (xy 55 45) (xy 55 55) (xy 45 55))))
      (gr_line (start 40 40) (end 60 40) (layer "Edge.Cuts"))
      (gr_line (start 40 60) (end 60 60) (layer "Edge.Cuts")))
    "##;

    fn render2(layer: &str, role: &str) -> String {
        let pcb = Pcb::parse_str(BOARD2).unwrap();
        let vb = board_viewbox(&pcb);
        render_layer(&pcb, "board.kicad_pcb", layer, role, vb)
    }

    #[test]
    fn roundrect_and_oval_pads_render_with_corner_radius() {
        let fcu = render2("F.Cu", "copper");
        // Roundrect/oval pads are oriented <rect>s carrying a corner radius.
        assert!(fcu.contains(r##"<rect"##));
        assert!(fcu.contains(r##" rx=""##), "rounded pad must emit rx");
    }

    #[test]
    fn mask_layer_grows_pad_by_solder_mask_margin() {
        // Pad 1 copper is 1.0 wide; on the mask layer it grows by 2*0.1 = 1.2.
        let mask = render2("F.Mask", "mask");
        assert!(mask.contains(r##"width="1.2""##), "mask aperture should be copper + 2*margin");
        let fcu = render2("F.Cu", "copper");
        assert!(fcu.contains(r##"width="1""##), "copper pad keeps its true width");
    }

    #[test]
    fn arcs_render_as_true_arc_paths() {
        let silk = render2("F.SilkS", "silkscreen");
        // The silk arc is a real <path … A …>, not a chord polyline.
        assert!(silk.contains(r##"<path d="M"##));
        assert!(silk.contains(" A "));
    }

    #[test]
    fn arc_path_flags_follow_the_mid_point() {
        // Quarter arc (1,0)->(0.707,0.707)->(0,1): minor arc in the +angle
        // direction, so large=0, sweep=1 and the radius is 1.
        let d = arc_path_d(Pt { x: 1.0, y: 0.0 }, Pt { x: 0.7071, y: 0.7071 }, Pt { x: 0.0, y: 1.0 }).unwrap();
        assert_eq!(d, "M 1,0 A 1,1 0 0 1 0,1");
        // Collinear points have no finite circle -> fall back (None).
        assert!(arc_path_d(Pt { x: 0.0, y: 0.0 }, Pt { x: 1.0, y: 0.0 }, Pt { x: 2.0, y: 0.0 }).is_none());
    }

    #[test]
    fn keepout_zone_drawn_as_dashed_outline_not_copper() {
        let fcu = render2("F.Cu", "copper");
        assert!(fcu.contains(r##"data-zone-type="keepout""##));
        assert!(fcu.contains("stroke-dasharray"));
    }

    #[test]
    fn hidden_text_skipped_placeholder_expanded_and_dnp_marked() {
        let silk = render2("F.SilkS", "silkscreen");
        // Two footprint texts sit on F.SilkS: a hidden reference and a visible
        // `${REFERENCE}` marker. Only the visible one is plotted, so exactly one
        // <text> element appears (the hidden reference is skipped).
        assert_eq!(silk.matches("<text").count(), 1, "hidden reference must be skipped");
        // The placeholder is expanded to the real designator, none left literal.
        assert!(silk.contains(">R9</tspan>"), "expanded ${{REFERENCE}} should read R9");
        assert!(!silk.contains("${REFERENCE}"));
        // DNP footprints get an X on the fab layer.
        let fab = render2("F.Fab", "fab");
        assert!(fab.contains(r##"data-dnp="1""##));
    }

    const BOARD_TEXT: &str = r##"
    (kicad_pcb (version 20241229)
      (layers (0 "F.Cu" signal) (37 "F.SilkS" user) (44 "Edge.Cuts" user)
              (49 "F.Fab" user) (41 "User.2" user "FabNotes"))
      (net 0 "")
      (gr_text "line1\nline2" (at 150 150 0) (layer "User.2") (uuid "g1")
        (effects (font (size 1 1)) (justify left bottom)))
      (footprint "R_0402" (layer "F.Cu") (uuid "fp1") (at 50 60 90)
        (fp_text user "R1" (at 0 0 90) (layer "F.Fab")
          (effects (font (size 0.26 0.26)))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 80) (layer "Edge.Cuts")))
    "##;

    #[test]
    fn text_is_thin_scaled_and_rotated() {
        let pcb = Pcb::parse_str(BOARD_TEXT).unwrap();
        let vb = board_viewbox(&pcb);
        let fab = render_layer(&pcb, "b.kicad_pcb", "F.Fab", "fab", vb);
        // Filled glyphs only — no inherited root stroke (the "very thick" report).
        assert!(fab.contains(r##"stroke="none""##));
        // KiCad height 0.26 mm -> SVG font-size 0.26*4/3 (cap-height match) with NO
        // minimum-size floor: 0.26 mm fab text must NOT be inflated to a 0.5 mm clamp
        // (that overflowed the fab box — "R104 looks much bigger").
        assert!(fab.contains(r##"font-size="0.3467""##), "0.26 mm fab text must not be clamped: {fab}");
        // A 90°-rotated footprint rotates its fab text too (keep-upright, rotate(-90)).
        assert!(fab.contains("transform=\"rotate(-90"), "fab text must follow the part angle: {fab}");
    }

    #[test]
    fn multiline_text_stacks_with_justify() {
        let pcb = Pcb::parse_str(BOARD_TEXT).unwrap();
        let vb = board_viewbox(&pcb);
        let notes = render_layer(&pcb, "b.kicad_pcb", "User.2", "user", vb);
        // Hard newlines become one <tspan> per line inside a single <text>.
        assert_eq!(notes.matches("<tspan").count(), 2, "two lines -> two tspans");
        assert_eq!(notes.matches("<text").count(), 1);
        assert!(notes.contains(">line1</tspan>") && notes.contains(">line2</tspan>"));
        // `(justify left …)` -> left-anchored text.
        assert!(notes.contains(r##"text-anchor="start""##));
    }

    #[test]
    fn trailing_blank_line_lifts_bottom_justified_text() {
        // KiCad keeps "A\n\n" as a 2-line block (only the final terminator newline is
        // dropped, like wxStringSplit). Bottom-justified, that lifts the visible "A" one
        // interline above its anchor — the F.Silkscreen "CAN_H\n\n" label sat a line too
        // low when we popped *every* trailing blank. font = 1*4/3, line_h = font*1.2 = 1.6.
        let board = r##"
        (kicad_pcb (version 20241229)
          (layers (44 "Edge.Cuts" user) (41 "User.2" user "FabNotes"))
          (net 0 "")
          (gr_text "A\n\n" (at 10 10 0) (layer "User.2") (uuid "g1")
            (effects (font (size 1 1)) (justify left bottom)))
          (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts"))
          (gr_line (start 0 0) (end 0 50) (layer "Edge.Cuts")))
        "##;
        let pcb = Pcb::parse_str(board).unwrap();
        let vb = board_viewbox(&pcb);
        let svg = render_layer(&pcb, "b.kicad_pcb", "User.2", "user", vb);
        // The trailing blank row emits no tspan of its own ...
        assert_eq!(svg.matches("<tspan").count(), 1, "only the glyph-bearing line emits a tspan: {svg}");
        // ... but it reserves height: "A" is shifted up one interline (dy=-1.6), not 0.
        assert!(svg.contains(r##"dy="-1.6">A</tspan>"##), "trailing blank must lift A one line: {svg}");
    }

    #[test]
    fn blank_line_between_notes_keeps_its_height() {
        // A blank line BETWEEN two notes must reserve a full row, so the second note's dy
        // spans two interlines. An empty <tspan dy> alone would collapse the gap (the
        // missing spacing between numbered fab notes). Top-justified: first_dy=0,
        // line_h = 1*4/3*1.2 = 1.6, so "B" lands at 2*line_h = 3.2.
        let board = r##"
        (kicad_pcb (version 20241229)
          (layers (44 "Edge.Cuts" user) (41 "User.2" user "FabNotes"))
          (net 0 "")
          (gr_text "A\n\nB" (at 10 10 0) (layer "User.2") (uuid "g1")
            (effects (font (size 1 1)) (justify left top)))
          (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts"))
          (gr_line (start 0 0) (end 0 50) (layer "Edge.Cuts")))
        "##;
        let pcb = Pcb::parse_str(board).unwrap();
        let vb = board_viewbox(&pcb);
        let svg = render_layer(&pcb, "b.kicad_pcb", "User.2", "user", vb);
        assert_eq!(svg.matches("<tspan").count(), 2, "two glyph-bearing lines: {svg}");
        assert!(svg.contains(r##"dy="0">A</tspan>"##), "first line at the anchor: {svg}");
        assert!(svg.contains(r##"dy="3.2">B</tspan>"##), "blank row must add a full interline: {svg}");
    }

    #[test]
    fn viewbox_grows_to_include_off_board_text() {
        let pcb = Pcb::parse_str(BOARD_TEXT).unwrap();
        let (vx, _vy, vw, _vh) = board_viewbox(&pcb);
        // The note anchors at x=150, well outside the 0..100 board outline; the shared
        // viewBox must take it in or the layer SVG clips it ("FabNotes not visible").
        assert!(vx + vw >= 150.0, "viewBox right edge {} must reach the note at x=150", vx + vw);
    }

    const BOARD_GRTEXT_ANGLE: &str = r##"
    (kicad_pcb (version 20241229)
      (layers (37 "F.SilkS" user) (44 "Edge.Cuts" user))
      (net 0 "")
      (gr_text "SIDE-LABEL" (at 157.565 128.5304 -90) (layer "F.SilkS")
        (effects (font (size 1 1)) (justify bottom)))
      (gr_line (start 0 0) (end 200 0) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 200) (layer "Edge.Cuts")))
    "##;

    #[test]
    fn board_text_uses_literal_angle_not_kept_upright() {
        let pcb = Pcb::parse_str(BOARD_GRTEXT_ANGLE).unwrap();
        let vb = board_viewbox(&pcb);
        let silk = render_layer(&pcb, "b.kicad_pcb", "F.SilkS", "silkscreen", vb);
        // gr_text at -90° is plotted by KiCad at its literal angle: SVG rotate(-(-90)) =
        // rotate(90). The schematic keep-upright clamp would flip it to rotate(-90)
        // (reads upside-down) — that was the "silkscreen text orientation wrong" report,
        // so board text must NOT be kept upright.
        assert!(silk.contains("rotate(90 157.565 128.5304)"), "board text must use the literal angle: {silk}");
        assert!(!silk.contains("rotate(-90"), "board text must not be kept upright: {silk}");
    }

    const BOARD_PAPER: &str = r##"
    (kicad_pcb (version 20241229) (generator_version "9.0") (paper "A3")
      (title_block (title "MyBoard") (rev "B") (comment 1 "first"))
      (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user))
      (net 0 "")
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 80) (layer "Edge.Cuts")))
    "##;

    #[test]
    fn paper_and_title_block_parsed() {
        let pcb = Pcb::parse_str(BOARD_PAPER).unwrap();
        assert_eq!(pcb.paper.as_deref(), Some("A3"));
        assert_eq!(pcb.title.as_deref(), Some("MyBoard"));
        assert_eq!(pcb.rev.as_deref(), Some("B"));
        assert_eq!(pcb.generator_version.as_deref(), Some("9.0"));
        assert_eq!(pcb.comments, vec!["first".to_string()]);
        // The shared viewBox spans the full A3 page (420 mm), not just the 100 mm board,
        // so the board reads in its sheet context.
        let (_vx, _vy, vw, _vh) = board_viewbox(&pcb);
        assert!(vw >= 420.0, "viewBox must span the A3 page, got {vw}");
    }

    #[test]
    fn worksheet_layer_drawn_for_known_paper() {
        let dir = std::env::temp_dir().join(format!("extract_ws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pcb_path = dir.join("board.kicad_pcb");
        std::fs::write(&pcb_path, BOARD_PAPER).unwrap();
        let mut emit = |_: Msg| {};
        let theme = crate::theme::Theme::default();
        let svgs = extract_pcb(&pcb_path, &dir, &theme, &mut emit).unwrap().svgs;
        // A synthetic "Drawing Sheet" layer carries the frame + title block.
        let ws = svgs.iter().find(|v| v["role"] == "worksheet").expect("worksheet layer emitted");
        assert_eq!(ws["layer"], "Drawing Sheet");
        let svg = std::fs::read_to_string(dir.join("pcb/worksheet.svg")).unwrap();
        assert!(svg.contains(r#"data-primitive="worksheet""#));
        assert!(svg.contains("Title: MyBoard"), "title block: {svg}");
        assert!(svg.contains("Size: A3"));
        assert!(svg.contains("Rev: B"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_worksheet_without_paper() {
        // BOARD2 declares no `(paper …)`, so no drawing sheet is synthesised.
        let pcb = Pcb::parse_str(BOARD2).unwrap();
        assert!(pcb.paper.is_none());
        let dir = std::env::temp_dir().join(format!("extract_nows_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pcb_path = dir.join("board.kicad_pcb");
        std::fs::write(&pcb_path, BOARD2).unwrap();
        let mut emit = |_: Msg| {};
        let theme = crate::theme::Theme::default();
        let svgs = extract_pcb(&pcb_path, &dir, &theme, &mut emit).unwrap().svgs;
        assert!(!svgs.iter().any(|v| v["role"] == "worksheet"), "no paper -> no worksheet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_path_resolution_candidates() {
        let board = Path::new("C:/proj");
        // ${KIPRJMOD} resolves to the board dir.
        let c = resolve_candidates("${KIPRJMOD}/m/part.step", board);
        assert_eq!(c, vec![PathBuf::from("C:/proj/m/part.step")]);
        // A model-dir var with no env set still yields install-dir fallbacks to try.
        let c = resolve_candidates("${KICAD9_3DMODEL_DIR}/R.3dshapes/R.step", board);
        assert!(c.iter().any(|p| p.ends_with("R.3dshapes/R.step")));
        assert!(!c.is_empty(), "unset model-dir var falls back to common install dirs");
        // A relative path is tried against the board first.
        let c = resolve_candidates("sub/foo.wrl", board);
        assert_eq!(c.first(), Some(&PathBuf::from("C:/proj/sub/foo.wrl")));
    }

    #[test]
    fn copies_models_and_step_sibling() {
        let dir = std::env::temp_dir().join(format!("extract_models_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mdir = dir.join("m");
        std::fs::create_dir_all(&mdir).unwrap();
        // A render mesh + its solid sibling, referenced project-relative.
        std::fs::write(mdir.join("part.wrl"), b"wrl").unwrap();
        std::fs::write(mdir.join("part.step"), b"step").unwrap();
        let board = format!(
            r#"
            (kicad_pcb (version 20241229)
              (layers (0 "F.Cu" signal))
              (footprint "fp" (layer "F.Cu") (uuid "u1") (at 1 2 0)
                (model "${{KIPRJMOD}}/m/part.wrl"
                  (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 90)))))
            "#
        );
        let pcb = Pcb::parse_str(&board).unwrap();
        let pcb_path = dir.join("board.kicad_pcb");
        std::fs::write(&pcb_path, &board).unwrap();
        let mut emit = |_: Msg| {};
        write_models(&pcb, &pcb_path, &dir, &mut emit).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("models/models.json")).unwrap())
                .unwrap();
        assert_eq!(doc["schema"], "extract.models.a0");
        assert_eq!(doc["files"], 2, "wrl + step sibling copied");
        let m = &doc["models"][0]["models"][0];
        assert_eq!(m["file"], "models/files/part.wrl");
        assert_eq!(m["step"], "models/files/part.step");
        assert_eq!(m["format"], "wrl");
        // The copied files actually exist.
        assert!(dir.join("models/files/part.wrl").is_file());
        assert!(dir.join("models/files/part.step").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
