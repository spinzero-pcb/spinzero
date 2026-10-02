//! OrCAD / Allegro PCB Editor board databases (`.brd`).

pub mod blocks;
pub mod header;
pub mod reader;

use std::path::Path;

pub use blocks::{Block, Data, Layer, Stream};
pub use header::{Header, Units, Ver};

/// True when the file starts like an Allegro board database.
pub fn sniff_path(p: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).is_ok() && sniff(&b)
}

/// The magic's top half names the format family (0x0013 = 16.x, 0x0014 =
/// 17.x, 0x0015 = 18.x, 0x0016 = 19.x).
pub fn sniff(b: &[u8]) -> bool {
    b.len() >= 4 && {
        let m = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        matches!(m >> 16, 0x000F..=0x0016)
    }
}

/// A decoded board database.
pub struct Db {
    pub header: Header,
    pub strings: std::collections::HashMap<u32, String>,
    pub stream: Stream,
}

impl Db {
    pub fn s(&self, id: u32) -> &str {
        self.strings.get(&id).map(String::as_str).unwrap_or("")
    }
}

pub fn parse(data: &[u8]) -> Result<Db, String> {
    if !sniff(data) {
        return Err("not an Allegro / OrCAD PCB Editor board".into());
    }
    let header = header::read_header(data)?;
    let (strings, end) = header::read_strings(data, header.string_count)?;
    let stream = blocks::read_stream(data, end, &header);
    Ok(Db { header, strings, stream })
}

pub fn open(path: &Path) -> Result<Db, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&data)
}
