//! Lean per-sheet schematic SVG.
//!
//! The viewer doesn't need a pixel-faithful KiCad render — it needs each wire,
//! junction, label, pin and component to carry the `data-uuid` the cross-probe
//! indexes are keyed on, so a click resolves to a net or component. So we emit a
//! compact, monochrome SVG: geometry only, every element tagged with its uuid
//! and a `class`/`data-*` the app themes via CSS. Crucially there is no embedded
//! JSON `<metadata>` copy of the design — that block was ~80% of the bytes in the
//! tool we're replacing and nothing reads it.
//!
//! Fidelity targets (verified against KiCad's own render of a dense production
//! design): pin names and
//! numbers, smooth arcs, DNP cross-outs, net-class directive flags, free sheet
//! graphics, hierarchical/global label flags, embedded symbol text, and per-net
//! colours resolved from the project's net classes (passed in as `colors`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use eda_parse_kicad::schematic::{
    Fill, HJustify, Label, LabelKind, LibPin, LibSymbol, NetclassFlag, Schematic, Shape,
    SymbolInstance, TextEffects, VJustify,
};

use crate::geom;

/// Map from a graphical element's uuid to the hex colour its net class assigns.
/// Built by the pipeline from the design model + the `.kicad_pro`; empty when no
/// colours apply (then everything renders monochrome and the app themes it).
pub type NetColors = BTreeMap<String, String>;

/// Padding around the drawn extent, in mm.
const PAD: f64 = 2.54;

/// Font stack: prefer KiCad's stroke font (Newstroke / its osifont cousin) when
/// the host has it, falling back to a clean sans so the SVG stays legible
/// standalone. KiCad's default text is the proportional Newstroke face.
pub const FONT_FAMILY: &str = "'Newstroke','osifont','Noto Sans','DejaVu Sans',sans-serif";

/// KiCad draws DNP markers and not-yet-placed annotations in this red.
const DNP_RED: &str = "#EB0000";

/// KiCad's default schematic theme draws the drawing sheet (border + title block)
/// in this dark red — the "worksheet" colour. The standalone SVG and the app
/// (which doesn't theme the worksheet) both show it; used as the fallback when no
/// KiCad theme is reachable.
pub(crate) const WORKSHEET_RED: &str = "#840000";

/// KiCad-default junction dot fill (the `schematic.junction` colour), the fallback
/// when no theme is read.
const JUNCTION_FALLBACK: &str = "#000000";

/// KiCad's default wire/line width: 6 mil = 0.1524 mm. Fallback for the SVG root
/// stroke-width when the project's `default_line_thickness` isn't read.
const LINE_WIDTH_FALLBACK: f64 = 0.1524;

/// The handful of colours/widths the renderer *bakes* into the SVG, resolved from
/// the active KiCad theme + the project's drawing defaults (each falling back to
/// KiCad's own default). This is deliberately small: net-class colours are
/// per-net, and the full palette (wire/pin/body/label/…) is themed app-side from
/// the design model's `theme` block — these are only the values the standalone SVG
/// needs and the app does not re-theme (the worksheet frame and the DNP marker).
#[derive(Clone)]
pub struct SchStyle {
    pub worksheet: String,
    pub dnp: String,
    pub junction: String,
    pub line_width: f64,
}

impl Default for SchStyle {
    fn default() -> Self {
        SchStyle {
            worksheet: WORKSHEET_RED.to_string(),
            dnp: DNP_RED.to_string(),
            junction: JUNCTION_FALLBACK.to_string(),
            line_width: LINE_WIDTH_FALLBACK,
        }
    }
}

impl SchStyle {
    /// Resolve from a KiCad theme + drawing defaults, keeping the KiCad-Default
    /// fallback for any value the theme/project doesn't supply.
    pub fn from_kicad(theme: &crate::theme::Theme, drawing: &crate::theme::Drawing) -> Self {
        let mut s = SchStyle::default();
        if let Some(c) = theme.sch("worksheet") {
            s.worksheet = c.to_string();
        }
        if let Some(c) = theme.sch("dnp_marker") {
            s.dnp = c.to_string();
        }
        if let Some(c) = theme.sch("junction") {
            s.junction = c.to_string();
        }
        if let Some(w) = drawing.line_thickness_mm {
            if w > 0.0 {
                s.line_width = w;
            }
        }
        s
    }
}

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

/// XML-escape a text payload (label strings).
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

/// `#RRGGBB` for a parsed `(r, g, b, a)` colour.
fn color_hex((r, g, b, _): (u8, u8, u8, f64)) -> String {
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

/// ` fill="#RRGGBB"` (plus ` fill-opacity` when translucent) for a baked KiCad
/// fill colour — a sheet background or a text_box fill. Baked as a presentation
/// attribute: the app themes these primitives' *stroke* only, so the fill wins.
fn fill_color_attr(col: (u8, u8, u8, f64)) -> String {
    let mut out = format!(r#" fill="{}""#, color_hex(col));
    if col.3 < 1.0 {
        let _ = write!(out, r#" fill-opacity="{}""#, c(col.3));
    }
    out
}

/// Index of the `}` that closes the `{` at `open`, honouring nesting; `None` if
/// the brace is never closed (then it is treated as a literal character).
fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (j, &ch) in chars.iter().enumerate().skip(open) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

/// KiCad `UnescapeString` token → its character, or `None` for an unknown token
/// (left as the literal `{token}`). Covers the printable escapes that turn up in
/// net/label names — most importantly `{slash}` (a literal `/`, which KiCad
/// escapes because `/` is the hierarchy-path separator).
fn unescape_token(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "slash" => "/",
        "backslash" => "\\",
        "brace" => "{",
        "dollar" => "$",
        "lt" => "<",
        "gt" => ">",
        "colon" => ":",
        "tab" => "\t",
        "return" => "\n",
        "dblquote" => "\"",
        _ => return None,
    })
}

/// Render KiCad text markup as the *inner* content of an SVG `<text>`/`<tspan>`:
/// overbar `~{…}` becomes an overline, `_{…}`/`^{…}` sub/superscript (all
/// nestable), and `{token}` character escapes (`{slash}`→`/`, …) are resolved.
/// Plain text with no markup returns exactly `esc(text)`, so a caller that just
/// dumps a string is unaffected. KiCad shows e.g. `~{crst}{slash}out2` as an
/// overlined "crst" then "/out2"; emitting the raw markup instead made labels far
/// wider than KiCad draws them, so neighbouring labels/notes overran each other.
pub(crate) fn render_markup(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        // Style group: ~{…} (overline), ^{…} (super), _{…} (sub).
        if matches!(ch, '~' | '_' | '^') && chars.get(i + 1) == Some(&'{') {
            if let Some(close) = matching_brace(&chars, i + 1) {
                let inner: String = chars[i + 2..close].iter().collect();
                let body = render_markup(&inner);
                match ch {
                    '~' => {
                        let _ = write!(out, r#"<tspan text-decoration="overline">{body}</tspan>"#);
                    }
                    '^' => {
                        let _ = write!(out, r#"<tspan font-size="70%" baseline-shift="super">{body}</tspan>"#);
                    }
                    _ => {
                        let _ = write!(out, r#"<tspan font-size="70%" baseline-shift="sub">{body}</tspan>"#);
                    }
                }
                i = close + 1;
                continue;
            }
        }
        // Character escape: {token}. Unknown / unbalanced braces fall through to
        // the literal path below.
        if ch == '{' {
            if let Some(close) = matching_brace(&chars, i) {
                let token: String = chars[i + 1..close].iter().collect();
                if let Some(rep) = unescape_token(&token) {
                    out.push_str(&esc(rep));
                    i = close + 1;
                    continue;
                }
            }
        }
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
        i += 1;
    }
    out
}

/// The plain visible text of a markup string (markup stripped, escapes resolved),
/// for sizing a label's box. Mirrors `render_markup` without the SVG wrapping.
/// Also used by the geometry IR to resolve KiCad escapes (e.g. `{dblquote}`→`"`)
/// before handing text to the GPU overlay, which renders it as-is (no markup pass).
pub(crate) fn display_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if matches!(ch, '~' | '_' | '^') && chars.get(i + 1) == Some(&'{') {
            if let Some(close) = matching_brace(&chars, i + 1) {
                let inner: String = chars[i + 2..close].iter().collect();
                out.push_str(&display_text(&inner));
                i = close + 1;
                continue;
            }
        }
        if ch == '{' {
            if let Some(close) = matching_brace(&chars, i) {
                let token: String = chars[i + 1..close].iter().collect();
                if let Some(rep) = unescape_token(&token) {
                    out.push_str(rep);
                    i = close + 1;
                    continue;
                }
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// KiCad text `size` (glyph height, mm) → SVG `font-size` (em). KiCad's glyph
/// height is ~3/4 of the requested em, so the renderer draws text at `size*4/3`
/// (see `text_attrs`); width estimates must use the same scale or they under-measure.
const FONT_EM: f64 = 4.0 / 3.0;

/// Per-glyph advance for printable ASCII as a fraction of the em (`size*FONT_EM`),
/// measured from the embedded Newstroke font (`public/fonts/newstroke.ttf`) and
/// rounded up to 0.05 for headroom. Indexed by `ch - 0x20` over 0x20..=0x7E. KiCad's
/// stroke glyphs vary widely (`i`≈0.25 em, `m`/`-` ≈ 1.0 em), so a single average
/// over- or under-shoots; summing real advances keeps a text_box wrap inside its box.
#[rustfmt::skip]
const ADVANCE_EM: [u8; 95] = [
    70, 35, 55, 80, 60, 90, 90, 40, 35, 40, 50, 85, 50, 100, 40, 80,
    80, 70, 80, 80, 70, 80, 70, 80, 75, 65, 35, 35, 80, 100, 80, 65,
    100, 80, 75, 80, 75, 70, 70, 80, 75, 25, 60, 75, 60, 75, 70, 80,
    70, 80, 70, 70, 70, 75, 85, 95, 80, 85, 80, 35, 80, 35, 60, 100,
    35, 60, 60, 60, 65, 65, 55, 60, 60, 30, 40, 60, 35, 95, 60, 70,
    65, 65, 45, 55, 55, 70, 70, 95, 65, 65, 70, 40, 20, 40, 55,
];

/// Advance width (mm) of one glyph at KiCad text `size`. Non-ASCII (CJK, accented)
/// falls back to a mid-width estimate.
fn glyph_advance(ch: char, size: f64) -> f64 {
    let em = if (' '..='~').contains(&ch) {
        ADVANCE_EM[ch as usize - 0x20] as f64 / 100.0
    } else {
        0.70
    };
    size * FONT_EM * em
}

/// Rendered width (mm) of one line at `size`, with KiCad markup stripped.
fn text_width(line: &str, size: f64) -> f64 {
    display_text(line).chars().map(|c| glyph_advance(c, size)).sum()
}

/// Word-wrap `text` to fit within `max_w` mm at the given font `size` (mm),
/// preserving the existing hard newlines. KiCad reflows a `(text_box …)` to the
/// box width; we sum the per-glyph advances (`text_width`) so a line never exceeds
/// the box, instead of a single-average estimate that over-packed wide glyphs and
/// overran the boundary. A single word longer than the line is left whole (never
/// chopped mid-word), matching KiCad.
fn wrap_text(text: &str, max_w: f64, size: f64) -> String {
    if max_w <= 0.0 {
        return text.to_string();
    }
    let space = glyph_advance(' ', size);
    let mut out = String::with_capacity(text.len());
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
        }
        let mut cur = 0.0_f64; // visible width (mm) on the current line
        for (wi, word) in line.split(' ').enumerate() {
            let wlen = text_width(word, size);
            if wi == 0 {
                out.push_str(word);
                cur = wlen;
            } else if cur + space + wlen <= max_w {
                out.push(' ');
                out.push_str(word);
                cur += space + wlen;
            } else {
                out.push('\n');
                out.push_str(word);
                cur = wlen;
            }
        }
    }
    out
}

/// KiCad's fallback image resolution when a PNG carries no `pHYs` chunk.
const DEFAULT_DPI: f64 = 300.0;

/// Placed size of an embedded image, in mm. KiCad sizes a bitmap from its pixel
/// dimensions, its intrinsic resolution (the PNG `pHYs` DPI, falling back to
/// 300), and the stored scale: `mm = px / dpi * 25.4 * scale`. Honouring the
/// embedded DPI matters — e.g. the current-sense reference images declare 120 DPI,
/// so assuming 300 shrank them to 2/5 of KiCad's size.
fn image_size(pw: u32, ph: u32, scale: f64, dpi: (f64, f64)) -> (f64, f64) {
    (pw as f64 / dpi.0 * 25.4 * scale, ph as f64 / dpi.1 * 25.4 * scale)
}

/// The placed `(width, height)` in mm of an embedded image, or a 10 mm square
/// when its bytes aren't a readable PNG.
fn image_dims_mm(img: &eda_parse_kicad::schematic::SchImage) -> (f64, f64) {
    match png_dims(&img.data) {
        Some((pw, ph)) => image_size(pw, ph, img.scale, png_dpi(&img.data).unwrap_or((DEFAULT_DPI, DEFAULT_DPI))),
        None => (10.0, 10.0),
    }
}

/// Decode the leading bytes of a base64 payload (whitespace tolerated, stops at
/// padding) — enough to read a PNG header.
fn b64_decode_prefix(s: &str, want: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => break, // '=' padding or unexpected byte
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            if out.len() >= want {
                break;
            }
        }
    }
    out
}

/// Pixel dimensions of a base64 PNG, read from its IHDR chunk.
fn png_dims(b64: &str) -> Option<(u32, u32)> {
    let b = b64_decode_prefix(b64, 24);
    if b.len() < 24 || &b[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes([b[16], b[17], b[18], b[19]]);
    let h = u32::from_be_bytes([b[20], b[21], b[22], b[23]]);
    Some((w, h))
}

/// `(dpi_x, dpi_y)` from a base64 PNG's `pHYs` chunk, when it declares pixels per
/// metre (unit 1). `pHYs` precedes `IDAT`, so a short prefix reaches it; absent or
/// in unknown units, the caller falls back to KiCad's 300 DPI default.
fn png_dpi(b64: &str) -> Option<(f64, f64)> {
    let b = b64_decode_prefix(b64, 256);
    let mut i = 8; // skip the 8-byte PNG signature
    while i + 12 <= b.len() {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let typ = &b[i + 4..i + 8];
        if typ == b"pHYs" && i + 8 + 9 <= b.len() {
            let ppu_x = u32::from_be_bytes([b[i + 8], b[i + 9], b[i + 10], b[i + 11]]);
            let ppu_y = u32::from_be_bytes([b[i + 12], b[i + 13], b[i + 14], b[i + 15]]);
            let unit = b[i + 16];
            if unit == 1 && ppu_x > 0 && ppu_y > 0 {
                // pixels-per-metre → pixels-per-inch.
                return Some((ppu_x as f64 * 0.0254, ppu_y as f64 * 0.0254));
            }
            return None;
        }
        if typ == b"IDAT" {
            break; // pixel data started; no pHYs present
        }
        i += 12 + len;
    }
    None
}

/// Running bounding box of the sheet's geometry.
#[derive(Clone, Copy)]
struct Extent {
    minx: f64,
    miny: f64,
    maxx: f64,
    maxy: f64,
}

impl Extent {
    fn new() -> Self {
        Extent { minx: f64::MAX, miny: f64::MAX, maxx: f64::MIN, maxy: f64::MIN }
    }
    fn add(&mut self, x: f64, y: f64) {
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }
    fn valid(&self) -> bool {
        self.minx <= self.maxx && self.miny <= self.maxy
    }
}

/// Format a transformed point list as an SVG `points` string.
fn pts_attr(pts: &[(f64, f64)]) -> String {
    let mut out = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{},{}", c(*x), c(*y));
    }
    out
}

/// Colour assigned to an element uuid by its net class, if any.
fn color_for<'a>(colors: &'a NetColors, uuid: &str) -> Option<&'a str> {
    if uuid.is_empty() {
        None
    } else {
        colors.get(uuid).map(String::as_str)
    }
}

/// Inline `--nc` custom property carrying a net-class colour. The standalone SVG
/// reads the baked `stroke`/`fill`; the app's CSS theme (which recolours B&W with
/// `!important`) instead resolves `var(--nc, <default>)`, so a net colour set
/// here wins over the default palette without the extractor knowing the palette.
fn nc_style(color: Option<&str>) -> String {
    color.map(|c| format!(r#" style="--nc:{c}""#)).unwrap_or_default()
}

/// Drawing-sheet context for the title block — the sheet's place in the
/// hierarchy plus the effective title-block fields (the pipeline resolves these:
/// a sub-sheet inherits the root sheet's fields and project text variables are
/// expanded). `None` falls back to this sheet's own raw fields.
#[derive(Clone, Copy)]
pub struct SheetFrame<'a> {
    pub number: i64,
    /// KiCad page label (from `sheet_instances`); empty → fall back to `number`.
    pub page: &'a str,
    pub total: i64,
    /// Hierarchical sheet path shown on the "Sheet:" line (e.g. "/encoder/").
    pub name: &'a str,
    /// Source file shown on the "File:" line (e.g. "encoder.kicad_sch").
    pub file: &'a str,
    pub title: &'a str,
    pub company: &'a str,
    pub rev: &'a str,
    pub date: &'a str,
    /// KiCad generator version (`generator_version`), shown as "KiCad E.D.A. <v>".
    pub version: &'a str,
    /// Title-block comment lines, in order.
    pub comments: &'a [String],
}

/// Resolved title-block content for `render_frame`, decoupled from the schematic
/// so the PCB worksheet can build one from its own `(paper …)` / `(title_block …)`.
/// All fields are already resolved (sub-sheet inheritance, text-variable expansion);
/// empty strings render as blank cells.
pub(crate) struct FrameData {
    pub title: String,
    pub company: String,
    pub rev: String,
    pub date: String,
    /// KiCad generator version, shown as "KiCad E.D.A. <v>".
    pub version: String,
    pub comments: Vec<String>,
    /// Paper token shown on the "Size:" line (e.g. "A3").
    pub paper: String,
    /// "Sheet:" line (hierarchical path; empty for a board).
    pub sheet_path: String,
    /// "File:" line.
    pub file: String,
    /// "Id:" line (e.g. "1/3"; empty to omit).
    pub id: String,
}

/// Build the schematic's `FrameData`, honouring the resolved `SheetFrame` over the
/// sheet's own raw fields (the old in-`render_frame` precedence, lifted out).
fn frame_data_from(sch: &Schematic, frame: Option<SheetFrame>) -> FrameData {
    let own = |o: &Option<String>| o.clone().unwrap_or_default();
    match frame {
        Some(f) => FrameData {
            title: f.title.to_string(),
            company: f.company.to_string(),
            rev: f.rev.to_string(),
            date: f.date.to_string(),
            version: f.version.to_string(),
            comments: f.comments.to_vec(),
            paper: own(&sch.paper),
            sheet_path: f.name.to_string(),
            file: f.file.to_string(),
            id: {
                let pg = if f.page.is_empty() { f.number.to_string() } else { f.page.to_string() };
                format!("{pg}/{}", f.total)
            },
        },
        None => FrameData {
            title: own(&sch.title),
            company: own(&sch.company),
            rev: own(&sch.rev),
            date: own(&sch.date),
            version: own(&sch.generator_version),
            comments: sch.comments.clone(),
            paper: own(&sch.paper),
            sheet_path: String::new(),
            file: String::new(),
            id: String::new(),
        },
    }
}

/// Landscape page dimensions (w, h) in mm for a KiCad paper token (or explicit
/// `User` dims). `None` for an unknown size — then no frame is drawn and the
/// viewBox stays content-only.
fn page_size(sch: &Schematic) -> Option<(f64, f64)> {
    page_dims(sch.paper.as_deref(), sch.paper_dims)
}

/// Page dimensions for a paper token / explicit `User` dims — shared by the
/// schematic and the PCB worksheet.
pub(crate) fn page_dims(paper: Option<&str>, dims: Option<(f64, f64)>) -> Option<(f64, f64)> {
    if let Some((w, h)) = dims {
        if w > 0.0 && h > 0.0 {
            return Some((w, h));
        }
    }
    let paper = paper?;
    let portrait = paper.contains("portrait");
    let (w, h) = match paper.split_whitespace().next().unwrap_or(paper) {
        "A5" => (210.0, 148.0),
        "A4" => (297.0, 210.0),
        "A3" => (420.0, 297.0),
        "A2" => (594.0, 420.0),
        "A1" => (841.0, 594.0),
        "A0" => (1189.0, 841.0),
        "A" | "USLetter" => (279.4, 215.9),
        "B" | "USLedger" => (431.8, 279.4),
        "C" => (558.8, 431.8),
        "D" => (863.6, 558.8),
        "E" => (1117.6, 863.6),
        "USLegal" => (355.6, 215.9),
        _ => return None,
    };
    Some(if portrait { (h, w) } else { (w, h) })
}

/// Approximate bounding box `(x0, y0, x1, y1)` a note occupies, so the extent
/// (and thus viewBox) covers its full text — a left-justified note runs to the
/// right of its anchor, which the old anchor-only extent clipped. A text_box
/// uses its declared `size`; plain text is estimated from its glyph metrics.
fn note_bounds(note: &eda_parse_kicad::schematic::TextNote) -> (f64, f64, f64, f64) {
    if let Some((bw, bh)) = note.box_size {
        return (note.at.x, note.at.y, note.at.x + bw, note.at.y + bh);
    }
    let size = note.effects.size.max(0.1);
    let n = note.text.split('\n').count().max(1) as f64;
    // Widest line's real rendered width (the old char-count × average under-measured
    // and could clip a wide plain note from the viewBox).
    let w = note.text.split('\n').map(|l| text_width(l, size)).fold(0.0_f64, f64::max);
    let h = n * size * 1.2;
    let (x0, x1) = match note.effects.h_justify {
        HJustify::Left => (note.at.x, note.at.x + w),
        HJustify::Right => (note.at.x - w, note.at.x),
        HJustify::Center => (note.at.x - w / 2.0, note.at.x + w / 2.0),
    };
    let (y0, y1) = match note.effects.v_justify {
        VJustify::Top => (note.at.y, note.at.y + h),
        VJustify::Bottom => (note.at.y - h, note.at.y),
        VJustify::Center => (note.at.y - h / 2.0, note.at.y + h / 2.0),
    };
    (x0, y0, x1, y1)
}

/// Render the drawing-sheet frame + title block: KiCad's default page template
/// (not design content). Reproduces KiCad's geometry measured off its own plotted
/// output — a double border with a row/column reference band, and the bottom-right
/// title block with its Sheet/File/Title/Size/Date/Rev/KiCad/Id cells. Baked in the
/// worksheet red the app doesn't theme, so standalone SVG and app match.
pub(crate) fn render_frame(s: &mut String, w: f64, h: f64, fd: &FrameData, worksheet: &str) {
    const M_OUT: f64 = 10.0; // outer page border
    const M_IN: f64 = 12.0; // inner content border (reference band is the 2 mm between)
    let _ = write!(
        s,
        r##"<g data-primitive="worksheet" stroke="{worksheet}" stroke-width="0.15" fill="{worksheet}" font-family="{FONT_FAMILY}">"##
    );
    // Outer + inner border rectangles.
    let _ = write!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none"/><rect x="{}" y="{}" width="{}" height="{}" fill="none"/>"#,
        c(M_OUT), c(M_OUT), c(w - 2.0 * M_OUT), c(h - 2.0 * M_OUT),
        c(M_IN), c(M_IN), c(w - 2.0 * M_IN), c(h - 2.0 * M_IN)
    );

    // Row/column reference markers in the band: numbers 1..N along top+bottom,
    // letters A.. down left+right, ~50 mm per division (KiCad's scheme; A4 → 6×4).
    let inner_w = w - 2.0 * M_IN;
    let inner_h = h - 2.0 * M_IN;
    let cols = ((inner_w / 50.0).round() as i64).max(1);
    let rows_n = ((inner_h / 50.0).round() as i64).max(1);
    let band = (M_OUT + M_IN) / 2.0; // text centre line inside the 2 mm band
    let tick = |s: &mut String, x1: f64, y1: f64, x2: f64, y2: f64| {
        let _ = write!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#, c(x1), c(y1), c(x2), c(y2));
    };
    let mark = |s: &mut String, x: f64, y: f64, t: &str| {
        let _ = write!(
            s,
            r##"<text x="{}" y="{}" font-size="1.8" stroke="none" text-anchor="middle" dominant-baseline="central">{}</text>"##,
            c(x), c(y), esc(t)
        );
    };
    for i in 0..cols {
        let x0 = M_IN + inner_w * i as f64 / cols as f64;
        let xc = x0 + inner_w / cols as f64 / 2.0;
        let num = (i + 1).to_string();
        mark(s, xc, band, &num); // top
        mark(s, xc, h - band, &num); // bottom
        if i > 0 {
            tick(s, x0, M_OUT, x0, M_IN); // top band divider
            tick(s, x0, h - M_IN, x0, h - M_OUT); // bottom band divider
        }
    }
    for j in 0..rows_n {
        let y0 = M_IN + inner_h * j as f64 / rows_n as f64;
        let yc = y0 + inner_h / rows_n as f64 / 2.0;
        // `j % 26` keeps the band letter in A..Z and avoids a u8 overflow panic on an
        // absurd (malformed) page height; real pages never exceed a handful of rows.
        let letter = ((b'A' + (j % 26) as u8) as char).to_string();
        mark(s, band, yc, &letter); // left
        mark(s, w - band, yc, &letter); // right
        if j > 0 {
            tick(s, M_OUT, y0, M_IN, y0); // left band divider
            tick(s, w - M_IN, y0, w - M_OUT, y0); // right band divider
        }
    }

    // ---- title block (bottom-right, anchored to the inner border corner) ----
    let FrameData { title, company, rev, date, version, comments, paper, sheet_path, file, id } = fd;

    let right = w - M_IN;
    let bottom = h - M_IN;
    let left = right - 108.0; // title block 108 mm wide
    let top = bottom - 32.0; // 32 mm tall
    // Horizontal cell lines, distances above the bottom border.
    let y = |up: f64| bottom - up;
    let (y1, y2, y3, y4) = (y(3.5), y(6.5), y(10.5), y(16.5));
    // Vertical cell dividers.
    let v_rev = right - 23.9; // Rev/Id column (spans the bottom two rows)
    let v_date = right - 88.0; // Size | Date split (Size/Date/Rev row)

    // Outer box + row separators.
    let _ = write!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="none"/>"#,
        c(left), c(top), c(right - left), c(bottom - top)
    );
    for yl in [y1, y2, y3, y4] {
        tick(s, left, yl, right, yl);
    }
    tick(s, v_rev, y2, v_rev, bottom); // Rev/Id divider (bottom two rows)
    tick(s, v_date, y1, v_date, y2); // Size | Date divider

    // Cell text — left-aligned with a small inset; baseline ~0.9 mm above each line.
    let cell = |s: &mut String, x: f64, baseline: f64, size: f64, bold: bool, italic: bool, t: &str| {
        if t.is_empty() {
            return;
        }
        let b = if bold { r#" font-weight="bold""# } else { "" };
        let i = if italic { r#" font-style="italic""# } else { "" };
        let _ = write!(
            s,
            r##"<text x="{}" y="{}" font-size="{}" stroke="none"{b}{i}>{}</text>"##,
            c(x), c(baseline), c(size), esc(t)
        );
    };
    let pad = 1.3;
    // Sheet / File (two lines in the row y4..y3).
    cell(s, left + pad, y4 + 2.4, 1.5, false, false, &format!("Sheet: {sheet_path}"));
    cell(s, left + pad, y4 + 5.1, 1.5, false, false, &format!("File: {file}"));
    // Title (large, bold italic) in row y3..y2.
    cell(s, left + pad, y2 - 1.1, 2.8, true, true, &format!("Title: {title}"));
    // Size / Date / Rev in row y2..y1.
    cell(s, left + pad, y1 - 0.9, 1.5, false, false, &format!("Size: {paper}"));
    cell(s, v_date + pad, y1 - 0.9, 1.5, false, false, &format!("Date: {date}"));
    cell(s, v_rev + pad, y1 - 0.9, 1.5, true, false, &format!("Rev: {rev}"));
    // Bottom row y1..bottom: KiCad's drawing sheet prints the application/version
    // string ("KiCad E.D.A. <v>") at the bottom-left and the sheet Id at the right.
    if !version.is_empty() {
        cell(s, left + pad, bottom - 0.9, 1.5, false, false, &format!("KiCad E.D.A. {version}"));
    }
    cell(s, v_rev + pad, bottom - 0.9, 1.5, false, false, &format!("Id: {id}"));

    // Company, then the title-block comment lines, stacked upward in the open area
    // above the Sheet/File row — KiCad's order (company nearest the rows, comments
    // above it). Bounded so a long comment list can't climb past the box top.
    let mut up = y4 - 2.6;
    for text in std::iter::once(company).chain(comments.iter()) {
        if up < top + 1.0 {
            break;
        }
        if !text.is_empty() {
            cell(s, left + pad, up, 1.5, false, false, text);
            up -= 2.7;
        }
    }

    s.push_str("</g>");
}

/// Render one sheet to an enriched SVG. Every element is wrapped in a
/// `<g data-primitive=… data-uuid=…>` group: the viewer's CSS themes by
/// `data-primitive` (B&W → KiCad palette) and its hit-testing resolves
/// `g[data-uuid]` → net/component and `[data-designator][data-pin]` → pin.
/// `colors` maps element uuids to the colours their net classes assign.
pub fn render_sheet(sch: &Schematic, colors: &NetColors, frame: Option<SheetFrame>, style: &SchStyle) -> String {
    // ----- drawing extent -----
    let mut ext = Extent::new();
    for w in &sch.wires {
        ext.add(w.a.x, w.a.y);
        ext.add(w.b.x, w.b.y);
    }
    for j in &sch.junctions {
        ext.add(j.at.x, j.at.y);
    }
    for e in &sch.bus_entries {
        ext.add(e.a.x, e.a.y);
        ext.add(e.b.x, e.b.y);
    }
    for l in &sch.labels {
        ext.add(l.at.x, l.at.y);
    }
    for note in &sch.notes {
        let (x0, y0, x1, y1) = note_bounds(note);
        ext.add(x0, y0);
        ext.add(x1, y1);
    }
    for f in &sch.netclass_flags {
        ext.add(f.at.x, f.at.y);
    }
    for g in &sch.graphics {
        for (x, y) in shape_points(&g.shape) {
            ext.add(x, y);
        }
    }
    for sr in &sch.sheets {
        ext.add(sr.at.x, sr.at.y);
        ext.add(sr.at.x + sr.size.0, sr.at.y + sr.size.1);
    }
    for img in &sch.images {
        let (w, h) = image_dims_mm(img);
        ext.add(img.at.x - w / 2.0, img.at.y - h / 2.0);
        ext.add(img.at.x + w / 2.0, img.at.y + h / 2.0);
    }
    for sym in &sch.symbols {
        if let Some(lib) = sch.lib_for(sym) {
            let tf =
                |px, py| geom::place_mm(sym.at.x, sym.at.y, sym.at.angle, sym.mirror.as_deref(), px, py);
            if let Some((min, max)) = lib.bbox {
                for (px, py) in [(min.x, min.y), (max.x, max.y), (min.x, max.y), (max.x, min.y)] {
                    let (x, y) = tf(px, py);
                    ext.add(x, y);
                }
            }
            for pin in &lib.pins {
                if pin.unit == 0 || pin.unit == sym.unit {
                    let (x, y) = tf(pin.at.x, pin.at.y);
                    ext.add(x, y);
                }
            }
        }
    }
    // The viewBox is the padded content box union the page rectangle: the whole
    // page template shows (so the title block reads), and any content that runs
    // past the page edge — KiCad still draws it — is never clipped (e.g. the CAN
    // sheet's wide note text, which the old content-only box cut off). With no
    // content, a known page alone defines the box; otherwise a default square.
    let page = page_size(sch);
    let (mut minx, mut miny, mut maxx, mut maxy) = if ext.valid() {
        (ext.minx - PAD, ext.miny - PAD, ext.maxx + PAD, ext.maxy + PAD)
    } else if page.is_some() {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        (0.0, 0.0, 100.0, 100.0)
    };
    if let Some((pw, ph)) = page {
        minx = minx.min(0.0);
        miny = miny.min(0.0);
        maxx = maxx.max(pw);
        maxy = maxy.max(ph);
    }
    let (vx, vy, vw, vh) = (minx, miny, maxx - minx, maxy - miny);

    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" fill="none" stroke="#000000" stroke-width="{}" font-family="{}">"##,
        c(vx), c(vy), c(vw), c(vh), c(style.line_width), FONT_FAMILY
    );

    // Drawing-sheet frame + title block (the page template), behind the design.
    if let Some((pw, ph)) = page {
        render_frame(&mut s, pw, ph, &frame_data_from(sch, frame), &style.worksheet);
    }

    // Embedded images first (background).
    for img in &sch.images {
        let (w, h) = image_dims_mm(img);
        let _ = write!(
            s,
            r#"<g data-primitive="image" data-uuid="{}"><image x="{}" y="{}" width="{}" height="{}" href="data:image/png;base64,{}"/></g>"#,
            esc(&img.uuid), c(img.at.x - w / 2.0), c(img.at.y - h / 2.0), c(w), c(h), img.data
        );
    }

    // Free graphic primitives drawn on the sheet (annotations, dividers). KiCad
    // draws these in the schematic "graphic lines" colour (the same note blue in the
    // user's theme); the app themes `data-primitive="graphic"` to match, so an
    // explicit colour is baked via `--gc` only when the file sets one. Dashed strokes
    // (callout brackets like "place close to ADC pin") get a dash pattern.
    let id = |x, y| (x, y);
    for g in &sch.graphics {
        let color_attr = g
            .color
            .map(|c| {
                let hex = color_hex(c);
                format!(r#" stroke="{hex}" style="--gc:{hex}""#)
            })
            .unwrap_or_default();
        let dash_attr = if g.dashed {
            // Dash sized off the line width (KiCad-ish), with a sane floor for the
            // common width-0 (use-default-pen) case.
            let w = if g.width > 0.0 { g.width } else { 0.152 };
            format!(r#" stroke-dasharray="{} {}""#, c(w * 6.0), c(w * 3.0))
        } else {
            String::new()
        };
        let _ = write!(
            s,
            r#"<g data-primitive="graphic" data-uuid="{}"{color_attr}{dash_attr}>"#,
            esc(&g.uuid)
        );
        render_shape(&mut s, &g.shape, g.width, &id);
        s.push_str("</g>");
    }

    // Wires / buses.
    for w in &sch.wires {
        let prim = if w.is_bus { "bus" } else { "wire" };
        let stroke = color_for(colors, &w.uuid)
            .map(str::to_string)
            .or_else(|| w.color.map(color_hex));
        let attr = stroke
            .as_deref()
            .map(|c| format!(r#" stroke="{c}" style="--nc:{c}""#))
            .unwrap_or_default();
        let _ = write!(
            s,
            r#"<g data-primitive="{prim}" data-uuid="{}"{attr}><polyline points="{},{} {},{}"/></g>"#,
            esc(&w.uuid), c(w.a.x), c(w.a.y), c(w.b.x), c(w.b.y)
        );
    }
    // Bus entries (diagonal wire→bus taps).
    for e in &sch.bus_entries {
        let _ = write!(
            s,
            r#"<g data-primitive="bus-entry" data-uuid="{}"><polyline points="{},{} {},{}"/></g>"#,
            esc(&e.uuid), c(e.a.x), c(e.a.y), c(e.b.x), c(e.b.y)
        );
    }
    // Junctions.
    for j in &sch.junctions {
        let ncol = color_for(colors, &j.uuid)
            .map(str::to_string)
            .or_else(|| j.color.map(color_hex));
        let jstyle = nc_style(ncol.as_deref());
        let fill = ncol.unwrap_or_else(|| style.junction.clone());
        let r = if j.diameter > 0.0 { j.diameter / 2.0 } else { 0.45 };
        let _ = write!(
            s,
            r#"<g data-primitive="junction" data-uuid="{}"{jstyle}><circle cx="{}" cy="{}" r="{}" fill="{}"/></g>"#,
            esc(&j.uuid), c(j.at.x), c(j.at.y), c(r), fill
        );
    }
    // No-connect X marks.
    for nc in &sch.no_connects {
        let (x, y) = (nc.x, nc.y);
        let _ = write!(
            s,
            r#"<g data-primitive="no-connect"><polyline points="{},{} {},{}"/><polyline points="{},{} {},{}"/></g>"#,
            c(x - 0.6), c(y - 0.6), c(x + 0.6), c(y + 0.6), c(x - 0.6), c(y + 0.6), c(x + 0.6), c(y - 0.6)
        );
    }

    // Net-class directive flags (the small "net directive" tags on a wire).
    for f in &sch.netclass_flags {
        render_netclass_flag(&mut s, f, colors);
    }

    // Placed symbols.
    for sym in &sch.symbols {
        render_symbol(&mut s, sch, sym, &style.dnp);
    }

    // Hierarchical sheet symbols (drawn on this parent sheet).
    for sr in &sch.sheets {
        let (w, h) = sr.size;
        let _ = write!(
            s,
            r#"<g data-primitive="sheet-symbol" data-uuid="{}" data-sheet-name="{}" data-at-x-nm="{}" data-at-y-nm="{}" data-size-x-nm="{}" data-size-y-nm="{}">"#,
            esc(&sr.uuid),
            esc(&sr.name),
            (sr.at.x * 1e6) as i64,
            (sr.at.y * 1e6) as i64,
            (w * 1e6) as i64,
            (h * 1e6) as i64,
        );
        // KiCad lets each sheet carry its own background colour; bake it so the
        // canvas shows it (the app themes only the sheet *border*, so the fill wins).
        let fill = sr.fill.map(fill_color_attr).unwrap_or_default();
        let _ = write!(
            s,
            r#"<rect x="{}" y="{}" width="{}" height="{}"{fill}/>"#,
            c(sr.at.x), c(sr.at.y), c(w.max(0.1)), c(h.max(0.1))
        );
        let _ = write!(
            s,
            r#"<text class="sch-sheet-name" x="{}" y="{}" font-size="1.524" stroke="none">{}</text>"#,
            c(sr.at.x), c(sr.at.y - 0.6), esc(&sr.name)
        );
        for p in &sr.pins {
            emit_sheet_pin_label(&mut s, sr, p, colors);
        }
        s.push_str("</g>");
    }

    // Labels (local / global / hierarchical).
    for l in &sch.labels {
        render_label(&mut s, l, colors);
    }

    // Free-text notes (multi-line, honouring KiCad's justification). A text_box
    // also draws its border rectangle (unless its stroke type is `none`), its fill,
    // and wraps its text to the box so it doesn't overrun the boundary.
    for note in &sch.notes {
        // KiCad lets a note carry its own `(color …)` (the demo boards colour-code
        // their annotations — orange titles, teal I/O tags). Bake it as `--nc` so
        // the app's `var(--nc, --sch-note)` rule keeps that colour instead of the
        // `!important` note-blue clobbering it; uncoloured notes fall back to blue.
        let ncol = note.effects.color.map(color_hex);
        // A note may carry a `(href …)` hyperlink; expose it as data-href so the viewer
        // can make the text clickable (batch3 item 5).
        let href_attr = match &note.effects.href {
            Some(h) if !h.is_empty() => format!(r#" data-href="{}""#, esc(h)),
            _ => String::new(),
        };
        let _ = write!(
            s,
            r#"<g data-primitive="text" data-uuid="{}"{}{}>"#,
            esc(&note.uuid),
            href_attr,
            nc_style(ncol.as_deref())
        );
        if let Some((bw, bh)) = note.box_size {
            // Draw the rectangle when there's a border OR a fill to show. The fill
            // is baked; the border colour comes from the app's note-stroke rule, so
            // a fill-only box pins `stroke-width="0"` to suppress that forced stroke.
            if note.border_width.is_some() || note.box_fill.is_some() {
                let fill = note.box_fill.map(fill_color_attr).unwrap_or_else(|| r#" fill="none""#.to_string());
                let stroke = match note.border_width {
                    Some(b) => format!(r#" stroke-width="{}""#, c(if b > 0.0 { b } else { 0.0762 })),
                    None => r#" stroke-width="0""#.to_string(),
                };
                let _ = write!(
                    s,
                    r#"<rect x="{}" y="{}" width="{}" height="{}"{fill}{stroke}/>"#,
                    c(note.at.x), c(note.at.y), c(bw.abs()), c(bh.abs())
                );
            }
            // `at` is the box's top-left corner; place the text per its justification
            // (inset by the margins) and wrap it to the inner width.
            let (ml, mt, mr, mb) = note.box_margins.unwrap_or((0.0, 0.0, 0.0, 0.0));
            let (bw, bh) = (bw.abs(), bh.abs());
            let tx = match note.effects.h_justify {
                HJustify::Left => note.at.x + ml,
                HJustify::Right => note.at.x + bw - mr,
                HJustify::Center => note.at.x + bw / 2.0,
            };
            let ty = match note.effects.v_justify {
                VJustify::Top => note.at.y + mt,
                VJustify::Bottom => note.at.y + bh - mb,
                VJustify::Center => note.at.y + bh / 2.0,
            };
            let wrapped = wrap_text(&note.text, bw - ml - mr, note.effects.size);
            emit_text_block(&mut s, &wrapped, tx, ty, note.at.angle, &note.effects, None);
        } else {
            emit_text_block(&mut s, &note.text, note.at.x, note.at.y, note.at.angle, &note.effects, None);
        }
        s.push_str("</g>");
    }

    s.push_str("</svg>");
    s
}

/// Render one placed symbol: body graphics, embedded text, pins (stub + name +
/// number), a DNP cross when not-populated, and the visible property fields.
fn render_symbol(s: &mut String, sch: &Schematic, sym: &SymbolInstance, dnp: &str) {
    let Some(lib) = sch.lib_for(sym) else { return };
    let is_power = lib.power || sym.lib_id.starts_with("power:");
    let prim = if is_power { "power-symbol" } else { "symbol" };
    let designator = sym.reference().unwrap_or("");
    // KiCad dims a do-not-populate part — body, pins and every field — and strikes
    // it through in red. Group opacity dims the whole symbol in both the themed app
    // (it survives the CSS `!important` colour overrides) and the standalone SVG;
    // `data-dnp` lets the app target it further if it wants.
    let dnp_attr = if sym.dnp { r#" data-dnp="1" opacity="0.45""# } else { "" };
    let _ = write!(
        s,
        r#"<g data-primitive="{prim}" data-uuid="{}" data-ref="{}"{dnp_attr}>"#,
        esc(&sym.uuid), esc(designator)
    );
    let tf = |px, py| geom::place_mm(sym.at.x, sym.at.y, sym.at.angle, sym.mirror.as_deref(), px, py);
    let on_unit = |unit: u32| unit == 0 || unit == sym.unit;

    // Body graphics. KiCad draws background fills behind everything, so emit them
    // first — otherwise an opaque body rect paints over the symbol's detail.
    for g in lib.graphics.iter().filter(|g| on_unit(g.unit)) {
        if g.shape.fill() == Fill::Background {
            render_shape(s, &g.shape, g.width, &tf);
        }
    }
    for g in lib.graphics.iter().filter(|g| on_unit(g.unit)) {
        if g.shape.fill() != Fill::Background {
            render_shape(s, &g.shape, g.width, &tf);
        }
    }
    // Embedded body text (e.g. an opamp's labels, a gate driver's "Logic Input").
    for t in lib.texts.iter().filter(|t| on_unit(t.unit)) {
        let (x, y) = tf(t.at.x, t.at.y);
        // Symbol body text angles are written in decidegrees (900 == 90°).
        let lib_angle = if t.at.angle.abs() >= 360.0 { t.at.angle / 10.0 } else { t.at.angle };
        let angle = sym.at.angle + lib_angle;
        let eff = oriented_effects(&t.effects, sym.at.angle, angle, sym.mirror.as_deref());
        emit_text_block(s, &t.text, x, y, angle, &eff, None);
    }

    // Pins: stub line (net-addressable), then name + number text.
    for pin in &lib.pins {
        if !on_unit(pin.unit) {
            continue;
        }
        render_pin(s, sym, lib, pin, &tf);
    }

    // Visible property fields at their stored absolute positions. Their angle is
    // stored relative to the symbol's rotation, so subtract it (KiCad then keeps
    // the text upright). Reference/Value that exist but are hidden are honoured
    // (skipped) — never re-rendered by the fallback below.
    let has_ref = sym.properties.iter().any(|p| p.key == "Reference");
    let has_val = sym.properties.iter().any(|p| p.key == "Value");
    for p in &sym.properties {
        if p.effects.hidden || p.value.is_empty() {
            continue;
        }
        let Some(at) = p.at else { continue };
        let eff = oriented_effects(&p.effects, sym.at.angle, at.angle - sym.at.angle, sym.mirror.as_deref());
        // KiCad colours fields by role: the value field uses its `value` colour and
        // any other user field uses the `fields` colour — both distinct from the
        // `reference` colour the generic symbol-text rule applies. Class them so the
        // app themes each from the right KiCad key (Reference stays on the generic rule).
        let cls = match p.key.as_str() {
            "Reference" => "",
            "Value" => r#" class="sch-val""#,
            _ => r#" class="sch-field""#,
        };
        let _ = write!(
            s,
            r#"<text{cls} x="{}" y="{}"{}{}>{}</text>"#,
            c(at.x), c(at.y), text_attrs(&eff, None),
            text_transform(at.angle - sym.at.angle, eff.mirror, at.x, at.y), esc(&p.value)
        );
    }
    // Fallback only for fields KiCad left position-less (rare): place above the
    // body. A field that exists but is hidden is intentionally omitted.
    if !is_power {
        if let Some((min, max)) = lib.bbox {
            let (cx, _) = tf((min.x + max.x) / 2.0, max.y);
            let top = [tf(min.x, min.y), tf(max.x, max.y), tf(min.x, max.y), tf(max.x, min.y)]
                .iter()
                .map(|p| p.1)
                .fold(f64::MAX, f64::min);
            if !has_ref && !designator.is_empty() {
                let _ = write!(
                    s,
                    r#"<text x="{}" y="{}" font-size="1.27" text-anchor="middle" stroke="none">{}</text>"#,
                    c(cx), c(top - 0.6), esc(designator)
                );
            }
            if !has_val {
                if let Some(v) = sym.property("Value").filter(|v| !v.is_empty()) {
                    let _ = write!(
                        s,
                        r#"<text x="{}" y="{}" font-size="1.27" text-anchor="middle" stroke="none">{}</text>"#,
                        c(cx), c(top - 0.6 - 1.6), esc(v)
                    );
                }
            }
        }
    }
    s.push_str("</g>");

    // DNP marker: KiCad strikes a do-not-populate part through with a red cross.
    // Emitted *after* (outside) the symbol group on purpose:
    //   • the symbol group is dimmed (`opacity=0.45`) like KiCad greys the part, but
    //     KiCad draws the cross itself at full strength — keeping it in the group
    //     dimmed the cross too ("the cross is not dull in the original" feedback);
    //   • inside the group the app's symbol-stroke CSS recolours every shape to the
    //     body outline, burying the cross's baked DNP-marker colour. Outside, nothing
    //     overrides it, so the colour the theme assigned (from the raw project) shows.
    // The cross spans the symbol *body* (graphics on the active unit), not the full
    // pin-inclusive bbox: a one-sided part like a connector (pins only on one edge)
    // otherwise gets a cross skewed off the body centre, while a symmetric part (C3)
    // looked fine either way.
    if sym.dnp {
        let bb = body_bbox(lib, sym.unit)
            .or_else(|| lib.bbox.map(|(mn, mx)| (mn.x, mn.y, mx.x, mx.y)));
        if let Some((minx, miny, maxx, maxy)) = bb {
            let p1 = tf(minx, miny);
            let p2 = tf(maxx, maxy);
            let p3 = tf(minx, maxy);
            let p4 = tf(maxx, miny);
            let _ = write!(
                s,
                r#"<g data-primitive="dnp" stroke="{dnp}" stroke-width="0.3"><polyline points="{},{} {},{}"/><polyline points="{},{} {},{}"/></g>"#,
                c(p1.0), c(p1.1), c(p2.0), c(p2.1), c(p3.0), c(p3.1), c(p4.0), c(p4.1)
            );
        }
    }
}

/// Bounding box `(minx, miny, maxx, maxy)` of a library symbol's body graphics on
/// the active unit, in library coordinates — pins excluded. KiCad anchors the DNP
/// cross to the body, so a part with pins on a single edge still gets a centred
/// cross. `None` when the symbol has no body graphics (then the caller falls back
/// to the full pin-inclusive bbox).
fn body_bbox(lib: &LibSymbol, unit: u32) -> Option<(f64, f64, f64, f64)> {
    let mut ext = Extent::new();
    for g in lib.graphics.iter().filter(|g| g.unit == 0 || g.unit == unit) {
        for (x, y) in shape_points(&g.shape) {
            ext.add(x, y);
        }
    }
    ext.valid().then_some((ext.minx, ext.miny, ext.maxx, ext.maxy))
}

/// Render one pin: the stub polyline (always, for cross-probe addressability —
/// but omitted for hidden pins to match KiCad), then the pin name (inside the
/// body, past its end by the name offset) and pin number (above the stub),
/// unless the symbol hides them.
fn render_pin(
    s: &mut String,
    sym: &SymbolInstance,
    lib: &LibSymbol,
    pin: &LibPin,
    tf: &impl Fn(f64, f64) -> (f64, f64),
) {
    let uuid = sym.pin_uuid(&pin.number).unwrap_or("");
    let designator = sym.reference().unwrap_or("");
    let (ex, ey) = tf(pin.at.x, pin.at.y); // connection end (where wires attach)
    let a = pin.at.angle.to_radians();
    let (bx, by) = tf(pin.at.x + pin.length * a.cos(), pin.at.y + pin.length * a.sin());
    // The pin (stub + name + number) is symbol geometry, drawn in the component
    // colour — net-class colours apply to the attached wire, not the pin.
    let _ = write!(
        s,
        r#"<g data-primitive="pin" data-uuid="{}" data-designator="{}" data-pin="{}">"#,
        esc(uuid), esc(designator), esc(&pin.number)
    );
    if !pin.hidden {
        let _ = write!(s, r#"<polyline points="{},{} {},{}"/>"#, c(ex), c(ey), c(bx), c(by));
    }
    // Screen-space pin direction (connection end → body end), snapped cardinal.
    let (dx, dy) = (bx - ex, by - ey);
    let horizontal = dx.abs() >= dy.abs();

    if !pin.hidden && !lib.pin_names_hidden && !pin.name.is_empty() && pin.name != "~" {
        let off = lib.pin_name_offset.max(0.0);
        let size = pin.name_size;
        // Name sits just inside the body, past the pin's body end.
        // `sch-pin-name` lets the app theme it from KiCad's `pin_name` colour,
        // distinct from the pin *number* (KiCad gives them different colours).
        if horizontal {
            if dx > 0.0 {
                // body to the right → name extends right, left-anchored.
                emit_pin_text(s, &pin.name, bx + off, by, false, "start", size, "sch-pin-name");
            } else {
                emit_pin_text(s, &pin.name, bx - off, by, false, "end", size, "sch-pin-name");
            }
        } else if dy < 0.0 {
            // body above → vertical name reading upward, extends up.
            emit_pin_text(s, &pin.name, bx, by - off, true, "start", size, "sch-pin-name");
        } else {
            emit_pin_text(s, &pin.name, bx, by + off, true, "end", size, "sch-pin-name");
        }
    }

    if !pin.hidden && !lib.pin_numbers_hidden && !pin.number.is_empty() {
        let size = pin.number_size;
        let (mx, my) = ((ex + bx) / 2.0, (ey + by) / 2.0);
        let perp = 0.4 + size / 2.0;
        // Number sits above a horizontal pin / left of a vertical pin, centred.
        // `sch-pin-number` → KiCad's `pin_number` colour (a separate red).
        if horizontal {
            emit_pin_text(s, &pin.number, mx, my - perp, false, "middle", size, "sch-pin-number");
        } else {
            emit_pin_text(s, &pin.number, mx - perp, my, true, "middle", size, "sch-pin-number");
        }
    }
    s.push_str("</g>");
}

/// Emit a single line of pin text. `vertical` rotates it to read bottom-to-top
/// (KiCad keeps pin text upright); `anchor` is the SVG text-anchor; `cls` is the
/// class the app themes by (pin name vs number get distinct KiCad colours).
fn emit_pin_text(s: &mut String, text: &str, x: f64, y: f64, vertical: bool, anchor: &str, size_mm: f64, cls: &str) {
    let transform = if vertical {
        format!(r#" transform="rotate(-90 {} {})""#, c(x), c(y))
    } else {
        String::new()
    };
    let _ = write!(
        s,
        r#"<text class="{}" x="{}" y="{}" font-size="{}" text-anchor="{}" dominant-baseline="central" stroke="none"{}>{}</text>"#,
        cls, c(x), c(y), c(size_mm * 4.0 / 3.0), anchor, transform, render_markup(text)
    );
}

/// Render a net-class directive flag: the stub from its attachment point to the
/// terminal glyph, the glyph, and the class name when not hidden.
fn render_netclass_flag(s: &mut String, f: &NetclassFlag, colors: &NetColors) {
    // KiCad's directive-label leader points away from the wire by its spin style:
    // angle 0 → up, 90 → left, 180 → down, 270 → right (screen space, Y-down).
    // That is the vector (−sinθ, −cosθ), not (cosθ, sinθ) — verified against the
    // CAN sheet's five flags (e.g. the angle-0 flag's glyph sits directly above).
    let a = f.at.angle.to_radians();
    let (ex, ey) = (f.at.x - f.length * a.sin(), f.at.y - f.length * a.cos());
    // The directive draws in its net class's colour (same colour KiCad tints the
    // class's wires with); the glyph is filled with it, the stub stroked with it.
    let color = color_for(colors, &f.uuid);
    let stroke = color.map(|c| format!(r#" stroke="{c}""#)).unwrap_or_default();
    let fill = color.unwrap_or("none");
    let _ = write!(
        s,
        r#"<g data-primitive="netclass-flag" data-uuid="{}"{stroke}>"#,
        esc(&f.uuid)
    );
    let _ = write!(s, r#"<polyline points="{},{} {},{}"/>"#, c(f.at.x), c(f.at.y), c(ex), c(ey));
    match f.shape.as_str() {
        "rectangle" | "dot_rectangle" => {
            let r = 0.8;
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}"/>"#,
                c(ex - r), c(ey - r), c(2.0 * r), c(2.0 * r)
            );
        }
        _ => {
            let _ = write!(s, r#"<circle cx="{}" cy="{}" r="0.7" fill="{fill}"/>"#, c(ex), c(ey));
        }
    }
    if !f.netclass_hidden && !f.netclass.is_empty() {
        let at = f.netclass_at.unwrap_or(f.at);
        emit_text_block(s, &f.netclass, at.x, at.y, at.angle, &f.effects, None);
    }
    s.push_str("</g>");
}

/// Render a sheet-symbol pin's hierarchical label. KiCad draws the pin name just
/// *inside* the sheet box, reading away from the edge the pin sits on. We pick the
/// edge from the pin's position relative to the box, then anchor the text inward
/// so it never spills past the boundary — the old code left every label
/// start-anchored, so right-edge pins (e.g. the root sheet's driver outputs) ran
/// outside the box and vertical-edge pins weren't rotated.
fn emit_sheet_pin_label(
    s: &mut String,
    sr: &eda_parse_kicad::schematic::SheetRef,
    p: &eda_parse_kicad::schematic::SheetPin,
    colors: &NetColors,
) {
    const GAP: f64 = 0.6; // clearance from the box edge to the text, mm
    const SIZE: f64 = 1.27; // KiCad default sheet-pin glyph height
    let font = c(SIZE * 4.0 / 3.0);
    let (x0, y0) = (sr.at.x, sr.at.y);
    let (x1, y1) = (sr.at.x + sr.size.0, sr.at.y + sr.size.1);
    // Nearest box edge decides the side (and thus the anchor/orientation).
    let (dl, dr) = ((p.at.x - x0).abs(), (p.at.x - x1).abs());
    let (dt, db) = ((p.at.y - y0).abs(), (p.at.y - y1).abs());
    let min = dl.min(dr).min(dt).min(db);
    let name = render_markup(&p.name);
    // Sheet pins are net-class driven, like labels: the app themes them via
    // `var(--nc, --sch-sheet-label)`, so bake the net colour (when the pin's net has
    // a class colour) as `--nc` + a matching fill. The `sch-sheet-pin` class lets
    // the app theme the rest from the KiCad palette without assuming a colour.
    let col = color_for(colors, &p.uuid);
    let nc = nc_style(col);
    let fill = col.map(|c| format!(r#" fill="{c}""#)).unwrap_or_default();
    let attrs = format!(r#"class="sch-sheet-pin"{nc} font-size="{font}" dominant-baseline="central" stroke="none"{fill}"#);
    if min == dl {
        // Left edge → text runs right, into the box.
        let _ = write!(s, r#"<text {attrs} x="{}" y="{}" text-anchor="start">{name}</text>"#, c(p.at.x + GAP), c(p.at.y));
    } else if min == dr {
        // Right edge → text runs left, into the box.
        let _ = write!(s, r#"<text {attrs} x="{}" y="{}" text-anchor="end">{name}</text>"#, c(p.at.x - GAP), c(p.at.y));
    } else if min == dt {
        // Top edge → vertical text dropping down into the box.
        let (tx, ty) = (c(p.at.x), c(p.at.y + GAP));
        let _ = write!(s, r#"<text {attrs} x="{tx}" y="{ty}" text-anchor="end" transform="rotate(-90 {tx} {ty})">{name}</text>"#);
    } else {
        // Bottom edge → vertical text rising up into the box.
        let (tx, ty) = (c(p.at.x), c(p.at.y - GAP));
        let _ = write!(s, r#"<text {attrs} x="{tx}" y="{ty}" text-anchor="start" transform="rotate(-90 {tx} {ty})">{name}</text>"#);
    }
}

/// KiCad's I/O "flag shape" for a global/hierarchical label (`(shape …)`), which
/// selects the drawn glyph. The connection-end arrow tells the reader the signal
/// direction; without it every label reads as an output (the old single glyph).
#[derive(Clone, Copy, PartialEq)]
enum FlagShape {
    Input,
    Output,
    Bidi,
    TriState,
    Passive,
}

impl FlagShape {
    fn from_kicad(s: Option<&str>) -> Self {
        match s {
            Some("input") => FlagShape::Input,
            Some("output") => FlagShape::Output,
            Some("bidirectional") => FlagShape::Bidi,
            Some("tri_state") => FlagShape::TriState,
            Some("passive") => FlagShape::Passive,
            // KiCad's no-/unknown-shape fallback; also keeps the pre-shape glyph
            // (a single output-style pentagon) for any label that carries none.
            _ => FlagShape::Output,
        }
    }
}

/// Small flag glyph for a *hierarchical* label, in local space (connection at the
/// origin, text reading +x). The pointed end encodes direction: output points
/// away from the wire (+x), input back at the wire (origin), bidi/tri-state both,
/// passive is a plain rectangle. Output is byte-for-byte the old single glyph.
fn hier_flag_points(shape: FlagShape, h: f64) -> Vec<(f64, f64)> {
    let half = h * 0.6;
    let tip = h * 1.2;
    match shape {
        FlagShape::Output => {
            vec![(0.0, -half), (tip, -half), (tip + half, 0.0), (tip, half), (0.0, half), (0.0, -half)]
        }
        FlagShape::Input => {
            vec![(0.0, 0.0), (half, -half), (tip + half, -half), (tip + half, half), (half, half), (0.0, 0.0)]
        }
        FlagShape::Bidi | FlagShape::TriState => vec![
            (0.0, 0.0),
            (half, -half),
            (tip, -half),
            (tip + half, 0.0),
            (tip, half),
            (half, half),
            (0.0, 0.0),
        ],
        FlagShape::Passive => {
            vec![(0.0, -half), (tip + half, -half), (tip + half, half), (0.0, half), (0.0, -half)]
        }
    }
}

/// Outline of a *global* label's box (KiCad draws the text inside an elongated
/// hexagon "tag"), in local space (connection at the origin, text reading +x).
/// `tw` is the displayed text width (mm). Returns the closed outline plus the
/// body's left edge + length so the caller can centre the text inside it. The
/// connection end is arrowed by the I/O shape, mirroring KiCad's appearance.
fn global_box_points(shape: FlagShape, h: f64, tw: f64) -> (Vec<(f64, f64)>, f64, f64) {
    let margin = h * 0.375;
    let hs = h * 0.5 + margin; // half box height
    let arrow = hs; // arrow depth = half height (45° point, as KiCad)
    let len = tw + 2.0 * margin; // body length around the text
    let pts = match shape {
        FlagShape::Output => {
            vec![(0.0, -hs), (len, -hs), (len + arrow, 0.0), (len, hs), (0.0, hs), (0.0, -hs)]
        }
        FlagShape::Input => vec![
            (0.0, 0.0),
            (arrow, -hs),
            (len + arrow, -hs),
            (len + arrow, hs),
            (arrow, hs),
            (0.0, 0.0),
        ],
        FlagShape::Bidi | FlagShape::TriState => vec![
            (0.0, 0.0),
            (arrow, -hs),
            (len + arrow, -hs),
            (len + 2.0 * arrow, 0.0),
            (len + arrow, hs),
            (arrow, hs),
            (0.0, 0.0),
        ],
        FlagShape::Passive => vec![(0.0, -hs), (len, -hs), (len, hs), (0.0, hs), (0.0, -hs)],
    };
    let body_left = match shape {
        FlagShape::Input | FlagShape::Bidi | FlagShape::TriState => arrow,
        _ => 0.0,
    };
    (pts, body_left, len)
}

/// Render a net label. Local labels are bare text on the wire; global labels draw
/// KiCad's hexagon "tag" wrapping the text; hierarchical labels draw a small I/O
/// flag glyph beside the text. A net-class colour tints the *text* only — KiCad
/// draws the flag/tag outline in the label-layer colour (global red, hier olive)
/// regardless of the net colour, and recolours just the text.
fn render_label(s: &mut String, l: &Label, colors: &NetColors) {
    // Global and hierarchical labels are both "port" glyphs, but KiCad colours them
    // differently (label_global vs label_hier) — so tag the kind, letting the app
    // theme each from its own palette key instead of assuming one colour for both.
    let (prim, kind) = match l.kind {
        LabelKind::Local => ("label", ""),
        LabelKind::Global => ("port", r#" data-kind="global""#),
        LabelKind::Hierarchical => ("port", r#" data-kind="hier""#),
    };
    let color = color_for(colors, &l.uuid);
    let _ = write!(
        s,
        r#"<g data-primitive="{prim}"{kind} data-uuid="{}"{}>"#,
        esc(&l.uuid),
        nc_style(color)
    );

    // Place a local-space point at l.at, rotated by the label angle. KiCad label
    // angles are CCW in the *displayed* (Y-up) view, but sheet space here is Y-down
    // — so rotate by −angle (flip the sin/Y terms). Horizontal labels (0/180) are
    // unaffected; vertical ones (90/270) would otherwise land on the wrong side of
    // the wire (BOARD_TEMP on the PTC sheet pointed onto its own wire).
    let ang = l.at.angle.to_radians();
    let (ca, sa) = (ang.cos(), ang.sin());
    let place = |lx: f64, ly: f64| (l.at.x + lx * ca + ly * sa, l.at.y - lx * sa + ly * ca);

    match l.kind {
        LabelKind::Global => {
            // The tag wraps the text, so it must be sized from the *displayed* text
            // (markup stripped) — the raw `~{…}{slash}…` is much longer. The width
            // estimate is per-glyph in the *rendered* font, not KiCad's stroke size:
            // text renders at `size·4/3`, and Newstroke's mean advance is ~0.68 of
            // that, so ~0.9·size per char keeps a real string ("boot_mode") inside
            // the tag instead of spilling out of it (the 0.6 here sized the box to
            // the glyph height, which is narrower than the text actually draws).
            let h = l.effects.size.max(0.5);
            let tw = display_text(&l.text).chars().count() as f64 * h * 0.9;
            let (local, body_left, len) =
                global_box_points(FlagShape::from_kicad(l.shape.as_deref()), h, tw);
            let pts: Vec<(f64, f64)> = local.iter().map(|&(x, y)| place(x, y)).collect();
            let _ = write!(s, r#"<polyline points="{}"/>"#, pts_attr(&pts));
            // Text centred in the box body and kept upright.
            let (cx, cy) = place(body_left + len / 2.0, 0.0);
            let mut fx = l.effects.clone();
            fx.h_justify = HJustify::Center;
            fx.v_justify = VJustify::Center;
            emit_text_block(s, &l.text, cx, cy, l.at.angle, &fx, color);
        }
        LabelKind::Hierarchical => {
            let h = l.effects.size.max(0.5);
            let local = hier_flag_points(FlagShape::from_kicad(l.shape.as_deref()), h);
            let pts: Vec<(f64, f64)> = local.iter().map(|&(x, y)| place(x, y)).collect();
            let _ = write!(s, r#"<polyline points="{}"/>"#, pts_attr(&pts));
            // Text reads outward, just past the flag; default unjustified to left.
            let mut fx = l.effects.clone();
            if fx.h_justify == HJustify::Center {
                fx.h_justify = HJustify::Left;
            }
            let gap = h * 2.0;
            let (tx, ty) = (l.at.x + gap * ca, l.at.y - gap * sa);
            emit_text_block(s, &l.text, tx, ty, l.at.angle, &fx, color);
        }
        LabelKind::Local => {
            // KiCad lifts the text a small perpendicular gap off the wire
            // (SCH_LABEL::GetSchematicTextOffset) so the name floats just above a
            // horizontal wire / just left of a vertical one rather than on the copper.
            let mut fx = l.effects.clone();
            if fx.h_justify == HJustify::Center {
                fx.h_justify = HJustify::Left;
            }
            let gap = l.effects.size.max(0.5) * 0.2;
            let a = ((l.at.angle % 360.0) + 360.0) % 360.0;
            let (tx, ty) = if (a - 90.0).abs() < 1.0 || (a - 270.0).abs() < 1.0 {
                (l.at.x - gap, l.at.y) // vertical text → nudge left of the wire
            } else {
                (l.at.x, l.at.y - gap) // horizontal text → nudge above the wire
            };
            emit_text_block(s, &l.text, tx, ty, l.at.angle, &fx, color);
        }
    }
    s.push_str("</g>");
}

/// Emit a (possibly multi-line) text block honouring KiCad justification and the
/// keep-upright rule. `color` overrides the effects fill when set (net classes).
fn emit_text_block(
    s: &mut String,
    text: &str,
    x: f64,
    y: f64,
    angle: f64,
    effects: &TextEffects,
    color: Option<&str>,
) {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // Split into lines the way KiCad's wxStringSplit does: a SINGLE trailing newline is
    // just the line terminator and is dropped — that keeps a bottom-justified note from
    // floating a line high (the demo board's "Possible Comms" title sat right on the
    // body note below it when we kept it). But any further blank rows are real lines
    // KiCad reserves height for, so "X\n\n" is a 2-line block; bottom-justified that
    // lifts "X" one interline above its anchor. Popping *every* trailing blank (the old
    // `while`) dropped that row and sat the text a line too low.
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    let n = lines.len() as f64;
    let line_h = effects.size * 4.0 / 3.0 * 1.2;
    // First-line offset along the (pre-rotation) text-down axis for vertical
    // justification of the whole block: top → first line at y; center → block
    // straddles y; bottom → last line at y.
    let first_dy = match effects.v_justify {
        VJustify::Top => 0.0,
        VJustify::Center => -(n - 1.0) / 2.0 * line_h,
        VJustify::Bottom => -(n - 1.0) * line_h,
    };
    let _ = write!(
        s,
        r#"<text x="{}" y="{}"{}{}>"#,
        c(x), c(y), text_attrs(effects, color),
        text_transform(angle, effects.mirror, x, y)
    );
    // A blank line carries its height onto the NEXT glyph-bearing line. An empty
    // `<tspan dy=…>` has no glyph for the browser to hang the dy on, so its row would
    // collapse — the missing gaps between blank-separated note lines. Folding the
    // advance into the following tspan keeps the blank rows without an empty element.
    let mut pending = 0.0;
    for (i, line) in lines.iter().enumerate() {
        let step = if i == 0 { first_dy } else { line_h };
        if line.is_empty() {
            pending += step;
            continue;
        }
        let _ = write!(s, r#"<tspan x="{}" dy="{}">{}</tspan>"#, c(x), c(step + pending), render_markup(line));
        pending = 0.0;
    }
    s.push_str("</text>");
}

/// Adjust a field's justification for the symbol's orientation so it extends the
/// way KiCad draws it. KiCad keeps glyphs upright/readable, but the anchor side
/// (which corner the text grows from) follows the symbol's rotation+mirror, and
/// the rule is *not* a function of the net text angle alone: a right-justified
/// field reads END on an un-rotated part (R30) but START on a 90°-rotated part
/// (R32) even though both render horizontally. So the flip is keyed on the symbol
/// rotation `sym_angle`, while `render_angle` (the field's on-screen angle =
/// `field_angle − sym_angle`) only decides whether the text is vertical.
///
/// The flip sets below were derived empirically and check against every visible
/// field of a dense production design (vs KiCad's own PDF): horizontal text flips H
/// for `sym_angle ∈ {90°, 180°}`, vertical text flips H for `{180°, 270°}` (the
/// keep-upright readable range shifts by 90° between orientations); `mirror y`
/// flips H, `mirror x` flips V, composed on top.
fn oriented_effects(e: &TextEffects, sym_angle: f64, render_angle: f64, mirror: Option<&str>) -> TextEffects {
    let norm = |a: f64| ((a % 360.0) + 360.0) % 360.0;
    let near = |a: f64, t: f64| (a - t).abs() < 1.0;
    let sa = norm(sym_angle);
    let sa90 = near(sa, 90.0);
    let sa180 = near(sa, 180.0);
    let sa270 = near(sa, 270.0);
    // Vertical when the on-screen angle is an odd multiple of 90° (reads bottom→top).
    let vertical = near(norm(render_angle) % 180.0, 90.0);
    let mut flip_h = if vertical { sa180 || sa270 } else { sa90 || sa180 };
    let mut flip_v = sa180 || sa270;
    // A mirror flips the justify on its own screen axis. For upright text that means
    // mirror-y → horizontal justify, mirror-x → vertical justify. But vertical
    // (rotated 90°) text has its screen X/Y axes swapped, so the same mirror flips the
    // *other* justify. Verified against KiCad's own plot: the mirror-y vertical
    // resistor ref/value fields (R43/R44/R48 …) keep their left anchor and read
    // upward — flipping their horizontal justify wrongly drove the text downward.
    match mirror {
        Some("y") if vertical => flip_v = !flip_v,
        Some("y") => flip_h = !flip_h,
        Some("x") if vertical => flip_h = !flip_h,
        Some("x") => flip_v = !flip_v,
        _ => {}
    }
    let mut e = e.clone();
    if flip_h {
        e.h_justify = match e.h_justify {
            HJustify::Left => HJustify::Right,
            HJustify::Right => HJustify::Left,
            HJustify::Center => HJustify::Center,
        };
    }
    if flip_v {
        e.v_justify = match e.v_justify {
            VJustify::Top => VJustify::Bottom,
            VJustify::Bottom => VJustify::Top,
            VJustify::Center => VJustify::Center,
        };
    }
    e
}

/// Common SVG text presentation attributes for a KiCad `(effects)` block:
/// KiCad's glyph height maps to the SVG em via ~4/3, justification sets the
/// anchor and baseline, and weight/style/face/colour are emitted when set.
/// `color_override` (a net-class colour) wins over the effects colour.
fn text_attrs(e: &TextEffects, color_override: Option<&str>) -> String {
    let anchor = match e.h_justify {
        HJustify::Left => "start",
        HJustify::Right => "end",
        HJustify::Center => "middle",
    };
    let baseline = match e.v_justify {
        VJustify::Top => "hanging",
        VJustify::Bottom => "auto",
        VJustify::Center => "central",
    };
    // `stroke="none"`: glyphs are filled from the (Newstroke) font only. Otherwise
    // they inherit the SVG root stroke and render as a fill+outline — far heavier
    // than KiCad's thin stroke-font text (the "very thick" report).
    let mut out = String::from(r#" stroke="none""#);
    let _ = write!(
        out,
        r#" font-size="{}" text-anchor="{}" dominant-baseline="{}""#,
        c(e.size * 4.0 / 3.0),
        anchor,
        baseline
    );
    if e.bold {
        out.push_str(r#" font-weight="bold""#);
    }
    if e.italic {
        out.push_str(r#" font-style="italic""#);
    }
    if let Some(face) = &e.face {
        let _ = write!(out, r#" font-family="{}""#, esc(face));
    }
    if let Some(col) = color_override {
        let _ = write!(out, r#" fill="{}""#, col);
    } else if let Some((r, g, b, _)) = e.color {
        let _ = write!(out, r##" fill="#{:02X}{:02X}{:02X}""##, r, g, b);
    }
    out
}

/// `transform="…"` for a text element: an optional horizontal mirror about its
/// anchor plus the rotation. KiCad CCW → SVG CW, and KiCad keeps text upright, so
/// a 180°/270° field renders at angle−180 (0°/90°) — glyphs are never upside-down.
pub(crate) fn text_transform(angle: f64, mirror: bool, x: f64, y: f64) -> String {
    let mut a = angle % 360.0;
    if a < 0.0 {
        a += 360.0;
    }
    if a >= 180.0 {
        a -= 180.0;
    }
    let mut parts: Vec<String> = Vec::new();
    if mirror {
        parts.push(format!("matrix(-1 0 0 1 {} 0)", c(2.0 * x)));
    }
    if a != 0.0 {
        parts.push(format!("rotate({} {} {})", c(-a), c(x), c(y)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(r#" transform="{}""#, parts.join(" "))
    }
}

/// SVG `fill` value for a symbol-graphic fill style. Background fills become the
/// `#FFFFFF` the app recolors to its body-fill token; outline fills are solid
/// foreground (`#000000`, themed dark by the symbol stroke rules).
fn fill_attr(f: Fill) -> &'static str {
    match f {
        Fill::None => "none",
        Fill::Background => "#FFFFFF",
        Fill::Outline => "#000000",
    }
}

/// Coordinate points of a shape, for extent computation (untransformed).
fn shape_points(shape: &Shape) -> Vec<(f64, f64)> {
    match shape {
        Shape::Rect { a, b, .. } => vec![(a.x, a.y), (b.x, b.y)],
        Shape::Poly { pts, .. } => pts.iter().map(|p| (p.x, p.y)).collect(),
        Shape::Circle { center, radius, .. } => vec![
            (center.x - radius, center.y - radius),
            (center.x + radius, center.y + radius),
        ],
        Shape::Arc { start, mid, end } => vec![(start.x, start.y), (mid.x, mid.y), (end.x, end.y)],
    }
}

/// Circumcentre of three points, or `None` when they are collinear.
fn circumcenter(
    (ax, ay): (f64, f64),
    (bx, by): (f64, f64),
    (cx, cy): (f64, f64),
) -> Option<(f64, f64)> {
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-9 {
        return None;
    }
    let a2 = ax * ax + ay * ay;
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    Some((ux, uy))
}

/// SVG path `d` for a circular arc through start→mid→end (already transformed to
/// sheet coordinates). Falls back to a two-segment polyline when degenerate.
fn arc_path(start: (f64, f64), mid: (f64, f64), end: (f64, f64)) -> String {
    let Some((cx, cy)) = circumcenter(start, mid, end) else {
        return format!(
            "M {} {} L {} {} L {} {}",
            c(start.0), c(start.1), c(mid.0), c(mid.1), c(end.0), c(end.1)
        );
    };
    let r = ((start.0 - cx).powi(2) + (start.1 - cy).powi(2)).sqrt();
    let ang = |p: (f64, f64)| (p.1 - cy).atan2(p.0 - cx);
    let two_pi = std::f64::consts::TAU;
    let norm = |x: f64| ((x % two_pi) + two_pi) % two_pi;
    let (a_s, a_m, a_e) = (ang(start), ang(mid), ang(end));
    // Does the increasing-angle (SVG-positive) arc from start to end pass mid?
    let incr_se = norm(a_e - a_s);
    let incr_sm = norm(a_m - a_s);
    let go_incr = incr_sm < incr_se;
    let span = if go_incr { incr_se } else { two_pi - incr_se };
    let large = if span > std::f64::consts::PI { 1 } else { 0 };
    let sweep = if go_incr { 1 } else { 0 };
    format!(
        "M {} {} A {} {} 0 {} {} {} {}",
        c(start.0), c(start.1), c(r), c(r), large, sweep, c(end.0), c(end.1)
    )
}

/// Emit one library-symbol body shape, transformed to sheet coordinates. `width`
/// is the shape's explicit stroke width (mm); `0` inherits the SVG default so
/// most symbols stay hairline, while a deliberately heavy stroke (e.g. the
/// flux-concentrator "C" at 3.048 mm) renders at its true thickness.
fn render_shape(s: &mut String, shape: &Shape, width: f64, tf: &impl Fn(f64, f64) -> (f64, f64)) {
    let w = if width > 0.0 {
        format!(r#" stroke-width="{}""#, c(width))
    } else {
        String::new()
    };
    match shape {
        Shape::Rect { a, b, fill } => {
            let pts = [
                tf(a.x, a.y),
                tf(b.x, a.y),
                tf(b.x, b.y),
                tf(a.x, b.y),
                tf(a.x, a.y),
            ];
            let _ = write!(
                s,
                r##"<polygon points="{}" fill="{}"{w}/>"##,
                pts_attr(&pts),
                fill_attr(*fill)
            );
        }
        Shape::Poly { pts, fill } => {
            let tp: Vec<(f64, f64)> = pts.iter().map(|p| tf(p.x, p.y)).collect();
            if *fill == Fill::None {
                let _ = write!(s, r#"<polyline points="{}"{w}/>"#, pts_attr(&tp));
            } else {
                let _ = write!(s, r##"<polygon points="{}" fill="{}"{w}/>"##, pts_attr(&tp), fill_attr(*fill));
            }
        }
        Shape::Circle { center, radius, fill } => {
            let (cx, cy) = tf(center.x, center.y);
            let _ = write!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="{}"{w}/>"##,
                c(cx), c(cy), c(*radius), fill_attr(*fill)
            );
        }
        Shape::Arc { start, mid, end } => {
            let d = arc_path(tf(start.x, start.y), tf(mid.x, mid.y), tf(end.x, end.y));
            let _ = write!(s, r#"<path d="{}"{w}/>"#, d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCH: &str = r#"
    (kicad_sch
      (lib_symbols (symbol "Device:R" (symbol "R_1_1"
        (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
        (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (pin "1" (uuid "r1p1")) (pin "2" (uuid "r1p2")) (instances))
      (wire (pts (xy 100 102.54) (xy 100 110)) (uuid "w1"))
      (junction (at 100 102.54) (uuid "j1"))
      (label "MID" (at 100 102.54 0) (uuid "lbl1"))
      (text "DNP for 48V variant\nsee rev notes" (at 120 100 0) (uuid "note1"))
      (image (at 130 90) (scale 1.0) (uuid "img1")
        (data "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")))
    "#;

    fn render(sch: &Schematic) -> String {
        render_sheet(sch, &NetColors::new(), None, &SchStyle::default())
    }

    // Anchor side a field renders with, for the oriented-justify rule. Pinned to
    // KiCad's own output for a production design's encoder sheet (every case derived from
    // the plotted PDF): a field's anchor follows the *symbol* rotation, not the net
    // text angle, so right-justify reads END un-rotated (R30) but START at 90° (R32).
    #[test]
    fn field_justify_follows_symbol_orientation() {
        let eff = |h: HJustify| {
            let mut e = TextEffects::default();
            e.h_justify = h;
            e
        };
        // (sym_angle, render_angle, base_h, mirror) -> expected on-screen h_justify
        let l = HJustify::Left;
        let r = HJustify::Right;
        let cases = [
            // horizontal text
            (0.0, 0.0, l, None, HJustify::Left),       // TP33 etc
            (0.0, 0.0, r, None, HJustify::Right),      // R30/R35/9.1k
            (0.0, 0.0, r, Some("x"), HJustify::Right), // R29/R34/4.7k (mirror x = V only)
            (0.0, 0.0, l, Some("y"), HJustify::Right), // mirror y flips H
            (90.0, 0.0, r, None, HJustify::Left),      // R32/R36/100R-top
            (90.0, 0.0, r, Some("x"), HJustify::Left), // R47/R33/100R-bot
            (180.0, 0.0, l, None, HJustify::Right),    // C139/TP63
            (270.0, 180.0, l, None, HJustify::Left),   // D13/D8/D7/D9 refs (render horizontal)
            // vertical text (render angle ~90 → reads bottom→top)
            (270.0, 90.0, l, None, HJustify::Right),   // D13/D8 SMAJ value (anchor at top, text down)
            (0.0, 90.0, l, None, HJustify::Left),      // vertical, sym 0, left
            (0.0, 90.0, r, None, HJustify::Right),     // vertical, sym 0, right
            // vertical text + mirror: the mirror flips the justify on the OTHER screen
            // axis than for upright text. Verified against KiCad's plot of the
            // mirror-y vertical resistor ref/value fields (R43/R44/R48): they keep the
            // left anchor and read upward — a mirror-y must NOT flip H here.
            (0.0, 90.0, l, Some("y"), HJustify::Left),  // R43/R44/R48 ref+value
            (0.0, 90.0, l, Some("x"), HJustify::Right), // mirror-x vertical flips H instead
        ];
        for (sa, ra, base, mir, want) in cases {
            let got = oriented_effects(&eff(base), sa, ra, mir).h_justify;
            assert_eq!(got, want, "sym={sa} render={ra} base={base:?} mirror={mir:?}");
        }
    }

    #[test]
    fn renders_lean_svg_with_uuids() {
        let sch = Schematic::parse_str(SCH).unwrap();
        let svg = render(&sch);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        // Elements wrapped in data-primitive groups the viewer CSS themes.
        assert!(svg.contains(r#"<g data-primitive="wire" data-uuid="w1">"#));
        assert!(svg.contains(r#"<g data-primitive="junction" data-uuid="j1">"#));
        assert!(svg.contains(r#"data-primitive="port" data-uuid="lbl1""#)
            || svg.contains(r#"data-primitive="label" data-uuid="lbl1""#));
        assert!(svg.contains(r#"<g data-primitive="symbol" data-uuid="r1" data-ref="R1">"#));
        // Pin is its own group, net-addressable and pin-clickable.
        assert!(svg.contains(r#"data-primitive="pin" data-uuid="r1p1" data-designator="R1" data-pin="1""#));
        // Notes and images are rendered (LLM/vision context).
        assert!(svg.contains(r#"<g data-primitive="text" data-uuid="note1">"#));
        assert!(svg.contains("DNP for 48V variant"));
        assert!(svg.contains(r#"<g data-primitive="image" data-uuid="img1">"#));
        assert!(svg.contains("data:image/png;base64,"));
        // No embedded JSON metadata blob (the bloat we removed).
        assert!(!svg.contains("<metadata"));
    }

    #[test]
    fn note_hyperlink_emits_data_href() {
        // A `(text …)` note carrying `(effects (href …))` is rendered as a clickable
        // group: the viewer opens the link when the text is clicked (batch3 item 5).
        let src = r#"
        (kicad_sch
          (text "Link to design sheet" (at 100 100 0) (uuid "lnk1")
            (effects (font (size 1.27 1.27)) (href "https://example.com/x")))
          (text "plain" (at 50 50 0) (uuid "n2") (effects (font (size 1.27 1.27)))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(r#"data-uuid="lnk1" data-href="https://example.com/x""#));
        // A note without a link carries no data-href.
        assert!(svg.contains(r#"data-primitive="text" data-uuid="n2">"#));
    }

    #[test]
    fn reads_png_dimensions() {
        // 1x1 PNG.
        let d = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        assert_eq!(png_dims(d), Some((1, 1)));
        assert_eq!(png_dims("not-a-png"), None);
    }

    #[test]
    fn renders_pin_names_numbers_and_dnp() {
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:U" (in_bom yes)
            (symbol "U_0_1" (rectangle (start -5 5) (end 5 -5) (fill (type background))))
            (symbol "U_1_1"
              (pin input line (at -7.62 2.54 0) (length 2.54)
                (name "IN" (effects (font (size 1.27 1.27))))
                (number "1" (effects (font (size 1.27 1.27))))))))
          (symbol (lib_id "lib:U") (at 100 100 0) (unit 1) (uuid "u1") (dnp yes)
            (property "Reference" "U1") (pin "1" (uuid "u1p1")) (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // Pin name and number carry distinct classes so the app themes them with
        // KiCad's separate pin_name / pin_number colours (not one shared colour).
        assert!(svg.contains(r#"class="sch-pin-name""#) && svg.contains(">IN</text>"), "pin name drawn + classed");
        assert!(svg.contains(r#"class="sch-pin-number""#) && svg.contains(">1</text>"), "pin number drawn + classed");
        assert!(svg.contains(r#"data-primitive="dnp""#), "dnp cross drawn");
        // The whole DNP symbol is dimmed (greyed) — body, pins and fields.
        assert!(
            svg.contains(r#"data-ref="U1" data-dnp="1" opacity="0.45""#),
            "dnp symbol group dimmed"
        );
    }

    #[test]
    fn dnp_cross_spans_body_not_pins_and_renders_undimmed() {
        // A one-sided connector: body box on the right, a single pin extending left.
        // The DNP cross must straddle the body box centre (not the pin-inclusive
        // bbox, which would skew it left over the pins), and must be emitted OUTSIDE
        // the dimmed symbol group so KiCad's bright marker isn't greyed with the body.
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:CONN"
            (symbol "CONN_0_1" (rectangle (start 0 2.54) (end -2.54 -7.62) (fill (type background))))
            (symbol "CONN_1_1" (pin passive line (at -7.62 0 0) (length 5.08)
              (name "P1" (effects (font (size 1.27 1.27))))
              (number "1" (effects (font (size 1.27 1.27))))))))
          (symbol (lib_id "lib:CONN") (at 50 50 0) (unit 1) (uuid "j1") (dnp yes)
            (property "Reference" "J9" (at 52 50 0)) (pin "1" (uuid "j1p1")) (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // Cross corners come from the body box (placed at 50,50; tf is x→50+x, y→50−y):
        // x ∈ {47.46, 50}, never the pin tip at screen x=42.38.
        assert!(svg.contains(r#"points="47.46,57.62 50,47.46""#), "cross diagonal on body box: {svg}");
        assert!(svg.contains(r#"points="47.46,47.46 50,57.62""#), "cross anti-diagonal on body box");
        assert!(!svg.contains("42.38 50,"), "cross must not reach the pin tip (full-bbox skew)");
        // The cross is a sibling AFTER the symbol group closes (full opacity, not dimmed),
        // and keeps its baked DNP-marker colour (default red here).
        assert!(
            svg.contains(r##"</g><g data-primitive="dnp" stroke="#EB0000" stroke-width="0.3">"##),
            "dnp cross emitted outside the dimmed group, undimmed: {svg}"
        );
        // The dimmed symbol group itself must not contain the cross.
        let grp = &svg[svg.find(r#"data-ref="J9""#).unwrap()..];
        let body_end = grp.find("</g>").unwrap();
        assert!(!grp[..body_end].contains(r#"data-primitive="dnp""#), "cross not inside symbol group");
    }

    #[test]
    fn heavy_lib_arc_keeps_its_stroke_width() {
        // The flux-concentrator "C" is a 3.048 mm arc; it must not collapse to a
        // hairline (feedback 17).
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:FC"
            (symbol "FC_0_1" (arc (start -0.2 -17) (mid -15.5 -11.7) (end -0.6 -5.3)
              (stroke (width 3.048)) (fill (type none))))))
          (symbol (lib_id "lib:FC") (at 71 55 0) (unit 1) (uuid "fc1")
            (property "Reference" "FC1") (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains("<path d=\"M "), "arc renders as a path");
        assert!(svg.contains(r#" stroke-width="3.048""#), "heavy arc keeps its width");
    }

    #[test]
    fn netclass_flag_leader_points_by_spin() {
        // angle 0 → leader up (−y), angle 270 → leader right (+x).
        let src = r#"
        (kicad_sch
          (netclass_flag "" (length 2.54) (shape round) (at 100 100 0) (uuid "nf0")
            (property "Netclass" "Iso" (effects (hide yes))))
          (netclass_flag "" (length 2.54) (shape round) (at 100 100 270) (uuid "nf270")
            (property "Netclass" "Iso" (effects (hide yes)))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(r#"points="100,100 100,97.46"#), "angle 0 leader goes up");
        assert!(svg.contains(r#"points="100,100 102.54,100"#), "angle 270 leader goes right");
    }

    #[test]
    fn text_box_draws_a_border() {
        let src = r#"
        (kicad_sch (paper "A4")
          (text_box "NOTE:\nheatsink" (at 46 132 0) (size 50 14)
            (stroke (width 0) (type default)) (fill (type none))
            (effects (font (size 1.27 1.27)) (justify left top)) (uuid "tb1")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(
            svg.contains(r#"<g data-primitive="text" data-uuid="tb1"><rect x="46" y="132" width="50" height="14""#),
            "text_box border rect drawn at its box"
        );
    }

    #[test]
    fn text_box_fills_and_wraps_long_text() {
        // The vme-wren root sheet's GPIO call-out: a yellow-filled box whose long
        // sentence has no hard newlines, so it must reflow to the box width instead
        // of running past the border (the bug: text overran the box with no fill).
        let src = r#"
        (kicad_sch (paper "A4")
          (text_box "Really slow VME control signals (some direction pins, Geographical Address and manual address switch) are handled by a GPIO expander to save on FPGA pins."
            (at 68.58 237.49 0) (size 40.64 20.32) (margins 0.9525 0.9525 0.9525 0.9525)
            (stroke (width 0) (type default)) (fill (type color) (color 255 255 194 1))
            (effects (font (size 1.27 1.27)) (justify left top)) (uuid "gpio")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // The pale-yellow fill is baked onto the box rect (the app themes only the
        // box stroke, so a presentation-attribute fill survives).
        assert!(svg.contains(r##"fill="#FFFFC2""##), "box fill colour baked: {svg}");
        // One long sentence, no `\n` → every <tspan> after the first is a wrap.
        let tspans = svg.matches("<tspan").count();
        assert!(tspans >= 3, "long text wrapped onto multiple lines, got {tspans}: {svg}");
    }

    #[test]
    fn wrap_text_keeps_words_whole_and_within_width() {
        let text = "Really slow VME control signals (some direction pins, Geographical Address and manual address switch) are handled by a GPIO expander to save on FPGA pins.";
        // Inner width of the vme-wren box (40.64 − 2·0.9525), 1.27 mm font.
        let inner = 40.64 - 2.0 * 0.9525;
        let wrapped = wrap_text(text, inner, 1.27);
        assert!(wrapped.split('\n').count() >= 3, "reflowed to several lines: {wrapped:?}");
        for line in wrapped.split('\n') {
            // Real rendered width stays inside the box (the bug: lines overran it). A
            // lone word wider than the box is the only allowed exception (left whole).
            let single_word = !line.trim().contains(' ');
            assert!(
                text_width(line, 1.27) <= inner || single_word,
                "line over the box width: {line:?}"
            );
        }
        // No word is chopped: the only inserted breaks replaced spaces, so undoing
        // them reproduces the source exactly.
        assert_eq!(wrapped.replace('\n', " "), text, "wrap split a word");
    }

    #[test]
    fn sheet_symbol_bakes_its_fill_colour() {
        // The root page tints sheets: a white sheet bakes #FFFFFF; a transparent
        // (alpha-0) sheet draws no fill so the canvas paper shows through.
        let src = r#"
        (kicad_sch (paper "A4")
          (sheet (at 50 50) (size 30 20)
            (stroke (width 0.1524) (type solid)) (fill (color 255 255 255 1.0000)) (uuid "s-white")
            (property "Sheetname" "io_drivers" (at 50 49 0) (effects (font (size 1.27 1.27))))
            (property "Sheetfile" "io.kicad_sch" (at 50 71 0) (effects (font (size 1.27 1.27)))))
          (sheet (at 100 50) (size 30 20)
            (stroke (width 0.1524) (type solid)) (fill (color 0 0 0 0.0000)) (uuid "s-none")
            (property "Sheetname" "misc" (at 100 49 0) (effects (font (size 1.27 1.27))))
            (property "Sheetfile" "misc.kicad_sch" (at 100 71 0) (effects (font (size 1.27 1.27))))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(
            svg.contains(r##"<rect x="50" y="50" width="30" height="20" fill="#FFFFFF"/>"##),
            "white sheet rect carries its baked fill: {svg}"
        );
        assert!(
            svg.contains(r#"<rect x="100" y="50" width="30" height="20"/>"#),
            "alpha-0 sheet rect stays unfilled: {svg}"
        );
    }

    #[test]
    fn pin_name_overbar_renders() {
        // SN74LVTH125 output-enable pin "1~{OE}" → "1" then overlined "OE", not the
        // raw `~{OE}` markup.
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "Standard Logic:SN74LVTH125RGY"
            (symbol "SN74LVTH125RGY_0_1" (rectangle (start -5 5) (end 5 -5) (fill (type none))))
            (symbol "SN74LVTH125RGY_1_1"
              (pin tri_state line (at -7.62 0 0) (length 2.54)
                (name "1~{OE}" (effects (font (size 1.27 1.27))))
                (number "1" (effects (font (size 1.27 1.27))))))))
          (symbol (lib_id "Standard Logic:SN74LVTH125RGY") (at 100 100 0) (unit 1) (uuid "u1")
            (property "Reference" "U1") (pin "1" (uuid "u1p1")) (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(
            svg.contains(r#"1<tspan text-decoration="overline">OE</tspan>"#),
            "pin-name overbar rendered: {svg}"
        );
        assert!(!svg.contains("~{OE}"), "raw overbar markup leaked: {svg}");
    }

    #[test]
    fn draws_frame_and_title_block_for_known_paper() {
        let src = r#"(kicad_sch (paper "A4") (title_block (title "Demo") (rev "1")))"#;
        let sch = Schematic::parse_str(src).unwrap();
        let comments = vec!["Licensed under Apache 2.0".to_string()];
        let frame = SheetFrame {
            number: 2, page: "", total: 5, name: "/power/", file: "power.kicad_sch",
            title: "Demo", company: "Acme", rev: "1", date: "2026",
            version: "9.0", comments: &comments,
        };
        let svg = render_sheet(&sch, &NetColors::new(), Some(frame), &SchStyle::default());
        // viewBox spans the full A4 page so nothing clips.
        assert!(svg.contains(r#"viewBox="0 0 297 210""#), "page-sized viewBox");
        assert!(svg.contains(r#"data-primitive="worksheet""#), "frame drawn");
        // KiCad-style title block cells: Title, Sheet path, File, Rev, Id.
        assert!(svg.contains(">Title: Demo</text>"));
        assert!(svg.contains(">Sheet: /power/</text>"));
        assert!(svg.contains(">File: power.kicad_sch</text>"));
        assert!(svg.contains(">Rev: 1</text>"));
        assert!(svg.contains(">Id: 2/5</text>"));
        // KiCad shows its version at the bottom-left; the company and comment lines
        // stack above the title rows, both inherited from the title block.
        assert!(svg.contains(">KiCad E.D.A. 9.0</text>"), "KiCad version line: {svg}");
        assert!(svg.contains(">Acme</text>"), "company shown");
        assert!(svg.contains(">Licensed under Apache 2.0</text>"), "comment line shown");
    }

    #[test]
    fn hidden_value_field_is_not_rendered() {
        // A test point whose Value is present but hidden must not show as text.
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:TP"
            (pin_names (offset 0.762) (hide yes)) (pin_numbers (hide yes))
            (symbol "TP_0_1" (circle (center 0 0) (radius 0.5) (fill (type none))))
            (symbol "TP_1_1" (pin passive line (at 0 0 90) (length 2.54)
              (name "1" (effects (font (size 1.27 1.27))))
              (number "1" (effects (font (size 1.27 1.27))))))))
          (symbol (lib_id "lib:TP") (at 50 50 0) (unit 1) (uuid "tp1")
            (property "Reference" "TP1" (at 50 56 0))
            (property "Value" "TESTPOINT" (at 50 55 0) (effects (hide yes)))
            (pin "1" (uuid "tp1p1")) (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(">TP1</text>"), "reference shown");
        assert!(!svg.contains("TESTPOINT"), "hidden value must not render");
        // pin_names/pin_numbers hidden → the pin's "1" name/number are suppressed.
    }

    #[test]
    fn only_visible_property_fields_render_not_every_property() {
        // Regression for the jetson-agx-thor-baseboard report: library parts often
        // carry Footprint/Datasheet/Description/MPN/Manufacturer/Author/License fields,
        // all but a couple flagged hidden. Only the un-hidden ones (here Reference +
        // MPN) may plot; the rest must stay off the canvas. Mixes `(hide yes)` and the
        // bare `hide` token to cover both KiCad eras.
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:U"
            (symbol "U_0_1" (rectangle (start -2 2) (end 2 -2) (fill (type none))))
            (symbol "U_1_1" (pin passive line (at -5 0 0) (length 3)
              (name "A" (effects (font (size 1 1)))) (number "1" (effects (font (size 1 1))))))))
          (symbol (lib_id "lib:U") (at 50 50 0) (unit 1) (uuid "u1")
            (property "Reference" "U62" (at 50 44 0) (effects (font (size 1.27 1.27))))
            (property "Value" "LTC4412IS6" (at 60 44 0) (effects (font (size 1.27 1.27)) (hide yes)))
            (property "Footprint" "fp:SOT-23-6" (at 60 46 0) (effects (font (size 1.27 1.27)) (hide yes)))
            (property "Datasheet" "http://x/ds.pdf" (at 60 48 0) (effects (font (size 1.27 1.27)) hide))
            (property "Description" "Ideal diode controller" (at 60 50 0) (effects (font (size 1.27 1.27)) (hide yes)))
            (property "MPN" "LTC4412IS6#TRMPBF" (at 50 46 0) (effects (font (size 1.27 1.27))))
            (property "Manufacturer" "Analog Devices" (at 60 52 0) (effects (font (size 1.27 1.27)) hide))
            (property "License" "Apache-2.0" (at 60 54 0) (effects (font (size 1.27 1.27)) (hide yes)))
            (pin "1" (uuid "u1p1")) (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // The two visible fields render.
        assert!(svg.contains(">U62</text>"), "visible Reference renders");
        assert!(svg.contains(">LTC4412IS6#TRMPBF</text>"), "visible MPN renders");
        // None of the six hidden fields do.
        for hidden in ["LTC4412IS6<", "fp:SOT-23-6", "ds.pdf", "Ideal diode", "Analog Devices", "Apache-2.0"] {
            assert!(!svg.contains(hidden), "hidden field must not render: {hidden}");
        }
    }

    #[test]
    fn resolves_symbol_via_lib_name() {
        // Instance references power:GND but the cache is renamed GND_1 via lib_name.
        let src = r##"
        (kicad_sch
          (lib_symbols (symbol "GND_1" (power) (pin_names (offset 0) (hide yes)) (pin_numbers (hide yes))
            (symbol "GND_1_0_1" (polyline (pts (xy 0 0) (xy 0 -1.27) (xy 1.27 -1.27) (xy 0 -2.54) (xy -1.27 -1.27) (xy 0 -1.27)) (fill (type none))))
            (symbol "GND_1_1_1" (pin power_in line (at 0 0 270) (length 0) (name "~") (number "1")))))
          (symbol (lib_id "power:GND") (lib_name "GND_1") (at 50 50 0) (unit 1) (uuid "g1")
            (property "Reference" "#PWR01" (effects (hide yes)))
            (property "Value" "GND" (at 50 53 0))
            (pin "1" (uuid "g1p1")) (instances)))
        "##;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // The GND polyline (triangle) renders because lib_name resolved the symbol.
        assert!(svg.contains(r#"data-primitive="power-symbol" data-uuid="g1""#));
        assert!(svg.contains("<polyline"), "GND graphic drawn");
    }

    #[test]
    fn renders_sheet_graphics_and_netclass_flag() {
        let src = r#"
        (kicad_sch
          (polyline (pts (xy 10 10) (xy 20 10) (xy 20 20)) (stroke (width 0.1)) (uuid "g1"))
          (netclass_flag "" (length 2.54) (shape round) (at 30 30 0) (uuid "nf1")
            (property "Netclass" "Isolated" (at 31 28 0) (effects (hide yes)))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(r#"data-primitive="graphic" data-uuid="g1""#));
        assert!(svg.contains(r#"data-primitive="netclass-flag" data-uuid="nf1""#));
        // Hidden Netclass text must not be drawn, but the glyph circle is.
        assert!(!svg.contains("Isolated"));
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn label_kinds_carry_global_vs_hier_for_distinct_colours() {
        // Global and hierarchical labels are both "port" glyphs but KiCad colours them
        // differently (label_global red vs label_hier olive); the `data-kind` lets the
        // app theme each from its own palette key instead of assuming one colour.
        let src = r#"
        (kicad_sch
          (global_label "GBL" (shape input) (at 10 10 0) (effects (font (size 1.27 1.27))) (uuid "gl1"))
          (hierarchical_label "HIER" (shape input) (at 20 20 0) (effects (font (size 1.27 1.27))) (uuid "hl1"))
          (label "LOC" (at 30 30 0) (effects (font (size 1.27 1.27))) (uuid "lc1")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(r#"data-primitive="port" data-kind="global" data-uuid="gl1""#));
        assert!(svg.contains(r#"data-primitive="port" data-kind="hier" data-uuid="hl1""#));
        assert!(svg.contains(r#"data-primitive="label" data-uuid="lc1""#));
    }

    #[test]
    fn sheet_pin_label_takes_its_net_colour() {
        // Sheet pins are net-class driven: when the pin's net carries a class colour,
        // it's baked as `--nc` so the app tints the label like a net label.
        let src = r#"
        (kicad_sch
          (sheet (at 50 50) (size 20 20) (uuid "sh1")
            (property "Sheetname" "child" (at 50 49 0))
            (property "Sheetfile" "child.kicad_sch" (at 50 71 0))
            (pin "NET_A" input (at 50 55 0) (uuid "sp1"))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let mut colors = NetColors::new();
        colors.insert("sp1".into(), "#00C2C2".into());
        let svg = render_sheet(&sch, &colors, None, &SchStyle::default());
        assert!(svg.contains(r#"class="sch-sheet-pin""#), "sheet pin classed for theming: {svg}");
        assert!(
            svg.contains("--nc:#00C2C2") && svg.contains(">NET_A</text>"),
            "sheet pin tinted by its net-class colour"
        );
    }

    #[test]
    fn arc_becomes_smooth_path() {
        let src = r#"
        (kicad_sch
          (lib_symbols (symbol "lib:C2"
            (symbol "C2_0_1" (arc (start -1 -2) (mid -3 0) (end -1 2) (stroke (width 0.2)) (fill (type none))))))
          (symbol (lib_id "lib:C2") (at 50 50 0) (unit 1) (uuid "c2")
            (property "Reference" "FC1") (instances)))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains("<path d=\"M "), "arc renders as a path");
        assert!(svg.contains(" A "), "path uses an elliptical-arc command");
    }

    #[test]
    fn net_colors_apply_to_wire_and_label() {
        let sch = Schematic::parse_str(SCH).unwrap();
        let mut colors = NetColors::new();
        colors.insert("w1".into(), "#6A5EFF".into());
        colors.insert("lbl1".into(), "#6A5EFF".into());
        let svg = render_sheet(&sch, &colors, None, &SchStyle::default());
        assert!(svg.contains(r##"data-primitive="wire" data-uuid="w1" stroke="#6A5EFF""##));
        assert!(svg.contains(r##"fill="#6A5EFF""##), "label text takes the net colour");
    }

    #[test]
    fn vertical_hier_label_extends_away_from_its_wire() {
        // BOARD_TEMP on the PTC sheet: a 90° hierarchical label whose wire drops
        // downward (+Y) must put its flag and text ABOVE the connection (smaller Y),
        // not below where they overlap the wire. Pre-fix the text anchored at y=100.33
        // (below 97.79); KiCad puts it a `gap` (2*size = 2.54) above → y = 95.25.
        let src = r#"
        (kicad_sch
          (wire (pts (xy 81.28 97.79) (xy 81.28 99.06)) (uuid "w"))
          (hierarchical_label "BOARD_TEMP" (shape output) (at 81.28 97.79 90)
            (effects (font (size 1.27 1.27)) (justify left)) (uuid "hl")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert!(svg.contains(r#"data-primitive="port" data-kind="hier" data-uuid="hl""#));
        assert!(
            svg.contains(r#"<text x="81.28" y="95.25""#),
            "text offsets up, away from the downward wire: {svg}"
        );
    }

    #[test]
    fn markup_renders_overbar_and_resolves_char_escapes() {
        // KiCad shows `~{crst}{slash}out2` as overlined "crst" then "/out2".
        assert_eq!(
            render_markup("~{crst}{slash}out2"),
            r#"<tspan text-decoration="overline">crst</tspan>/out2"#
        );
        assert_eq!(display_text("~{crst}{slash}out2"), "crst/out2");
        // Plain text is untouched (so callers that just dumped a string are safe),
        // and a bare underscore (CAN_RX) is literal, not a subscript trigger.
        assert_eq!(render_markup("GPIO19"), "GPIO19");
        assert_eq!(render_markup("CAN_RX"), "CAN_RX");
        // An unbalanced / unknown brace is left literal, never dropped.
        assert_eq!(render_markup("a{b"), "a{b");
        assert_eq!(render_markup("${REFS}"), "${REFS}");
    }

    #[test]
    fn label_shape_selects_distinct_glyphs() {
        // Output keeps the legacy single pentagon; the other I/O shapes each differ,
        // so a sheet of bidirectional hier labels no longer all read as outputs.
        let out = hier_flag_points(FlagShape::Output, 1.27);
        assert_ne!(out, hier_flag_points(FlagShape::Input, 1.27));
        assert_ne!(out, hier_flag_points(FlagShape::Bidi, 1.27));
        assert_ne!(out, hier_flag_points(FlagShape::Passive, 1.27));
        assert_eq!(hier_flag_points(FlagShape::Bidi, 1.27), hier_flag_points(FlagShape::TriState, 1.27));
        // Global tags select the same way on the box outline.
        let (g_out, _, _) = global_box_points(FlagShape::Output, 1.27, 5.0);
        let (g_in, _, _) = global_box_points(FlagShape::Input, 1.27, 5.0);
        assert_ne!(g_out, g_in);
    }

    #[test]
    fn global_label_processes_markup_and_note_keeps_its_colour() {
        let src = r#"
        (kicad_sch
          (global_label "~{crst}{slash}out2" (shape bidirectional) (at 100 100 0)
            (effects (font (size 1.27 1.27)) (justify right)) (uuid "g1"))
          (text "Possible Comms" (at 50 50 0)
            (effects (font (size 1.27 1.27) (color 255 113 0 1)) (justify left bottom)) (uuid "t1")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // The label's markup is rendered, not dumped raw (the raw form overran neighbours).
        assert!(svg.contains(r#"<tspan text-decoration="overline">crst</tspan>"#));
        assert!(!svg.contains("~{crst}") && !svg.contains("{slash}"), "raw markup leaked: {svg}");
        // The orange note bakes its KiCad colour as --nc so the app keeps it.
        assert!(svg.contains("--nc:#FF7100"), "note colour baked as --nc: {svg}");
    }

    #[test]
    fn trailing_blank_line_lifts_bottom_justified_text() {
        // KiCad keeps "A\n\n" as a 2-line block (only the final terminator newline is
        // dropped, like wxStringSplit). Bottom-justified, that lifts the visible "A" one
        // interline above its anchor — a bottom-justified note sat a line too low when we
        // popped *every* trailing blank. font = 1*4/3, line_h = font*1.2 = 1.6.
        let src = r#"
        (kicad_sch
          (text "A\n\n" (at 10 10 0)
            (effects (font (size 1 1)) (justify left bottom)) (uuid "t1")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        // The trailing blank row emits no tspan of its own (the frame uses <text>, not
        // <tspan>, so the note is the only tspan source) ...
        assert_eq!(svg.matches("<tspan").count(), 1, "only the glyph-bearing line emits a tspan: {svg}");
        // ... but it reserves height: "A" is shifted up one interline (dy=-1.6), not 0.
        assert!(svg.contains(r#"dy="-1.6">A</tspan>"#), "trailing blank must lift A one line: {svg}");
    }

    #[test]
    fn blank_line_between_notes_keeps_its_height() {
        // A blank line BETWEEN two lines must reserve a full row, so the second line's dy
        // spans two interlines. An empty <tspan dy> alone would collapse the gap (the
        // missing spacing between blank-separated note lines). Top-justified: first_dy=0,
        // line_h = 1*4/3*1.2 = 1.6, so "B" lands at 2*line_h = 3.2.
        let src = r#"
        (kicad_sch
          (text "A\n\nB" (at 10 10 0)
            (effects (font (size 1 1)) (justify left top)) (uuid "t1")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let svg = render(&sch);
        assert_eq!(svg.matches("<tspan").count(), 2, "two glyph-bearing lines: {svg}");
        assert!(svg.contains(r#"dy="0">A</tspan>"#), "first line at the anchor: {svg}");
        assert!(svg.contains(r#"dy="3.2">B</tspan>"#), "blank row must add a full interline: {svg}");
    }
}
