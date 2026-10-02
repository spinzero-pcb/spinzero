//! The `Library` stream: format version, fonts, page defaults and the global
//! string table every property index in the file resolves against.

use super::bytes::{Cur, Res};

/// What a Capture compound file says it is in its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// `OrCAD Windows Design` — a `.DSN`.
    Design,
    /// `OrCAD Windows Library` — an `.OLB`.
    Library,
}

/// One Windows LOGFONT as Capture stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    /// lfHeight: negative means character height in logical units. Capture's
    /// logical unit is the page's 1/100 inch.
    pub height: i32,
    pub width: i32,
    /// Tenths of a degree.
    pub escapement: i32,
    pub weight: i32,
    pub italic: bool,
    pub face: String,
}

/// Page geometry and border settings. Shared by the library defaults and by
/// every page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageSettings {
    pub created: u32,
    pub modified: u32,
    /// Page size in mils, or micrometres when `metric`.
    pub width: u32,
    pub height: u32,
    /// Grid pitch in the same units (100 mil by default).
    pub pin_to_pin: u32,
    pub horizontal_count: u16,
    pub vertical_count: u16,
    pub horizontal_width: u32,
    pub vertical_width: u32,
    pub horizontal_letters: bool,
    pub horizontal_ascending: bool,
    pub vertical_letters: bool,
    pub vertical_ascending: bool,
    pub metric: bool,
    pub border_displayed: bool,
    pub grid_ref_displayed: bool,
    pub title_block_displayed: bool,
    pub ansi_grid_refs: bool,
}

impl PageSettings {
    /// Fixed 156-byte layout.
    pub fn read(c: &mut Cur) -> Res<PageSettings> {
        let start = c.pos;
        let created = c.u32()?;
        let modified = c.u32()?;
        c.skip(16)?;
        let width = c.u32()?;
        let height = c.u32()?;
        let pin_to_pin = c.u32()?;
        c.skip(2)?;
        let horizontal_count = c.u16()?;
        let vertical_count = c.u16()?;
        c.skip(2)?;
        let horizontal_width = c.u32()?;
        let vertical_width = c.u32()?;
        c.skip(48)?;
        let horizontal_letters = c.u32()? != 0;
        c.skip(4)?;
        let horizontal_ascending = c.u32()? != 0;
        let vertical_letters = c.u32()? != 0;
        c.skip(4)?;
        let vertical_ascending = c.u32()? != 0;
        let metric = c.u32()? != 0;
        let border_displayed = c.u32()? != 0;
        let _border_printed = c.u32()?;
        let grid_ref_displayed = c.u32()? != 0;
        let _grid_ref_printed = c.u32()?;
        let title_block_displayed = c.u32()? != 0;
        let _title_block_printed = c.u32()?;
        let ansi_grid_refs = c.u32()? != 0;
        debug_assert_eq!(c.pos - start, 156);
        Ok(PageSettings {
            created,
            modified,
            width,
            height,
            pin_to_pin,
            horizontal_count,
            vertical_count,
            horizontal_width,
            vertical_width,
            horizontal_letters,
            horizontal_ascending,
            vertical_letters,
            vertical_ascending,
            metric,
            border_displayed,
            grid_ref_displayed,
            title_block_displayed,
            ansi_grid_refs,
        })
    }

    /// Page size in mils, whatever the stored unit.
    pub fn size_mils(&self) -> (f64, f64) {
        if self.metric {
            (self.width as f64 / 25.4, self.height as f64 / 25.4)
        } else {
            (self.width as f64, self.height as f64)
        }
    }
}

/// The decoded `Library` stream.
#[derive(Debug, Clone)]
pub struct LibraryInfo {
    pub role: FileRole,
    pub version: (u16, u16),
    pub created: u32,
    pub modified: u32,
    pub fonts: Vec<Font>,
    /// Display names of the eight user part fields, in fixed order.
    pub part_fields: Vec<String>,
    pub page_defaults: PageSettings,
    /// The global string table. Index 0 is the empty string.
    pub strings: Vec<String>,
    /// Part aliases (alias, part).
    pub aliases: Vec<(String, String)>,
    /// The root schematic folder a design names. Empty for a library, or when
    /// the optional tail could not be read.
    pub root_name: String,
}

impl LibraryInfo {
    /// True for the modern grammar (Capture 16 onward).
    pub fn modern(&self) -> bool {
        self.version.0 >= 3
    }

    /// Resolve a string index; out of range resolves to empty.
    pub fn s(&self, idx: u32) -> &str {
        self.strings.get(idx as usize).map(String::as_str).unwrap_or("")
    }

    /// Resolve property pairs into (name, value) strings.
    pub fn props(&self, pairs: &[(u32, u32)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|&(a, b)| (self.s(a).to_string(), self.s(b).to_string()))
            .collect()
    }
}

const DESIGN_INTRO: &str = "OrCAD Windows Design";
const LIBRARY_INTRO: &str = "OrCAD Windows Library";

/// Read just enough of a `Library` stream to say what the file is.
pub fn sniff(data: &[u8]) -> Option<(FileRole, (u16, u16))> {
    let mut c = Cur::new(data);
    let intro = c.fixed_str(32).ok()?;
    let role = if intro.trim_end().starts_with(DESIGN_INTRO) {
        FileRole::Design
    } else if intro.trim_end().starts_with(LIBRARY_INTRO) {
        FileRole::Library
    } else {
        return None;
    };
    let major = c.u16().ok()?;
    let minor = c.u16().ok()?;
    Some((role, (major, minor)))
}

impl LibraryInfo {
    pub fn read(data: &[u8]) -> Res<LibraryInfo> {
        let mut c = Cur::new(data);
        let Some((role, version)) = sniff(data) else {
            return c.err("not an OrCAD Capture Library stream");
        };
        c.skip(36)?;
        let created = c.u32()?;
        let modified = c.u32()?;
        c.skip(4)?;
        let n_fonts = c.u16()?.saturating_sub(1);
        let mut fonts = Vec::with_capacity(n_fonts as usize);
        for _ in 0..n_fonts {
            let height = c.i32()?;
            let width = c.i32()?;
            let escapement = c.i32()?;
            let _orientation = c.i32()?;
            let weight = c.i32()?;
            let italic = c.u8()? != 0;
            c.skip(7)?;
            let face = c.fixed_str(32)?;
            fonts.push(Font { height, width, escapement, weight, italic, face });
        }
        if version.0 >= 2 {
            let n = c.u16()? as usize;
            c.skip(2 * n)?;
        } else {
            c.skip(2 * 17)?;
        }
        c.skip(8)?;
        let mut part_fields = Vec::with_capacity(8);
        for _ in 0..8 {
            part_fields.push(c.lstr()?);
        }
        let page_defaults = PageSettings::read(&mut c)?;
        let n_strings = if version.0 >= 3 { c.u32()? as usize } else { c.u16()? as usize };
        // Each string costs at least three bytes; a count that cannot fit is a
        // mis-read, and an empty table would silently blank every name.
        if n_strings > c.left() / 3 + 1 {
            return c.err(format!("string count {n_strings} cannot fit"));
        }
        let mut strings = Vec::with_capacity(n_strings);
        for _ in 0..n_strings {
            strings.push(c.lstr()?);
        }
        // The tail is optional in the sense that libraries stop early.
        let mut aliases = Vec::new();
        let mut root_name = String::new();
        if let Ok(n) = c.u16() {
            for _ in 0..n {
                let a = c.lstr()?;
                let p = c.lstr()?;
                aliases.push((a, p));
            }
            if role == FileRole::Design && c.skip(8).is_ok() {
                root_name = c.lstr().unwrap_or_default();
            }
        }
        Ok(LibraryInfo {
            role,
            version,
            created,
            modified,
            fonts,
            part_fields,
            page_defaults,
            strings,
            aliases,
            root_name,
        })
    }
}
