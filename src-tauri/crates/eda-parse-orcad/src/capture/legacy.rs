//! The pre-2003 grammar (Library version below 3: Capture 9.x and 10.x).
//!
//! Three things differ from the modern grammar, everywhere at once:
//!
//! - a structure is its short prefix alone — no long prefixes, so no stated
//!   length, and no marker;
//! - property pairs are u16 string indices, not u32;
//! - a symbol primitive is one type byte and its body, with no doubled type
//!   byte and no length envelope.
//!
//! The bodies themselves are the modern ones field for field, with the
//! string-index words narrowed to u16 where the modern body holds a u32.
//! Without stated lengths nothing can be skipped: every reader here consumes
//! its record exactly, and a stream is read straight through. A failure ends
//! that stream; what was read before it is kept.

use super::bytes::{Cur, Res};
use super::cache::{Cache, Device, Package, PartCell, PinNumber, DEVICE, PACKAGE, PART_CELL};
use super::hierarchy::{OccNet, OccPin, OccTree, Occurrence, Scope};
use super::library::{LibraryInfo, PageSettings};
use super::page::{
    st as pst, Alias, Block, BlockPin, BusEntry, Graphic, NetGroup, Orient, Page, PageNet, PinInst, PlacedPart, Wire,
};
use super::symbol::{prim_body, st, DisplayProp, PartGeneral, PinType, Prim, SymPin, SymbolDef};

/// A short prefix: type, i16 pair count, that many (u16 name, u16 value).
struct Prefix {
    kind: u8,
    props: Vec<(u32, u32)>,
}

fn prefix(c: &mut Cur) -> Res<Prefix> {
    let kind = c.u8()?;
    let n = c.i16()?;
    if n > 4096 {
        return c.err(format!("structure type {kind} claims {n} properties"));
    }
    let mut props = Vec::with_capacity(n.max(0) as usize);
    for _ in 0..n.max(0) {
        let a = c.u16()?;
        let b = c.u16()?;
        // 0xFFFF is an empty index.
        let w = |v: u16| if v == 0xFFFF { 0 } else { v as u32 };
        props.push((w(a), w(b)));
    }
    Ok(Prefix { kind, props })
}

fn expect(c: &mut Cur, kinds: &[u8]) -> Res<Prefix> {
    let at = c.pos;
    let p = prefix(c)?;
    if !kinds.contains(&p.kind) {
        c.pos = at;
        return c.err(format!("expected structure type {kinds:?}, found {}", p.kind));
    }
    Ok(p)
}

/// A u16 string index (0xFFFF is empty).
fn sidx(c: &mut Cur, lib: &LibraryInfo) -> Res<String> {
    let v = c.u16()?;
    Ok(if v == 0xFFFF { String::new() } else { lib.s(v as u32).to_string() })
}

thread_local! {
    /// The display-property width of the stream being read. The oldest
    /// streams store a 10-byte body (no display mode, no terminator); one
    /// width holds for a whole stream, so a stream that does not read in the
    /// long form is read again in the short one.
    static SHORT_DISPLAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Read a whole stream, in the long display-property form and, failing that,
/// the short one.
fn either_width<T>(short_first: bool, mut read: impl FnMut() -> Res<T>) -> Res<T> {
    SHORT_DISPLAY.with(|s| s.set(short_first));
    let first = read();
    if first.is_ok() {
        return first;
    }
    SHORT_DISPLAY.with(|s| s.set(!short_first));
    let second = read();
    SHORT_DISPLAY.with(|s| s.set(false));
    second.or(first)
}

fn display_prop(c: &mut Cur, lib: &LibraryInfo) -> Res<DisplayProp> {
    expect(c, &[st::DISPLAY_PROP])?;
    let name = sidx(c, lib)?;
    let x = c.i16()?;
    let y = c.i16()?;
    let rf = c.u16()?;
    let (color, mode) = if SHORT_DISPLAY.with(|s| s.get()) {
        // A font-size byte, then the colour; shown as the value.
        let _size = c.u8()?;
        (c.u8()?, 0x100)
    } else {
        let color = c.u8()?;
        let mode = c.u16()?;
        if c.u8()? != 0 {
            return c.err("display property is not terminated");
        }
        (color, mode)
    };
    Ok(DisplayProp { name, x, y, font: rf & 0x3FFF, rotation: (rf >> 14) as u8, color, mode })
}

fn display_list(c: &mut Cur, lib: &LibraryInfo) -> Res<Vec<DisplayProp>> {
    let n = c.u16()?;
    let mut v = Vec::with_capacity(n as usize);
    for _ in 0..n {
        v.push(display_prop(c, lib)?);
    }
    Ok(v)
}

// -------------------------------------------------------------- symbols

const PRIM_TYPES: [u8; 11] = [40, 41, 42, 43, 44, 45, 46, 47, 48, 87, 90];

fn prim(c: &mut Cur) -> Res<Prim> {
    let t = c.u8()?;
    if t == 48 {
        return vector(c);
    }
    if !PRIM_TYPES.contains(&t) {
        c.pos -= 1;
        return c.err(format!("unknown primitive type {t}"));
    }
    prim_body(t, c)
}

/// A vector group: its prefix, its offset, a counted list of children each
/// behind a zero pad byte, then its name.
fn vector(c: &mut Cur) -> Res<Prim> {
    expect(c, &[48])?;
    let at = (c.i16()? as i32, c.i16()? as i32);
    let n = c.u16()?;
    let mut prims = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let t = c.u8()?;
        if c.peek_u8() == Some(0) {
            c.skip(1)?;
        }
        if t == 48 {
            prims.push(vector(c)?);
        } else if PRIM_TYPES.contains(&t) {
            prims.push(prim_body(t, c)?);
        } else {
            return c.err(format!("unknown vector primitive type {t}"));
        }
    }
    let name = c.lstr()?;
    Ok(Prim::Group { at, name, prims })
}

fn pin(c: &mut Cur, lib: &LibraryInfo, slot: usize) -> Res<Option<SymPin>> {
    if c.peek_u8() == Some(0) {
        c.skip(1)?;
        return Ok(None);
    }
    let p = expect(c, &[st::PIN_SCALAR, st::PIN_BUS])?;
    let name = c.lstr()?;
    let start = (c.i32()?, c.i32()?);
    let hot = (c.i32()?, c.i32()?);
    let shape = c.u16()?;
    c.skip(2)?;
    let etype = PinType::from_code(c.u32()?);
    // The pin's type repeats, then three reserved bytes and its display list.
    let mut display = Vec::new();
    if c.peek_u8() == Some(p.kind) && c.peek_at(4).map(|b| b < 0x20).unwrap_or(false) {
        c.skip(4)?;
        display = display_list(c, lib)?;
    }
    Ok(Some(SymPin { slot, name, bus: p.kind == st::PIN_BUS, start, hot, shape, etype, display }))
}

/// A library part's tail: four strings and the flag word.
fn general(c: &mut Cur) -> Res<PartGeneral> {
    Ok(PartGeneral {
        implementation_path: c.lstr()?,
        implementation: c.lstr()?,
        reference_prefix: c.lstr()?,
        part_value: c.lstr()?,
        flags: c.u16()?,
    })
}

/// A symbol of any symbol type at the cursor, prefix included.
pub fn symbol(c: &mut Cur, lib: &LibraryInfo) -> Res<SymbolDef> {
    let p = prefix(c)?;
    symbol_body(c, p.kind, &p.props, lib)
}

/// A standalone symbol stream (`Symbols/<name>` of a library).
pub fn symbol_stream(data: &[u8], lib: &LibraryInfo) -> Res<SymbolDef> {
    either_width(lib.version.0 < 2, || symbol(&mut Cur::new(data), lib))
}

fn symbol_body(c: &mut Cur, kind: u8, props: &[(u32, u32)], lib: &LibraryInfo) -> Res<SymbolDef> {
    let name = c.lstr()?;
    let source_lib = c.lstr()?;
    let color = c.u32()?;
    let n = c.u16()?;
    let mut prims = Vec::with_capacity(n as usize);
    for _ in 0..n {
        prims.push(prim(c)?);
    }
    let bbox = (c.i16()?, c.i16()?, c.i16()?, c.i16()?);
    let mut pins = Vec::new();
    let mut display = Vec::new();
    let mut gen = None;
    // A body nested in a placed graphic ends at its box.
    if kind != st::STH_IN_PAGES0 {
        let np = c.u16()? as usize;
        for slot in 0..np {
            if let Some(p) = pin(c, lib, slot)? {
                pins.push(p);
            }
        }
        display = display_list(c, lib)?;
        if kind == st::LIBRARY_PART {
            gen = Some(general(c)?);
        }
    }
    Ok(SymbolDef {
        kind,
        name,
        source_lib,
        props: lib.props(props),
        color,
        prims,
        bbox,
        pins,
        display,
        general: gen,
        prim_mismatches: 0,
    })
}

// -------------------------------------------------------------- cache and packages

fn device(c: &mut Cur, lib: &LibraryInfo) -> Res<Device> {
    let p = expect(c, &[DEVICE])?;
    let unit = c.lstr()?;
    let _ref = c.lstr()?;
    let n = c.u16()? as usize;
    let mut pins = Vec::with_capacity(n);
    for _ in 0..n {
        if c.left() >= 2 && c.buf[c.pos] == 0xFF && c.buf[c.pos + 1] == 0xFF {
            c.skip(2)?;
            pins.push(None);
            continue;
        }
        let number = c.lstr()?;
        let cfg = c.u8()?;
        pins.push(Some(PinNumber { number, ignored: cfg & 0x80 != 0 }));
    }
    Ok(Device { unit, pins, props: lib.props(&p.props) })
}

fn package(c: &mut Cur, lib: &LibraryInfo) -> Res<Package> {
    let p = expect(c, &[PACKAGE])?;
    let name = c.lstr()?;
    let source_lib = c.lstr()?;
    let ref_prefix = c.lstr()?;
    let _unknown = c.lstr()?;
    let footprint = c.lstr()?;
    let n = c.u16()?;
    let mut devices = Vec::with_capacity(n as usize);
    for _ in 0..n {
        devices.push(device(c, lib)?);
    }
    Ok(Package { name, source_lib, ref_prefix, footprint, props: lib.props(&p.props), devices })
}

fn part_cell(c: &mut Cur, lib: &LibraryInfo) -> Res<PartCell> {
    let p = expect(c, &[PART_CELL])?;
    let name = c.lstr()?;
    let source_lib = c.lstr()?;
    let n = c.u16()?;
    let mut views = Vec::with_capacity(n as usize);
    for _ in 0..n {
        views.push(c.lstr()?);
    }
    Ok(PartCell { name, source_lib, views, props: lib.props(&p.props) })
}

/// The `Cache` stream: a zero marker, then four counted sections of named
/// groups of stored revisions, as in the modern grammar.
pub fn read_cache(data: &[u8], lib: &LibraryInfo) -> Res<Cache> {
    either_width(lib.version.0 < 2, || cache_once(data, lib))
}

fn cache_once(data: &[u8], lib: &LibraryInfo) -> Res<Cache> {
    let mut cache = Cache::default();
    let mut c = Cur::new(data);
    if c.u16()? != 0 {
        return c.err("cache does not open with its zero marker");
    }
    for section in 0..4 {
        let groups = c.u16()?;
        for _ in 0..groups {
            let group = c.lstr()?;
            let variants = c.u16()?;
            for _ in 0..variants {
                let _src = c.lstr()?;
                let _created = c.u32()?;
                let _modified = c.u32()?;
                let entry_type = c.u8()?;
                c.skip(1)?;
                if c.peek_u8() != Some(entry_type) {
                    return c.err(format!("cache entry declares type {entry_type} but holds {:?}", c.peek_u8()));
                }
                match section {
                    0 | 1 => {
                        let s = symbol(&mut c, lib)?;
                        cache.symbols.entry(group.clone()).or_default().push(s);
                    }
                    2 => {
                        let p = part_cell(&mut c, lib)?;
                        cache.cells.entry(group.clone()).or_insert(p);
                    }
                    _ => {
                        let p = package(&mut c, lib)?;
                        cache.packages.entry(group.clone()).or_insert(p);
                    }
                }
            }
        }
    }
    if !c.is_done() {
        return c.err(format!("{} bytes after the four cache sections", c.left()));
    }
    Ok(cache)
}

/// One `Packages/<name>` stream: cells with their library parts, then the
/// package.
pub fn read_package_stream(data: &[u8], lib: &LibraryInfo, cache: &mut Cache) -> Res<()> {
    // Read into a scratch copy so a failed first attempt leaves no half-read
    // records behind.
    let merged = either_width(lib.version.0 < 2, || {
        let mut scratch = Cache::default();
        package_stream_once(data, lib, &mut scratch).map(|_| scratch)
    })?;
    for (k, v) in merged.symbols {
        let e = cache.symbols.entry(k).or_default();
        for (i, s) in v.into_iter().enumerate() {
            e.insert(i, s);
        }
    }
    cache.cells.extend(merged.cells);
    cache.packages.extend(merged.packages);
    Ok(())
}

fn package_stream_once(data: &[u8], lib: &LibraryInfo, cache: &mut Cache) -> Res<()> {
    let mut c = Cur::new(data);
    let cells = c.u16()?;
    for _ in 0..cells {
        let cell = part_cell(&mut c, lib)?;
        let parts = c.u16()?;
        for _ in 0..parts {
            let s = symbol(&mut c, lib)?;
            cache.symbols.entry(s.name.clone()).or_default().insert(0, s);
        }
        cache.cells.insert(cell.name.clone(), cell);
    }
    let pkg = package(&mut c, lib)?;
    cache.packages.insert(pkg.name.clone(), pkg);
    Ok(())
}

// -------------------------------------------------------------- pages

fn alias(c: &mut Cur) -> Res<Alias> {
    expect(c, &[pst::ALIAS])?;
    let pos = (c.i32()?, c.i32()?);
    let color = c.u32()?;
    let rotation = c.u32()?;
    let font = c.u32()?;
    let name = c.lstr()?;
    Ok(Alias { pos, rotation, font, color, name })
}

fn wire(c: &mut Cur, lib: &LibraryInfo) -> Res<Wire> {
    let p = expect(c, &[pst::WIRE_SCALAR, pst::WIRE_BUS])?;
    let db_id = c.u32()?;
    let net_id = c.u32()?;
    let color = c.u32()?;
    let a = (c.i32()?, c.i32()?);
    let b = (c.i32()?, c.i32()?);
    c.skip(1)?;
    let n = c.u16()?;
    let mut aliases = Vec::with_capacity(n as usize);
    for _ in 0..n {
        aliases.push(alias(c)?);
    }
    // The wire's displayed properties (a shown net name, say).
    let _display = display_list(c, lib)?;
    Ok(Wire { db_id, net_id, bus: p.kind == pst::WIRE_BUS, a, b, color, aliases, props: lib.props(&p.props), width: 0, style: 0 })
}

fn pin_inst(c: &mut Cur, lib: &LibraryInfo) -> Res<PinInst> {
    let p = expect(c, &[pst::PIN_INST_SCALAR, pst::PIN_INST_BUS])?;
    let index = c.i16()?;
    let pos = (c.i16()? as i32, c.i16()? as i32);
    let word_a = c.u32()?;
    let word_b = c.u32()?;
    let display = display_list(c, lib)?;
    Ok(PinInst { index, pos, bus: p.kind == pst::PIN_INST_BUS, word_a, word_b, props: lib.props(&p.props), display })
}

fn placed(c: &mut Cur, p: &Prefix, lib: &LibraryInfo) -> Res<PlacedPart> {
    c.skip(2)?;
    let source_lib = sidx(c, lib)?;
    c.skip(4)?;
    let cache_name = c.lstr()?;
    let db_id = c.u32()?;
    let (by1, bx1, by2, bx2) = (c.i16()?, c.i16()?, c.i16()?, c.i16()?);
    let pos = (c.i16()? as i32, c.i16()? as i32);
    let color = c.u8()?;
    let orient = Orient::from_byte(c.u8()?);
    c.skip(2)?;
    let display = display_list(c, lib)?;
    c.skip(1)?;
    let reference = c.lstr()?;
    let value = sidx(c, lib)?;
    c.skip(6)?;
    let np = c.u16()?;
    let mut pins = Vec::with_capacity(np as usize);
    for _ in 0..np {
        pins.push(pin_inst(c, lib)?);
    }
    let package = c.lstr()?;
    let unit_index = c.u16()?;
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
        props: lib.props(&p.props),
        display,
        pins,
    })
}

fn block(c: &mut Cur, p: &Prefix, lib: &LibraryInfo) -> Res<Block> {
    let name = sidx(c, lib)?;
    let _src = sidx(c, lib)?;
    c.skip(4)?;
    let cache_name = c.lstr()?;
    let db_id = c.u32()?;
    let _anchor = (c.i16()?, c.i16()?);
    let (y2, x2, x1, y1) = (c.i16()?, c.i16()?, c.i16()?, c.i16()?);
    c.skip(4)?;
    let display = display_list(c, lib)?;
    if c.u8()? != st::LIBRARY_PART {
        return c.err("hierarchical block has no inline library part");
    }
    let interface = symbol(c, lib)?;
    c.skip(14)?;
    let reference = c.lstr()?;
    let _value = sidx(c, lib)?;
    c.skip(2)?;
    let implementation_idx = sidx(c, lib)?;
    c.skip(2)?;
    let n = c.u16()?;
    let mut positions = Vec::with_capacity(n as usize);
    for _ in 0..n {
        positions.push(pin_inst(c, lib)?);
    }
    let props = lib.props(&p.props);
    let degenerate = positions.len() > 1 && positions.iter().all(|q| q.pos == positions[0].pos);
    let pins = interface
        .pins
        .iter()
        .enumerate()
        .map(|(k, sp)| {
            let inst = positions.get(k);
            let pos = match inst {
                Some(q) if !degenerate => q.pos,
                _ => (x1.min(x2) as i32 + sp.hot.0, y1.min(y2) as i32 + sp.hot.1),
            };
            BlockPin {
                name: sp.name.clone(),
                pos,
                bus: sp.bus,
                no_connect: inst.map(|q| q.no_connect()).unwrap_or(false),
                etype: sp.etype,
            }
        })
        .collect();
    let implementation = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Implementation"))
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| Some(implementation_idx).filter(|v| !v.is_empty()))
        .or_else(|| interface.general.as_ref().map(|g| g.implementation.clone()))
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
        interface: Some(interface),
    })
}

fn graphic(c: &mut Cur, kinds: &[u8], lib: &LibraryInfo) -> Res<Graphic> {
    let p = expect(c, kinds)?;
    let name = sidx(c, lib)?;
    let source_lib = sidx(c, lib)?;
    c.skip(4)?;
    let cache_name = c.lstr()?;
    let db_id = c.u32()?;
    let y = c.i16()? as i32;
    let x = c.i16()? as i32;
    let (y2, x2, x1, y1) = (c.i16()?, c.i16()?, c.i16()?, c.i16()?);
    let color = c.u8()?;
    let orient = Orient::from_byte(c.u8()?);
    c.skip(2)?;
    let display = display_list(c, lib)?;
    // A 0x02 flag introduces one nested body; anything else is the flag
    // byte of an empty slot and ends the record.
    let flag = c.u8()?;
    let body = if flag == st::STH_IN_PAGES0 {
        let sp = expect(c, &[st::STH_IN_PAGES0])?;
        Some(symbol_body(c, sp.kind, &sp.props, lib)?)
    } else {
        None
    };
    Ok(Graphic {
        kind: p.kind,
        db_id,
        name,
        cache_name,
        source_lib,
        pos: (x, y),
        bbox: (x1 as i32, y1 as i32, x2 as i32, y2 as i32),
        orient,
        color,
        props: lib.props(&p.props),
        display,
        body,
    })
}

fn graphics(c: &mut Cur, kinds: &[u8], trailer: usize, lib: &LibraryInfo) -> Res<Vec<Graphic>> {
    let n = c.u16()?;
    let mut v = Vec::with_capacity(n as usize);
    for _ in 0..n {
        v.push(graphic(c, kinds, lib)?);
        c.skip(trailer)?;
    }
    Ok(v)
}

/// One page stream, read straight through.
pub fn read_page(stream: &str, data: &[u8], lib: &LibraryInfo) -> Res<Page> {
    let long = either_width(lib.version.0 < 2, || {
        let p = page_once(stream, data, lib)?;
        match &p.truncated {
            Some(t) => Err(super::bytes::DecodeError { offset: 0, what: t.clone() }),
            None => Ok(p),
        }
    });
    // Neither width reads the whole page: keep what the expected width read.
    long.or_else(|_| {
        SHORT_DISPLAY.with(|s| s.set(lib.version.0 < 2));
        let p = page_once(stream, data, lib);
        SHORT_DISPLAY.with(|s| s.set(false));
        p
    })
}

fn page_once(stream: &str, data: &[u8], lib: &LibraryInfo) -> Res<Page> {
    let mut c = Cur::new(data);
    let p = expect(&mut c, &[pst::PAGE])?;
    let mut page = Page { stream: stream.to_string(), props: lib.props(&p.props), ..Page::default() };
    page.name = c.lstr()?;
    page.size_name = c.lstr()?;
    page.settings = PageSettings::read(&mut c)?;
    if let Err(e) = page_lists(&mut c, lib, &mut page) {
        page.truncated = Some(e.to_string());
    } else if c.left() > 0 && c.buf[c.pos..].iter().any(|&b| b != 0) {
        page.truncated = Some(format!("{} bytes after the last list", c.left()));
    }
    Ok(page)
}

fn page_lists(c: &mut Cur, lib: &LibraryInfo, page: &mut Page) -> Res<()> {
    page.title_blocks = graphics(c, &[pst::TITLEBLOCK], 12, lib)?;
    let n = c.u16()?;
    for _ in 0..n {
        let id = c.u32()?;
        let name = c.lstr()?;
        page.nets.push(PageNet { id, name });
    }
    let n = c.u16()?;
    for _ in 0..n {
        let id = c.u32()?;
        let name = c.lstr()?;
        let m = c.u16()?;
        let mut members = Vec::with_capacity(m as usize);
        for _ in 0..m {
            members.push(c.u32()?);
        }
        page.groups.push(NetGroup { id, name, members });
    }
    let n = c.u16()?;
    for _ in 0..n {
        let name = c.lstr()?;
        let id = c.u32()?;
        page.net_names.push(PageNet { id, name });
    }
    let n = c.u16()?;
    for _ in 0..n {
        page.wires.push(wire(c, lib)?);
    }
    let n = c.u16()?;
    for _ in 0..n {
        let p = expect(c, &[pst::PLACED_INSTANCE, pst::DRAWN_INSTANCE])?;
        if p.kind == pst::PLACED_INSTANCE {
            page.parts.push(placed(c, &p, lib)?);
        } else {
            page.blocks.push(block(c, &p, lib)?);
        }
    }
    page.ports = graphics(c, &[pst::PORT], 9, lib)?;
    page.globals = graphics(c, &[pst::GLOBAL], 5, lib)?;
    page.offpages = graphics(c, &[pst::OFFPAGE], 5, lib)?;
    let n = c.u16()?;
    for _ in 0..n {
        page.erc.push(graphic(c, &[pst::ERC_OBJECT], lib)?);
        for _ in 0..3 {
            c.lstr()?;
        }
    }
    let n = c.u16()?;
    for _ in 0..n {
        expect(c, &[pst::BUS_ENTRY])?;
        let color = c.u32()?;
        let a = (c.i32()?, c.i32()?);
        let b = (c.i32()?, c.i32()?);
        c.skip(8)?;
        page.bus_entries.push(BusEntry { a, b, color });
    }
    page.graphics = graphics(c, &pst::GRAPHICS, 0, lib)?;
    Ok(())
}

/// A folder's page display order (`Views/<folder>/Schematic`), stored last
/// page first.
pub fn page_order(data: &[u8]) -> Option<Vec<String>> {
    let mut c = Cur::new(data);
    prefix(&mut c).ok()?;
    let _name = c.lstr().ok()?;
    c.skip(4).ok()?;
    let n = c.u16().ok()?;
    let mut v = Vec::with_capacity(n as usize);
    for _ in 0..n {
        v.push(c.lstr().ok()?);
    }
    v.reverse();
    Some(v)
}

// -------------------------------------------------------------- hierarchy

const OCC_PART: u8 = 0x42;
const OCC_NET: u8 = 0x43;
const OCC_PIN_SCALAR: u8 = 0x44;
const OCC_PIN_BUS: u8 = 0x45;
const OCC_TITLE: u8 = 0x52;

fn scope(c: &mut Cur, lib: &LibraryInfo, depth: usize) -> Res<Scope> {
    if depth > 64 {
        return c.err("occurrence tree deeper than 64");
    }
    let mut s = Scope::default();
    let n = c.u16()?;
    for _ in 0..n {
        expect(c, &[OCC_NET])?;
        let db_id = c.u32()?;
        let name = c.lstr()?;
        s.nets.push(OccNet { db_id, name });
    }
    let n = c.u16()?;
    for _ in 0..n {
        expect(c, &[OCC_TITLE])?;
        c.skip(8)?;
    }
    let n = c.u16()?;
    for _ in 0..n {
        let p = expect(c, &[OCC_PART])?;
        let own_db_id = c.u32()?;
        let target_db_id = c.u32()?;
        let child_folder = c.lstr()?;
        let reference = c.lstr()?;
        let unit = Some(sidx(c, lib)?).filter(|u| !u.is_empty());
        let np = c.u16()?;
        let mut pins = Vec::with_capacity(np as usize);
        for _ in 0..np {
            let pp = expect(c, &[OCC_PIN_SCALAR, OCC_PIN_BUS])?;
            let db_id = c.u32()?;
            let index = c.u16()?;
            pins.push(OccPin { db_id, index, name: None, props: lib.props(&pp.props) });
        }
        let nested = scope(c, lib, depth + 1)?;
        s.occurrences.push(Occurrence {
            own_db_id,
            target_db_id,
            child_folder,
            reference,
            unit,
            props: lib.props(&p.props),
            pins,
            nested,
        });
    }
    Ok(s)
}

/// The occurrence tree, consumed exactly.
pub fn read_tree(data: &[u8], lib: &LibraryInfo) -> Res<Option<OccTree>> {
    let mut c = Cur::new(data);
    let view = c.lstr()?;
    c.skip(5)?;
    let n = c.u16()?;
    let mut power = Vec::with_capacity(n as usize);
    for _ in 0..n {
        expect(&mut c, &[OCC_PIN_SCALAR])?;
        let db_id = c.u32()?;
        let name = c.lstr()?;
        power.push(OccNet { db_id, name });
    }
    let root = scope(&mut c, lib, 0)?;
    if !c.is_done() {
        return c.err(format!("{} bytes after the occurrence tree", c.left()));
    }
    Ok(Some(OccTree { view, power, root }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::library::FileRole;

    fn lib(strings: &[&str], major: u16) -> LibraryInfo {
        LibraryInfo {
            role: FileRole::Design,
            version: (major, 0),
            created: 0,
            modified: 0,
            fonts: Vec::new(),
            part_fields: Vec::new(),
            page_defaults: PageSettings::default(),
            strings: strings.iter().map(|s| s.to_string()).collect(),
            aliases: Vec::new(),
            root_name: String::new(),
        }
    }

    #[test]
    fn a_prefix_carries_u16_property_indices_with_ffff_empty() {
        let b = [0x0D, 2, 0, 1, 0, 2, 0, 0xFF, 0xFF, 1, 0];
        let mut c = Cur::new(&b);
        let p = prefix(&mut c).unwrap();
        assert_eq!(p.kind, 13);
        assert_eq!(p.props, vec![(1, 2), (0, 1)]);
        assert!(c.is_done());
    }

    #[test]
    fn display_properties_read_in_either_width() {
        let l = lib(&["", "Value"], 2);
        // Long form: name, x, y, rotation/font, colour, u16 mode, terminator.
        let long = [0x27, 0xFF, 0xFF, 1, 0, 10, 0, 0xEC, 0xFF, 0, 0, 0x30, 0, 1, 0];
        let d = display_prop(&mut Cur::new(&long), &l).unwrap();
        assert_eq!((d.name.as_str(), d.x, d.y, d.mode), ("Value", 10, -20, 0x100));
        // Short form: a font-size byte and the colour, nothing more.
        let short = [0x27, 0xFF, 0xFF, 1, 0, 10, 0, 0xEC, 0xFF, 0, 0, 0x30, 1];
        assert!(display_prop(&mut Cur::new(&short), &l).is_err());
        let d = either_width(false, || {
            let mut c = Cur::new(&short);
            let d = display_prop(&mut c, &l)?;
            if c.is_done() { Ok(d) } else { c.err("trailing") }
        })
        .unwrap();
        assert_eq!((d.x, d.color, d.mode), (10, 1, 0x100));
    }

    #[test]
    fn a_primitive_is_its_type_byte_and_the_modern_body() {
        let mut b = vec![41u8];
        for v in [0i32, 0, 30, 10, 0, 0] {
            b.extend(v.to_le_bytes());
        }
        let mut c = Cur::new(&b);
        assert!(matches!(prim(&mut c).unwrap(), Prim::Line { b: (30, 10), .. }));
        assert!(c.is_done());
    }
}
