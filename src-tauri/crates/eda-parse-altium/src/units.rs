//! Coordinate and colour conversion at the parser boundary.
//!
//! Altium units never leak past this module: everything downstream is
//! millimetres in the bundle's own space.

/// One schematic coordinate unit is 10 mil.
const SCH_UNIT_MM: f64 = 0.254;
/// Schematic `*_Frac` companions are hundred-thousandths of that unit.
const FRAC_DIV: f64 = 100_000.0;
/// One board coordinate unit is 1/10000 mil.
const BOARD_UNIT_MM: f64 = 0.0254 / 10_000.0;

/// Schematic coordinate (10-mil units plus an optional fraction) to millimetres.
pub fn sch_mm(v: i64, frac: i64) -> f64 {
    (v as f64 + frac as f64 / FRAC_DIV) * SCH_UNIT_MM
}

/// Board coordinate (1/10000 mil) to millimetres.
pub fn board_mm(v: i64) -> f64 {
    v as f64 * BOARD_UNIT_MM
}

/// A board value already written as a decimal with a unit suffix (`43328.3462mil`).
pub fn mil_mm(v: f64) -> f64 {
    v * 0.0254
}

/// Altium's schematic `LineWidth` enum (0 smallest, 1 small, 2 medium,
/// 3 large) as a stroke width in millimetres.
///
/// The scale is the sheet's own: 1, 3 and 5 coordinate units for small, medium
/// and large, and half a unit for smallest. Small and medium are measured —
/// Altium's own on-screen geometry draws them at exactly 1 and 3 units — and
/// the corpus contains no object using either of the other two.
pub fn sch_line_width_mm(code: i64) -> f64 {
    let units = match code {
        2 => 3.0,
        3 => 5.0,
        1 => 1.0,
        _ => 0.5,
    };
    units * SCH_UNIT_MM
}

/// Ratio of a font's full glyph cell (ascender plus descender) to its em.
///
/// Altium sizes schematic text by the CELL, not by the em: a `Size` of 10 means
/// ten sheet units from the top of the ascender to the bottom of the descender.
/// SVG's `font-size` is the em, so the cell has to be divided by this to land
/// on the same glyph. The value is Times New Roman's — (1825 + 443) / 2048 —
/// which is the face the whole corpus uses, and it is what Altium's own
/// on-screen geometry reports: a `Size` of 10 draws at an em of 9.03 units.
const FONT_CELL_PER_EM: f64 = 1.1074;

/// An Altium schematic font `Size` as an SVG em height in millimetres.
///
/// Reading the number as POINTS instead — which is what it looks like — makes
/// every string on the sheet 54% too large, and nothing fails.
pub fn sch_font_mm(size: i64) -> f64 {
    size as f64 * SCH_UNIT_MM / FONT_CELL_PER_EM
}

/// The zone ruler for a `SheetStyle`: columns, rows, and the margin band's
/// width in sheet units.
///
/// Altium prints the ruler round the page border and reviewers cite it ("C3"),
/// so the counts have to be the design's own. Style 1 (A3) and style 6 (B) are
/// confirmed against Altium's own on-screen geometry — five by four and six by
/// four, both with a 20-unit band. The rest follow Altium's published table for
/// each paper size and are not exercised by the corpus.
pub fn sheet_zones(style: i64) -> (i64, i64, i64) {
    match style {
        0 => (4, 4, 20),   // A4
        1 => (5, 4, 20),   // A3
        2 => (6, 5, 30),   // A2
        3 => (8, 6, 30),   // A1
        4 => (10, 7, 30),  // A0
        5 => (4, 4, 20),   // A
        7 => (6, 4, 30),   // C
        8 => (8, 4, 30),   // D
        9 => (16, 4, 40),  // E
        10 => (4, 4, 20),  // Letter
        11 => (5, 4, 20),  // Legal
        12 => (6, 4, 30),  // Tabloid
        _ => (6, 4, 20),   // B, Altium's own default
    }
}

/// Width of a string in EMs, estimated from the character classes of Times New
/// Roman — the face the whole corpus uses.
///
/// This is an estimate, not a measurement: the renderer has no font metrics, and
/// the alternative for a text frame is no wrapping at all, which puts a 3468
/// character disclaimer on one line running off the page. The classes below are
/// Times' own proportions to within a few percent, so a wrapped line lands
/// within a word of where Altium puts it.
pub fn text_width_em(text: &str) -> f64 {
    text.chars().map(char_width_em).sum()
}

/// One character's advance, in ems.
fn char_width_em(c: char) -> f64 {
    match c {
        ' ' => 0.25,
        'i' | 'j' | 'l' | 'I' | '.' | ',' | ';' | ':' | '\'' | '|' | '!' | '`' => 0.28,
        'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '/' | '\\' => 0.33,
        'm' | 'w' | 'M' | 'W' => 0.83,
        '0'..='9' => 0.5,
        'A'..='Z' => 0.67,
        _ => 0.5,
    }
}

/// Altium colours are BGR integers, not RGB. `#RRGGBB` out.
pub fn bgr_hex(v: i64) -> String {
    let v = v as u32;
    let (b, g, r) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// The sheet style of a `.SchDoc` that omits `SheetStyle`: **A4** (style 0).
///
/// Altium leaves out any key whose value is 0, and A4 is style 0. So a missing
/// key means A4 (1150 x 760 units), not B. The page is what the Y flip is
/// anchored to, so a wrong size puts every object out of place and nothing
/// fails. Evidence: the MB1419 CAN sheet has no key, its template is
/// `Schematic_A4_Landscape`, and Altium's title block prints "Size A4".
pub const DEFAULT_SHEET_STYLE: i64 = 0;

/// Sheet size in millimetres for a `SheetStyle` index, following Altium's own
/// table. An unknown style falls back to [`DEFAULT_SHEET_STYLE`]'s size.
pub fn sheet_style_mm(style: i64) -> (f64, f64) {
    // Altium's styles are stored in 10-mil units; converted here once.
    let (w, h) = match style {
        0 => (1150, 760),   // A4
        1 => (1550, 1110),  // A3
        2 => (2230, 1570),  // A2
        3 => (3150, 2230),  // A1
        4 => (4460, 3150),  // A0
        5 => (950, 750),    // A
        6 => (1500, 950),   // B
        7 => (2000, 1500),  // C
        8 => (3200, 2000),  // D
        9 => (4200, 3200),  // E
        10 => (1100, 850),  // Letter
        11 => (1400, 850),  // Legal
        12 => (1700, 1100), // Tabloid
        _ => (1500, 950),   // B, Altium's own default
    };
    (w as f64 * SCH_UNIT_MM, h as f64 * SCH_UNIT_MM)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Altium omits a key whose value is 0, so a missing `SheetStyle` is A4.
    #[test]
    fn an_unstated_sheet_style_is_a4() {
        assert_eq!(sheet_style_mm(DEFAULT_SHEET_STYLE), sheet_style_mm(0));
        let (w, h) = sheet_style_mm(DEFAULT_SHEET_STYLE);
        assert!((w - 1150.0 * 0.254).abs() < 1e-9 && (h - 760.0 * 0.254).abs() < 1e-9);
        assert_eq!(sheet_zones(DEFAULT_SHEET_STYLE), (4, 4, 20));
        assert_eq!(sheet_zones(6), (6, 4, 20), "B, confirmed against Altium");
        assert_eq!(sheet_zones(1), (5, 4, 20), "A3, confirmed against Altium");
    }

    #[test]
    fn schematic_units_are_ten_mil_with_a_fraction() {
        assert!((sch_mm(100, 0) - 25.4).abs() < 1e-9);
        assert!((sch_mm(0, 50_000) - 0.127).abs() < 1e-9);
    }

    #[test]
    fn board_units_are_ten_thousandths_of_a_mil() {
        assert!((board_mm(10_000) - 0.0254).abs() < 1e-12);
        assert!((mil_mm(1000.0) - 25.4).abs() < 1e-9);
    }

    /// Corner case 10: colours are BGR, so a naive read swaps red and blue.
    #[test]
    fn colours_are_bgr() {
        assert_eq!(bgr_hex(8_388_608), "#000080", "Altium's default navy wire");
        assert_eq!(bgr_hex(128), "#800000", "default maroon outline / net label");
        assert_eq!(bgr_hex(0xFFFFFF), "#FFFFFF");
    }
}
