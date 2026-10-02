//! Capture pages rendered as sheet SVGs.
//!
//! Same contract as the KiCad and Altium renderers: one lean SVG per sheet
//! instance in millimetres, Y down, where every object carries the
//! `data-uuid` the cross-probe indexes are keyed on and a `data-primitive` the
//! app themes by CSS. The ids are the ones the netlist and the component
//! table already use (`oc:<db id>`, `oc:<part>:<slot>` for a pin), so a click
//! on the sheet resolves through the same indexes as on any other tool's.
//!
//! What is Capture's is the look: the design cache's own symbol artwork, the
//! cached power / port / off-page bodies, Capture's pin decorations and the
//! page border and title block the design stores. Capture keeps colours in
//! the user's preferences rather than in the design, so the palette is its
//! factory default.
//!
//! Capture already works Y down in 10 mil units, so the only transform is the
//! scale and each object's own placement (`Orient::place`).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use eda_parse_orcad::capture::library::Font;
use eda_parse_orcad::capture::page::{Graphic, Orient, Page, PlacedPart};
use eda_parse_orcad::capture::symbol::{DisplayProp, LineStyle, Prim, SymPin, SymbolDef};
use eda_parse_orcad::capture::CaptureDoc;

use super::design::{part_id, resolve};
use super::{mm, SheetInstance};
use crate::design::Design;

/// Padding around the page, in mm.
const PAD: f64 = 2.54;

/// Face for text whose font the design does not name.
const FONT_FALLBACK: &str = "Arial,'Liberation Sans','DejaVu Sans',sans-serif";

/// Capture's factory colours, in the keys the frontend's theme map reads.
pub fn palette() -> BTreeMap<String, String> {
    [
        ("wire", "#000080"),
        ("bus", "#000080"),
        ("junction", "#000080"),
        ("no_connect", "#000080"),
        ("label_local", "#000000"),
        ("label_global", "#000000"),
        ("label_hier", "#000000"),
        ("note", "#000000"),
        ("sheet", "#800000"),
        ("sheet_name", "#000000"),
        ("reference", "#000000"),
        ("value", "#000000"),
        ("fields", "#000000"),
        ("component_outline", "#800000"),
        ("component_body", "#FFFFC0"),
        ("pin", "#800000"),
        ("pin_name", "#000000"),
        ("pin_number", "#000000"),
        ("worksheet", "#000000"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

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
            // Control characters are not XML.
            ch if (ch as u32) < 0x20 && ch != '\t' => {}
            _ => out.push(ch),
        }
    }
    out
}

fn uuid_attr(u: &str) -> String {
    if u.is_empty() {
        String::new()
    } else {
        format!(r#" data-uuid="{}""#, esc(u))
    }
}

fn line_width(l: &LineStyle) -> f64 {
    match l.width {
        1 => 0.3,
        2 => 0.6,
        _ => 0.15,
    }
}

fn dash(l: &LineStyle) -> &'static str {
    match l.style {
        1 => r#" stroke-dasharray="1.2 0.6""#,
        2 => r#" stroke-dasharray="0.3 0.4""#,
        3 => r#" stroke-dasharray="1.2 0.4 0.3 0.4""#,
        4 => r#" stroke-dasharray="1.2 0.4 0.3 0.4 0.3 0.4""#,
        _ => "",
    }
}

/// How a symbol body lands on the page: its placement and the box it is
/// placed by.
#[derive(Clone, Copy)]
struct Placer {
    orient: Orient,
    origin: (i32, i32),
    body: (i32, i32, i32, i32),
}

impl Placer {
    fn pt(&self, p: (i32, i32)) -> (f64, f64) {
        let q = self.orient.place(p, self.origin, self.body);
        (mm(q.0), mm(q.1))
    }
}

fn body_of(s: &SymbolDef) -> (i32, i32, i32, i32) {
    (s.bbox.0 as i32, s.bbox.1 as i32, s.bbox.2 as i32, s.bbox.3 as i32)
}

struct Ctx<'a> {
    fonts: &'a [Font],
}

impl Ctx<'_> {
    /// Character height in mm of a 1-based font index (0 = the default).
    fn font_mm(&self, idx: u16) -> f64 {
        let f = self.fonts.get((idx as usize).max(1) - 1).or_else(|| self.fonts.first());
        // A negative LOGFONT height is the character height, in the page's
        // logical unit — Capture's 10 mil database unit.
        f.map(|f| mm(f.height.abs().max(1))).unwrap_or(mm(9))
    }

    /// Average character width in mm, for wrapping.
    fn char_mm(&self, idx: u16) -> f64 {
        let f = self.fonts.get((idx as usize).max(1) - 1).or_else(|| self.fonts.first());
        match f {
            Some(f) if f.width != 0 => mm(f.width.abs()),
            _ => self.font_mm(idx) * 0.5,
        }
    }

    fn font_face(&self, idx: u16) -> Option<&str> {
        self.fonts.get((idx as usize).max(1) - 1).map(|f| f.face.as_str()).filter(|f| !f.is_empty())
    }
}

/// A text run: `at` is the top-left of its box before rotation, `turns`
/// counter-clockwise quarter turns about that corner.
fn emit_text(s: &mut String, ctx: &Ctx, text: &str, at: (f64, f64), font: u16, turns: u8, anchor: &str, fill: &str) {
    if text.trim().is_empty() {
        return;
    }
    let size = ctx.font_mm(font);
    // Baseline sits about one ascent below the box top.
    let (x, y) = at;
    let rot = match turns & 3 {
        0 => String::new(),
        t => format!(r#" transform="rotate({} {} {})""#, -90 * t as i32, c(x), c(y)),
    };
    let face = ctx.font_face(font).map(|f| format!(r#" font-family="'{}',{FONT_FALLBACK}""#, esc(f))).unwrap_or_default();
    let _ = write!(
        s,
        r#"<text x="{}" y="{}" font-size="{}" text-anchor="{anchor}" fill="{fill}" stroke="none"{face}{rot}>{}</text>"#,
        c(x),
        c(y + size * 0.8),
        c(size),
        esc(text)
    );
}

/// Break a note into lines: at its own line breaks, then greedily at word
/// boundaries to fit `width` mm. A box too narrow for one word keeps the word.
fn wrap(text: &str, width: f64, char_mm: f64) -> Vec<String> {
    let max = if width > char_mm { (width / char_mm.max(1e-3)).floor() as usize } else { usize::MAX };
    let mut out = Vec::new();
    for para in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        let mut cur = String::new();
        for word in para.split(' ') {
            let need = if cur.is_empty() { word.chars().count() } else { cur.chars().count() + 1 + word.chars().count() };
            if need > max && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur.push_str(word);
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
        }
        out.push(cur);
    }
    out
}

fn ellipse_point(cx: f64, cy: f64, rx: f64, ry: f64, ray: (f64, f64)) -> (f64, f64, f64) {
    // Parameter angle of the ray through (ray) from the centre, on screen Y
    // down measured counter-clockwise.
    let t = (-(ray.1 - cy) / ry.max(1e-9)).atan2((ray.0 - cx) / rx.max(1e-9));
    (cx + rx * t.cos(), cy - ry * t.sin(), t)
}

/// Draw symbol primitives through a placement.
fn emit_prims(s: &mut String, ctx: &Ctx, prims: &[Prim], pl: &Placer, stroke: &str, fill_color: &str) {
    for p in prims {
        match p {
            Prim::Line { a, b, line } => {
                let (x1, y1) = pl.pt(*a);
                let (x2, y2) = pl.pt(*b);
                let _ = write!(
                    s,
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}"{}/>"#,
                    c(x1), c(y1), c(x2), c(y2), c(line_width(line)), dash(line)
                );
            }
            Prim::Rect { a, b, line, fill } => {
                let (x1, y1) = pl.pt(*a);
                let (x2, y2) = pl.pt(*b);
                let f = if fill.is_solid() { fill_color } else { "none" };
                let _ = write!(
                    s,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{f}" stroke="{stroke}" stroke-width="{}"{}/>"#,
                    c(x1.min(x2)), c(y1.min(y2)), c((x2 - x1).abs()), c((y2 - y1).abs()), c(line_width(line)), dash(line)
                );
            }
            Prim::Ellipse { a, b, line, fill } => {
                let (x1, y1) = pl.pt(*a);
                let (x2, y2) = pl.pt(*b);
                let f = if fill.is_solid() { fill_color } else { "none" };
                let _ = write!(
                    s,
                    r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="{f}" stroke="{stroke}" stroke-width="{}"{}/>"#,
                    c((x1 + x2) / 2.0), c((y1 + y2) / 2.0), c((x2 - x1).abs() / 2.0), c((y2 - y1).abs() / 2.0), c(line_width(line)), dash(line)
                );
            }
            Prim::Arc { a, b, start, end, line } => {
                // Work in symbol space, where the arc runs counter-clockwise
                // on screen, then place the end points. A mirror reverses the
                // sense; a quarter turn swaps the radii.
                let (cx, cy) = ((a.0 + b.0) as f64 / 2.0, (a.1 + b.1) as f64 / 2.0);
                let (rx, ry) = ((b.0 - a.0).abs() as f64 / 2.0, (b.1 - a.1).abs() as f64 / 2.0);
                let (sx, sy, ts) = ellipse_point(cx, cy, rx, ry, (start.0 as f64, start.1 as f64));
                let (ex, ey, te) = ellipse_point(cx, cy, rx, ry, (end.0 as f64, end.1 as f64));
                let span = (te - ts).rem_euclid(std::f64::consts::TAU);
                let full = span < 1e-6;
                let place = |x: f64, y: f64| pl.pt((x.round() as i32, y.round() as i32));
                let (psx, psy) = place(sx, sy);
                let (pex, pey) = place(ex, ey);
                let (prx, pry) = if pl.orient.turns % 2 == 1 { (ry, rx) } else { (rx, ry) };
                let (prx, pry) = (mm(1) * prx, mm(1) * pry);
                if full {
                    let (pcx, pcy) = place(cx, cy);
                    let _ = write!(
                        s,
                        r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="none" stroke="{stroke}" stroke-width="{}"/>"#,
                        c(pcx), c(pcy), c(prx), c(pry), c(line_width(line))
                    );
                } else {
                    // Counter-clockwise on a Y-down screen is SVG's sweep 0.
                    let sweep = if pl.orient.mirror { 1 } else { 0 };
                    let large = if span > std::f64::consts::PI { 1 } else { 0 };
                    let _ = write!(
                        s,
                        r#"<path d="M {} {} A {} {} 0 {large} {sweep} {} {}" fill="none" stroke="{stroke}" stroke-width="{}"{}/>"#,
                        c(psx), c(psy), c(prx), c(pry), c(pex), c(pey), c(line_width(line)), dash(line)
                    );
                }
            }
            Prim::Polyline { points, line } | Prim::Polygon { points, line, .. } => {
                if points.len() < 2 {
                    continue;
                }
                let pts: Vec<String> = points.iter().map(|p| {
                    let (x, y) = pl.pt(*p);
                    format!("{},{}", c(x), c(y))
                }).collect();
                let (tag, f) = match p {
                    Prim::Polygon { fill, .. } => ("polygon", if fill.is_solid() { fill_color } else { "none" }),
                    _ => ("polyline", "none"),
                };
                let _ = write!(
                    s,
                    r#"<{tag} points="{}" fill="{f}" stroke="{stroke}" stroke-width="{}"{}/>"#,
                    pts.join(" "), c(line_width(line)), dash(line)
                );
            }
            Prim::Bezier { points, line } => {
                if points.len() < 4 {
                    continue;
                }
                let p0 = pl.pt(points[0]);
                let mut d = format!("M {} {}", c(p0.0), c(p0.1));
                for ch in points[1..].chunks_exact(3) {
                    let (a, b, e) = (pl.pt(ch[0]), pl.pt(ch[1]), pl.pt(ch[2]));
                    let _ = write!(d, " C {} {} {} {} {} {}", c(a.0), c(a.1), c(b.0), c(b.1), c(e.0), c(e.1));
                }
                let _ = write!(s, r#"<path d="{d}" fill="none" stroke="{stroke}" stroke-width="{}"{}/>"#, c(line_width(line)), dash(line));
            }
            Prim::Text { a, b, font, text, .. } => {
                // The text box's top-left after placement, turned with the
                // symbol. Capture word-wraps a note inside its box, so the
                // lines are broken here at the box's width.
                let (x1, y1) = pl.pt(*a);
                let (x2, y2) = pl.pt(*b);
                let vertical = pl.orient.turns % 2 == 1;
                let at = (x1.min(x2), y1.min(y2));
                let at = if vertical { (at.0, y1.max(y2)) } else { at };
                let width = if vertical { (y2 - y1).abs() } else { (x2 - x1).abs() };
                let size = ctx.font_mm(*font);
                for (i, line) in wrap(text, width, ctx.char_mm(*font)).iter().enumerate() {
                    let d = size * 1.15 * i as f64;
                    let p = if vertical { (at.0 + d, at.1) } else { (at.0, at.1 + d) };
                    emit_text(s, ctx, line, p, *font, pl.orient.turns, "start", stroke);
                }
            }
            Prim::Image { a, b, .. } => {
                // Embedded pictures are not decoded; their frame shows where
                // one sits.
                let (x1, y1) = pl.pt(*a);
                let (x2, y2) = pl.pt(*b);
                let _ = write!(
                    s,
                    r##"<rect data-kind="image" x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#808080" stroke-width="0.1" stroke-dasharray="0.6 0.4"/>"##,
                    c(x1.min(x2)), c(y1.min(y2)), c((x2 - x1).abs()), c((y2 - y1).abs())
                );
            }
            Prim::Group { at, prims, .. } => {
                // A group's children are relative to its own position.
                let shifted: Vec<Prim> = prims.iter().map(|p| shift(p, *at)).collect();
                emit_prims(s, ctx, &shifted, pl, stroke, fill_color);
            }
        }
    }
}

fn shift(p: &Prim, d: (i32, i32)) -> Prim {
    let m = |q: &(i32, i32)| (q.0 + d.0, q.1 + d.1);
    let mut p = p.clone();
    match &mut p {
        Prim::Rect { a, b, .. } | Prim::Ellipse { a, b, .. } | Prim::Line { a, b, .. } | Prim::Image { a, b, .. } => {
            *a = m(a);
            *b = m(b);
        }
        Prim::Arc { a, b, start, end, .. } => {
            *a = m(a);
            *b = m(b);
            *start = m(start);
            *end = m(end);
        }
        Prim::Polygon { points, .. } | Prim::Polyline { points, .. } | Prim::Bezier { points, .. } => {
            for q in points.iter_mut() {
                *q = m(q);
            }
        }
        Prim::Text { a, b, origin, .. } => {
            *a = m(a);
            *b = m(b);
            *origin = m(origin);
        }
        Prim::Group { at, .. } => *at = m(at),
    }
    p
}

/// A pin: its leg, Capture's dot and clock decorations, and the name and
/// number when the part shows them.
#[allow(clippy::too_many_arguments)]
fn emit_pin(s: &mut String, ctx: &Ctx, pin: &SymPin, pl: &Placer, uuid: &str, designator: &str, number: Option<&str>, names: bool, numbers: bool) {
    if pin.hidden() {
        return;
    }
    let pal = palette();
    let (hx, hy) = pl.pt(pin.hot);
    let (sx, sy) = pl.pt(pin.start);
    let _ = write!(
        s,
        r#"<g data-primitive="pin"{} data-designator="{}" data-pin="{}">"#,
        uuid_attr(uuid),
        esc(designator),
        esc(number.unwrap_or(&pin.name))
    );
    let stroke = &pal["pin"];
    let len = (sx - hx).hypot(sy - hy);
    // Unit vector from the connection point into the body.
    let (dx, dy) = if len > 1e-9 { ((sx - hx) / len, (sy - hy) / len) } else { (0.0, 0.0) };
    let dot_r = mm(2);
    let leg_end = if pin.dot() && len > dot_r * 2.0 { (sx - dx * dot_r * 2.0, sy - dy * dot_r * 2.0) } else { (sx, sy) };
    let w = if pin.bus { 0.45 } else { 0.15 };
    let _ = write!(
        s,
        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}"/>"#,
        c(hx), c(hy), c(leg_end.0), c(leg_end.1), c(w)
    );
    if pin.dot() && len > dot_r * 2.0 {
        let _ = write!(
            s,
            r#"<circle cx="{}" cy="{}" r="{}" fill="none" stroke="{stroke}" stroke-width="0.15"/>"#,
            c(sx - dx * dot_r), c(sy - dy * dot_r), c(dot_r)
        );
    }
    if pin.clock() {
        // A small wedge inside the body edge, pointing in.
        let (px, py) = (-dy, dx);
        let k = mm(3);
        let _ = write!(
            s,
            r#"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{stroke}" stroke-width="0.15"/>"#,
            c(sx + px * k), c(sy + py * k), c(sx + dx * k), c(sy + dy * k), c(sx - px * k), c(sy - py * k)
        );
    }
    let vertical = dx.abs() < dy.abs();
    let size = ctx.font_mm(0);
    if names && !pin.name.is_empty() {
        // Inside the body, just past the pin's inner end.
        let gap = mm(3);
        let (nx, ny) = (sx + dx * gap, sy + dy * gap);
        let anchor = if (vertical && dy < 0.0) || (!vertical && dx < 0.0) { "end" } else { "start" };
        let at = if vertical { (nx + size * 0.4, ny) } else { (nx, ny - size * 0.4) };
        let _ = write!(s, r#"<g data-kind="pin-name">"#);
        emit_text(s, ctx, &pin.name, at, 0, if vertical { 1 } else { 0 }, anchor, &pal["pin_name"]);
        s.push_str("</g>");
    }
    if numbers {
        if let Some(n) = number.filter(|n| !n.is_empty()) {
            // Above the leg, centred on it.
            let (mx, my) = ((hx + sx) / 2.0, (hy + sy) / 2.0);
            let at = if vertical { (mx - size * 0.2, my) } else { (mx, my - size * 1.1) };
            let _ = write!(s, r#"<g data-kind="pin-number">"#);
            emit_text(s, ctx, n, at, 0, if vertical { 1 } else { 0 }, "middle", &pal["pin_number"]);
            s.push_str("</g>");
        }
    }
    s.push_str("</g>");
}

/// Display text for one displayed property.
fn display_text(dp: &DisplayProp, value: &str) -> Option<String> {
    // The mode byte sits in the high half of the stored word: 1 value only,
    // 2 name and value, 3 name only, 4 both when there is a value.
    let mode = if dp.mode > 0xFF { dp.mode >> 8 } else { dp.mode };
    match mode {
        1 => Some(value.to_string()),
        2 => Some(format!("{} = {value}", dp.name)),
        3 => Some(dp.name.clone()),
        4 if !value.is_empty() => Some(format!("{} = {value}", dp.name)),
        _ => None,
    }
    .filter(|t| !t.trim().is_empty())
}

fn emit_part(s: &mut String, ctx: &Ctx, doc: &CaptureDoc, inst: &SheetInstance, part: &PlacedPart) {
    let pal = palette();
    let r = resolve(doc, inst, part);
    let shown_ref = format!("{}{}", r.designator, r.section);
    let _ = write!(
        s,
        r#"<g data-primitive="symbol"{} data-ref="{}">"#,
        uuid_attr(&part_id(part.db_id)),
        esc(&r.designator)
    );
    let Some(sym) = r.symbol else {
        // No cached body: the stored box shows where the part sits.
        let (x1, y1, x2, y2) = part.bbox;
        let _ = write!(
            s,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{}" stroke-width="0.15" stroke-dasharray="0.6 0.4"/>"#,
            c(mm(x1.min(x2))), c(mm(y1.min(y2))), c(mm((x2 - x1).abs())), c(mm((y2 - y1).abs())), pal["component_outline"]
        );
        emit_text(s, ctx, &shown_ref, (mm(x1.min(x2)), mm(y1.min(y2)) - ctx.font_mm(0) * 1.2), 0, 0, "start", &pal["reference"]);
        s.push_str("</g>");
        return;
    };
    let pl = Placer { orient: part.orient, origin: part.pos, body: body_of(sym) };
    emit_prims(s, ctx, &sym.prims, &pl, &pal["component_outline"], &pal["component_body"]);
    let general = sym.general.clone().unwrap_or_default();
    let names = sym.general.as_ref().map(|g| g.pin_names_visible()).unwrap_or(true);
    let numbers = sym.general.as_ref().map(|g| g.pin_numbers_visible()).unwrap_or(true);
    let _ = general;
    for pin in &sym.pins {
        let uuid = format!("{}:{}", part_id(part.db_id), pin.slot);
        let number = r.pin_number(pin.slot);
        emit_pin(s, ctx, pin, &pl, &uuid, &r.designator, number.as_deref(), names, numbers);
    }
    for dp in &part.display {
        let (value, key, kind) = if dp.name.eq_ignore_ascii_case("Part Reference") || dp.name.eq_ignore_ascii_case("Reference") {
            (shown_ref.clone(), "reference", "designator")
        } else if dp.name.eq_ignore_ascii_case("Value") {
            (r.prop("Value").map(str::to_string).unwrap_or_else(|| part.value.clone()), "value", "field")
        } else {
            (r.prop(&dp.name).unwrap_or("").to_string(), "fields", "field")
        };
        let Some(text) = display_text(dp, &value) else { continue };
        let at = (mm(part.pos.0 + dp.x as i32), mm(part.pos.1 + dp.y as i32));
        let _ = write!(s, r#"<g data-primitive="text" data-kind="{kind}">"#);
        emit_text(s, ctx, &text, at, dp.font, dp.rotation, "start", &pal[key]);
        s.push_str("</g>");
    }
    s.push_str("</g>");
}

/// Name text beside a power / port / off-page body, on the side away from
/// where the wire meets it. The design stores no position for it.
fn emit_name_beside(s: &mut String, ctx: &Ctx, name: &str, body: (f64, f64, f64, f64), hot: Option<(f64, f64)>, fill: &str) {
    let (x1, y1, x2, y2) = body;
    let (cx, cy) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
    let size = ctx.font_mm(0);
    let (dx, dy) = match hot {
        Some((hx, hy)) => (cx - hx, cy - hy),
        None => (0.0, -1.0),
    };
    let gap = 0.5;
    if dx.abs() > dy.abs() {
        // Horizontal: text continues past the far side.
        if dx > 0.0 {
            emit_text(s, ctx, name, (x2 + gap, cy - size * 0.5), 0, 0, "start", fill);
        } else {
            emit_text(s, ctx, name, (x1 - gap, cy - size * 0.5), 0, 0, "end", fill);
        }
    } else if dy < 0.0 {
        emit_text(s, ctx, name, (cx, y1 - gap - size), 0, 0, "middle", fill);
    } else {
        emit_text(s, ctx, name, (cx, y2 + gap), 0, 0, "middle", fill);
    }
}

fn emit_graphic_symbol(s: &mut String, ctx: &Ctx, doc: &CaptureDoc, g: &Graphic, prim: &str, uuid: &str, name: Option<&str>) {
    let pal = palette();
    let _ = write!(s, r#"<g data-primitive="{prim}"{}>"#, uuid_attr(uuid));
    let sym = g.body.as_ref().filter(|b| !b.prims.is_empty() || !b.pins.is_empty()).or_else(|| doc.cache.symbol(&g.cache_name));
    let mut hot = None;
    let mut extent = None;
    if let Some(sym) = sym {
        let pl = Placer { orient: g.orient, origin: g.origin(), body: body_of(sym) };
        emit_prims(s, ctx, &sym.prims, &pl, &pal["component_outline"], &pal["component_body"]);
        hot = sym.pins.first().map(|p| pl.pt(p.hot));
        let (a, b) = (pl.pt((sym.bbox.0 as i32, sym.bbox.1 as i32)), pl.pt((sym.bbox.2 as i32, sym.bbox.3 as i32)));
        extent = Some((a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)));
    }
    let (x1, y1, x2, y2) = g.bbox;
    let ext = extent.unwrap_or((mm(x1.min(x2)), mm(y1.min(y2)), mm(x1.max(x2)), mm(y1.max(y2))));
    if let Some(n) = name {
        let key = match prim {
            "power-symbol" => "label_hier",
            "hier-label" => "label_hier",
            _ => "label_global",
        };
        emit_name_beside(s, ctx, n, ext, hot, &pal[key]);
    }
    s.push_str("</g>");
}

/// Junction dots where three or more connections meet: wire ends, and a
/// wire end landing on another wire's span.
fn junctions(page: &Page, part_pins: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut count: HashMap<(i32, i32), usize> = HashMap::new();
    for w in page.wires.iter().filter(|w| !w.bus) {
        *count.entry(w.a).or_default() += 1;
        *count.entry(w.b).or_default() += 1;
    }
    for p in part_pins {
        if let Some(n) = count.get_mut(p) {
            *n += 1;
        }
    }
    let on_span = |p: (i32, i32), a: (i32, i32), b: (i32, i32)| {
        if p == a || p == b {
            return false;
        }
        let cross = (b.0 - a.0) as i64 * (p.1 - a.1) as i64 - (b.1 - a.1) as i64 * (p.0 - a.0) as i64;
        cross == 0 && p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
    };
    let ends: Vec<(i32, i32)> = count.keys().copied().collect();
    for e in ends {
        if page.wires.iter().filter(|w| !w.bus).any(|w| on_span(e, w.a, w.b)) {
            *count.entry(e).or_default() += 2;
        }
    }
    let mut out: Vec<(i32, i32)> = count.into_iter().filter(|(_, n)| *n >= 3).map(|(p, _)| p).collect();
    out.sort();
    out
}

fn emit_border(s: &mut String, page: &Page, w: f64, h: f64) {
    let st = &page.settings;
    if !st.border_displayed {
        return;
    }
    let m = 5.0_f64.min(w / 20.0);
    let _ = write!(
        s,
        r##"<g data-primitive="worksheet"><rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#000000" stroke-width="0.25"/>"##,
        c(m), c(m), c(w - 2.0 * m), c(h - 2.0 * m)
    );
    if st.grid_ref_displayed {
        let o = m * 0.5;
        let _ = write!(s, r##"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#000000" stroke-width="0.15"/>"##, c(o), c(o), c(w - 2.0 * o), c(h - 2.0 * o));
        let label = |i: usize, letters: bool, n: usize, ascending: bool| -> String {
            let k = if ascending { i } else { n - 1 - i };
            if letters {
                ((b'A' + (k % 26) as u8) as char).to_string()
            } else {
                (k + 1).to_string()
            }
        };
        let nx = st.horizontal_count.max(1) as usize;
        let ny = st.vertical_count.max(1) as usize;
        let (iw, ih) = (w - 2.0 * m, h - 2.0 * m);
        let fs = (m * 0.5).min(2.5);
        for i in 0..nx {
            let x = m + iw * (i as f64 + 0.5) / nx as f64;
            let t = label(i, st.horizontal_letters, nx, st.horizontal_ascending);
            for y in [o + (m - o) / 2.0, h - o - (m - o) / 2.0] {
                let _ = write!(s, r##"<text x="{}" y="{}" font-size="{}" text-anchor="middle" fill="#000000" stroke="none">{t}</text>"##, c(x), c(y + fs * 0.35), c(fs));
            }
            if i > 0 {
                let xb = m + iw * i as f64 / nx as f64;
                let _ = write!(s, r##"<line x1="{0}" y1="{1}" x2="{0}" y2="{2}" stroke="#000000" stroke-width="0.15"/><line x1="{0}" y1="{3}" x2="{0}" y2="{4}" stroke="#000000" stroke-width="0.15"/>"##, c(xb), c(o), c(m), c(h - m), c(h - o));
            }
        }
        for i in 0..ny {
            let y = m + ih * (i as f64 + 0.5) / ny as f64;
            let t = label(i, st.vertical_letters, ny, st.vertical_ascending);
            for x in [o + (m - o) / 2.0, w - o - (m - o) / 2.0] {
                let _ = write!(s, r##"<text x="{}" y="{}" font-size="{}" text-anchor="middle" fill="#000000" stroke="none">{t}</text>"##, c(x), c(y + fs * 0.35), c(fs));
            }
            if i > 0 {
                let yb = m + ih * i as f64 / ny as f64;
                let _ = write!(s, r##"<line x1="{1}" y1="{0}" x2="{2}" y2="{0}" stroke="#000000" stroke-width="0.15"/><line x1="{3}" y1="{0}" x2="{4}" y2="{0}" stroke="#000000" stroke-width="0.15"/>"##, c(yb), c(o), c(m), c(w - m), c(w - o));
            }
        }
    }
    s.push_str("</g>");
}

/// Values a title block's displayed properties resolve to.
fn title_value(name: &str, g: &Graphic, inst: &SheetInstance, total: i64, page: &Page) -> String {
    let own = g.prop(name).map(str::to_string).filter(|v| !v.is_empty());
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "page number" => inst.info.sheet_number.to_string(),
        "page count" => total.to_string(),
        "page size" => page.size_name.clone(),
        "page create date" | "page modify date" => own.unwrap_or_default(),
        _ => own.unwrap_or_default(),
    }
}

fn emit_title_block(s: &mut String, ctx: &Ctx, doc: &CaptureDoc, inst: &SheetInstance, total: i64, page: &Page, g: &Graphic) {
    let _ = write!(s, r#"<g data-primitive="worksheet" data-kind="title-block"{}>"#, uuid_attr(&part_id(g.db_id)));
    // A title block stores its box as corner and size, not two corners: the
    // size matches the cached body's on every title block in the corpus.
    let origin = (g.bbox.0, g.bbox.1);
    let sym = g.body.as_ref().filter(|b| !b.prims.is_empty()).or_else(|| doc.cache.symbol(&g.cache_name));
    if let Some(sym) = sym {
        let pl = Placer { orient: g.orient, origin, body: body_of(sym) };
        emit_prims(s, ctx, &sym.prims, &pl, "#000000", "none");
    }
    for dp in &g.display {
        let v = title_value(&dp.name, g, inst, total, page);
        let Some(text) = display_text(dp, &v) else { continue };
        let at = (mm(origin.0 + dp.x as i32), mm(origin.1 + dp.y as i32));
        emit_text(s, ctx, &text, at, dp.font, dp.rotation, "start", "#000000");
    }
    s.push_str("</g>");
}

pub fn render_sheet(doc: &CaptureDoc, inst: &SheetInstance, total: i64, _model: &Design) -> String {
    let page = &doc.folders[inst.folder].pages[inst.page];
    let ctx = Ctx { fonts: &doc.lib.fonts };
    let pal = palette();
    let st = &page.settings;
    let unit_mm = if st.metric { 0.001 } else { 0.0254 };
    let (w, h) = ((st.width as f64 * unit_mm).max(10.0), (st.height as f64 * unit_mm).max(10.0));

    // The view is the page plus anything drawn off it, so a stray part is
    // seen in the wrong place rather than not at all.
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f64, 0.0f64, w, h);
    let mut grow = |x: f64, y: f64| {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    };
    for wr in &page.wires {
        grow(mm(wr.a.0), mm(wr.a.1));
        grow(mm(wr.b.0), mm(wr.b.1));
    }
    for p in &page.parts {
        grow(mm(p.bbox.0), mm(p.bbox.1));
        grow(mm(p.bbox.2), mm(p.bbox.3));
    }
    let (vx, vy, vw, vh) = (x0 - PAD, y0 - PAD, x1 - x0 + 2.0 * PAD, y1 - y0 + 2.0 * PAD);

    let mut s = String::with_capacity(64 * 1024);
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" fill="none" stroke="#000000" stroke-width="0.15" font-family="{FONT_FALLBACK}">"##,
        c(vx), c(vy), c(vw), c(vh)
    );
    emit_border(&mut s, page, w, h);
    if st.title_block_displayed || !page.title_blocks.is_empty() {
        for g in &page.title_blocks {
            emit_title_block(&mut s, &ctx, doc, inst, total, page, g);
        }
    }

    // Free page artwork.
    for g in &page.graphics {
        let _ = write!(s, r#"<g data-primitive="graphic"{}>"#, uuid_attr(&part_id(g.db_id)));
        if let Some(b) = &g.body {
            let pl = Placer { orient: g.orient, origin: g.origin(), body: body_of(b) };
            emit_prims(&mut s, &ctx, &b.prims, &pl, "#000000", "none");
        }
        s.push_str("</g>");
    }

    // Buses under wires, wires under everything else.
    for wire in page.wires.iter().filter(|w| w.bus).chain(page.wires.iter().filter(|w| !w.bus)) {
        let prim = if wire.bus { "bus" } else { "wire" };
        let (ax, ay, bx, by) = (mm(wire.a.0), mm(wire.a.1), mm(wire.b.0), mm(wire.b.1));
        let width = if wire.bus { 0.5 } else { 0.15 };
        let _ = write!(
            s,
            r#"<g data-primitive="{prim}"{}><line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round"/></g>"#,
            uuid_attr(&part_id(wire.db_id)), c(ax), c(ay), c(bx), c(by), pal[prim], c(width)
        );
        // Net aliases ride on their wire.
        for (i, al) in wire.aliases.iter().enumerate() {
            let _ = write!(s, r#"<g data-primitive="label"{}>"#, uuid_attr(&format!("{}:a{i}", part_id(wire.db_id))));
            let turns = ((al.rotation & 3) as u8) % 4;
            let size = ctx.font_mm(al.font as u16);
            // The stored point is the text's bottom-left.
            let at = if turns % 2 == 1 { (mm(al.pos.0) - size, mm(al.pos.1)) } else { (mm(al.pos.0), mm(al.pos.1) - size) };
            emit_text(&mut s, &ctx, &al.name, at, al.font as u16, turns, "start", &pal["label_local"]);
            s.push_str("</g>");
        }
    }
    for be in &page.bus_entries {
        let _ = write!(
            s,
            r#"<g data-primitive="bus-entry"><line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="0.15"/></g>"#,
            c(mm(be.a.0)), c(mm(be.a.1)), c(mm(be.b.0)), c(mm(be.b.1)), pal["wire"]
        );
    }

    let pins: Vec<(i32, i32)> = page.parts.iter().flat_map(|p| p.pins.iter().map(|q| q.pos)).collect();
    for (x, y) in junctions(page, &pins) {
        let _ = write!(
            s,
            r#"<g data-primitive="junction"{}><circle cx="{}" cy="{}" r="0.5" fill="{}" stroke="none"/></g>"#,
            uuid_attr(&format!("oc:j:{x}:{y}")), c(mm(x)), c(mm(y)), pal["junction"]
        );
    }

    for part in &page.parts {
        emit_part(&mut s, &ctx, doc, inst, part);
        // No-connect markers on pins the designer closed.
        for p in part.pins.iter().filter(|p| p.no_connect()) {
            let (x, y) = (mm(p.pos.0), mm(p.pos.1));
            let r = 0.75;
            let _ = write!(
                s,
                r#"<g data-primitive="no-connect"{}><line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{nc}" stroke-width="0.15"/><line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{nc}" stroke-width="0.15"/></g>"#,
                uuid_attr(&format!("{}:nc{}", part_id(part.db_id), p.slot())),
                c(x - r), c(y - r), c(x + r), c(y + r), c(x - r), c(y + r), c(x + r), c(y - r),
                nc = pal["no_connect"]
            );
        }
    }

    for b in &page.blocks {
        let (x1, y1, x2, y2) = b.rect;
        let _ = write!(s, r#"<g data-primitive="sheet"{} data-ref="{}">"#, uuid_attr(&part_id(b.db_id)), esc(&b.name));
        let _ = write!(
            s,
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="{}" stroke-width="0.25"/>"#,
            c(mm(x1.min(x2))), c(mm(y1.min(y2))), c(mm((x2 - x1).abs())), c(mm((y2 - y1).abs())), pal["sheet"]
        );
        let size = ctx.font_mm(0);
        let label = if b.reference.is_empty() { b.name.clone() } else { b.reference.clone() };
        let _ = write!(s, r#"<g data-primitive="text" data-kind="sheet-name">"#);
        emit_text(&mut s, &ctx, &label, (mm(x1.min(x2)), mm(y1.min(y2)) - size * 1.2), 0, 0, "start", &pal["sheet_name"]);
        if !b.implementation.is_empty() && b.implementation != label {
            emit_text(&mut s, &ctx, &b.implementation, (mm(x1.min(x2)), mm(y1.max(y2)) + 0.3), 0, 0, "start", &pal["sheet_name"]);
        }
        s.push_str("</g>");
        let (cx, _) = ((mm(x1) + mm(x2)) / 2.0, 0.0);
        for bp in &b.pins {
            let (px, py) = (mm(bp.pos.0), mm(bp.pos.1));
            let inward = if px < cx { 1.0 } else { -1.0 };
            let _ = write!(s, r#"<g data-primitive="sheet-entry"{}>"#, uuid_attr(&format!("{}:{}", part_id(b.db_id), bp.name)));
            let _ = write!(
                s,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}"/>"#,
                c(px), c(py), c(px + inward * 1.27), c(py), pal["sheet"], if bp.bus { "0.45" } else { "0.15" }
            );
            let anchor = if inward > 0.0 { "start" } else { "end" };
            emit_text(&mut s, &ctx, &bp.name, (px + inward * 1.8, py - size * 0.5), 0, 0, anchor, &pal["sheet_name"]);
            s.push_str("</g>");
        }
        s.push_str("</g>");
    }

    for g in &page.globals {
        emit_graphic_symbol(&mut s, &ctx, doc, g, "power-symbol", &part_id(g.db_id), Some(&g.name));
    }
    for g in &page.offpages {
        emit_graphic_symbol(&mut s, &ctx, doc, g, "global-label", &part_id(g.db_id), Some(&g.name));
    }
    for g in &page.ports {
        emit_graphic_symbol(&mut s, &ctx, doc, g, "hier-label", &part_id(g.db_id), Some(&g.name));
    }
    for g in &page.erc {
        emit_graphic_symbol(&mut s, &ctx, doc, g, "no-connect", &part_id(g.db_id), None);
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_wraps_at_its_box_and_keeps_its_own_breaks() {
        assert_eq!(wrap("one two three", 100.0, 1.0), vec!["one two three"]);
        assert_eq!(wrap("one two three", 7.0, 1.0), vec!["one two", "three"]);
        assert_eq!(wrap("a\r\nb", 100.0, 1.0), vec!["a", "b"]);
        // A word wider than the box stays whole.
        assert_eq!(wrap("unbreakable", 3.0, 1.0), vec!["unbreakable"]);
        // No usable width: no wrapping at all.
        assert_eq!(wrap("one two", 0.0, 1.0), vec!["one two"]);
    }

    #[test]
    fn displayed_properties_follow_their_mode() {
        let dp = |mode| DisplayProp { name: "Value".into(), x: 0, y: 0, font: 0, rotation: 0, color: 0, mode };
        assert_eq!(display_text(&dp(0x100), "10k").as_deref(), Some("10k"));
        assert_eq!(display_text(&dp(0x200), "10k").as_deref(), Some("Value = 10k"));
        assert_eq!(display_text(&dp(0x300), "10k").as_deref(), Some("Value"));
        assert_eq!(display_text(&dp(0x400), "").as_deref(), None);
        assert_eq!(display_text(&dp(0), "10k"), None);
        assert_eq!(display_text(&dp(0x100), "  "), None, "blank is not ink");
    }

    #[test]
    fn markup_is_escaped_and_control_characters_dropped() {
        assert_eq!(esc("<A&B>\"\u{1}"), "&lt;A&amp;B&gt;&quot;");
    }
}
