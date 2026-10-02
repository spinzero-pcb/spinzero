//! Symbol definitions as Capture caches them: the drawing primitives, the pins,
//! and the display-property records that say where a property's text sits.
//!
//! Coordinates are Capture's database unit (10 mil), symbol space, Y down.

use super::bytes::{Cur, Res};
use super::framing::{expect_frame, read_frame, Frame, MARKER};
use super::library::LibraryInfo;

/// Structure types this module decodes.
pub mod st {
    pub const STH_IN_PAGES0: u8 = 2;
    pub const LIBRARY_PART: u8 = 24;
    pub const PIN_SCALAR: u8 = 26;
    pub const PIN_BUS: u8 = 27;
    pub const GLOBAL_SYMBOL: u8 = 33;
    pub const PORT_SYMBOL: u8 = 34;
    pub const OFFPAGE_SYMBOL: u8 = 35;
    pub const DISPLAY_PROP: u8 = 39;
    pub const SYMBOL_VECTOR: u8 = 48;
    pub const TITLEBLOCK_SYMBOL: u8 = 64;
    pub const ERC_SYMBOL: u8 = 75;
    pub const BOOKMARK_SYMBOL: u8 = 76;
}

/// A drawing primitive inside a symbol body.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Rect { a: (i32, i32), b: (i32, i32), line: LineStyle, fill: Fill },
    Ellipse { a: (i32, i32), b: (i32, i32), line: LineStyle, fill: Fill },
    Line { a: (i32, i32), b: (i32, i32), line: LineStyle },
    /// An elliptical arc inside the box `a`..`b`, drawn counter-clockwise from
    /// the ray through `start` to the ray through `end` (screen Y down).
    Arc { a: (i32, i32), b: (i32, i32), start: (i32, i32), end: (i32, i32), line: LineStyle },
    Polygon { points: Vec<(i32, i32)>, line: LineStyle, fill: Fill },
    Polyline { points: Vec<(i32, i32)>, line: LineStyle },
    /// Cubic Bézier: 1 + 3n control points.
    Bezier { points: Vec<(i32, i32)>, line: LineStyle },
    Text { a: (i32, i32), b: (i32, i32), origin: (i32, i32), font: u16, text: String },
    /// An embedded picture. `data` is the stored payload (a DIB for a bitmap,
    /// a compound file or `~~CI_IMAGE~~` image for an OLE object).
    Image { a: (i32, i32), b: (i32, i32), ole: bool, data: Vec<u8> },
    /// A named group of primitives, placed at `at` within the symbol.
    Group { at: (i32, i32), name: String, prims: Vec<Prim> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineStyle {
    /// 0 solid, 1 dash, 2 dot, 3 dash-dot, 4 dash-dot-dot, 5 default.
    pub style: u32,
    /// 0 thin, 1 medium, 2 wide, 3 default.
    pub width: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fill {
    /// 0 solid, 1 none, 2 hatched.
    pub style: u32,
    pub hatch: u32,
}

impl Fill {
    pub fn is_solid(&self) -> bool {
        self.style == 0
    }
}

/// Where and how one property is drawn on an instance.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayProp {
    pub name: String,
    pub x: i16,
    pub y: i16,
    /// 1-based font index into the Library font table; 0 = default.
    pub font: u16,
    /// Quarter turns.
    pub rotation: u8,
    pub color: u8,
    /// 0 none, 1 value only, 2 name and value, 3 name only, 4 both if value.
    pub mode: u16,
}

/// A pin as the symbol defines it.
#[derive(Debug, Clone, PartialEq)]
pub struct SymPin {
    /// Slot index in the symbol's pin list. A device's pin-number list is
    /// indexed by this, so skipped slots keep their place.
    pub slot: usize,
    pub name: String,
    pub bus: bool,
    /// Free end of the pin leg.
    pub start: (i32, i32),
    /// Connection point.
    pub hot: (i32, i32),
    /// Decoration bits: 0x02 clock, 0x04 dot, 0x80 power-pin style. Bits
    /// 0..1 select the pin length/shape.
    pub shape: u16,
    pub etype: PinType,
    pub display: Vec<DisplayProp>,
}

impl SymPin {
    /// Bit 7 hides the pin. A hidden pin of power type is Capture's implicit
    /// supply connection: it joins the global net its NAME names, with no wire.
    pub fn hidden(&self) -> bool {
        self.shape & 0x80 != 0
    }

    pub fn clock(&self) -> bool {
        self.shape & 0x02 != 0
    }
    pub fn dot(&self) -> bool {
        self.shape & 0x04 != 0
    }
    /// Capture's "zero length" pin shape (bits 0..1 = 3? short/zero-length).
    pub fn zero_length(&self) -> bool {
        self.start == self.hot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinType {
    Input,
    Bidirectional,
    Output,
    OpenCollector,
    Passive,
    TriState,
    OpenEmitter,
    Power,
    Unknown(u32),
}

impl PinType {
    pub fn from_code(c: u32) -> PinType {
        match c {
            0 => PinType::Input,
            1 => PinType::Bidirectional,
            2 => PinType::Output,
            3 => PinType::OpenCollector,
            4 => PinType::Passive,
            5 => PinType::TriState,
            6 => PinType::OpenEmitter,
            7 => PinType::Power,
            n => PinType::Unknown(n),
        }
    }

    /// The bundle's upper-case electrical token, shared with the other
    /// front-ends.
    pub fn token(&self) -> &'static str {
        match self {
            PinType::Input => "INPUT",
            PinType::Bidirectional => "BIDIRECTIONAL",
            PinType::Output => "OUTPUT",
            PinType::OpenCollector => "OPEN_COLLECTOR",
            PinType::Passive => "PASSIVE",
            PinType::TriState => "TRI_STATE",
            PinType::OpenEmitter => "OPEN_EMITTER",
            PinType::Power => "POWER_IN",
            PinType::Unknown(_) => "PASSIVE",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PinType::Input => "input",
            PinType::Bidirectional => "bidirectional",
            PinType::Output => "output",
            PinType::OpenCollector => "open_collector",
            PinType::Passive => "passive",
            PinType::TriState => "tri_state",
            PinType::OpenEmitter => "open_emitter",
            PinType::Power => "power_in",
            PinType::Unknown(_) => "unspecified",
        }
    }
}

/// The tail a library part carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartGeneral {
    pub implementation_path: String,
    pub implementation: String,
    pub reference_prefix: String,
    pub part_value: String,
    /// bit0 pin names visible, bit1 pin names rotate, bit2 pin numbers hidden.
    pub flags: u16,
}

impl PartGeneral {
    pub fn pin_names_visible(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn pin_numbers_visible(&self) -> bool {
        self.flags & 4 == 0
    }
}

/// A symbol body: a cached library part view, a power/port/off-page symbol, a
/// title block, or a nested body inside a placed graphic.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolDef {
    pub kind: u8,
    pub name: String,
    pub source_lib: String,
    pub props: Vec<(String, String)>,
    pub color: u32,
    pub prims: Vec<Prim>,
    /// Body box (x1, y1, x2, y2).
    pub bbox: (i16, i16, i16, i16),
    pub pins: Vec<SymPin>,
    pub display: Vec<DisplayProp>,
    pub general: Option<PartGeneral>,
    /// Primitives whose stored length disagreed with what their layout read.
    /// Counted, not fatal: the frame says where the next one starts.
    pub prim_mismatches: usize,
}

impl SymbolDef {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Read a display-property record (type 39) at the cursor.
pub fn read_display_prop(c: &mut Cur, lib: &LibraryInfo) -> Res<DisplayProp> {
    let f = expect_frame(c, &[st::DISPLAY_PROP])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = lib.s(b.u32()?).to_string();
    let x = b.i16()?;
    let y = b.i16()?;
    let rf = b.u16()?;
    let color = b.u8()?;
    let mode = b.u16()?;
    c.seek(f.end())?;
    Ok(DisplayProp { name, x, y, font: rf & 0x3FFF, rotation: (rf >> 14) as u8, color, mode })
}

/// A u16-counted list of display properties.
pub fn read_display_list(c: &mut Cur, lib: &LibraryInfo) -> Res<Vec<DisplayProp>> {
    let n = c.u16()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        out.push(read_display_prop(c, lib)?);
    }
    Ok(out)
}

fn pt(c: &mut Cur) -> Res<(i32, i32)> {
    Ok((c.i32()?, c.i32()?))
}

fn line_style(c: &mut Cur) -> Res<LineStyle> {
    Ok(LineStyle { style: c.u32()?, width: c.u32()? })
}

fn fill(c: &mut Cur) -> Res<Fill> {
    Ok(Fill { style: c.u32()?, hatch: c.u32()? })
}

/// Point lists are 16-bit and stored Y first.
fn points(c: &mut Cur) -> Res<Vec<(i32, i32)>> {
    let n = c.u16()? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let y = c.i16()? as i32;
        let x = c.i16()? as i32;
        v.push((x, y));
    }
    Ok(v)
}

/// Skip the separators Capture writes between primitives: a bare marker with
/// its counted trail, or eight zero bytes.
fn skip_separators(c: &mut Cur) {
    loop {
        if c.left() >= 8 && c.buf[c.pos..c.pos + 4] == MARKER {
            let save = c.pos;
            c.pos += 4;
            match c.u32() {
                Ok(t) if (t as usize) <= c.left() => {
                    c.pos += t as usize;
                    continue;
                }
                _ => {
                    c.pos = save;
                    return;
                }
            }
        }
        if c.left() >= 8 && c.buf[c.pos..c.pos + 8] == [0; 8] {
            c.pos += 8;
            continue;
        }
        return;
    }
}

/// Skip only marker separators (not zero runs, which a box can begin with).
fn skip_marker_separators(c: &mut Cur) {
    while c.left() >= 8 && c.buf[c.pos..c.pos + 4] == MARKER {
        let save = c.pos;
        c.pos += 4;
        match c.u32() {
            Ok(t) if (t as usize) <= c.left() => c.pos += t as usize,
            _ => {
                c.pos = save;
                return;
            }
        }
    }
}

/// Decode one primitive body of type `t`. Returns the primitive.
pub(crate) fn prim_body(t: u8, c: &mut Cur) -> Res<Prim> {
    Ok(match t {
        40 | 43 => {
            let a = pt(c)?;
            let b = pt(c)?;
            let line = line_style(c)?;
            let fl = fill(c)?;
            if t == 40 {
                Prim::Rect { a, b, line, fill: fl }
            } else {
                Prim::Ellipse { a, b, line, fill: fl }
            }
        }
        41 => {
            let a = pt(c)?;
            let b = pt(c)?;
            Prim::Line { a, b, line: line_style(c)? }
        }
        42 => {
            let a = pt(c)?;
            let b = pt(c)?;
            let start = pt(c)?;
            let end = pt(c)?;
            Prim::Arc { a, b, start, end, line: line_style(c)? }
        }
        44 => {
            let line = line_style(c)?;
            let fl = fill(c)?;
            Prim::Polygon { points: points(c)?, line, fill: fl }
        }
        45 => {
            let line = line_style(c)?;
            Prim::Polyline { points: points(c)?, line }
        }
        87 => {
            let line = line_style(c)?;
            Prim::Bezier { points: points(c)?, line }
        }
        46 => {
            let a = pt(c)?;
            let b = pt(c)?;
            let origin = pt(c)?;
            let font = c.u16()?;
            c.skip(2)?;
            Prim::Text { a, b, origin, font, text: c.lstr()? }
        }
        47 => {
            let a = pt(c)?;
            let b = pt(c)?;
            c.skip(16)?;
            let n = c.u32()? as usize;
            Prim::Image { a, b, ole: false, data: c.bytes(n)?.to_vec() }
        }
        90 => {
            let a = pt(c)?;
            let b = pt(c)?;
            c.skip(16)?;
            let rest = c.left();
            Prim::Image { a, b, ole: true, data: c.bytes(rest)?.to_vec() }
        }
        _ => return c.err(format!("unknown primitive type {t}")),
    })
}

/// A primitive record: two equal type bytes, u32 length (counting itself and
/// the pad that follows), u32 zero, body. The legacy convention does not count
/// the length word and pad; both are accepted when the body reads exactly.
fn read_prim(c: &mut Cur, lib: &LibraryInfo, mismatches: &mut usize) -> Res<Prim> {
    let start = c.pos;
    let t = c.u8()?;
    if t == 48 {
        // A vector group opens its own framed chain at the next byte.
        // Its stored length stops one byte short of what it holds: the
        // group's closing name runs one byte past the stop (measured on every
        // group in the corpus). So the body is read against the enclosing
        // record's end, and the reader continues from where it actually ends.
        let f = expect_frame(c, &[48])?;
        let mut inner = Cur::at(c.buf, f.body, c.end);
        let g = read_vector_body(&mut inner, lib, mismatches)?;
        c.seek(inner.pos.max(f.end()))?;
        return Ok(g);
    }
    // Inside a group the two type bytes are separated by a zero.
    if c.peek_u8() == Some(0) && c.peek_at(1) == Some(t) {
        c.skip(1)?;
    }
    if c.u8()? != t {
        c.pos = start;
        return c.err("primitive type bytes disagree");
    }
    let header = c.pos - start;
    let len = c.u32()? as usize;
    c.skip(4)?;
    let modern_end = start + header + len;
    let legacy_end = start + header + 8 + len;
    // Text keeps undecoded padding after its string in every era.
    if t == 46 {
        let end = legacy_end.min(c.end);
        let mut b = Cur::at(c.buf, c.pos, end);
        let p = prim_body(t, &mut b)?;
        c.seek(end)?;
        return Ok(p);
    }
    let bound = legacy_end.min(c.end).max(modern_end.min(c.end));
    let mut b = Cur::at(c.buf, c.pos, bound);
    let p = prim_body(t, &mut b)?;
    if b.pos == modern_end || b.pos == legacy_end {
        c.seek(b.pos)?;
    } else if t == 90 {
        c.seek(bound)?;
    } else {
        *mismatches += 1;
        c.seek(modern_end.min(c.end))?;
    }
    Ok(p)
}

/// A vector group's body: its offset, a counted list of children, then its
/// name.
fn read_vector_body(c: &mut Cur, lib: &LibraryInfo, mismatches: &mut usize) -> Res<Prim> {
    let at = (c.i16()? as i32, c.i16()? as i32);
    let n = c.u16()?;
    let mut prims = Vec::with_capacity(n as usize);
    for _ in 0..n {
        skip_separators(c);
        prims.push(read_prim(c, lib, mismatches)?);
    }
    skip_separators(c);
    let name = c.lstr().unwrap_or_default();
    Ok(Prim::Group { at, name, prims })
}

/// Read a pin (type 26/27) or a skipped slot (a single zero byte).
fn read_pin(c: &mut Cur, lib: &LibraryInfo, slot: usize) -> Res<Option<SymPin>> {
    if c.peek_u8() == Some(0) {
        c.skip(1)?;
        return Ok(None);
    }
    let f = expect_frame(c, &[st::PIN_SCALAR, st::PIN_BUS])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = b.lstr()?;
    let start = pt(&mut b)?;
    let hot = pt(&mut b)?;
    let shape = b.u16()?;
    b.skip(2)?;
    let etype = PinType::from_code(b.u32()?);
    let mut display = Vec::new();
    if b.left() >= 6 && b.peek_u8() == Some(f.kind) {
        b.skip(4)?;
        display = read_display_list(&mut b, lib)?;
    }
    c.seek(f.end())?;
    Ok(Some(SymPin { slot, name, bus: f.kind == st::PIN_BUS, start, hot, shape, etype, display }))
}

/// Decode a symbol body framed by `f`. The cursor's end must be `f.end()`.
pub fn read_symbol_body(c: &mut Cur, f: &Frame, lib: &LibraryInfo) -> Res<SymbolDef> {
    let mut mism = 0usize;
    let name = c.lstr()?;
    let source_lib = c.lstr()?;
    let color = c.u32()?;
    let n = c.u16()?;
    let mut prims = Vec::with_capacity(n as usize);
    for _ in 0..n {
        skip_separators(c);
        prims.push(read_prim(c, lib, &mut mism)?);
    }
    // The body box follows the primitives. Its end is normally the next stop
    // above the cursor; a record carrying a legacy trailer puts eight more
    // bytes in front of it, and a record holding a vector group has stops one
    // byte short per group, so the box is read where the primitives end
    // unless the stop clearly says there is a trailer to step over.
    skip_marker_separators(c);
    let stop = f.next_stop_above(c.pos);
    if stop >= c.pos + 16 {
        c.seek(stop - 8)?;
    }
    let bbox = (c.i16()?, c.i16()?, c.i16()?, c.i16()?);
    let mut pins = Vec::new();
    let mut display = Vec::new();
    let mut general = None;
    let has_pins = matches!(
        f.kind,
        st::LIBRARY_PART
            | st::GLOBAL_SYMBOL
            | st::PORT_SYMBOL
            | st::OFFPAGE_SYMBOL
            | st::TITLEBLOCK_SYMBOL
            | st::ERC_SYMBOL
            | st::BOOKMARK_SYMBOL
    );
    if has_pins && c.left() >= 2 {
        let np = c.u16()? as usize;
        for slot in 0..np {
            if let Some(p) = read_pin(c, lib, slot)? {
                pins.push(p);
            }
        }
        if c.left() >= 2 {
            display = read_display_list(c, lib)?;
        }
        if f.kind == st::LIBRARY_PART {
            general = read_general(c);
        }
    }
    Ok(SymbolDef {
        kind: f.kind,
        name,
        source_lib,
        props: lib.props(&f.props),
        color,
        prims,
        bbox,
        pins,
        display,
        general,
        prim_mismatches: mism,
    })
}

/// The library-part tail: four strings and a flag word that end exactly at
/// the record end. When they do not, only the flags are taken from the last
/// two bytes.
fn read_general(c: &mut Cur) -> Option<PartGeneral> {
    let save = c.pos;
    let full = (|| -> Res<PartGeneral> {
        let implementation_path = c.lstr()?;
        let implementation = c.lstr()?;
        let reference_prefix = c.lstr()?;
        let part_value = c.lstr()?;
        let flags = c.u16()?;
        Ok(PartGeneral { implementation_path, implementation, reference_prefix, part_value, flags })
    })();
    match full {
        Ok(g) if c.is_done() => Some(g),
        _ => {
            c.pos = save;
            if c.left() >= 2 {
                c.pos = c.end - 2;
                let flags = c.u16().ok()?;
                Some(PartGeneral { flags, ..Default::default() })
            } else {
                None
            }
        }
    }
}

/// Read a framed symbol of any symbol type at the cursor.
pub fn read_symbol(c: &mut Cur, lib: &LibraryInfo) -> Res<SymbolDef> {
    let f = read_frame(c)?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let s = read_symbol_body(&mut b, &f, lib)?;
    c.seek(f.end())?;
    Ok(s)
}
