//! OrCAD / Allegro PCB Editor board files (`.brd`).

use std::path::Path;

/// True when the file starts like an Allegro board database.
pub fn sniff_path(p: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).is_ok() && sniff(&b)
}

/// The magic's top half names the format family (0x0013 = 16.x, 0x0014 =
/// 17.x, 0x0015 = 18.0 onward).
pub fn sniff(b: &[u8]) -> bool {
    b.len() >= 4 && {
        let m = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        matches!(m >> 16, 0x0012..=0x0016)
    }
}
