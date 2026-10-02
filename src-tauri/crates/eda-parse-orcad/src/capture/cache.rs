//! The design cache: every symbol and package a design places, stored inside
//! the `.DSN` so the design opens without its libraries.
//!
//! The `Cache` stream is a zero u16 then four counted sections — loose symbols
//! (power, port, off-page, title block…), library parts (one per symbol view,
//! `R.Normal`, `R.Convert`), part cells (the view list of a part) and packages
//! (the physical part: footprint, reference prefix and one device per section
//! with that section's pin numbers). Each section is a list of named groups; a
//! group holds every stored revision of that name, and the first is the one in
//! use.
//!
//! The per-part `Packages/<name>` streams hold the same three kinds for parts
//! edited locally in the design; where both exist the local copy wins.

use std::collections::BTreeMap;

use super::bytes::{Cur, Res};
use super::framing::{expect_frame, read_frame};
use super::library::LibraryInfo;
use super::symbol::{read_symbol_body, SymbolDef};

pub const PART_CELL: u8 = 6;
pub const PACKAGE: u8 = 31;
pub const DEVICE: u8 = 32;

/// One pin-number slot of a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinNumber {
    pub number: String,
    /// The pin is marked "ignore" for this package section.
    pub ignored: bool,
}

/// One section ("device") of a package.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Section suffix, e.g. `A`.
    pub unit: String,
    /// Indexed by the symbol's pin slot, including skipped slots.
    pub pins: Vec<Option<PinNumber>>,
    pub props: Vec<(String, String)>,
}

/// The physical part.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub name: String,
    pub source_lib: String,
    pub ref_prefix: String,
    pub footprint: String,
    pub props: Vec<(String, String)>,
    pub devices: Vec<Device>,
}

impl Package {
    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// A part cell: the views a part offers.
#[derive(Debug, Clone, PartialEq)]
pub struct PartCell {
    pub name: String,
    pub source_lib: String,
    pub views: Vec<String>,
    pub props: Vec<(String, String)>,
}

/// Everything the cache yields.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    /// Library-part views and loose symbols by cache name. Every stored
    /// revision is kept; the first is the default.
    pub symbols: BTreeMap<String, Vec<SymbolDef>>,
    pub packages: BTreeMap<String, Package>,
    pub cells: BTreeMap<String, PartCell>,
    /// Records whose frame was readable but whose body was not.
    pub undecoded: Vec<String>,
}

impl Cache {
    /// The default (first) definition of a cache name.
    pub fn symbol(&self, name: &str) -> Option<&SymbolDef> {
        self.symbols.get(name).and_then(|v| v.first()).or_else(|| {
            self.symbols
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.first())
        })
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages.get(name).or_else(|| {
            self.packages.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
        })
    }

    pub fn symbol_count(&self) -> usize {
        self.symbols.values().map(Vec::len).sum()
    }
}

fn read_device(c: &mut Cur, lib: &LibraryInfo) -> Res<Device> {
    let f = expect_frame(c, &[DEVICE])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let unit = b.lstr()?;
    let _ref = b.lstr()?;
    let n = b.u16()? as usize;
    let mut pins = Vec::with_capacity(n);
    for _ in 0..n {
        if b.left() >= 2 && b.buf[b.pos] == 0xFF && b.buf[b.pos + 1] == 0xFF {
            b.skip(2)?;
            pins.push(None);
            continue;
        }
        let number = b.lstr()?;
        let cfg = b.u8()?;
        pins.push(Some(PinNumber { number, ignored: cfg & 0x80 != 0 }));
    }
    c.seek(f.end())?;
    Ok(Device { unit, pins, props: lib.props(&f.props) })
}

/// Read a type-31 package at the cursor.
pub fn read_package(c: &mut Cur, lib: &LibraryInfo) -> Res<Package> {
    let f = expect_frame(c, &[PACKAGE])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = b.lstr()?;
    let source_lib = b.lstr()?;
    let ref_prefix = b.lstr()?;
    let _unknown = b.lstr()?;
    let footprint = b.lstr()?;
    let n = b.u16()?;
    let mut devices = Vec::with_capacity(n as usize);
    for _ in 0..n {
        devices.push(read_device(&mut b, lib)?);
    }
    c.seek(f.end())?;
    Ok(Package { name, source_lib, ref_prefix, footprint, props: lib.props(&f.props), devices })
}

/// Read a type-6 part cell at the cursor.
pub fn read_part_cell(c: &mut Cur, lib: &LibraryInfo) -> Res<PartCell> {
    let f = expect_frame(c, &[PART_CELL])?;
    let mut b = Cur::at(c.buf, f.body, f.end());
    let name = b.lstr()?;
    let source_lib = b.lstr()?;
    let mut views = Vec::new();
    if let Ok(n) = b.u16() {
        for _ in 0..n {
            match b.lstr() {
                Ok(v) => views.push(v),
                Err(_) => break,
            }
        }
    }
    c.seek(f.end())?;
    Ok(PartCell { name, source_lib, views, props: lib.props(&f.props) })
}

/// Read the `Cache` stream.
pub fn read_cache(data: &[u8], lib: &LibraryInfo) -> Res<Cache> {
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
                let at = c.pos;
                let f = read_frame(&mut c)?;
                if f.kind != entry_type {
                    return c.err(format!(
                        "cache entry declares type {entry_type} but frames as {}",
                        f.kind
                    ));
                }
                c.pos = at;
                match section {
                    0 | 1 => {
                        let mut b = Cur::at(c.buf, f.body, f.end());
                        match read_symbol_body(&mut b, &f, lib) {
                            Ok(s) => cache.symbols.entry(group.clone()).or_default().push(s),
                            Err(e) => cache.undecoded.push(format!("{group}: {e}")),
                        }
                    }
                    2 => match read_part_cell(&mut c, lib) {
                        Ok(p) => {
                            cache.cells.entry(group.clone()).or_insert(p);
                        }
                        Err(e) => cache.undecoded.push(format!("{group}: {e}")),
                    },
                    _ => match read_package(&mut c, lib) {
                        Ok(p) => {
                            cache.packages.entry(group.clone()).or_insert(p);
                        }
                        Err(e) => cache.undecoded.push(format!("{group}: {e}")),
                    },
                }
                c.seek(f.end())?;
            }
        }
    }
    if !c.is_done() {
        return c.err(format!("{} bytes after the four cache sections", c.left()));
    }
    Ok(cache)
}

/// Merge one `Packages/<name>` stream (a locally edited part) into the cache.
/// Layout: u16 cell count; per cell the cell, a u16 count and that many
/// library parts; then the package.
pub fn read_package_stream(data: &[u8], lib: &LibraryInfo, cache: &mut Cache) -> Res<()> {
    let mut c = Cur::new(data);
    let cells = c.u16()?;
    for _ in 0..cells {
        let cell = read_part_cell(&mut c, lib)?;
        let parts = c.u16()?;
        for _ in 0..parts {
            let f = read_frame(&mut c)?;
            let mut b = Cur::at(c.buf, f.body, f.end());
            match read_symbol_body(&mut b, &f, lib) {
                Ok(s) => {
                    let v = cache.symbols.entry(s.name.clone()).or_default();
                    // A local edit takes precedence over the cached copy.
                    v.insert(0, s);
                }
                Err(e) => cache.undecoded.push(format!("{}: {e}", cell.name)),
            }
            c.seek(f.end())?;
        }
        cache.cells.insert(cell.name.clone(), cell);
    }
    let pkg = read_package(&mut c, lib)?;
    cache.packages.insert(pkg.name.clone(), pkg);
    Ok(())
}
