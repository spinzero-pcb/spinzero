//! One schematic page (`Views/<folder>/Pages/<page>`).
//!
//! The stream is a single framed structure of type 10 whose body is a run of
//! u16-counted lists in a fixed order: title blocks, the page's net records,
//! its bus (net-group) records, the net name table, wires, placed parts and
//! hierarchical blocks, ports, power symbols, off-page connectors, saved ERC
//! markers, bus entries and free graphics.
//!
//! Capture stores its own connectivity here: every wire names a net id, and
//! the net table names the id. Pins do not carry a net; a pin joins whatever
//! wire its connection point touches.
//!
//! Page coordinates are Capture's database unit (10 mil), Y down.

use super::bytes::{Cur, Res};
use super::framing::{expect_frame, read_frame, Frame};
use super::library::{LibraryInfo, PageSettings};
use super::symbol::{read_display_list, read_symbol_body, DisplayProp, SymbolDef};

pub mod st {
    pub const PAGE: u8 = 10;
    pub const DRAWN_INSTANCE: u8 = 12;
    pub const PLACED_INSTANCE: u8 = 13;
    pub const PIN_INST_SCALAR: u8 = 16;
    pub const PIN_INST_BUS: u8 = 17;
    pub const WIRE_SCALAR: u8 = 20;
    pub const WIRE_BUS: u8 = 21;
    pub const PORT: u8 = 23;
    pub const BUS_ENTRY: u8 = 29;
    pub const GLOBAL: u8 = 37;
    pub const OFFPAGE: u8 = 38;
    pub const ALIAS: u8 = 49;
    pub const TITLEBLOCK: u8 = 65;
    pub const ERC_OBJECT: u8 = 77;
    pub const GRAPHICS: [u8; 10] = [55, 56, 57, 58, 59, 60, 61, 62, 88, 89];
}

/// Quarter turns plus mirror, as an instance stores its placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orient {
    /// Counter-clockwise quarter turns on screen.
    pub turns: u8,
    /// Mirrored about the vertical axis before rotation.
    pub mirror: bool,
}

impl Orient {
    pub fn from_byte(b: u8) -> Orient {
        Orient { turns: b & 3, mirror: b & 4 != 0 }
    }

    /// The linear part of the placement: mirror X first, then `turns`
    /// counter-clockwise quarter turns, each (x, y) → (y, −x) in Capture's
    /// Y-down space.
    pub fn linear(&self, p: (i32, i32)) -> (i32, i32) {
        let (mut x, mut y) = p;
        if self.mirror {
            x = -x;
        }
        for _ in 0..self.turns {
            let (nx, ny) = (y, -x);
            x = nx;
            y = ny;
        }
        (x, y)
    }

    /// Map a symbol-space point onto the page.
    ///
    /// The stored position is the top-left corner of the TRANSFORMED body box,
    /// not the transformed symbol origin: after the turn and the mirror the
    /// box is shifted back so its minimum corner lands on `origin`. Fitted on
    /// every placed part in the corpus, whose pins are stored both in symbol
    /// space and at their absolute page positions.
    pub fn place(&self, p: (i32, i32), origin: (i32, i32), body: (i32, i32, i32, i32)) -> (i32, i32) {
        let (x1, y1, x2, y2) = body;
        let corners = [(x1, y1), (x2, y1), (x2, y2), (x1, y2)].map(|c| self.linear(c));
        let mx = corners.iter().map(|c| c.0).min().unwrap_or(0);
        let my = corners.iter().map(|c| c.1).min().unwrap_or(0);
        let t = self.linear(p);
        (origin.0 + t.0 - mx, origin.1 + t.1 - my)
    }
}

/// A named net on one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageNet {
    pub id: u32,
    pub name: String,
}

/// A bus: a net-group record naming its member nets.
#[derive(Debug, Clone, PartialEq)]
pub struct NetGroup {
    pub id: u32,
    pub name: String,
    pub members: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alias {
    pub pos: (i32, i32),
    pub rotation: u32,
    pub font: u32,
    pub color: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub db_id: u32,
    pub net_id: u32,
    pub bus: bool,
    pub a: (i32, i32),
    pub b: (i32, i32),
    pub color: u32,
    pub aliases: Vec<Alias>,
    pub props: Vec<(String, String)>,
    pub width: u32,
    pub style: u32,
}

/// A pin of a placed part, at its absolute page position.
#[derive(Debug, Clone, PartialEq)]
pub struct PinInst {
    /// One-based pin slot in the symbol; negative marks an explicit
    /// no-connect on that slot.
    pub index: i16,
    pub pos: (i32, i32),
    pub bus: bool,
    pub props: Vec<(String, String)>,
    pub display: Vec<DisplayProp>,
}

impl PinInst {
    pub fn no_connect(&self) -> bool {
        self.index < 0
    }

    /// The zero-based symbol pin slot this instance pin stands for.
    pub fn slot(&self) -> usize {
        ((self.index as i32).abs() - 1).max(0) as usize
    }
}

/// A part placed on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPart {
    pub db_id: u32,
    /// Cache key of the symbol view drawn (`R.Normal`).
    pub cache_name: String,
    pub source_lib: String,
    /// Reference as stored on the instance; may be the `R?` template when the
    /// design is annotated per occurrence.
    pub reference: String,
    pub value: String,
    pub pos: (i32, i32),
    pub orient: Orient,
    /// (x1, y1, x2, y2) including displayed text.
    pub bbox: (i32, i32, i32, i32),
    pub color: u8,
    /// Package name and zero-based section within it.
    pub package: String,
    pub unit_index: u16,
    pub props: Vec<(String, String)>,
    pub display: Vec<DisplayProp>,
    pub pins: Vec<PinInst>,
}

impl PlacedPart {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A pin on a hierarchical block.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockPin {
    pub name: String,
    pub pos: (i32, i32),
    pub bus: bool,
    pub no_connect: bool,
    pub etype: super::symbol::PinType,
}

/// A hierarchical block: the parent side of a child schematic.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub db_id: u32,
    /// The block's Name property.
    pub name: String,
    pub cache_name: String,
    pub reference: String,
    /// Implementation (child schematic folder) as the block's properties name it.
    pub implementation: String,
    pub rect: (i32, i32, i32, i32),
    pub props: Vec<(String, String)>,
    pub display: Vec<DisplayProp>,
    pub pins: Vec<BlockPin>,
    /// The inline part that defines the pin interface.
    pub interface: Option<SymbolDef>,
}

impl Block {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A placed symbol other than a part: power symbol, off-page connector, port,
/// title block, ERC marker or a free graphic.
#[derive(Debug, Clone, PartialEq)]
pub struct Graphic {
    pub kind: u8,
    pub db_id: u32,
    /// Logical name (the net a power symbol drives, a port's name).
    pub name: String,
    /// Cache symbol name.
    pub cache_name: String,
    pub source_lib: String,
    pub pos: (i32, i32),
    pub bbox: (i32, i32, i32, i32),
    pub orient: Orient,
    pub color: u8,
    pub props: Vec<(String, String)>,
    pub display: Vec<DisplayProp>,
    /// A body drawn by the instance itself (free graphics carry one).
    pub body: Option<SymbolDef>,
}

impl Graphic {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BusEntry {
    pub a: (i32, i32),
    pub b: (i32, i32),
    pub color: u32,
}

/// One decoded page.
#[derive(Debug, Clone, Default)]
pub struct Page {
    /// The stream name under `Pages/`.
    pub stream: String,
    pub name: String,
    pub size_name: String,
    pub settings: PageSettings,
    pub props: Vec<(String, String)>,
    pub title_blocks: Vec<Graphic>,
    pub nets: Vec<PageNet>,
    pub groups: Vec<NetGroup>,
    /// The page's net name table (id → name). One id may be listed more than
    /// once; every name is kept.
    pub net_names: Vec<PageNet>,
    pub wires: Vec<Wire>,
    pub parts: Vec<PlacedPart>,
    pub blocks: Vec<Block>,
    pub ports: Vec<Graphic>,
    pub globals: Vec<Graphic>,
    pub offpages: Vec<Graphic>,
    pub erc: Vec<Graphic>,
    pub bus_entries: Vec<BusEntry>,
    pub graphics: Vec<Graphic>,
    /// Lists the reader could not finish, with the reason. Everything read
    /// before the failure is kept.
    pub truncated: Option<String>,
}

fn read_alias(c: &mut Cur) -> Res<Alias> {
    let f = expect_frame(c, &[st::ALIAS])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let pos = (b.i32()?, b.i32()?);
    let color = b.u32()?;
    let rotation = b.u32()?;
    let font = b.u32()?;
    let name = b.lstr()?;
    c.seek(f.end())?;
    Ok(Alias { pos, rotation, font, color, name })
}

fn read_wire(c: &mut Cur, lib: &LibraryInfo) -> Res<Wire> {
    let f = expect_frame(c, &[st::WIRE_SCALAR, st::WIRE_BUS])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let db_id = b.u32()?;
    let net_id = b.u32()?;
    let color = b.u32()?;
    let a = (b.i32()?, b.i32()?);
    let bb = (b.i32()?, b.i32()?);
    b.skip(1)?;
    let n = b.u16()?;
    let mut aliases = Vec::with_capacity(n as usize);
    for _ in 0..n {
        aliases.push(read_alias(&mut b)?);
    }
    let np = b.u16()?;
    for _ in 0..np {
        let pf = read_frame(&mut b)?;
        b.seek(pf.end())?;
    }
    let width = b.u32().unwrap_or(0);
    let style = b.u32().unwrap_or(0);
    c.seek(f.end())?;
    Ok(Wire {
        db_id,
        net_id,
        bus: f.kind == st::WIRE_BUS,
        a,
        b: bb,
        color,
        aliases,
        props: lib.props(&f.props),
        width,
        style,
    })
}

fn read_pin_inst(c: &mut Cur, lib: &LibraryInfo) -> Res<PinInst> {
    let f = expect_frame(c, &[st::PIN_INST_SCALAR, st::PIN_INST_BUS])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let index = b.i16()?;
    let pos = (b.i16()? as i32, b.i16()? as i32);
    b.skip(8)?;
    let display = if b.left() >= 2 { read_display_list(&mut b, lib)? } else { Vec::new() };
    c.seek(f.end())?;
    Ok(PinInst { index, pos, bus: f.kind == st::PIN_INST_BUS, props: lib.props(&f.props), display })
}

fn read_placed(c: &mut Cur, f: &Frame, lib: &LibraryInfo) -> Res<PlacedPart> {
    let mut b = Cur::at(c.buf, f.body, f.end());
    b.skip(4)?;
    let source_lib = lib.s(b.u32()?).to_string();
    let cache_name = b.lstr()?;
    let db_id = b.u32()?;
    let (by1, bx1, by2, bx2) = (b.i16()?, b.i16()?, b.i16()?, b.i16()?);
    let pos = (b.i16()? as i32, b.i16()? as i32);
    let color = b.u8()?;
    let orient = Orient::from_byte(b.u8()?);
    b.skip(2)?;
    let display = read_display_list(&mut b, lib)?;
    b.skip(1)?;
    let reference = b.lstr()?;
    let value = lib.s(b.u32()?).to_string();
    b.skip(10)?;
    let np = b.u16()?;
    let mut pins = Vec::with_capacity(np as usize);
    for _ in 0..np {
        pins.push(read_pin_inst(&mut b, lib)?);
    }
    let package = b.lstr()?;
    let unit_index = b.u16()?;
    Ok(PlacedPart {
        db_id,
        cache_name,
        source_lib,
        reference,
        value,
        pos,
        orient,
        bbox: (bx1 as i32, by1 as i32, bx2 as i32, by2 as i32),
        color,
        package,
        unit_index,
        props: lib.props(&f.props),
        display,
        pins,
    })
}

fn read_block(c: &mut Cur, f: &Frame, lib: &LibraryInfo) -> Res<Block> {
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = lib.s(b.u32()?).to_string();
    let _src = b.u32()?;
    let cache_name = b.lstr()?;
    let db_id = b.u32()?;
    let _anchor = (b.i16()?, b.i16()?);
    let (y2, x2, x1, y1) = (b.i16()?, b.i16()?, b.i16()?, b.i16()?);
    b.skip(4)?;
    let display = read_display_list(&mut b, lib)?;
    let mut interface = None;
    if b.peek_u8() == Some(24) {
        b.skip(1)?;
        let pf = read_frame(&mut b)?;
        let mut pb = Cur::at(b.buf, pf.body, pf.end());
        interface = read_symbol_body(&mut pb, &pf, lib).ok();
        b.seek(pf.end())?;
    }
    // The reference starts at the second-to-last stop of the block's chain.
    if f.stops.len() >= 2 {
        let s = f.stops[f.stops.len() - 2];
        if s >= b.pos && s <= f.end() {
            b.seek(s)?;
        }
    }
    let reference = b.lstr().unwrap_or_default();
    let mut positions = Vec::new();
    if b.skip(14).is_ok() {
        if let Ok(n) = b.u16() {
            for _ in 0..n {
                match read_pin_inst(&mut b, lib) {
                    Ok(p) => positions.push(p),
                    Err(_) => break,
                }
            }
        }
    }
    let props = lib.props(&f.props);
    let mut pins = Vec::new();
    if let Some(i) = &interface {
        // When Capture never re-laid the block out, every position record
        // reports one point; the interface's own hot points are then placed
        // relative to the block's corner instead.
        let degenerate =
            positions.len() > 1 && positions.iter().all(|p| p.pos == positions[0].pos);
        for (k, sp) in i.pins.iter().enumerate() {
            let inst = positions.get(k);
            let pos = match inst {
                Some(p) if !degenerate => p.pos,
                _ => (x1.min(x2) as i32 + sp.hot.0, y1.min(y2) as i32 + sp.hot.1),
            };
            pins.push(BlockPin {
                name: sp.name.clone(),
                pos,
                bus: sp.bus,
                no_connect: inst.map(|p| p.no_connect()).unwrap_or(false),
                etype: sp.etype,
            });
        }
    }
    let implementation = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Implementation"))
        .map(|(_, v)| v.clone())
        .or_else(|| interface.as_ref().and_then(|i| i.general.as_ref()).map(|g| g.implementation.clone()))
        .unwrap_or_default();
    Ok(Block {
        db_id,
        name,
        cache_name,
        reference,
        implementation,
        rect: (x1 as i32, y1 as i32, x2 as i32, y2 as i32),
        props,
        display,
        pins,
        interface,
    })
}

/// Read a graphic-instance body (ports, power symbols, off-page connectors,
/// title blocks, ERC markers and free graphics all share it).
fn read_graphic(c: &mut Cur, f: &Frame, lib: &LibraryInfo) -> Res<Graphic> {
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = lib.s(b.u32()?).to_string();
    let source_lib = lib.s(b.u32()?).to_string();
    let cache_name = b.lstr()?;
    let db_id = b.u32()?;
    let y = b.i16()? as i32;
    let x = b.i16()? as i32;
    let (y2, x2, x1, y1) = (b.i16()?, b.i16()?, b.i16()?, b.i16()?);
    let color = b.u8()?;
    let orient = Orient::from_byte(b.u8()?);
    b.skip(2)?;
    let display = read_display_list(&mut b, lib)?;
    let mut body = None;
    if b.peek_u8() == Some(2) {
        b.skip(1)?;
        let pf = read_frame(&mut b)?;
        let mut pb = Cur::at(b.buf, pf.body, pf.end());
        body = read_symbol_body(&mut pb, &pf, lib).ok();
    }
    Ok(Graphic {
        kind: f.kind,
        db_id,
        name,
        cache_name,
        source_lib,
        pos: (x, y),
        bbox: (x1 as i32, y1 as i32, x2 as i32, y2 as i32),
        orient,
        color,
        props: lib.props(&f.props),
        display,
        body,
    })
}

fn graphic_list(
    c: &mut Cur,
    lib: &LibraryInfo,
    kinds: &[u8],
    trailer: usize,
) -> Res<Vec<Graphic>> {
    let n = c.u16()?;
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let f = expect_frame(c, kinds)?;
        out.push(read_graphic(c, &f, lib)?);
        c.seek(f.end())?;
        c.skip(trailer)?;
    }
    Ok(out)
}

/// A raw (unframed) net record.
fn read_net_record(c: &mut Cur) -> Res<PageNet> {
    c.skip(9)?;
    let id = c.u32()?;
    let name = c.lstr()?;
    c.skip(16)?;
    Ok(PageNet { id, name })
}

/// Read one page stream.
pub fn read_page(stream: &str, data: &[u8], lib: &LibraryInfo) -> Res<Page> {
    let mut c = Cur::new(data);
    let f = expect_frame(&mut c, &[st::PAGE])?;
    let mut page = Page { stream: stream.to_string(), props: lib.props(&f.props), ..Page::default() };
    let mut b = Cur::at(c.buf, f.body, f.end());
    page.name = b.lstr()?;
    page.size_name = b.lstr()?;
    page.settings = PageSettings::read(&mut b)?;
    if let Err(e) = read_page_lists(&mut b, lib, &mut page) {
        page.truncated = Some(e.to_string());
    } else if b.left() == 4 && b.buf[b.pos..b.pos + 4] == [0, 0, 0, 0] {
        // Two further u16 list counts close every page; both are zero in
        // every corpus page, so what they would list is unknown.
    } else if !b.is_done() {
        page.truncated = Some(format!("{} bytes after the last list", b.left()));
    }
    Ok(page)
}

fn read_page_lists(b: &mut Cur, lib: &LibraryInfo, page: &mut Page) -> Res<()> {
    page.title_blocks = graphic_list(b, lib, &[st::TITLEBLOCK], 0)?;
    let n = b.u16()?;
    for _ in 0..n {
        page.nets.push(read_net_record(b)?);
    }
    let n = b.u16()?;
    for _ in 0..n {
        let net = read_net_record(b)?;
        let m = b.u16()?;
        let mut members = Vec::with_capacity(m as usize);
        for _ in 0..m {
            members.push(b.u32()?);
        }
        page.groups.push(NetGroup { id: net.id, name: net.name, members });
    }
    let n = b.u16()?;
    for _ in 0..n {
        let name = b.lstr()?;
        let id = b.u32()?;
        page.net_names.push(PageNet { id, name });
    }
    let n = b.u16()?;
    for _ in 0..n {
        page.wires.push(read_wire(b, lib)?);
    }
    let n = b.u16()?;
    for _ in 0..n {
        let f = expect_frame(b, &[st::PLACED_INSTANCE, st::DRAWN_INSTANCE])?;
        if f.kind == st::PLACED_INSTANCE {
            page.parts.push(read_placed(b, &f, lib)?);
        } else {
            page.blocks.push(read_block(b, &f, lib)?);
        }
        b.seek(f.end())?;
    }
    page.ports = graphic_list(b, lib, &[st::PORT], 0)?;
    page.globals = graphic_list(b, lib, &[st::GLOBAL], 5)?;
    page.offpages = graphic_list(b, lib, &[st::OFFPAGE], 5)?;
    page.erc = graphic_list(b, lib, &[st::ERC_OBJECT], 0)?;
    let n = b.u16()?;
    for _ in 0..n {
        let f = expect_frame(b, &[st::BUS_ENTRY])?;
        let mut e = Cur::at(b.buf, f.body, f.end());
        let color = e.u32()?;
        let a = (e.i32()?, e.i32()?);
        let bb = (e.i32()?, e.i32()?);
        page.bus_entries.push(BusEntry { a, b: bb, color });
        b.seek(f.end())?;
    }
    page.graphics = graphic_list(b, lib, &st::GRAPHICS, 0)?;
    Ok(())
}
