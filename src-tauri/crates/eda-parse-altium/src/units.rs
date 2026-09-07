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

/// Altium colours are BGR integers, not RGB. `#RRGGBB` out.
pub fn bgr_hex(v: i64) -> String {
    let v = v as u32;
    let (b, g, r) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Sheet size in millimetres for a `SheetStyle` index, following Altium's own
/// table. Unknown styles fall back to A4.
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
        _ => (1150, 760),
    };
    (w as f64 * SCH_UNIT_MM, h as f64 * SCH_UNIT_MM)
}

#[cfg(test)]
mod tests {
    use super::*;

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
