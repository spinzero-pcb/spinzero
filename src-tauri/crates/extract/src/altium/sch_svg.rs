//! Altium schematic sheet SVG.
//!
//! Same contract as the KiCad renderer (`crate::svg`): a lean, per-sheet SVG in
//! millimetres where every object carries the `data-uuid` the cross-probe
//! indexes are keyed on and a `data-primitive` the app themes by CSS. Nothing
//! downstream needs to know which tool drew the sheet.
//!
//! What is Altium's, and deliberately so (plan §6), is the *look*: the design's
//! own symbol artwork rather than a redrawn approximation, Altium's port and
//! power-port shapes, its pin decorations, its title-block frame, and the
//! colours the file itself carries. Altium stores a colour on every object, so
//! unlike KiCad — where the palette lives in an application theme — the class
//! defaults here are derived from the document ([`palette`]) and an object that
//! disagrees with its class bakes its own colour plus a `--nc` custom property,
//! which is the same override channel the KiCad path uses for net colours.
//!
//! Coordinates arrive Y-up from the bottom-left of the sheet and leave Y-down
//! in millimetres from its top-left, which is the space the viewer mounts.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eda_parse_altium::sch::{
    self, ComponentKind, GShape, Graphic, Pin, Port, PowerPort, Pt, SchDoc, SchText, SheetSymbol,
};
use eda_parse_altium::units;

use super::oid;

/// Padding around the drawn extent, in mm — matches the KiCad renderer so the
/// two look the same when a report puts them side by side.
const PAD: f64 = 2.54;

/// Fallback face for a font the sheet's table does not name. Altium's own
/// default is Times New Roman, which is what the whole corpus uses.
const FONT_FALLBACK: &str = "'Times New Roman','Liberation Serif','DejaVu Serif',serif";

/// Format a coordinate with trimmed precision (µm) to keep the file small.
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

/// XML-escape a text payload.
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

/// An Altium coordinate (already `UNIT`-scaled) in millimetres.
fn mm(v: i64) -> f64 {
    units::sch_mm(0, v)
}

/// What a sheet needs to know about the design around it to resolve its title
/// block. Everything here is a string the sheet may name with `=Something`.
pub struct SheetCtx<'a> {
    pub number: i64,
    pub total: i64,
    /// Hierarchical path of this sheet instance (`/Power/Gate_Drv/`).
    pub sheet_path: &'a str,
    /// The document's own file name (`Gate_Drv.SchDoc`).
    pub file_name: &'a str,
    /// Its full path on disk, which Altium's `=DocumentFullPathAndName` wants.
    pub full_path: &'a str,
    pub project_name: &'a str,
    /// The project file's `[ParameterN]` entries.
    pub project_params: &'a BTreeMap<String, String>,
}

/// Resolve Altium's `=Name` special strings against the sheet, the project, and
/// the handful of values only the extraction knows.
///
/// Altium stores the computed ones (`SheetNumber`, `DocumentName`, …) as
/// document parameters whose text is a literal `*` placeholder, so the sheet's
/// own parameter table cannot be consulted first — it would print an asterisk
/// where the page number goes.
struct Specials {
    computed: BTreeMap<String, String>,
    document: BTreeMap<String, String>,
    project: BTreeMap<String, String>,
}

/// Special strings this renderer computes rather than looks up. They always
/// have a value, which is why [`special_draws_nothing`] can rule them out.
const COMPUTED_SPECIALS: [&str; 5] = [
    "SHEETNUMBER",
    "SHEETTOTAL",
    "DOCUMENTNAME",
    "DOCUMENTFULLPATHANDNAME",
    "PROJECTNAME",
];

/// True when a `=Special` resolves to an EMPTY string, so the renderer draws no
/// group for it (D3.7).
///
/// The schematic geometry has to apply the same rule or the two disagree: a row
/// with no group is a change the viewer can report and never frame. It is a
/// narrow case — the special has to resolve, and resolve to nothing, which only
/// a project parameter defined as empty does — so the test is stated once, here,
/// beside the resolution it mirrors.
pub fn special_draws_nothing(
    text: &str,
    doc: &SchDoc,
    project: &BTreeMap<String, String>,
) -> bool {
    let Some(name) = text.strip_prefix('=') else {
        return false;
    };
    let key = name.to_ascii_uppercase();
    if COMPUTED_SPECIALS.contains(&key.as_str()) {
        return false;
    }
    // A document parameter wins, and an empty or `*` one is not a value at all.
    if doc
        .parameters
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(name) && !p.text.is_empty() && p.text != "*")
    {
        return false;
    }
    project
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.is_empty())
        .unwrap_or(false)
}

impl Specials {
    fn new(doc: &SchDoc, ctx: &SheetCtx) -> Specials {
        let mut computed = BTreeMap::new();
        let mut put = |k: &str, v: String| computed.insert(k.to_ascii_uppercase(), v);
        put(COMPUTED_SPECIALS[0], ctx.number.to_string());
        put(COMPUTED_SPECIALS[1], ctx.total.to_string());
        put(COMPUTED_SPECIALS[2], ctx.file_name.to_string());
        put(COMPUTED_SPECIALS[3], ctx.full_path.to_string());
        put(COMPUTED_SPECIALS[4], ctx.project_name.to_string());
        let document: BTreeMap<String, String> = doc
            .parameters
            .iter()
            // A `*` is Altium's "compute this at draw time" placeholder, not a
            // value; keeping it would print an asterisk in the title block.
            .filter(|p| !p.text.is_empty() && p.text != "*")
            .map(|p| (p.name.to_ascii_uppercase(), p.text.clone()))
            .collect();
        let project = ctx
            .project_params
            .iter()
            .map(|(k, v)| (k.to_ascii_uppercase(), v.clone()))
            .collect();
        Specials { computed, document, project }
    }

    /// Expand a text that may be a special string. Anything unresolved is left
    /// as its own literal: a blank title block hides that a field is missing,
    /// and a visible `=PRJ_Customer` says exactly what the design did not set.
    fn expand(&self, text: &str) -> String {
        let Some(name) = text.strip_prefix('=') else {
            return text.to_string();
        };
        let key = name.to_ascii_uppercase();
        self.computed
            .get(&key)
            .or_else(|| self.document.get(&key))
            .or_else(|| self.project.get(&key))
            .cloned()
            .unwrap_or_else(|| text.to_string())
    }
}

/// The sheet's class colours, in the keys the frontend's theme map reads.
///
/// Altium keeps no palette file: every object carries its own colour, so a
/// class default is whatever that class overwhelmingly *is* on this sheet. The
/// modal colour is used rather than a first-seen one so a single recoloured
/// wire cannot redefine what "wire" means for the whole document.
pub fn palette(sheets: &[&SchDoc]) -> BTreeMap<String, String> {
    let mut votes: BTreeMap<&'static str, BTreeMap<String, usize>> = BTreeMap::new();
    let mut vote = |key: &'static str, color: &str| {
        if !color.is_empty() {
            *votes.entry(key).or_default().entry(color.to_string()).or_default() += 1;
        }
    };
    for doc in sheets {
        for w in &doc.wires {
            vote("wire", &w.color);
        }
        for b in &doc.buses {
            vote("bus", &b.color);
        }
        for j in &doc.junctions {
            vote("junction", &j.color);
        }
        for n in &doc.no_ercs {
            vote("no_connect", &n.color);
        }
        // An empty label is not drawn, so its colour is not a vote.
        for l in doc.net_labels.iter().filter(|l| !l.text.trim().is_empty()) {
            vote("label_local", &l.color);
        }
        for p in &doc.ports {
            vote("label_global", &p.text_color);
        }
        for p in &doc.power_ports {
            vote("label_hier", &p.color);
        }
        for t in doc.texts.iter().filter(|t| !t.template) {
            vote("note", &t.color);
        }
        for s in &doc.sheet_symbols {
            vote("sheet", &s.color);
            vote("sheet_name", &s.name_color);
        }
        for s in &doc.param_sets {
            vote("netclass_flag", &s.parameters.first().map(|p| p.color.clone()).unwrap_or_default());
        }
        for comp in &doc.components {
            vote("reference", &comp.designator_color);
            for g in &comp.graphics {
                vote("component_outline", &g.color);
                if let Some(f) = &g.fill {
                    vote("component_body", f);
                }
            }
            for p in &comp.pins {
                vote("pin", &p.color);
                vote("pin_name", &p.color);
                vote("pin_number", &p.color);
            }
            for p in &comp.parameters {
                vote(if p.name.eq_ignore_ascii_case("Value") { "value" } else { "fields" }, &p.color);
            }
        }
    }
    votes
        .into_iter()
        .filter_map(|(k, counts)| {
            counts
                .into_iter()
                // Ties break on the colour string so the palette is
                // byte-deterministic across runs.
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(color, _)| (k.to_string(), color))
        })
        .collect()
}

/// `data-uuid` for an object, and nothing for one with no handle at all.
///
/// Altium's junction record carries no `UniqueID`, so ids come through
/// [`super::oid`], which falls back to the object's position. An empty attribute
/// would look like an identity to every consumer that reads one — the
/// cross-probe indexes, the diff engine's pairing — and every junction on the
/// sheet would share it.
fn uuid_attr(uuid: &str) -> String {
    if uuid.is_empty() {
        String::new()
    } else {
        format!(r#" data-uuid="{}""#, esc(uuid))
    }
}

/// Bake an object's own colour when it disagrees with its class default.
///
/// `--nc` is the same custom property the KiCad path uses for a net-class
/// override, so the app's existing `var(--nc, …)` rules honour it unchanged.
fn override_attr(color: &str, class_default: Option<&String>) -> String {
    if color.is_empty() || class_default.map(|d| d == color).unwrap_or(false) {
        return String::new();
    }
    format!(r#" data-color="{color}" style="--nc:{color}""#)
}

/// Everything the emitters need that is not the object itself.
struct Ctx<'a> {
    /// Sheet height in mm — the Y flip is about the page, so a primitive's
    /// coordinates never depend on where another primitive sits.
    height: f64,
    fonts: &'a sch::SheetProps,
    palette: &'a BTreeMap<String, String>,
    specials: Specials,
}

impl Ctx<'_> {
    /// Altium space (Y-up, origin bottom-left) to the bundle's (Y-down, mm).
    fn xy(&self, p: Pt) -> (f64, f64) {
        (mm(p.x), self.height - mm(p.y))
    }

    /// Font family, glyph height (mm), and the bold/italic flags of a `FontID`.
    /// A `FontID` the table does not cover falls back to Altium's own default.
    fn font(&self, id: i64) -> (String, f64, bool, bool) {
        let i = (id - 1).max(0) as usize;
        let family = match self.fonts.fonts.get(i) {
            Some(n) if !n.is_empty() => format!("'{}',{FONT_FALLBACK}", n.replace('\'', "")),
            _ => FONT_FALLBACK.to_string(),
        };
        let size = units::sch_font_mm(self.fonts.font_sizes.get(i).copied().unwrap_or(10));
        (
            family,
            size,
            self.fonts.font_bold.get(i).copied().unwrap_or(false),
            self.fonts.font_italic.get(i).copied().unwrap_or(false),
        )
    }

    fn class(&self, key: &str) -> Option<&String> {
        self.palette.get(key)
    }
}

/// Altium's justification grid (0 bottom-left … 8 top-right) as the SVG
/// `text-anchor` and `dominant-baseline` pair. The vertical sense flips with the
/// Y axis: Altium's "bottom" is below the anchor in a Y-up sheet, which is the
/// baseline once the page is the other way up.
/// A mirrored text's justification, with its horizontal half reversed.
///
/// Altium keeps mirrored text READABLE — it never draws the glyphs backwards —
/// and reverses which side of its anchor the string runs to, so a designator
/// still sits outside the symbol it labels rather than across it. The corpus
/// mirrors 2683 strings: 2484 parameters, 122 free texts and 77 designators.
fn mirror_justify(justify: i64, mirrored: bool) -> i64 {
    if !mirrored {
        return justify;
    }
    let j = justify.clamp(0, 8);
    (j / 3) * 3
        + match j % 3 {
            0 => 2,
            2 => 0,
            middle => middle,
        }
}

fn anchor(justify: i64) -> (&'static str, &'static str) {
    let j = justify.clamp(0, 8);
    let h = match j % 3 {
        0 => "start",
        1 => "middle",
        _ => "end",
    };
    let v = match j / 3 {
        0 => "alphabetic",
        1 => "central",
        _ => "hanging",
    };
    (h, v)
}

/// Altium rotations are counter-clockwise in a Y-up sheet, so they run the other
/// way once the page is flipped.
fn rotate_attr(orientation: i64, x: f64, y: f64) -> String {
    let deg = -(orientation.rem_euclid(4) * 90);
    if deg == 0 {
        String::new()
    } else {
        format!(r#" transform="rotate({deg} {} {})""#, c(x), c(y))
    }
}

/// Altium marks an overbar by following each character with a backslash
/// (`R\E\S\E\T\`). The bar is emitted as an SVG overline over the whole run
/// rather than per character, which is what the string means.
fn markup(text: &str) -> (String, bool) {
    if !text.contains('\\') {
        return (esc(text), false);
    }
    let stripped: String = text.chars().filter(|&ch| ch != '\\').collect();
    // Only a fully-marked run is a bar; a lone backslash is a path separator.
    let marked = text.len() >= stripped.len() * 2;
    (esc(&stripped), marked && !stripped.is_empty())
}

/// Emit one line of text with Altium's own placement.
#[allow(clippy::too_many_arguments)]
fn emit_text(
    s: &mut String,
    ctx: &Ctx,
    text: &str,
    at: Pt,
    font: i64,
    justify: i64,
    orientation: i64,
    color: &str,
    class_default: Option<&String>,
) {
    if text.is_empty() {
        return;
    }
    let (x, y) = ctx.xy(at);
    let (family, size, bold, italic) = ctx.font(font);
    let (h, v) = anchor(justify);
    let (body, bar) = markup(text);
    let weight = if bold { r#" font-weight="bold""# } else { "" };
    let style = if italic { r#" font-style="italic""# } else { "" };
    let bar_attr = if bar { r#" text-decoration="overline""# } else { "" };
    let fill = if color.is_empty() { "#000000" } else { color };
    let over = override_attr(color, class_default);
    let _ = write!(
        s,
        r#"<text x="{}" y="{}" font-family="{}" font-size="{}" text-anchor="{h}" dominant-baseline="{v}" fill="{fill}" stroke="none"{weight}{style}{bar_attr}{over}{}>{body}</text>"#,
        c(x),
        c(y),
        esc(&family),
        c(size),
        rotate_attr(orientation, x, y),
    );
}

/// Draw one graphic primitive as SVG geometry. The caller owns the wrapping
/// group and its `data-*`; this writes shapes only.
fn emit_shape(s: &mut String, ctx: &Ctx, g: &Graphic) {
    let w = units::sch_line_width_mm(g.width);
    let stroke = if g.color.is_empty() { "#000000".to_string() } else { g.color.clone() };
    let fill = g.fill.clone().unwrap_or_else(|| "none".to_string());
    // Altium's dashed and dotted styles, scaled off the pen so a thick dashed
    // line does not read as a solid one.
    let dash = match g.style {
        1 => format!(r#" stroke-dasharray="{} {}""#, c(w * 6.0), c(w * 3.0)),
        2 => format!(r#" stroke-dasharray="{} {}""#, c(w), c(w * 3.0)),
        3 => format!(
            r#" stroke-dasharray="{} {} {} {}""#,
            c(w * 6.0),
            c(w * 3.0),
            c(w),
            c(w * 3.0)
        ),
        _ => String::new(),
    };
    let common = format!(r#" fill="{fill}" stroke="{stroke}" stroke-width="{}"{dash}"#, c(w));
    match &g.shape {
        GShape::Line { a, b } => {
            let (x1, y1) = ctx.xy(*a);
            let (x2, y2) = ctx.xy(*b);
            let _ = write!(
                s,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}"{dash}/>"#,
                c(x1), c(y1), c(x2), c(y2), c(w)
            );
        }
        GShape::Polyline { pts, start_shape, end_shape, shape_size } => {
            let pts: Vec<(f64, f64)> = pts.iter().map(|p| ctx.xy(*p)).collect();
            let _ = write!(
                s,
                r#"<polyline points="{}" fill="none" stroke="{stroke}" stroke-width="{}"{dash}/>"#,
                points_attr(&pts), c(w)
            );
            let size = mm((*shape_size).max(1) * sch::UNIT) * 2.0;
            if let (Some(&a), Some(&b)) = (pts.first(), pts.get(1)) {
                emit_line_end(s, *start_shape, a, b, size, &stroke);
            }
            if pts.len() >= 2 {
                let (a, b) = (pts[pts.len() - 1], pts[pts.len() - 2]);
                emit_line_end(s, *end_shape, a, b, size, &stroke);
            }
        }
        GShape::Polygon { pts } => {
            let pts: Vec<(f64, f64)> = pts.iter().map(|p| ctx.xy(*p)).collect();
            let _ = write!(s, r#"<polygon points="{}"{common}/>"#, points_attr(&pts));
        }
        GShape::Rect { min, max } => {
            let (x1, y1) = ctx.xy(*min);
            let (x2, y2) = ctx.xy(*max);
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{}" height="{}"{common}/>"#,
                c(x1.min(x2)), c(y1.min(y2)), c((x2 - x1).abs()), c((y2 - y1).abs())
            );
        }
        GShape::Ellipse { c: ctr, rx, ry } => {
            let (x, y) = ctx.xy(*ctr);
            let _ = write!(
                s,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}"{common}/>"#,
                c(x), c(y), c(mm(*rx)), c(mm(*ry))
            );
        }
        GShape::Arc { c: ctr, rx, ry, start_deg, end_deg } => {
            emit_arc(s, ctx, *ctr, *rx, *ry, *start_deg, *end_deg, &stroke, w, &dash);
        }
    }
}

fn points_attr(pts: &[(f64, f64)]) -> String {
    pts.iter()
        .map(|(x, y)| format!("{},{}", c(*x), c(*y)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An arc, as an SVG path. A sweep of a full turn or more is the whole ellipse,
/// which `A` cannot express (its start and end would coincide).
#[allow(clippy::too_many_arguments)]
fn emit_arc(
    s: &mut String,
    ctx: &Ctx,
    center: Pt,
    rx: i64,
    ry: i64,
    start_deg: f64,
    end_deg: f64,
    stroke: &str,
    w: f64,
    dash: &str,
) {
    let (cx, cy) = ctx.xy(center);
    let (rx, ry) = (mm(rx), mm(ry));
    let sweep = {
        let d = end_deg - start_deg;
        if d <= 0.0 {
            d + 360.0
        } else {
            d
        }
    };
    if sweep >= 359.99 {
        let _ = write!(
            s,
            r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="none" stroke="{stroke}" stroke-width="{}"{dash}/>"#,
            c(cx), c(cy), c(rx), c(ry), c(w)
        );
        return;
    }
    // Y-down flips the sense of the angle, which is also why the sweep flag is
    // 1: an Altium arc runs counter-clockwise in its own space, and that reads
    // clockwise once the page is the other way up.
    let at = |deg: f64| {
        let r = deg.to_radians();
        (cx + rx * r.cos(), cy - ry * r.sin())
    };
    let (sx, sy) = at(start_deg);
    let (ex, ey) = at(end_deg);
    let large = if sweep > 180.0 { 1 } else { 0 };
    let _ = write!(
        s,
        r#"<path d="M {} {} A {} {} 0 {large} 1 {} {}" fill="none" stroke="{stroke}" stroke-width="{}"{dash}/>"#,
        c(sx), c(sy), c(rx), c(ry), c(ex), c(ey), c(w)
    );
}

/// Altium's line-end decorations. `tip` is the end point and `from` the
/// neighbouring vertex, so the glyph points the way the line runs.
fn emit_line_end(s: &mut String, kind: i64, tip: (f64, f64), from: (f64, f64), size: f64, stroke: &str) {
    if kind == 0 || size <= 0.0 {
        return;
    }
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    // An arrow points along the line; a tail points back down it.
    let back = matches!(kind, 3 | 4);
    let dir = if back { -1.0 } else { 1.0 };
    let base = (tip.0 - ux * size * dir, tip.1 - uy * size * dir);
    let half = size * 0.4;
    match kind {
        1 | 3 => {
            let _ = write!(
                s,
                r#"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{stroke}" stroke-width="{}"/>"#,
                c(base.0 + px * half), c(base.1 + py * half),
                c(tip.0), c(tip.1),
                c(base.0 - px * half), c(base.1 - py * half),
                c(size * 0.15)
            );
        }
        2 | 4 => {
            let _ = write!(
                s,
                r#"<polygon points="{},{} {},{} {},{}" fill="{stroke}" stroke="none"/>"#,
                c(tip.0), c(tip.1),
                c(base.0 + px * half), c(base.1 + py * half),
                c(base.0 - px * half), c(base.1 - py * half)
            );
        }
        5 => {
            let _ = write!(
                s,
                r#"<circle cx="{}" cy="{}" r="{}" fill="{stroke}" stroke="none"/>"#,
                c(tip.0), c(tip.1), c(half)
            );
        }
        _ => {
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{stroke}" stroke="none"/>"#,
                c(tip.0 - half), c(tip.1 - half), c(half * 2.0), c(half * 2.0)
            );
        }
    }
}

/// Render one sheet.
pub fn render_sheet(doc: &SchDoc, ctx: &SheetCtx, palette: &BTreeMap<String, String>) -> String {
    let height = mm(doc.sheet.height).max(1.0);
    let width = mm(doc.sheet.width).max(1.0);
    let ctxt = Ctx {
        height,
        fonts: &doc.sheet,
        palette,
        specials: Specials::new(doc, ctx),
    };

    // The viewBox is the page union whatever the design drew outside it —
    // Altium still draws off-page objects, and clipping them would hide a
    // stray part from the review rather than show it in the wrong place.
    let mut ext = Extent::new();
    ext.add(0.0, 0.0);
    ext.add(width, height);
    for p in every_point(doc) {
        let (x, y) = ctxt.xy(p);
        ext.add(x, y);
    }
    let (vx, vy) = (ext.minx.min(0.0) - PAD, ext.miny.min(0.0) - PAD);
    let (vw, vh) = (ext.maxx.max(width) + PAD - vx, ext.maxy.max(height) + PAD - vy);

    let mut s = String::new();
    let line = units::sch_line_width_mm(1);
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" fill="none" stroke="#000000" stroke-width="{}" font-family="{}">"##,
        c(vx), c(vy), c(vw), c(vh), c(line), FONT_FALLBACK
    );

    // The sheet's own paper colour, behind everything.
    if !doc.sheet.area_color.is_empty() {
        let _ = write!(
            s,
            r#"<rect data-primitive="sheet-area" x="0" y="0" width="{}" height="{}" fill="{}" stroke="none"/>"#,
            c(width), c(height), doc.sheet.area_color
        );
    }

    render_worksheet(&mut s, doc, &ctxt);

    // Images the designer placed on the sheet (the template's are in the frame).
    for img in doc.images.iter().filter(|i| !i.template) {
        emit_image(&mut s, &ctxt, img);
    }

    // Free sheet artwork.
    for g in &doc.graphics {
        let _ = write!(s, r#"<g data-primitive="graphic"{}>"#, uuid_attr(&oid(&g.uuid, "g", graphic_origin(g))));
        emit_shape(&mut s, &ctxt, g);
        s.push_str("</g>");
    }

    // Buses first, then wires: a wire crossing a bus should read on top.
    for (prim, list) in [("bus", &doc.buses), ("wire", &doc.wires)] {
        for w in list {
            if w.pts.len() < 2 {
                continue;
            }
            let pts: Vec<(f64, f64)> = w.pts.iter().map(|p| ctxt.xy(*p)).collect();
            let stroke = if w.color.is_empty() { "#000000" } else { &w.color };
            let _ = write!(
                s,
                r#"<g data-primitive="{prim}"{}{}><polyline points="{}" fill="none" stroke="{stroke}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/></g>"#,
                uuid_attr(&oid(&w.uuid, prim, w.pts[0])),
                override_attr(&w.color, ctxt.class(prim)),
                points_attr(&pts),
                c(units::sch_line_width_mm(w.width))
            );
        }
    }

    for j in &doc.junctions {
        let (x, y) = ctxt.xy(j.at);
        let fill = if j.color.is_empty() { "#000000" } else { &j.color };
        let _ = write!(
            s,
            r#"<g data-primitive="junction"{}{}><circle cx="{}" cy="{}" r="{}" fill="{fill}" stroke="none"/></g>"#,
            uuid_attr(&oid(&j.uuid, "j", j.at)), override_attr(&j.color, ctxt.class("junction")), c(x), c(y), c(mm(sch::UNIT) * 0.6)
        );
    }

    for n in &doc.no_ercs {
        let (x, y) = ctxt.xy(n.at);
        let r = mm(sch::UNIT) * 0.6;
        let stroke = if n.color.is_empty() { "#FF0000" } else { &n.color };
        let _ = write!(
            s,
            r#"<g data-primitive="no-connect"{}{}><polyline points="{},{} {},{}" stroke="{stroke}"/><polyline points="{},{} {},{}" stroke="{stroke}"/></g>"#,
            uuid_attr(&oid(&n.uuid, "nc", n.at)), override_attr(&n.color, ctxt.class("no_connect")),
            c(x - r), c(y - r), c(x + r), c(y + r),
            c(x - r), c(y + r), c(x + r), c(y - r)
        );
    }

    for sym in &doc.sheet_symbols {
        emit_sheet_symbol(&mut s, &ctxt, sym);
    }

    for comp in &doc.components {
        emit_component(&mut s, &ctxt, comp);
    }

    for p in &doc.power_ports {
        emit_power_port(&mut s, &ctxt, p);
    }

    for p in &doc.ports {
        emit_port(&mut s, &ctxt, p);
    }

    for l in &doc.net_labels {
        // A label with no text draws nothing (D3.7), and it names nothing
        // either — see `altium::netlist`. Altium keeps 69 of them on the
        // MCU144E1 design, and an empty group is one the viewer counts and can
        // never show.
        if l.text.trim().is_empty() {
            continue;
        }
        let _ = write!(
            s,
            r#"<g data-primitive="label"{}>"#,
            uuid_attr(&oid(&l.uuid, "nl", l.at))
        );
        emit_text(&mut s, &ctxt, &l.text, l.at, l.font, 0, l.orientation, &l.color, ctxt.class("label_local"));
        s.push_str("</g>");
    }

    // Net-class and other directives placed on a wire.
    for ps in &doc.param_sets {
        let (x, y) = ctxt.xy(ps.at);
        let color = ps.parameters.first().map(|p| p.color.clone()).unwrap_or_default();
        let stroke = if color.is_empty() { "#FF0000" } else { &color };
        let r = mm(sch::UNIT);
        let _ = write!(
            s,
            r#"<g data-primitive="netclass-flag"{}{}><polyline points="{},{} {},{} {},{} {},{} {},{}" stroke="{stroke}"/>"#,
            uuid_attr(&oid(&ps.uuid, "ps", ps.at)), override_attr(&color, ctxt.class("netclass_flag")),
            c(x), c(y), c(x + r), c(y - r), c(x + r * 3.0), c(y - r),
            c(x + r * 3.0), c(y - r * 2.0), c(x + r), c(y - r * 2.0)
        );
        for (i, p) in ps.parameters.iter().filter(|p| !p.hidden).enumerate() {
            emit_text(
                &mut s,
                &ctxt,
                &p.text,
                Pt { x: ps.at.x + sch::UNIT * 4, y: ps.at.y + sch::UNIT * (2 - 2 * i as i64) },
                p.font,
                0,
                0,
                &p.color,
                ctxt.class("netclass_flag"),
            );
        }
        s.push_str("</g>");
    }

    // Compile masks. `altium::netlist` reports them rather than applying them
    // (M1, D1.3), so drawing the outline is how a reviewer sees that part of
    // the sheet claims to be excluded — an invisible mask is the worse answer.
    for r in &doc.regions {
        let (x1, y1) = ctxt.xy(r.min);
        let (x2, y2) = ctxt.xy(r.max);
        let _ = write!(
            s,
            r##"<g data-primitive="graphic" data-kind="compile-region"{}><rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#808080" stroke-width="{}" stroke-dasharray="{} {}"/></g>"##,
            uuid_attr(&oid(&r.uuid, "rg", r.min)),
            c(x1.min(x2)), c(y1.min(y2)), c((x2 - x1).abs()), c((y2 - y1).abs()),
            c(line), c(line * 6.0), c(line * 3.0)
        );
    }
    // A blanket is a closed region, not a rectangle, so it draws as its own
    // polygon rather than as the box that bounds it.
    for b in &doc.blankets {
        let _ = write!(
            s,
            r#"<g data-primitive="graphic" data-kind="compile-mask"{}>"#,
            uuid_attr(&oid(&b.uuid, "bl", graphic_origin(b)))
        );
        emit_shape(&mut s, &ctxt, b);
        s.push_str("</g>");
    }

    // Free text last so an annotation is never buried under the artwork.
    for t in doc.texts.iter().filter(|t| !t.template) {
        emit_free_text(&mut s, &ctxt, t);
    }

    s.push_str("</svg>");
    s
}

/// The drawing sheet: the page border plus everything the template owns. Drawn
/// as one `worksheet` group, which is the primitive the app already stacks
/// behind the design and paints in the worksheet colour.
fn render_worksheet(s: &mut String, doc: &SchDoc, ctx: &Ctx) {
    let has_template =
        !doc.template_graphics.is_empty() || doc.texts.iter().any(|t| t.template) || doc.sheet.border_on;
    if !has_template {
        return;
    }
    let _ = write!(s, r#"<g data-primitive="worksheet">"#);
    if doc.sheet.border_on {
        emit_page_border(s, doc, ctx);
    }
    for img in doc.images.iter().filter(|i| i.template) {
        emit_image(s, ctx, img);
    }
    for g in &doc.template_graphics {
        emit_shape(s, ctx, g);
    }
    for t in doc.texts.iter().filter(|t| t.template) {
        let text = ctx.specials.expand(&t.text);
        emit_text(s, ctx, &text, t.at, t.font, t.justify, t.orientation, &t.color, None);
    }
    s.push_str("</g>");
}

/// Altium's page border: two rectangles a margin band apart, and the zone ruler
/// between them.
///
/// The ruler is not decoration — a review cites it ("the DESAT network in C3"),
/// so the tick positions and the letters have to be the ones the designer sees.
/// Columns are numbered left to right and rows lettered from the TOP down, each
/// band a whole division of the PAGE (not of the inner rectangle), with a tick
/// at the far edge of every band.
fn emit_page_border(s: &mut String, doc: &SchDoc, ctx: &Ctx) {
    let (w, h) = (mm(doc.sheet.width), mm(doc.sheet.height));
    let band = mm(doc.sheet.margin * sch::UNIT);
    let pen = units::sch_line_width_mm(1);
    let _ = write!(
        s,
        r##"<rect x="0" y="0" width="{}" height="{}" fill="none" stroke="#000000" stroke-width="{}"/>"##,
        c(w), c(h), c(pen)
    );
    if band <= 0.0 || w <= band * 2.0 || h <= band * 2.0 {
        return;
    }
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#000000" stroke-width="{}"/>"##,
        c(band), c(band), c(w - band * 2.0), c(h - band * 2.0), c(pen)
    );
    let tick = |s: &mut String, x1: f64, y1: f64, x2: f64, y2: f64| {
        let _ = write!(
            s,
            r##"<polyline points="{},{} {},{}" fill="none" stroke="#000000" stroke-width="{}"/>"##,
            c(x1), c(y1), c(x2), c(y2), c(pen)
        );
    };
    let (_, size, _, _) = ctx.font(1);
    let label = |s: &mut String, text: &str, x: f64, y: f64| {
        let _ = write!(
            s,
            r##"<text x="{}" y="{}" font-size="{}" text-anchor="middle" dominant-baseline="central" fill="#000000" stroke="none">{}</text>"##,
            c(x), c(y), c(size), esc(text)
        );
    };
    let nx = doc.sheet.zones_x.max(1);
    for k in 1..=nx {
        let x = w * k as f64 / nx as f64;
        tick(s, x, 0.0, x, band);
        tick(s, x, h - band, x, h);
        let cx = w * (k as f64 - 0.5) / nx as f64;
        label(s, &k.to_string(), cx, band / 2.0);
        label(s, &k.to_string(), cx, h - band / 2.0);
    }
    let ny = doc.sheet.zones_y.max(1);
    for k in 1..=ny {
        // The tick sits at the top edge of each band once the page is Y-down.
        let y = h * (ny - k) as f64 / ny as f64;
        tick(s, 0.0, y, band, y);
        tick(s, w - band, y, w, y);
        // Row A is the top row, so the letter counts down the page.
        let letter = ((b'A' + (ny - k) as u8) as char).to_string();
        let cy = h * (ny as f64 - k as f64 + 0.5) / ny as f64;
        label(s, &letter, band / 2.0, cy);
        label(s, &letter, w - band / 2.0, cy);
    }
}

/// A placed image. The bytes are sniffed rather than trusted to the file name —
/// Altium keeps the designer's original path, extension and all, and an image
/// mislabelled `.png` would otherwise be served with the wrong MIME type.
fn emit_image(s: &mut String, ctx: &Ctx, img: &sch::SchImage) {
    let (x1, y1) = ctx.xy(img.min);
    let (x2, y2) = ctx.xy(img.max);
    let (x, y) = (x1.min(x2), y1.min(y2));
    let (w, h) = ((x2 - x1).abs(), (y2 - y1).abs());
    if img.data.is_empty() {
        // A LINKED image points at a file on the designer's own machine and
        // carries no bytes; the corpus has six of them on one sheet. Drawing
        // nothing hides that the sheet has a picture there, so the frame is
        // drawn and the count is reported — the same choice the board makes for
        // a special string it cannot resolve.
        let _ = write!(
            s,
            r##"<g data-primitive="image" data-kind="linked"{}><rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#B0B0B0" stroke-width="{}" stroke-dasharray="{} {}"/></g>"##,
            uuid_attr(&oid(&img.uuid, "im", img.min)), c(x), c(y), c(w), c(h),
            c(units::sch_line_width_mm(1)),
            c(units::sch_line_width_mm(1) * 4.0),
            c(units::sch_line_width_mm(1) * 2.0)
        );
        return;
    }
    // Altium's own copy is usually an uncompressed BMP — one title-block logo
    // is 1.3 MB, and base64 makes that 1.8 MB in a sheet whose drawing is 100 KB
    // — so a BMP is re-encoded losslessly as a PNG on the way out. Anything
    // already compressed passes through untouched.
    let (mime, bytes) = match eda_parse_altium::image::bmp_to_png(&img.data) {
        Some(png) => ("image/png", std::borrow::Cow::Owned(png)),
        None if img.data.starts_with(&[0xFF, 0xD8, 0xFF]) => ("image/jpeg", std::borrow::Cow::Borrowed(&img.data[..])),
        None if img.data.starts_with(b"GIF8") => ("image/gif", std::borrow::Cow::Borrowed(&img.data[..])),
        None if img.data.starts_with(b"BM") => ("image/bmp", std::borrow::Cow::Borrowed(&img.data[..])),
        None => ("image/png", std::borrow::Cow::Borrowed(&img.data[..])),
    };
    let _ = write!(
        s,
        r#"<g data-primitive="image"{}><image x="{}" y="{}" width="{}" height="{}" href="data:{mime};base64,{}"/></g>"#,
        uuid_attr(&oid(&img.uuid, "im", img.min)), c(x), c(y), c(w), c(h), base64(&bytes)
    );
}

/// Minimal base64 for the embedded-image data URI — the only place the crate
/// needs it, and not worth a dependency.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A hierarchical sheet symbol and its entries. The `data-at-*-nm` attributes
/// are the same nanometre box the KiCad path publishes, so the diff engine's
/// sheet anchoring reads an Altium sheet unchanged.
fn emit_sheet_symbol(s: &mut String, ctx: &Ctx, sym: &SheetSymbol) {
    let (x, y) = ctx.xy(Pt { x: sym.at.x, y: sym.at.y });
    let (w, h) = (mm(sym.xsize * sch::UNIT), mm(sym.ysize * sch::UNIT));
    let _ = write!(
        s,
        r#"<g data-primitive="sheet-symbol"{} data-sheet-name="{}" data-at-x-nm="{}" data-at-y-nm="{}" data-size-x-nm="{}" data-size-y-nm="{}">"#,
        uuid_attr(&oid(&sym.uuid, "ss", sym.at)),
        esc(&sym.name),
        (x * 1e6) as i64,
        (y * 1e6) as i64,
        (w * 1e6) as i64,
        (h * 1e6) as i64,
    );
    let fill = if sym.solid && !sym.area_color.is_empty() { sym.area_color.as_str() } else { "none" };
    let stroke = if sym.color.is_empty() { "#000000" } else { &sym.color };
    let _ = write!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/>"#,
        c(x), c(y), c(w.max(0.1)), c(h.max(0.1)), c(units::sch_line_width_mm(1))
    );
    emit_text(s, ctx, &sym.name, sym.name_at, sym.name_font, 0, 0, &sym.name_color, ctx.class("sheet_name"));
    emit_text(s, ctx, &sym.filename, sym.filename_at, sym.name_font, 0, 0, &sym.name_color, ctx.class("sheet_name"));
    for e in &sym.entries {
        let at = sym.entry_point(e);
        let (ex, ey) = ctx.xy(at);
        // The entry's flag points into the sheet, so its text reads away from
        // the body on whichever side Altium put it.
        let left = e.side == 0 || e.side == 2;
        let g = mm(sch::UNIT) * 1.5;
        let tip = if left { ex - g } else { ex + g };
        let _ = write!(
            s,
            r#"<g data-primitive="port" data-kind="hier"{}><polyline points="{},{} {},{} {},{} {},{} {},{}" fill="none" stroke="{}"/>"#,
            uuid_attr(&e.uuid),
            c(ex), c(ey - g / 2.0), c(tip), c(ey - g / 2.0), c(if left { tip - g / 2.0 } else { tip + g / 2.0 }), c(ey),
            c(tip), c(ey + g / 2.0), c(ex), c(ey + g / 2.0),
            if e.color.is_empty() { "#800000" } else { &e.color }
        );
        emit_text(
            s,
            ctx,
            &e.name,
            Pt { x: at.x + if left { -sch::UNIT * 3 } else { sch::UNIT * 3 }, y: at.y },
            e.text_font,
            if left { 5 } else { 3 },
            0,
            &e.text_color,
            ctx.class("label_hier"),
        );
        s.push_str("</g>");
    }
    s.push_str("</g>");
}

/// A placed component: its own artwork, its pins, and the fields Altium shows.
fn emit_component(s: &mut String, ctx: &Ctx, comp: &sch::Component) {
    // A multi-part symbol stores every part's children in one record list, and
    // an alternate display mode stores a second drawing of the same part. Only
    // the placed part in the placed mode is on the sheet; drawing the rest puts
    // another part's body on top of this one.
    let part = comp.current_part_id;
    let mode = comp.display_mode;
    let mine = |pid: i64, dm: i64| (pid == part || pid <= 0) && dm == mode;
    // A graphical decoration is not a component in the review's sense, but it
    // is still ink on the sheet; it draws under the same group so a click on it
    // resolves to something rather than to nothing.
    let prim = if comp.kind == ComponentKind::Graphical { "graphic" } else { "symbol" };
    let _ = write!(
        s,
        r#"<g data-primitive="{prim}"{} data-ref="{}">"#,
        uuid_attr(&oid(&comp.uuid, "c", comp.at)),
        esc(&comp.designator)
    );
    for g in comp.graphics.iter().filter(|g| mine(g.part_id, g.display_mode)) {
        emit_shape(s, ctx, g);
    }
    for p in comp.pins.iter().filter(|p| mine(p.part_id, p.display_mode)) {
        emit_pin(s, ctx, comp, p);
    }
    if !comp.designator_hidden && !comp.designator.is_empty() {
        let _ = write!(
            s,
            r#"<g data-primitive="text" data-kind="designator"{}>"#,
            uuid_attr(&comp.designator_uuid)
        );
        emit_text(
            s,
            ctx,
            &comp.designator,
            comp.designator_at,
            comp.designator_font,
            mirror_justify(0, comp.designator_mirrored),
            comp.designator_orientation,
            &comp.designator_color,
            ctx.class("reference"),
        );
        s.push_str("</g>");
    }
    // A parameter with no location is data, not ink — Altium keeps dozens of
    // them per part (supplier links, simulation models) and none are drawn.
    for p in comp.parameters.iter().filter(|p| !p.hidden && p.at != Pt::default()) {
        let key = if p.name.eq_ignore_ascii_case("Value") { "value" } else { "fields" };
        // `=Name` inside a component parameter names another of its own
        // parameters, which is the same rule the BOM builder applies.
        let text = match p.text.strip_prefix('=') {
            Some(r) => comp.param(r).unwrap_or(&p.text).to_string(),
            None => p.text.clone(),
        };
        // A parameter that resolves to nothing is not ink. Opening its group
        // anyway leaves an empty handle the viewer counts, highlights and can
        // never show — the same rule the board applies to blank text.
        if text.is_empty() {
            continue;
        }
        let _ = write!(s, r#"<g data-primitive="text" data-kind="field"{}>"#, uuid_attr(&p.uuid));
        let justify = mirror_justify(p.justify, p.mirrored);
        emit_text(s, ctx, &text, p.at, p.font, justify, p.orientation, &p.color, ctx.class(key));
        s.push_str("</g>");
    }
    s.push_str("</g>");
}

/// One pin: the stub, Altium's dot/clock decorations, and the name and number
/// when the pin's own flags say to draw them.
fn emit_pin(s: &mut String, ctx: &Ctx, comp: &sch::Component, p: &Pin) {
    // A hidden pin is connected without being drawn. An object with nothing to
    // draw is not a primitive — the same rule the board applies to text that
    // resolves to nothing — so it gets no group at all rather than an empty one
    // the viewer would count, highlight and never be able to show.
    if p.hidden() {
        return;
    }
    let _ = write!(
        s,
        r#"<g data-primitive="pin"{} data-designator="{}" data-pin="{}">"#,
        uuid_attr(&oid(&p.uuid, "pin", p.at)),
        esc(&comp.designator),
        esc(&p.number)
    );
    let (bx, by) = ctx.xy(p.at); // body end
    let (ex, ey) = ctx.xy(p.connection()); // free end, where a wire lands
    let stroke = if p.color.is_empty() { "#000000" } else { &p.color };
    let w = units::sch_line_width_mm(1);
    let dot = mm(sch::UNIT) * 0.6;
    // The active-low dot sits at the free end, so the stub stops short of it.
    let (sx, sy) = if p.outer_edge == 1 {
        let len = ((ex - bx).powi(2) + (ey - by).powi(2)).sqrt().max(1e-9);
        (ex - (ex - bx) / len * dot * 2.0, ey - (ey - by) / len * dot * 2.0)
    } else {
        (ex, ey)
    };
    let _ = write!(
        s,
        r#"<polyline points="{},{} {},{}" fill="none" stroke="{stroke}" stroke-width="{}"/>"#,
        c(bx), c(by), c(sx), c(sy), c(w)
    );
    if p.outer_edge == 1 {
        let _ = write!(
            s,
            r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{stroke}" stroke-width="{}"/>"#,
            c((sx + ex) / 2.0), c((sy + ey) / 2.0), c(dot), c(w)
        );
    }
    if p.inner == 9 {
        // The clock wedge points into the body from the body end.
        let len = ((ex - bx).powi(2) + (ey - by).powi(2)).sqrt().max(1e-9);
        let (ux, uy) = ((ex - bx) / len, (ey - by) / len);
        let (px, py) = (-uy, ux);
        let k = dot * 1.6;
        let _ = write!(
            s,
            r#"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{stroke}" stroke-width="{}"/>"#,
            c(bx + px * k), c(by + py * k),
            c(bx - ux * k), c(by - uy * k),
            c(bx - px * k), c(by - py * k),
            c(w)
        );
    }

    // Name and number sit in Altium's own places: the name just inside the body
    // past the stub's body end, the number over the middle of the stub.
    let len = ((ex - bx).powi(2) + (ey - by).powi(2)).sqrt();
    let (ux, uy) = if len > 1e-9 { ((ex - bx) / len, (ey - by) / len) } else { (1.0, 0.0) };
    let gap = mm(sch::UNIT) * 0.5;
    // Text stays upright: a vertical pin's name reads left to right, the way
    // Altium draws it.
    let name_right = ux < -0.5 || (ux.abs() < 0.5 && uy < 0.0);
    if p.show_name() && !p.name.is_empty() && p.name != "~" {
        let at = px_to_pt(ctx, bx - ux * gap, by - uy * gap);
        emit_text(
            s,
            ctx,
            &p.name,
            at,
            pin_font(p.name_font),
            if name_right { 3 } else { 5 },
            0,
            &p.color,
            ctx.class("pin_name"),
        );
    }
    if p.show_designator() && !p.number.is_empty() {
        let (mx, my) = ((bx + sx) / 2.0, (by + sy) / 2.0);
        let at = px_to_pt(ctx, mx, my - gap);
        emit_text(s, ctx, &p.number, at, pin_font(p.number_font), 1, 0, &p.color, ctx.class("pin_number"));
    }
    s.push_str("</g>");
}

/// A pin's own font when the designer overrode it, else the sheet's first —
/// which is the one Altium draws pin text with by default.
fn pin_font(over: i64) -> i64 {
    if over > 0 {
        over
    } else {
        1
    }
}

/// Turn a millimetre point back into the Altium coordinate the text emitter
/// takes, so pin text placement is expressed in the space it is computed in.
fn px_to_pt(ctx: &Ctx, x: f64, y: f64) -> Pt {
    let inv = |v: f64| (v / 0.254 * sch::UNIT as f64).round() as i64;
    Pt { x: inv(x), y: inv(ctx.height - y) }
}

/// A power port: Altium's glyph for the style, plus the net name.
fn emit_power_port(s: &mut String, ctx: &Ctx, p: &PowerPort) {
    let (x, y) = ctx.xy(p.at);
    let stroke = if p.color.is_empty() { "#800000" } else { &p.color };
    let u = mm(sch::UNIT);
    let w = units::sch_line_width_mm(1);
    let _ = write!(
        s,
        r#"<g data-primitive="power-symbol"{} data-kind="{}"{}>"#,
        uuid_attr(&oid(&p.uuid, "pp", p.at)),
        p.style,
        override_attr(&p.color, ctx.class("label_hier"))
    );
    // The glyph is drawn pointing up and then rotated with the port, which is
    // how Altium orients it.
    let rot = rotate_attr(p.orientation, x, y);
    let _ = write!(s, r#"<g{rot} stroke="{stroke}" stroke-width="{}" fill="none">"#, c(w));
    let stem = |s: &mut String, h: f64| {
        let _ = write!(s, r#"<polyline points="{},{} {},{}"/>"#, c(x), c(y), c(x), c(y - h));
    };
    match p.style {
        // Circle, arrow, bar and wave all sit on a stem.
        0 => {
            stem(s, u * 2.0);
            let _ = write!(s, r#"<circle cx="{}" cy="{}" r="{}"/>"#, c(x), c(y - u * 2.5), c(u * 0.5));
        }
        1 => {
            stem(s, u * 2.0);
            let _ = write!(
                s,
                r#"<polyline points="{},{} {},{} {},{}"/>"#,
                c(x - u * 0.7), c(y - u * 1.3), c(x), c(y - u * 2.5), c(x + u * 0.7), c(y - u * 1.3)
            );
        }
        2 => {
            stem(s, u * 2.0);
            let _ = write!(
                s,
                r#"<polyline points="{},{} {},{}"/>"#,
                c(x - u), c(y - u * 2.0), c(x + u), c(y - u * 2.0)
            );
        }
        3 => {
            stem(s, u * 2.0);
            let _ = write!(
                s,
                r#"<path d="M {} {} q {} {} {} 0 q {} {} {} 0"/>"#,
                c(x - u), c(y - u * 2.0), c(u * 0.5), c(-u), c(u), c(u * 0.5), c(u), c(u)
            );
        }
        // Power ground: three shortening bars.
        4 => {
            stem(s, u * 2.0);
            for (i, k) in [1.0f64, 0.66, 0.33].iter().enumerate() {
                let yy = y - u * (2.0 + i as f64 * 0.5);
                let _ = write!(
                    s,
                    r#"<polyline points="{},{} {},{}"/>"#,
                    c(x - u * k), c(yy), c(x + u * k), c(yy)
                );
            }
        }
        // Signal ground and earth: a triangle and a hatched bar.
        5 => {
            stem(s, u * 2.0);
            let _ = write!(
                s,
                r#"<polygon points="{},{} {},{} {},{}" fill="none"/>"#,
                c(x - u), c(y - u * 2.0), c(x + u), c(y - u * 2.0), c(x), c(y - u * 3.0)
            );
        }
        _ => {
            stem(s, u * 2.0);
            let _ = write!(
                s,
                r#"<polyline points="{},{} {},{}"/>"#,
                c(x - u), c(y - u * 2.0), c(x + u), c(y - u * 2.0)
            );
            for k in [-1.0f64, 0.0, 1.0] {
                let _ = write!(
                    s,
                    r#"<polyline points="{},{} {},{}"/>"#,
                    c(x + u * k * 0.6), c(y - u * 2.0), c(x + u * k * 0.6 - u * 0.4), c(y - u * 2.8)
                );
            }
        }
    }
    s.push_str("</g>");
    if p.show_net_name {
        // The name reads away from the glyph, on the side the port points.
        let (justify, at) = match p.orientation.rem_euclid(4) {
            1 => (1, Pt { x: p.at.x, y: p.at.y + sch::UNIT * 4 }),
            2 => (3, Pt { x: p.at.x + sch::UNIT * 4, y: p.at.y }),
            3 => (7, Pt { x: p.at.x, y: p.at.y - sch::UNIT * 4 }),
            _ => (5, Pt { x: p.at.x - sch::UNIT * 4, y: p.at.y }),
        };
        emit_text(s, ctx, &p.text, at, p.font, justify, 0, &p.color, ctx.class("label_hier"));
    }
    s.push_str("</g>");
}

/// A port — Altium's cross-sheet connector, which is the hierarchical label of
/// the KiCad vocabulary, so it carries the same `data-kind`.
fn emit_port(s: &mut String, ctx: &Ctx, p: &Port) {
    let (x, y) = ctx.xy(p.at);
    let w = mm(p.width * sch::UNIT);
    let h = mm(p.height.max(1) * sch::UNIT);
    let stroke = if p.color.is_empty() { "#800000" } else { &p.color };
    let fill = if p.area_color.is_empty() { "none" } else { &p.area_color };
    // Altium's body shape follows the I/O type: an input is notched on its left,
    // an output on its right, a bidirectional at both ends.
    let notch = h / 2.0;
    let (left_in, right_in) = match p.io_type {
        1 => (true, false),  // Input
        2 => (false, true),  // Output
        3 => (true, true),   // Bidirectional
        _ => (false, false), // Unspecified — a plain box
    };
    let (top, bot) = (y - h / 2.0, y + h / 2.0);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    if left_in {
        pts.push((x + notch, top));
        pts.push((x, y));
        pts.push((x + notch, bot));
    } else {
        pts.push((x, top));
        pts.push((x, bot));
    }
    if right_in {
        pts.push((x + w - notch, bot));
        pts.push((x + w, y));
        pts.push((x + w - notch, top));
    } else {
        pts.push((x + w, bot));
        pts.push((x + w, top));
    }
    let _ = write!(
        s,
        r#"<g data-primitive="port" data-kind="global"{}{}><polygon points="{}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/>"#,
        uuid_attr(&oid(&p.uuid, "p", p.at)),
        override_attr(&p.text_color, ctx.class("label_global")),
        points_attr(&pts),
        c(units::sch_line_width_mm(1))
    );
    // Alignment 1 is left, 2 right, anything else centred in the body.
    let (justify, at) = match p.alignment {
        1 => (3, Pt { x: p.at.x + sch::UNIT, y: p.at.y }),
        2 => (5, Pt { x: p.at.x + (p.width - 1) * sch::UNIT, y: p.at.y }),
        _ => (4, Pt { x: p.at.x + p.width * sch::UNIT / 2, y: p.at.y }),
    };
    emit_text(s, ctx, &p.name, at, p.font, justify, 0, &p.text_color, ctx.class("label_global"));
    s.push_str("</g>");
}

/// A label, text frame or note the designer placed on the sheet.
fn emit_free_text(s: &mut String, ctx: &Ctx, t: &SchText) {
    let text = ctx.specials.expand(&t.text);
    if text.is_empty() {
        return;
    }
    let _ = write!(s, r#"<g data-primitive="text"{}>"#, uuid_attr(&oid(&t.uuid, "t", t.at)));
    if let Some(corner) = t.corner {
        let (x1, y1) = ctx.xy(t.at);
        let (x2, y2) = ctx.xy(corner);
        let (x, y) = (x1.min(x2), y1.min(y2));
        let (bw, bh) = ((x2 - x1).abs(), (y2 - y1).abs());
        if t.show_border || t.fill.is_some() {
            let fill = t.fill.clone().unwrap_or_else(|| "none".into());
            let stroke = if t.show_border { "#000000" } else { "none" };
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/>"#,
                c(x), c(y), c(bw), c(bh), c(units::sch_line_width_mm(1))
            );
        }
        // A frame lays its lines out from the top of the box, breaking on the
        // file's own newlines and then on the box width when the frame asks to
        // wrap — which every frame in the corpus does. The width is estimated
        // from Times' character classes rather than measured, so a break can
        // land a word out; the alternative was a 3468-character disclaimer on
        // one line running off the page.
        let (_, size, _, _) = ctx.font(t.font);
        let inner_x = match t.justify % 3 {
            0 => x,
            1 => x + bw / 2.0,
            _ => x + bw,
        };
        // `~1` is Altium's own paragraph break inside a frame, and it is the
        // ONLY break the motherboard's disclaimer has — 3468 characters with no
        // newline in them and twenty tildes.
        let source = text.replace("~1", "
");
        let lines: Vec<String> = if t.word_wrap && bw > 0.0 {
            source.lines().flat_map(|l| wrap(l, bw / size)).collect()
        } else {
            source.lines().map(str::to_string).collect()
        };
        for (i, line) in lines.iter().enumerate() {
            let at = px_to_pt(ctx, inner_x, y + size * (i as f64 + 0.9));
            let justify = mirror_justify(t.justify % 3, t.mirrored);
            emit_text(s, ctx, line, at, t.font, justify, t.orientation, &t.color, ctx.class("note"));
        }
    } else {
        let justify = mirror_justify(t.justify, t.mirrored);
        emit_text(s, ctx, &text, t.at, t.font, justify, t.orientation, &t.color, ctx.class("note"));
    }
    s.push_str("</g>");
}

/// Break one line to a box `ems` wide, on spaces.
///
/// Greedy, which is what Altium does, and it never drops a word: a single word
/// too long for the box takes a line of its own and overruns rather than being
/// cut, because a truncated word reads as a different word.
fn wrap(line: &str, ems: f64) -> Vec<String> {
    if ems <= 0.0 || units::text_width_em(line) <= ems {
        return vec![line.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in line.split(' ') {
        if cur.is_empty() {
            cur.push_str(word);
            continue;
        }
        let candidate = units::text_width_em(&cur) + units::text_width_em(" ") + units::text_width_em(word);
        if candidate <= ems {
            cur.push(' ');
            cur.push_str(word);
        } else {
            out.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Running min/max of the drawn extent.
struct Extent {
    minx: f64,
    miny: f64,
    maxx: f64,
    maxy: f64,
}

impl Extent {
    fn new() -> Extent {
        Extent { minx: f64::MAX, miny: f64::MAX, maxx: f64::MIN, maxy: f64::MIN }
    }
    fn add(&mut self, x: f64, y: f64) {
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }
}

/// Every anchor point the sheet draws, for the view box.
fn every_point(doc: &SchDoc) -> Vec<Pt> {
    let mut out = Vec::new();
    for w in doc.wires.iter().chain(&doc.buses) {
        out.extend(w.pts.iter().copied());
    }
    for j in &doc.junctions {
        out.push(j.at);
    }
    for l in doc.net_labels.iter().filter(|l| !l.text.trim().is_empty()) {
        out.push(l.at);
    }
    for p in &doc.power_ports {
        out.push(p.at);
    }
    for p in &doc.ports {
        out.extend(p.terminals());
    }
    for t in &doc.texts {
        out.push(t.at);
        out.extend(t.corner);
    }
    for i in &doc.images {
        out.push(i.min);
        out.push(i.max);
    }
    for g in doc.graphics.iter().chain(&doc.template_graphics) {
        out.extend(graphic_points(g));
    }
    for sym in &doc.sheet_symbols {
        out.push(sym.at);
        out.push(Pt {
            x: sym.at.x + sym.xsize * sch::UNIT,
            y: sym.at.y - sym.ysize * sch::UNIT,
        });
    }
    for comp in &doc.components {
        if let Some((min, max)) = comp.bbox {
            out.push(min);
            out.push(max);
        }
        for g in &comp.graphics {
            out.extend(graphic_points(g));
        }
    }
    out
}

/// A graphic's first point — the origin its `oid` fallback is keyed on, so the
/// SVG group and the geometry row agree on an unnamed shape's handle.
pub(crate) fn graphic_origin(g: &Graphic) -> Pt {
    graphic_points(g).first().copied().unwrap_or_default()
}

pub(crate) fn graphic_points(g: &Graphic) -> Vec<Pt> {
    match &g.shape {
        GShape::Line { a, b } => vec![*a, *b],
        GShape::Polyline { pts, .. } | GShape::Polygon { pts } => pts.clone(),
        GShape::Rect { min, max } => vec![*min, *max],
        GShape::Ellipse { c, rx, ry } | GShape::Arc { c, rx, ry, .. } => {
            vec![Pt { x: c.x - rx, y: c.y - ry }, Pt { x: c.x + rx, y: c.y + ry }]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_altium::sch::{Component, Junction, NetLabel, SheetProps, TextKind, Wire};

    const U: i64 = sch::UNIT;

    fn sheet(doc: SchDoc) -> String {
        let params = BTreeMap::new();
        let ctx = SheetCtx {
            number: 2,
            total: 7,
            sheet_path: "/",
            file_name: "Sheet.SchDoc",
            full_path: "D:/x/Sheet.SchDoc",
            project_name: "Proj",
            project_params: &params,
        };
        let pal = palette(&[&doc]);
        render_sheet(&doc, &ctx, &pal)
    }

    fn a4() -> SheetProps {
        SheetProps {
            width: 1150 * U,
            height: 760 * U,
            fonts: vec!["Times New Roman".into()],
            font_sizes: vec![10],
            font_bold: vec![false],
            font_italic: vec![false],
            border_on: true,
            ..Default::default()
        }
    }

    /// The page is the Y-flip anchor, so a primitive's coordinates do not move
    /// because another primitive did — the same rule the board follows (D2.1).
    #[test]
    fn the_y_flip_is_about_the_page_not_the_content() {
        let doc = SchDoc {
            sheet: a4(),
            wires: vec![Wire {
                pts: vec![Pt { x: 100 * U, y: 760 * U }, Pt { x: 200 * U, y: 0 }],
                uuid: "w".into(),
                width: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let svg = sheet(doc);
        // 760 units up is the top of the page (y = 0); 0 is the bottom.
        assert!(svg.contains(r#"points="25.4,0 50.8,193.04""#), "{svg}");
    }

    /// The addressing contract the viewer's cross-probe indexes are keyed on.
    #[test]
    fn every_object_carries_its_uuid_and_primitive() {
        let doc = SchDoc {
            sheet: a4(),
            wires: vec![Wire { pts: vec![Pt { x: 0, y: 0 }, Pt { x: U, y: 0 }], uuid: "w1".into(), ..Default::default() }],
            junctions: vec![Junction { at: Pt { x: 0, y: 0 }, uuid: "j1".into(), ..Default::default() }],
            net_labels: vec![NetLabel { at: Pt { x: 0, y: 0 }, text: "NET".into(), uuid: "l1".into(), ..Default::default() }],
            components: vec![Component {
                designator: "R1".into(),
                designator_uuid: "d1".into(),
                designator_at: Pt { x: U, y: U },
                uuid: "c1".into(),
                current_part_id: 1,
                pins: vec![Pin {
                    number: "1".into(),
                    name: "A".into(),
                    conglomerate: 32 | sch::PIN_SHOW_NAME_BIT | sch::PIN_SHOW_DESIGNATOR_BIT,
                    length: 10,
                    part_id: 1,
                    uuid: "p1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let svg = sheet(doc);
        assert!(svg.contains(r#"data-primitive="wire" data-uuid="w1""#), "{svg}");
        assert!(svg.contains(r#"data-primitive="junction" data-uuid="j1""#));
        assert!(svg.contains(r#"data-primitive="label" data-uuid="l1""#));
        assert!(svg.contains(r#"data-primitive="symbol" data-uuid="c1" data-ref="R1""#));
        assert!(svg.contains(r#"data-primitive="pin" data-uuid="p1" data-designator="R1" data-pin="1""#));
        assert!(svg.contains(">R1<"), "the designator is drawn");
        assert!(svg.contains(">A<"), "the pin name is drawn");
        assert!(svg.contains(">1<"), "the pin number is drawn");
    }

    /// A hidden pin is connected without being drawn, and an object with nothing
    /// to draw is not a primitive. Confirmed against Altium's own renderer: on
    /// the corpus sheet where every pin is hidden, it draws none.
    #[test]
    fn a_hidden_pin_is_not_a_primitive() {
        let doc = SchDoc {
            sheet: a4(),
            components: vec![Component {
                designator: "U1".into(),
                uuid: "c".into(),
                current_part_id: 1,
                pins: vec![Pin {
                    number: "8".into(),
                    name: "VDD".into(),
                    conglomerate: 32 | sch::PIN_HIDDEN_BIT,
                    length: 10,
                    part_id: 1,
                    uuid: "p".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let svg = sheet(doc);
        assert!(!svg.contains(r#"data-primitive="pin""#), "no group at all: {svg}");
        assert!(!svg.contains(">VDD<"), "a hidden pin draws no name: {svg}");
    }

    /// Only the placed part of a multi-part symbol is on the sheet; drawing the
    /// others stacks another part's body on this one.
    #[test]
    fn only_the_placed_part_and_mode_are_drawn() {
        let g = |part: i64, mode: i64, uuid: &str| Graphic {
            shape: GShape::Rect { min: Pt { x: 0, y: 0 }, max: Pt { x: U, y: U } },
            color: "#800000".into(),
            fill: None,
            width: 1,
            style: 0,
            part_id: part,
            display_mode: mode,
            uuid: uuid.into(),
        };
        let doc = SchDoc {
            sheet: a4(),
            components: vec![Component {
                designator: "U1".into(),
                uuid: "c".into(),
                current_part_id: 2,
                display_mode: 0,
                graphics: vec![g(1, 0, "a"), g(2, 0, "b"), g(2, 1, "c")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let svg = sheet(doc);
        // Part 2 mode 0 is one rectangle; the other two must not be drawn.
        assert_eq!(svg.matches("<rect").count(), 2, "page border + one body: {svg}");
    }

    /// The class default comes from the document, and only an object that
    /// disagrees with it bakes its own colour.
    #[test]
    fn the_palette_is_the_documents_own_modal_colour() {
        let w = |color: &str, uuid: &str| Wire {
            pts: vec![Pt { x: 0, y: 0 }, Pt { x: U, y: 0 }],
            uuid: uuid.into(),
            color: color.into(),
            width: 1,
        };
        let doc = SchDoc {
            sheet: a4(),
            wires: vec![w("#000080", "a"), w("#000080", "b"), w("#FF0000", "c")],
            ..Default::default()
        };
        let pal = palette(&[&doc]);
        assert_eq!(pal.get("wire").map(String::as_str), Some("#000080"));
        let svg = sheet(doc);
        assert!(!svg.contains(r#"data-uuid="a" data-color"#), "the majority is not an override");
        assert!(svg.contains(r##"data-uuid="c" data-color="#FF0000""##), "{svg}");
    }

    /// An unresolved `=Name` stays visible: a blank title-block cell hides that
    /// the design never set the field.
    #[test]
    fn special_strings_resolve_or_stay_literal() {
        let doc = SchDoc {
            sheet: a4(),
            texts: vec![
                SchText {
                    word_wrap: true,                    kind: TextKind::Label,
                    at: Pt { x: U, y: U },
                    corner: None,
                    text: "=SheetNumber".into(),
                    color: "#000000".into(),
                    fill: None,
                    font: 1,
                    justify: 0,
                    orientation: 0,
                    mirrored: false,
                    show_border: false,
                    template: true,
                    uuid: "t1".into(),
                },
                SchText {
                    word_wrap: true,                    kind: TextKind::Label,
                    at: Pt { x: U, y: 2 * U },
                    corner: None,
                    text: "=PRJ_Customer".into(),
                    color: "#000000".into(),
                    fill: None,
                    font: 1,
                    justify: 0,
                    orientation: 0,
                    mirrored: false,
                    show_border: false,
                    template: true,
                    uuid: "t2".into(),
                },
            ],
            ..Default::default()
        };
        let svg = sheet(doc);
        assert!(svg.contains(">2<"), "the sheet number resolves: {svg}");
        assert!(svg.contains(">=PRJ_Customer<"), "an unset field says so: {svg}");
    }

    /// An unbalanced group does not fail: the browser reparents the rest of the
    /// sheet under whatever was left open, so a whole sheet quietly inherits one
    /// object's colour. This covers every emitter at once, which is why it
    /// builds a document with one of each.
    #[test]
    fn every_group_the_renderer_opens_is_closed() {
        let doc = SchDoc {
            sheet: a4(),
            wires: vec![Wire { pts: vec![Pt { x: 0, y: 0 }, Pt { x: U, y: 0 }], uuid: "w".into(), ..Default::default() }],
            buses: vec![Wire { pts: vec![Pt { x: 0, y: U }, Pt { x: U, y: U }], uuid: "b".into(), ..Default::default() }],
            junctions: vec![Junction { at: Pt { x: 0, y: 0 }, uuid: "j".into(), ..Default::default() }],
            net_labels: vec![NetLabel { at: Pt { x: 0, y: 0 }, text: "N".into(), uuid: "l".into(), ..Default::default() }],
            no_ercs: vec![sch::NoErc { at: Pt { x: U, y: U }, uuid: "x".into(), ..Default::default() }],
            ports: vec![Port { at: Pt { x: 0, y: 2 * U }, width: 40, name: "P".into(), io_type: 3, uuid: "p".into(), ..Default::default() }],
            power_ports: vec![PowerPort { at: Pt { x: U, y: 3 * U }, text: "VCC".into(), style: 4, show_net_name: true, uuid: "pp".into(), ..Default::default() }],
            sheet_symbols: vec![SheetSymbol {
                at: Pt { x: 10 * U, y: 30 * U },
                xsize: 20,
                ysize: 20,
                name: "Sub".into(),
                filename: "sub.SchDoc".into(),
                uuid: "sym".into(),
                // Several entries: the group each one opens was the one that
                // went unclosed, and one sheet in the corpus has 44 of them.
                entries: (0..4)
                    .map(|k| sch::SheetEntry {
                        name: format!("E{k}"),
                        side: k % 4,
                        distance_from_top: 2,
                        uuid: format!("e{k}"),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            param_sets: vec![sch::ParamSet {
                at: Pt { x: 2 * U, y: 2 * U },
                name: "NetClass".into(),
                uuid: "ps".into(),
                parameters: vec![sch::Param { name: "ClassName".into(), text: "HV".into(), ..Default::default() }],
            }],
            regions: vec![sch::Region { min: Pt { x: 0, y: 0 }, max: Pt { x: U, y: U }, uuid: "r".into() }],
            blankets: vec![Graphic {
                shape: GShape::Polygon { pts: vec![Pt { x: 0, y: 0 }, Pt { x: U, y: 0 }, Pt { x: U, y: U }] },
                color: "#FF0000".into(),
                fill: None,
                width: 0,
                style: 1,
                part_id: -1,
                display_mode: 0,
                uuid: "bl".into(),
            }],
            texts: vec![SchText {
                    word_wrap: true,                kind: TextKind::Note,
                at: Pt { x: 5 * U, y: 5 * U },
                corner: Some(Pt { x: 15 * U, y: 2 * U }),
                text: "one\ntwo".into(),
                color: "#000000".into(),
                fill: None,
                font: 1,
                justify: 6,
                orientation: 0,
                mirrored: false,
                show_border: true,
                template: false,
                uuid: "t".into(),
            }],
            components: vec![Component {
                designator: "R1".into(),
                designator_uuid: "d".into(),
                designator_at: Pt { x: U, y: U },
                uuid: "c".into(),
                current_part_id: 1,
                graphics: vec![Graphic {
                    shape: GShape::Rect { min: Pt { x: 0, y: 0 }, max: Pt { x: U, y: U } },
                    color: "#800000".into(),
                    fill: None,
                    width: 1,
                    style: 0,
                    part_id: 1,
                    display_mode: 0,
                    uuid: "g".into(),
                }],
                pins: vec![Pin {
                    number: "1".into(),
                    name: "A".into(),
                    conglomerate: 32 | sch::PIN_SHOW_NAME_BIT | sch::PIN_SHOW_DESIGNATOR_BIT,
                    length: 10,
                    part_id: 1,
                    uuid: "pin".into(),
                    ..Default::default()
                }],
                parameters: vec![sch::Param {
                    name: "Value".into(),
                    text: "10k".into(),
                    at: Pt { x: 2 * U, y: U },
                    font: 1,
                    uuid: "pv".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let svg = sheet(doc);
        let opens = svg.matches("<g ").count() + svg.matches("<g>").count();
        let closes = svg.matches("</g>").count();
        assert_eq!(opens, closes, "unbalanced groups in:\n{svg}");
    }

    /// Altium's justification grid, in the SVG's own vocabulary.
    #[test]
    fn justification_maps_onto_the_svg_anchors() {
        assert_eq!(anchor(0), ("start", "alphabetic"), "bottom-left");
        assert_eq!(anchor(4), ("middle", "central"), "centre");
        assert_eq!(anchor(8), ("end", "hanging"), "top-right");
    }

    /// An overbar is a per-character backslash in the file and one decoration
    /// in the output.
    #[test]
    fn an_overbarred_name_loses_its_backslashes() {
        assert_eq!(markup("R\\E\\S\\E\\T\\"), ("RESET".to_string(), true));
        assert_eq!(markup("PLAIN"), ("PLAIN".to_string(), false));
        assert_eq!(markup("C:\\x"), ("C:x".to_string(), false), "a lone slash is not a bar");
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::mirror_justify;

    /// Altium keeps mirrored text readable and reverses which side of its
    /// anchor the string runs to. The corpus mirrors 2683 strings, and drawing
    /// them with the un-mirrored anchor puts a designator across the symbol it
    /// labels instead of beside it.
    #[test]
    fn a_mirrored_text_reverses_its_horizontal_anchor_only() {
        assert_eq!(mirror_justify(0, false), 0, "an un-mirrored text is untouched");
        assert_eq!(mirror_justify(0, true), 2, "left becomes right");
        assert_eq!(mirror_justify(2, true), 0, "right becomes left");
        assert_eq!(mirror_justify(1, true), 1, "centred stays centred");
        // The vertical half is a different axis and does not move.
        assert_eq!(mirror_justify(6, true), 8);
        assert_eq!(mirror_justify(7, true), 7);
    }
}
