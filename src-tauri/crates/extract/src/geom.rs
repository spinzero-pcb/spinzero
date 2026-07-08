//! Integer (nanometre) geometry for schematic connectivity.
//!
//! KiCad schematic coordinates are millimetres with up to four decimals. Working
//! in rounded nanometres lets connectivity use exact integer equality and
//! exact collinearity tests, which sidesteps floating-point drift when matching
//! pin endpoints to wires.

/// Nanometres per millimetre.
pub const NM_PER_MM: f64 = 1_000_000.0;

/// Round a millimetre value to integer nanometres.
pub fn to_nm(mm: f64) -> i64 {
    (mm * NM_PER_MM).round() as i64
}

/// A point in nanometres.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct P {
    pub x: i64,
    pub y: i64,
}

/// Apply a symbol placement transform to a library-space point.
///
/// Library symbols are stored Y-up; the schematic canvas is Y-down, so the base
/// orientation flips Y (verified against real placements: a pin at relative
/// `(0, +3.81)` on a part placed at `y = 101.6` lands on a wire at `y = 97.79`).
/// The placement rotation is applied first, then the mirror in screen space
/// (`"x"` flips Y, `"y"` flips X) — validated against KiCad's own netlist on
/// boards with rotated *and* mirrored symbols (e.g. `angle 90, mirror x`).
pub fn place(at_x: f64, at_y: f64, angle: f64, mirror: Option<&str>, px: f64, py: f64) -> P {
    // Library Y-up -> screen Y-down.
    let (x, y) = (px, -py);
    // Screen-space rotation (Y-down) by the placement angle.
    let (s, c) = angle.to_radians().sin_cos();
    let mut rx = c * x + s * y;
    let mut ry = -s * x + c * y;
    // Mirror is applied after rotation, in screen space.
    match mirror {
        Some("y") => rx = -rx,
        Some("x") => ry = -ry,
        _ => {}
    }
    P {
        x: to_nm(at_x + rx),
        y: to_nm(at_y + ry),
    }
}

/// Apply a symbol placement transform and return the result in millimetres
/// (rather than the integer nanometres `place` yields) — used for bounding-box
/// geometry where exact integer connectivity is not needed.
pub fn place_mm(at_x: f64, at_y: f64, angle: f64, mirror: Option<&str>, px: f64, py: f64) -> (f64, f64) {
    let p = place(at_x, at_y, angle, mirror, px, py);
    (p.x as f64 / NM_PER_MM, p.y as f64 / NM_PER_MM)
}

/// True when point `p` lies on the closed segment `a`–`b` (collinear and within
/// the bounding box). All inputs are integer nanometres, so this is exact.
pub fn on_segment(a: P, b: P, p: P) -> bool {
    // Collinear: cross product of (b-a) and (p-a) is zero.
    let cross = (b.x - a.x) as i128 * (p.y - a.y) as i128
        - (b.y - a.y) as i128 * (p.x - a.x) as i128;
    if cross != 0 {
        return false;
    }
    // Within the bounding box of the segment.
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_transform_flips_y() {
        // Device:C pin 1 at relative (0, 3.81) on C12 placed at (260.35, 101.6, 0).
        let p = place(260.35, 101.6, 0.0, None, 0.0, 3.81);
        assert_eq!(p, P { x: to_nm(260.35), y: to_nm(97.79) });
        let p2 = place(260.35, 101.6, 0.0, None, 0.0, -3.81);
        assert_eq!(p2, P { x: to_nm(260.35), y: to_nm(105.41) });
    }

    #[test]
    fn on_segment_basic() {
        let a = P { x: 0, y: 0 };
        let b = P { x: 0, y: 1000 };
        assert!(on_segment(a, b, P { x: 0, y: 500 }));
        assert!(on_segment(a, b, P { x: 0, y: 0 }));
        assert!(!on_segment(a, b, P { x: 10, y: 500 }));
        assert!(!on_segment(a, b, P { x: 0, y: 2000 }));
    }
}
