//! The board database header and string table.
//!
//! An Allegro board (`.brd`, OrCAD PCB Editor's format too) is one binary
//! database: a header of roughly 4 KB, a string table at a fixed offset, then
//! an object stream of type-tagged blocks that carry no length of their own.
//! The format version — read from the magic — decides every block's size, so
//! it is resolved here first and threaded through everything else.

use super::reader::{R, Res};

/// Format generations, in order. Only the ordering matters to the readers:
/// fields appear and disappear at these boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ver {
    V160,
    V162,
    V164,
    V165,
    V166,
    V172,
    V174,
    V175,
    V180,
    V181,
    V190,
}

impl Ver {
    /// The generation a magic names. The low byte is a revision within a
    /// generation and never changes a layout.
    pub fn from_magic(magic: u32) -> Result<Ver, String> {
        Ok(match magic & 0xFFFF_FF00 {
            0x0013_0000 => Ver::V160,
            0x0013_0400 | 0x0013_0500 => Ver::V162,
            0x0013_0C00 => Ver::V164,
            0x0013_1000 => Ver::V165,
            0x0013_1500 => Ver::V166,
            0x0014_0400 | 0x0014_0500 | 0x0014_0600 | 0x0014_0700 => Ver::V172,
            0x0014_0900 | 0x0014_0E00 => Ver::V174,
            0x0014_1500 => Ver::V175,
            0x0015_0000 => Ver::V180,
            0x0015_0200 => Ver::V181,
            0x0016_0100 => Ver::V190,
            m if (m >> 16) <= 0x0012 => {
                return Err(
                    "this board predates Allegro 16.0, whose database format is not readable; \
                     open it in Allegro or OrCAD PCB Editor 16 or later and save it again"
                        .into(),
                )
            }
            _ => return Err(format!("unknown Allegro board format {magic:#010x}")),
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Ver::V160 => "16.0",
            Ver::V162 => "16.2",
            Ver::V164 => "16.4",
            Ver::V165 => "16.5",
            Ver::V166 => "16.6",
            Ver::V172 => "17.2",
            Ver::V174 => "17.4",
            Ver::V175 => "17.5",
            Ver::V180 => "18.0",
            Ver::V181 => "18.1",
            Ver::V190 => "19.0",
        }
    }
}

/// Board units as the header states them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Mils,
    Inches,
    Millimetres,
    Centimetres,
    Micrometres,
}

impl Units {
    /// Millimetres per unit before the divisor.
    pub fn mm(&self) -> f64 {
        match self {
            Units::Mils => 0.0254,
            Units::Inches => 25.4,
            Units::Millimetres => 1.0,
            Units::Centimetres => 10.0,
            Units::Micrometres => 0.001,
        }
    }
}

/// One header linked-list descriptor.
#[derive(Debug, Clone, Copy, Default)]
pub struct Chain {
    pub head: u32,
    pub tail: u32,
}

#[derive(Debug, Clone)]
pub struct Header {
    pub magic: u32,
    pub ver: Ver,
    /// 1 for a board, 2 for a footprint drawing.
    pub file_role: u32,
    pub object_count: u32,
    /// Allegro's own version text, e.g. `OrCAD PCB Editor 17.4 S020`.
    pub program: String,
    pub units: Units,
    pub divisor: u32,
    pub string_count: u32,
    /// End offset of the constraint-manager cross reference block (0x27),
    /// which is the one block that is sized by the header rather than itself.
    pub end_0x27: u32,
    pub nets: Chain,
    pub footprints: Chain,
    pub shapes: Chain,
    pub zones_and_rects: Chain,
    pub graphics: Chain,
    pub padstacks: Chain,
    pub constraints: Chain,
    pub tables: Chain,
    pub fields_and_text: Chain,
    /// Per-class custom layer lists: `layer_lists[class]` is the key of the
    /// 0x2A list naming that class's low subclasses (ETCH: the copper stack).
    pub layer_lists: Vec<u32>,
}

impl Header {
    /// Millimetres per database unit.
    pub fn scale(&self) -> f64 {
        self.units.mm() / self.divisor.max(1) as f64
    }
}

fn chain(r: &mut R, ver: Ver) -> Res<Chain> {
    let a = r.u32()?;
    let b = r.u32()?;
    // 18.0 swapped the word order.
    Ok(if ver >= Ver::V180 { Chain { head: a, tail: b } } else { Chain { head: b, tail: a } })
}

pub fn read_header(data: &[u8]) -> Result<Header, String> {
    let mut r = R::new(data);
    let e = |x: super::reader::Err| x.to_string();
    let magic = r.u32().map_err(e)?;
    let ver = Ver::from_magic(magic)?;
    let _u1a = r.u32().map_err(e)?;
    let file_role = r.u32().map_err(e)?;
    r.skip(8).map_err(e)?;
    let object_count = r.u32().map_err(e)?;
    r.skip(8).map_err(e)?;
    let (mut end_0x27, mut string_count) = (0, 0);
    if ver < Ver::V180 {
        r.skip(28).map_err(e)?;
    } else {
        r.skip(8).map_err(e)?;
        end_0x27 = r.u32().map_err(e)?;
        r.skip(8).map_err(e)?;
        string_count = r.u32().map_err(e)?;
        r.skip(4).map_err(e)?;
        for _ in 0..5 {
            chain(&mut r, ver).map_err(e)?;
        }
    }
    let mut lists = Vec::with_capacity(18);
    for _ in 0..18 {
        lists.push(chain(&mut r, ver).map_err(e)?);
    }
    if ver < Ver::V180 {
        r.skip(8).map_err(e)?; // 0x35 extents
    } else {
        chain(&mut r, ver).map_err(e)?;
    }
    chain(&mut r, ver).map_err(e)?; // 0x36 definitions
    if ver < Ver::V180 {
        chain(&mut r, ver).map_err(e)?;
    }
    chain(&mut r, ver).map_err(e)?;
    chain(&mut r, ver).map_err(e)?;
    if ver < Ver::V180 {
        r.skip(4).map_err(e)?;
    } else {
        chain(&mut r, ver).map_err(e)?;
        r.skip(4).map_err(e)?;
        if ver >= Ver::V181 {
            r.skip(32).map_err(e)?;
        }
        r.skip(4).map_err(e)?;
    }
    let expect = if ver < Ver::V180 { 0xF8 } else if ver < Ver::V181 { 0x124 } else { 0x144 };
    if r.pos != expect {
        return Err(format!("Allegro header misread: version text at {:#x}, expected {expect:#x}", r.pos));
    }
    let program = r.fixed_str(60).map_err(e)?;
    r.skip(8).map_err(e)?;
    r.skip(if ver < Ver::V180 { 17 * 4 } else { 9 * 4 }).map_err(e)?;
    let units = match r.u8().map_err(e)? {
        1 => Units::Mils,
        2 => Units::Inches,
        3 => Units::Millimetres,
        4 => Units::Centimetres,
        5 => Units::Micrometres,
        u => return Err(format!("unknown Allegro board units {u}")),
    };
    r.skip(3).map_err(e)?;
    r.skip(4).map_err(e)?;
    if ver < Ver::V180 {
        r.skip(4).map_err(e)?;
        end_0x27 = r.u32().map_err(e)?;
    }
    r.skip(4).map_err(e)?;
    if ver < Ver::V180 {
        string_count = r.u32().map_err(e)?;
    }
    r.skip(50 * 4 + 12).map_err(e)?;
    let divisor = r.u32().map_err(e)?;
    r.skip(if ver >= Ver::V181 { 102 * 4 } else { 110 * 4 }).map_err(e)?;
    let mut layer_lists = Vec::with_capacity(25);
    for _ in 0..25 {
        let _a = r.u32().map_err(e)?;
        layer_lists.push(r.u32().map_err(e)?);
    }
    Ok(Header {
        magic,
        ver,
        file_role,
        object_count,
        program,
        units,
        divisor,
        string_count,
        end_0x27,
        nets: lists[5],
        footprints: lists[9],
        shapes: lists[3],
        zones_and_rects: lists[7],
        graphics: lists[4],
        padstacks: lists[6],
        constraints: lists[12],
        tables: lists[15],
        fields_and_text: lists[10],
        layer_lists,
    })
}

/// The string table sits at a fixed offset: `count` entries of a u32 id and a
/// NUL-terminated string.
pub const STRING_TABLE: usize = 0x1200;

pub fn read_strings(data: &[u8], count: u32) -> Result<(std::collections::HashMap<u32, String>, usize), String> {
    let mut r = R::new(data);
    r.pos = STRING_TABLE;
    let mut out = std::collections::HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let id = r.u32().map_err(|e| e.to_string())?;
        let s = r.cstr(true).map_err(|e| e.to_string())?;
        out.insert(id, s);
    }
    Ok((out, r.pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_magic_names_the_generation_and_its_revision_byte_does_not() {
        assert_eq!(Ver::from_magic(0x0014_0400).unwrap(), Ver::V172);
        assert_eq!(Ver::from_magic(0x0014_04FF).unwrap(), Ver::V172);
        assert_eq!(Ver::from_magic(0x0013_1500).unwrap(), Ver::V166);
        assert_eq!(Ver::from_magic(0x0015_0200).unwrap(), Ver::V181);
        assert!(Ver::from_magic(0x0012_0000).unwrap_err().contains("predates Allegro 16.0"));
        assert!(Ver::from_magic(0x0014_9900).unwrap_err().contains("unknown"));
    }

    #[test]
    fn units_scale_by_the_divisor() {
        let h = |units, divisor| Header {
            magic: 0,
            ver: Ver::V172,
            file_role: 1,
            object_count: 0,
            program: String::new(),
            units,
            divisor,
            string_count: 0,
            end_0x27: 0,
            nets: Chain::default(),
            footprints: Chain::default(),
            shapes: Chain::default(),
            zones_and_rects: Chain::default(),
            graphics: Chain::default(),
            padstacks: Chain::default(),
            constraints: Chain::default(),
            tables: Chain::default(),
            fields_and_text: Chain::default(),
            layer_lists: Vec::new(),
        };
        assert!((h(Units::Millimetres, 10000).scale() - 0.0001).abs() < 1e-12);
        assert!((h(Units::Mils, 1000).scale() - 0.0000254).abs() < 1e-12);
    }

    #[test]
    fn strings_are_word_aligned() {
        let mut d = vec![0u8; STRING_TABLE];
        d.extend(7u32.to_le_bytes());
        d.extend(b"GND\0");
        d.extend(8u32.to_le_bytes());
        d.extend(b"+3V3\0\0\0\0");
        let (s, end) = read_strings(&d, 2).unwrap();
        assert_eq!(s[&7], "GND");
        assert_eq!(s[&8], "+3V3");
        assert_eq!(end % 4, 0);
    }
}
