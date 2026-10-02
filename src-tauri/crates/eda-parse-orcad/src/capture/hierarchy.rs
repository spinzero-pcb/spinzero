//! The occurrence tree (`Views/<root>/Hierarchy/Hierarchy`).
//!
//! A schematic folder placed by several hierarchical blocks is drawn once but
//! instantiated several times; each instantiation is an OCCURRENCE with its own
//! reference designators, unit assignments, property overrides and net names.
//! Capture keeps the whole instantiation tree here. A design annotated per
//! occurrence stores only the `R?` template on the placed part itself, so the
//! designators a BOM needs are read from this tree, not from the page.
//!
//! Every record opens with one long prefix, a short prefix carrying the
//! record's property overrides, and the marker. Records are read in order;
//! the type byte each position demands is checked, and a mismatch fails the
//! whole tree rather than resynchronising on a guess.

use super::bytes::{Cur, Res};
use super::framing::MARKER;
use super::library::LibraryInfo;

pub const OCC_PART: u8 = 0x42;
pub const OCC_NET: u8 = 0x43;
pub const OCC_PIN_SCALAR: u8 = 0x44;
pub const OCC_PIN_BUS: u8 = 0x45;
pub const OCC_TITLE: u8 = 0x52;
pub const OCC_GLOBAL: u8 = 0x5B;

/// A net name along one instantiation path.
#[derive(Debug, Clone, PartialEq)]
pub struct OccNet {
    pub db_id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OccPin {
    pub db_id: u32,
    /// Pin slot of a part occurrence.
    pub index: u16,
    /// Pin name, for the records that name a block pin (its child's port)
    /// rather than number a symbol pin.
    pub name: Option<String>,
    pub props: Vec<(String, String)>,
}

/// One part or block occurrence.
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub own_db_id: u32,
    /// The placed part (type 13) or block (type 12) on the parent page this
    /// occurrence instantiates.
    pub target_db_id: u32,
    /// The child schematic folder for a block; empty for a part.
    pub child_folder: String,
    pub reference: String,
    /// Package section override (`A`), when given.
    pub unit: Option<String>,
    /// Per-occurrence property overrides.
    pub props: Vec<(String, String)>,
    pub pins: Vec<OccPin>,
    pub nested: Scope,
}

impl Occurrence {
    pub fn is_block(&self) -> bool {
        !self.child_folder.is_empty()
    }

    pub fn prop(&self, name: &str) -> Option<&str> {
        self.props.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The occurrences visible at one point of an instantiation path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    pub nets: Vec<OccNet>,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OccTree {
    pub view: String,
    pub power: Vec<OccNet>,
    pub root: Scope,
}

impl OccTree {
    /// Every occurrence in the tree, depth first.
    pub fn count(&self) -> usize {
        fn walk(s: &Scope) -> usize {
            s.occurrences.iter().map(|o| 1 + walk(&o.nested)).sum()
        }
        walk(&self.root)
    }
}

/// Read an occurrence record header of type `kind`; returns its properties.
fn header(c: &mut Cur, kind: u8, lib: &LibraryInfo) -> Res<Vec<(String, String)>> {
    let at = c.pos;
    let t = c.u8()?;
    if t != kind {
        c.pos = at;
        return c.err(format!("occurrence record: expected type {kind:#x}, found {t:#x}"));
    }
    c.skip(4)?;
    if c.u32()? != 0 {
        return c.err("occurrence long prefix pad is not zero");
    }
    if c.u8()? != kind {
        return c.err("occurrence short prefix type disagrees");
    }
    let n = c.i16()?;
    let mut pairs = Vec::new();
    for _ in 0..n.max(0) {
        pairs.push((c.u32()?, c.u32()?));
    }
    if c.bytes(4)? != MARKER {
        return c.err("occurrence record has no marker");
    }
    let t = c.u32()? as usize;
    c.skip(t)?;
    Ok(lib.props(&pairs))
}

fn scope(c: &mut Cur, lib: &LibraryInfo, depth: usize) -> Res<Scope> {
    if depth > 64 {
        return c.err("occurrence tree deeper than 64");
    }
    let mut s = Scope::default();
    let n = c.u16()?;
    for _ in 0..n {
        header(c, OCC_NET, lib)?;
        let db_id = c.u32()?;
        let name = c.lstr()?;
        s.nets.push(OccNet { db_id, name });
    }
    let n = c.u16()?;
    for _ in 0..n {
        header(c, OCC_TITLE, lib)?;
        c.skip(8)?;
    }
    let n = c.u32()?;
    for _ in 0..n {
        header(c, OCC_GLOBAL, lib)?;
        c.skip(8)?;
    }
    if c.left() >= 8 && c.buf[c.pos..c.pos + 4] == MARKER {
        c.skip(4)?;
        let t = c.u32()? as usize;
        c.skip(t)?;
    }
    let n = c.u16()?;
    for _ in 0..n {
        let props = header(c, OCC_PART, lib)?;
        let own_db_id = c.u32()?;
        let target_db_id = c.u32()?;
        if c.u8()? != OCC_PART {
            return c.err("occurrence inner marker missing");
        }
        c.skip(8)?;
        let child_folder = c.lstr()?;
        let reference = c.lstr()?;
        let unit_idx = c.u32()?;
        let unit = (unit_idx != 0)
            .then(|| lib.strings.get(unit_idx as usize).cloned())
            .flatten()
            .filter(|u| !u.is_empty());
        let np = c.u16()?;
        let mut pins = Vec::with_capacity(np as usize);
        for _ in 0..np {
            let kind = c.peek_u8().unwrap_or(0);
            if kind != OCC_PIN_SCALAR && kind != OCC_PIN_BUS {
                return c.err(format!("occurrence pin: unexpected type {kind:#x}"));
            }
            let props = header(c, kind, lib)?;
            let db_id = c.u32()?;
            // A pin record whose id has the top bit set names a port (a
            // block pin) instead of numbering a symbol pin.
            if db_id & 0x8000_0000 == 0 {
                let index = c.u16()?;
                pins.push(OccPin { db_id, index, name: None, props });
            } else {
                let name = c.lstr()?;
                pins.push(OccPin { db_id, index: 0, name: Some(name), props });
            }
        }
        let nested = scope(c, lib, depth + 1)?;
        s.occurrences.push(Occurrence {
            own_db_id,
            target_db_id,
            child_folder,
            reference,
            unit,
            props,
            pins,
            nested,
        });
    }
    Ok(s)
}

/// Read the modern occurrence tree. A stream that does not open with the
/// occurrence type is an instance-annotated legacy design and has no tree.
pub fn read_tree(data: &[u8], lib: &LibraryInfo) -> Res<Option<OccTree>> {
    let mut c = Cur::new(data);
    if c.peek_u8() != Some(OCC_PART) {
        return Ok(None);
    }
    c.skip(9)?;
    let view = c.lstr()?;
    c.skip(7)?;
    let n = c.u16()?;
    let mut power = Vec::with_capacity(n as usize);
    for _ in 0..n {
        header(&mut c, OCC_PIN_SCALAR, lib)?;
        let db_id = c.u32()?;
        let name = c.lstr()?;
        power.push(OccNet { db_id, name });
    }
    let root = scope(&mut c, lib, 0)?;
    Ok(Some(OccTree { view, power, root }))
}
