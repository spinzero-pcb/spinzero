//! The object stream: every block the board database stores.
//!
//! Blocks carry no length. Each is a type byte, three header bytes, then a
//! layout fixed by the type AND the format generation (fields come and go at
//! 16.2, 16.4, 16.5, 17.2, 17.4, 17.5, 18.0, 18.1 and 19.0), so reading the
//! stream means sizing every type correctly — one wrong field and every later
//! block is misread. The walk is therefore checked: blocks start on four-byte
//! boundaries, the stream ends at a zero type byte, and the number of blocks
//! read is compared with the count the header states.
//!
//! What a review does not use is sized and skipped (`Block::Other`), so the
//! catalogue below is complete for sizing but keeps only what the board model
//! needs.

use std::collections::HashMap;

use super::header::{Header, Ver};
use super::reader::{Res, R};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Layer {
    pub class: u8,
    pub sub: u8,
}

pub mod class {
    pub const BOARD_GEOMETRY: u8 = 0x01;
    pub const COMPONENT_VALUE: u8 = 0x02;
    pub const DEVICE_TYPE: u8 = 0x03;
    pub const DRAWING_FORMAT: u8 = 0x04;
    pub const ETCH: u8 = 0x06;
    pub const MANUFACTURING: u8 = 0x07;
    pub const PACKAGE_GEOMETRY: u8 = 0x09;
    pub const PACKAGE_KEEPIN: u8 = 0x0A;
    pub const PACKAGE_KEEPOUT: u8 = 0x0B;
    pub const PIN: u8 = 0x0C;
    pub const REF_DES: u8 = 0x0D;
    pub const ROUTE_KEEPIN: u8 = 0x0E;
    pub const ROUTE_KEEPOUT: u8 = 0x0F;
    pub const TOLERANCE: u8 = 0x10;
    pub const USER_PART_NUMBER: u8 = 0x11;
    pub const VIA_CLASS: u8 = 0x12;
    pub const VIA_KEEPOUT: u8 = 0x13;
    pub const ANTI_ETCH: u8 = 0x14;
    pub const BOUNDARY: u8 = 0x15;
    pub const CONSTRAINTS_REGION: u8 = 0x16;
}

/// A value a field block (0x03) carries.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    None,
    Int(u32),
    Pair(u32, u32),
    Text(String),
    Words(Vec<u32>),
    Bytes(Vec<u8>),
}

/// One component of a padstack: a shape on one layer slot.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PadComp {
    pub kind: u8,
    pub w: i32,
    pub h: i32,
    /// Corner radius (rounded rectangle) or chamfer (chamfered rectangle).
    pub corner: i32,
    pub off_x: i32,
    pub off_y: i32,
    /// A custom shape (0x28) for shape-symbol pads.
    pub shape: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Padstack {
    pub name: u32,
    pub start_layer: u8,
    pub layer_count: u16,
    /// Finished drill (width), and height for a slot.
    pub drill_w: u32,
    pub drill_h: u32,
    pub plated: bool,
    /// Drill-type nibble from the 17.2+ header: 0x00 through, 0x10 via,
    /// 0x20/0xA0 SMD, 0x30 slot, 0x80 NPTH. `None` before 17.2.
    pub kind: Option<u8>,
    pub comps: Vec<PadComp>,
    /// Technical-layer slots before the per-copper groups.
    pub fixed: usize,
    /// Components per copper layer (antipad, thermal, pad[, keepout]).
    pub per_layer: usize,
}

impl Padstack {
    /// The pad (not antipad or thermal) on copper layer `i` of the padstack.
    pub fn pad_on(&self, i: usize) -> Option<&PadComp> {
        self.comps.get(self.fixed + i * self.per_layer + 2).filter(|c| c.kind != 0)
    }

    pub fn antipad_on(&self, i: usize) -> Option<&PadComp> {
        self.comps.get(self.fixed + i * self.per_layer).filter(|c| c.kind != 0)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Font {
    pub height: u32,
    pub width: u32,
    pub stroke: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    Arc { parent: u32, width: u32, start: (i32, i32), end: (i32, i32), center: (f64, f64), radius: f64, clockwise: bool },
    Field { code: u16, value: FieldValue },
    NetAssign { net: u32, item: u32 },
    Track { layer: Layer, net_assign: u32, first_seg: u32 },
    Component { device_type: u32, symbol: u32, first_inst: u32, first_pin: u32, fields: u32 },
    CompInst { fp_inst: u32, refdes: u32, first_pad: u32, fields: u32 },
    PinNumber { text: u32, pin_name: u32 },
    PinName { text: u32, pin_number: u32 },
    Pad { name: u32, at: (i32, i32), padstack: u32, rotation: u32, flags: u32 },
    Rect { layer: Layer, parent: u32, coords: [i32; 4], rotation: u32 },
    Graphic { layer: Layer, parent: u32, first_seg: u32 },
    Seg { parent: u32, width: u32, a: (i32, i32), b: (i32, i32) },
    Net { name: u32, assign: u32, fields: u32, match_group: u32 },
    Padstack(Box<Padstack>),
    ConstraintSet { name: u32, field: u32, records: Vec<[i32; 14]> },
    MatchGroup { member: u32, group: u32 },
    Shape { layer: Layer, ptr1: u32, first_seg: u32, first_keepout: u32, table: u32, coords: [i32; 4], dynamic: u32 },
    LayerList { names: Vec<LayerName> },
    FootprintDef { name: u32, first_inst: u32, fields: u32 },
    Table { subtype: u16, text: u32, p1: u32, p2: u32, p3: u32 },
    FootprintInst { bottom: bool, rotation: u32, at: (i32, i32), inst_ref: u32, graphic: u32, first_pad: u32, text: u32, assembly: u32, areas: u32 },
    Text { layer: Layer, sgraphic: u32, at: (i32, i32), rotation: u32, font: u8, align: u8, reversal: u8, group: u32 },
    StrGraphic { wrapper: u32, at: (i32, i32), text: String },
    PlacedPad { layer: Layer, net: u32, next_in_fp: u32, parent_fp: u32, pad: u32, pin_number: u32, coords: [i32; 4] },
    Via { layer: Layer, net: u32, at: (i32, i32), padstack: u32 },
    Keepout { layer: Layer, first_seg: u32 },
    Fonts(Vec<Font>),
    PtrArray { count: u32, ptrs: Vec<u32> },
    Other,
}

/// A layer list entry: the name directly (before 16.5) or by string id.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerName {
    Text(String),
    Id(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: u8,
    pub offset: usize,
    pub key: u32,
    pub next: u32,
    pub data: Data,
}

fn layer(r: &mut R) -> Res<Layer> {
    Ok(Layer { class: r.u8()?, sub: r.u8()? })
}

fn pt(r: &mut R) -> Res<(i32, i32)> {
    Ok((r.i32()?, r.i32()?))
}

fn coords4(r: &mut R) -> Res<[i32; 4]> {
    Ok([r.i32()?, r.i32()?, r.i32()?, r.i32()?])
}

const NO_0X27_END: &str = "constraint cross-reference block has no stated end";

/// Find where a 0x27 block with no stated end stops: the first four-byte
/// boundary at which three consecutive blocks read cleanly with keys that are
/// new and inside the range of keys already seen. Its payload is a list of
/// keys and zeros, which cannot satisfy that by accident three times running.
fn find_after_0x27(data: &[u8], from: usize, v: Ver, seen: &HashMap<u32, usize>, lo: u32, hi: u32) -> Option<usize> {
    let mut p = (from + 3) & !3;
    let span = hi.saturating_sub(lo).max(1) as u64;
    let (lo, hi) = (lo as u64, hi as u64 + span);
    while p + 64 < data.len() {
        let t = data[p];
        if (1..=0x3E).contains(&t) && t != 0x27 {
            let mut r = R::new(data);
            r.pos = p;
            let mut ok = 0;
            while ok < 3 {
                match read_block(&mut r, v, 0) {
                    Ok(Some(b)) if b.key != 0
                        && !seen.contains_key(&b.key)
                        && (b.key as u64) >= lo
                        && (b.key as u64) <= hi
                        && r.pos % 4 == 0 => ok += 1,
                    _ => break,
                }
            }
            if ok == 3 {
                return Some(p);
            }
        }
        p += 4;
    }
    None
}

/// Read one block at the cursor (positioned at its type byte).
fn read_block(r: &mut R, v: Ver, end_0x27: usize) -> Res<Option<Block>> {
    let offset = r.pos;
    let kind = r.u8()?;
    let ge = |x: Ver| v >= x;
    let mut key = 0;
    let mut next = 0;
    let data = match kind {
        0x00 => return Ok(None),
        0x01 => {
            r.skip(1)?;
            let _u = r.u8()?;
            let sub = r.u8()?;
            key = r.u32()?;
            next = r.u32()?;
            let parent = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let width = r.u32()?;
            let start = pt(r)?;
            let end = pt(r)?;
            let center = (r.f64()?, r.f64()?);
            let radius = r.f64()?;
            r.skip(16)?;
            Data::Arc { parent, width, start, end, center, radius, clockwise: sub & 0x40 != 0 }
        }
        0x03 => {
            r.skip(1)?;
            let code = r.u16()?;
            key = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let sub = r.u8()?;
            let _h2 = r.u8()?;
            let size = r.u16()? as usize;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let value = match sub {
                0x65 => FieldValue::None,
                0x64 | 0x66 | 0x67 | 0x6A => FieldValue::Int(r.u32()?),
                0x69 => FieldValue::Pair(r.u32()?, r.u32()?),
                0x68 | 0x6B | 0x6D | 0x6E | 0x6F | 0x71 | 0x73 | 0x78 => FieldValue::Text(r.fixed(size, true)?),
                0x6C => {
                    let n = r.u32()? as usize;
                    if n > 1_000_000 {
                        return r.err("field word list too long");
                    }
                    FieldValue::Words(r.words(n)?)
                }
                0x70 | 0x74 => {
                    let x0 = r.u16()? as usize;
                    let x1 = r.u16()? as usize;
                    FieldValue::Bytes(r.bytes(x1 + 4 * x0)?.to_vec())
                }
                0xF6 => FieldValue::Words(r.words(20)?),
                _ => match size {
                    4 => FieldValue::Int(r.u32()?),
                    8 => FieldValue::Pair(r.u32()?, r.u32()?),
                    _ => return r.err(format!("field subtype {sub:#x} of size {size} is not known")),
                },
            };
            Data::Field { code, value }
        }
        0x04 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            let net = r.u32()?;
            let item = r.u32()?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::NetAssign { net, item }
        }
        0x05 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let net_assign = r.u32()?;
            r.skip(4 * 8)?;
            if ge(Ver::V172) {
                r.skip(8)?;
            }
            let first_seg = r.u32()?;
            r.skip(8)?;
            Data::Track { layer, net_assign, first_seg }
        }
        0x06 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            let device_type = r.u32()?;
            let symbol = r.u32()?;
            let first_inst = r.u32()?;
            let _slot = r.u32()?;
            let first_pin = r.u32()?;
            let fields = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            Data::Component { device_type, symbol, first_inst, first_pin, fields }
        }
        0x07 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V172) {
                r.skip(12)?;
            }
            let fp_inst = r.u32()?;
            if !ge(Ver::V172) {
                r.skip(4)?;
            }
            let refdes = r.u32()?;
            let _func = r.u32()?;
            let fields = r.u32()?;
            r.skip(4)?;
            let first_pad = r.u32()?;
            Data::CompInst { fp_inst, refdes, first_pad, fields }
        }
        0x08 => {
            r.skip(3)?;
            key = r.u32()?;
            let mut text = 0;
            if ge(Ver::V172) {
                r.skip(4)?;
            } else {
                text = r.u32()?;
            }
            next = r.u32()?;
            if ge(Ver::V172) {
                text = r.u32()?;
            }
            let pin_name = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            Data::PinNumber { text, pin_name }
        }
        0x09 => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(16)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(20)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x0A => {
            r.skip(1)?;
            let _l = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(16 + 16 + 20)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x0C => {
            r.skip(1)?;
            let _l = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(8)?;
            r.skip(if ge(Ver::V172) { 12 } else { 4 })?;
            r.skip(4)?;
            if ge(Ver::V180) {
                r.skip(4)?;
            }
            r.skip(16 + 12)?;
            if ge(Ver::V174) && !ge(Ver::V180) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x0D => {
            r.skip(3)?;
            key = r.u32()?;
            let name = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let at = pt(r)?;
            let padstack = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let flags = r.u32()?;
            let rotation = r.u32()?;
            Data::Pad { name, at, padstack, rotation, flags }
        }
        0x0E => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let parent = r.u32()?;
            r.skip(12)?;
            if ge(Ver::V172) {
                r.skip(8)?;
            }
            let coords = coords4(r)?;
            r.skip(12)?;
            let rotation = r.u32()?;
            Data::Rect { layer, parent, coords, rotation }
        }
        0x0F => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            if !ge(Ver::V190) {
                r.skip(32)?;
            } else {
                r.skip(4)?;
            }
            if ge(Ver::V172) {
                next = r.u32()?;
            }
            r.skip(12)?;
            Data::Other
        }
        0x10 => {
            r.skip(3)?;
            key = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            r.skip(20)?;
            Data::Other
        }
        0x11 => {
            r.skip(3)?;
            key = r.u32()?;
            let text = r.u32()?;
            next = r.u32()?;
            let pin_number = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::PinName { text, pin_number }
        }
        0x12 => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(16)?;
            if ge(Ver::V165) {
                r.skip(4)?;
            }
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x14 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let parent = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let first_seg = r.u32()?;
            r.skip(8)?;
            Data::Graphic { layer, parent, first_seg }
        }
        0x15..=0x17 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            let parent = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let width = r.u32()?;
            let a = pt(r)?;
            let b = pt(r)?;
            Data::Seg { parent, width, a, b }
        }
        0x1B => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            let name = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            let assign = r.u32()?;
            r.skip(4)?;
            let fields = r.u32()?;
            let match_group = r.u32()?;
            r.skip(16)?;
            Data::Net { name, assign, fields, match_group }
        }
        0x1C => {
            let _b = r.u8()?;
            let n = r.u8()? as usize;
            let start_layer = r.u8()?;
            key = r.u32()?;
            next = r.u32()?;
            let name = r.u32()?;
            let (layer_count, drill_w, drill_h, plated, ps_kind);
            if !ge(Ver::V172) {
                let drill = r.u32()?;
                r.skip(20)?;
                let _mark = r.u8()?;
                let flags = r.u8()?;
                r.skip(2)?;
                r.skip(6)?;
                layer_count = r.u16()?;
                r.skip(16)?;
                r.skip(4)?;
                let slot_x = r.u32()?;
                let slot_y = r.u32()?;
                r.skip(4)?;
                if ge(Ver::V165) {
                    r.skip(4)?;
                }
                plated = flags & 0x01 != 0;
                ps_kind = None;
                if slot_y != 0 {
                    drill_w = slot_x;
                    drill_h = slot_y;
                } else {
                    drill_w = drill;
                    drill_h = 0;
                }
            } else {
                r.skip(12)?;
                let pt_a = r.u8()?;
                let _b2 = r.u8()?;
                let flags = r.u8()?;
                let _d = r.u8()?;
                r.skip(8)?;
                r.skip(4)?;
                layer_count = r.u16()?;
                r.skip(2)?;
                r.skip(16)?;
                let drill = r.u32()?;
                r.skip(8)?;
                let slot_x = r.u32()?;
                let slot_y = r.u32()?;
                r.skip(8)?;
                r.skip(16)?;
                r.skip(21 * 4)?;
                if ge(Ver::V180) {
                    r.skip(32)?;
                }
                plated = flags & 0x20 != 0;
                ps_kind = Some(pt_a & 0xF0);
                if slot_y != 0 {
                    drill_w = slot_x;
                    drill_h = slot_y;
                } else {
                    drill_w = drill;
                    drill_h = 0;
                }
            }
            if layer_count > 256 {
                return r.err(format!("padstack layer count {layer_count} is implausible"));
            }
            let fixed = if !ge(Ver::V165) {
                10
            } else if !ge(Ver::V172) {
                11
            } else {
                21
            };
            let per_layer = if ge(Ver::V172) { 4 } else { 3 };
            let total = fixed + layer_count as usize * per_layer;
            let mut comps = Vec::with_capacity(total);
            for i in 0..total {
                let kind = r.u8()?;
                r.skip(3)?;
                if ge(Ver::V172) {
                    r.skip(4)?;
                }
                let w = r.i32()?;
                let h = r.i32()?;
                let corner = if ge(Ver::V172) { r.i32()? } else { 0 };
                let off_x = r.i32()?;
                let off_y = r.i32()?;
                let shape;
                if ge(Ver::V172) {
                    r.skip(4)?;
                    shape = r.u32()?;
                } else {
                    shape = r.u32()?;
                    if i + 1 < total {
                        r.skip(4)?;
                    }
                }
                comps.push(PadComp { kind, w, h, corner, off_x, off_y, shape });
            }
            r.skip(n * if ge(Ver::V172) { 40 } else { 32 })?;
            Data::Padstack(Box::new(Padstack {
                name,
                start_layer,
                layer_count,
                drill_w,
                drill_h,
                plated,
                kind: ps_kind,
                comps,
                fixed,
                per_layer,
            }))
        }
        0x1D => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            let name = r.u32()?;
            let field = r.u32()?;
            let size_a = r.u16()? as usize;
            let size_b = r.u16()? as usize;
            let mut records = Vec::with_capacity(size_b);
            for _ in 0..size_b {
                let mut rec = [0i32; 14];
                for v in rec.iter_mut() {
                    *v = r.i32()?;
                }
                records.push(rec);
            }
            r.skip(size_a * 256)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            Data::ConstraintSet { name, field, records }
        }
        0x1E => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V162) {
                r.skip(4)?;
            }
            r.skip(4)?;
            let size = r.u32()? as usize;
            r.skip(size)?;
            r.align4()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x1F => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(12 + 2)?;
            let size = r.u16()? as usize;
            let n = if ge(Ver::V175) {
                size * 384 + 8
            } else if ge(Ver::V172) {
                size * 280 + 8
            } else if ge(Ver::V162) {
                size * 280 + 4
            } else {
                size * 240 + 4
            };
            r.skip(n)?;
            Data::Other
        }
        0x20 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(28)?;
            if ge(Ver::V174) {
                r.skip(40)?;
            }
            Data::Other
        }
        0x21 => {
            r.skip(3)?;
            let size = r.u32()? as usize;
            if size < 12 {
                return r.err("blob block shorter than its own header");
            }
            key = r.u32()?;
            r.skip(size - 12)?;
            Data::Other
        }
        0x22 => {
            r.skip(3)?;
            key = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(32)?;
            Data::Other
        }
        0x23 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(8 + 12 + 20 + 16)?;
            if ge(Ver::V164) {
                r.skip(16)?;
            }
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x24 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let parent = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let coords = coords4(r)?;
            r.skip(12)?;
            let rotation = r.u32()?;
            Data::Rect { layer, parent, coords, rotation }
        }
        0x26 => {
            r.skip(3)?;
            key = r.u32()?;
            let member = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let group = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::MatchGroup { member, group }
        }
        0x27 => {
            // Sized by the header: it runs to one byte before the stated end.
            // Some 16.x boards state no end at all (0); the walker then finds
            // the next block by search, see `read_stream`.
            let end = end_0x27.saturating_sub(1);
            if end <= r.pos || end > r.buf.len() {
                return r.err(NO_0X27_END);
            }
            r.pos = end;
            Data::Other
        }
        0x28 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let ptr1 = r.u32()?;
            r.skip(4)?;
            let mut dynamic = 0;
            if ge(Ver::V172) {
                dynamic = r.u32()?;
                r.skip(4)?;
            }
            r.skip(8)?;
            let first_keepout = r.u32()?;
            let first_seg = r.u32()?;
            r.skip(8)?;
            let mut table = 0;
            if ge(Ver::V172) {
                table = r.u32()?;
            }
            r.skip(4)?;
            if !ge(Ver::V172) {
                table = r.u32()?;
            }
            let coords = coords4(r)?;
            Data::Shape { layer, ptr1, first_seg, first_keepout, table, coords, dynamic }
        }
        0x29 => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(4 * 12)?;
            Data::Other
        }
        0x2A => {
            r.skip(1)?;
            let n = r.u16()? as usize;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let mut names = Vec::with_capacity(n);
            for _ in 0..n {
                if !ge(Ver::V165) {
                    names.push(LayerName::Text(r.fixed(36, true)?));
                } else {
                    let id = r.u32()?;
                    r.skip(8)?;
                    names.push(LayerName::Id(id));
                }
            }
            key = r.u32()?;
            Data::LayerList { names }
        }
        0x2B => {
            r.skip(3)?;
            key = r.u32()?;
            let name = r.u32()?;
            r.skip(4 + 16)?;
            next = r.u32()?;
            let first_inst = r.u32()?;
            r.skip(12)?;
            let fields = r.u32()?;
            r.skip(12)?;
            if ge(Ver::V164) {
                r.skip(4)?;
            }
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            Data::FootprintDef { name, first_inst, fields }
        }
        0x2C => {
            r.skip(1)?;
            let subtype = r.u16()?;
            key = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V172) {
                r.skip(12)?;
            }
            let text = r.u32()?;
            if !ge(Ver::V172) {
                r.skip(4)?;
            }
            let p1 = r.u32()?;
            let p2 = r.u32()?;
            let p3 = r.u32()?;
            r.skip(4)?;
            Data::Table { subtype, text, p1, p2, p3 }
        }
        0x2D => {
            let _b1 = r.u8()?;
            let side = r.u8()?;
            let _b2 = r.u8()?;
            key = r.u32()?;
            next = r.u32()?;
            let mut inst_ref = 0;
            if ge(Ver::V172) {
                r.skip(4)?;
            } else {
                inst_ref = r.u32()?;
            }
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            let rotation = r.u32()?;
            let at = pt(r)?;
            if ge(Ver::V172) {
                inst_ref = r.u32()?;
            }
            let graphic = r.u32()?;
            let first_pad = r.u32()?;
            let text = r.u32()?;
            let assembly = r.u32()?;
            let areas = r.u32()?;
            r.skip(8)?;
            Data::FootprintInst { bottom: side == 1, rotation, at, inst_ref, graphic, first_pad, text, assembly, areas }
        }
        0x2E => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(24)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x2F => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(24)?;
            Data::Other
        }
        0x30 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let (mut font, mut align, mut reversal) = (0, 0, 0);
            if ge(Ver::V172) {
                r.skip(8)?;
                font = r.u8()?;
                let _flags = r.u8()?;
                align = r.u8()?;
                reversal = r.u8()?;
                r.skip(4)?;
            }
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let sgraphic = r.u32()?;
            let mut group = 0;
            if ge(Ver::V172) {
                group = r.u32()?;
            } else {
                r.skip(4)?;
                font = r.u8()?;
                let _flags = r.u8()?;
                align = r.u8()?;
                reversal = r.u8()?;
            }
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let at = pt(r)?;
            r.skip(4)?;
            let rotation = r.u32()?;
            if !ge(Ver::V172) {
                group = r.u32()?;
            }
            Data::Text { layer, sgraphic, at, rotation, font, align, reversal, group }
        }
        0x31 => {
            r.skip(3)?;
            key = r.u32()?;
            let wrapper = r.u32()?;
            let at = pt(r)?;
            r.skip(2)?;
            let len = r.u16()? as usize;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let text = r.fixed(len, true)?;
            Data::StrGraphic { wrapper, at, text }
        }
        0x32 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let net = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let next_in_fp = r.u32()?;
            let parent_fp = r.u32()?;
            r.skip(4)?;
            let pad = r.u32()?;
            r.skip(8)?;
            let pin_number = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(8)?;
            let coords = coords4(r)?;
            Data::PlacedPad { layer, net, next_in_fp, parent_fp, pad, pin_number, coords }
        }
        0x33 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            let net = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let at = pt(r)?;
            r.skip(4)?;
            let padstack = r.u32()?;
            r.skip(16 + 16)?;
            Data::Via { layer, net, at, padstack }
        }
        0x34 => {
            r.skip(1)?;
            let layer = layer(r)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(4)?;
            let first_seg = r.u32()?;
            r.skip(8)?;
            Data::Keepout { layer, first_seg }
        }
        0x35 => {
            r.skip(3 + 120)?;
            Data::Other
        }
        0x36 => {
            r.skip(1)?;
            let code = r.u16()?;
            key = r.u32()?;
            next = r.u32()?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            let items = r.u32()? as usize;
            let count = r.u32()? as usize;
            r.skip(8)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            if items > 1_000_000 || count > items {
                return r.err("definition table counts are implausible");
            }
            let mut fonts = Vec::new();
            for i in 0..items {
                match code {
                    0x02 => {
                        r.skip(32 + 56)?;
                        if ge(Ver::V164) {
                            r.skip(12)?;
                        }
                        if ge(Ver::V172) {
                            r.skip(8)?;
                        }
                    }
                    0x03 => {
                        r.skip(if ge(Ver::V172) { 64 } else { 32 })?;
                        if ge(Ver::V174) {
                            r.skip(4)?;
                        }
                    }
                    0x05 => {
                        r.skip(28)?;
                        if ge(Ver::V175) {
                            r.skip(4)?;
                        }
                    }
                    0x06 => {
                        r.skip(8)?;
                        if !ge(Ver::V172) {
                            r.skip(200)?;
                        }
                    }
                    0x08 => {
                        r.skip(8)?;
                        let height = r.u32()?;
                        let width = r.u32()?;
                        if ge(Ver::V174) && !ge(Ver::V190) {
                            r.skip(4)?;
                        }
                        r.skip(12)?;
                        let stroke = r.u32()?;
                        if ge(Ver::V172) {
                            r.skip(32)?;
                        }
                        if i < count {
                            fonts.push(Font { height, width, stroke });
                        }
                    }
                    0x0B => r.skip(1016)?,
                    0x0C => r.skip(232)?,
                    0x0D => r.skip(200)?,
                    0x0F => r.skip(20)?,
                    0x10 => {
                        r.skip(108)?;
                        if ge(Ver::V180) {
                            r.skip(4)?;
                        }
                    }
                    0x12 => r.skip(1052)?,
                    _ => return r.err(format!("definition table of unknown kind {code:#x}")),
                }
            }
            if code == 0x08 {
                Data::Fonts(fonts)
            } else {
                Data::Other
            }
        }
        0x37 => {
            r.skip(3)?;
            key = r.u32()?;
            let _group = r.u32()?;
            next = r.u32()?;
            let _cap = r.u32()?;
            let count = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let ptrs = r.words(100)?;
            Data::PtrArray { count, ptrs }
        }
        0x38 => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(4)?;
            if !ge(Ver::V166) {
                r.skip(20)?;
            } else {
                r.skip(8)?;
            }
            r.skip(28)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x39 => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(8 + 44)?;
            Data::Other
        }
        0x3A => {
            r.skip(3)?;
            key = r.u32()?;
            next = r.u32()?;
            r.skip(4)?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            Data::Other
        }
        0x3B => {
            r.skip(3)?;
            let len = r.u32()? as usize;
            r.skip(128 + 32 + 8)?;
            if ge(Ver::V172) {
                r.skip(4)?;
            }
            r.skip(len)?;
            r.align4()?;
            Data::Other
        }
        0x3C => {
            r.skip(3)?;
            key = r.u32()?;
            if ge(Ver::V174) {
                r.skip(4)?;
            }
            let n = r.u32()? as usize;
            if n > 1_000_000 {
                return r.err("key list too long");
            }
            r.skip(4 * n)?;
            Data::Other
        }
        0x3E => {
            r.skip(3)?;
            key = r.u32()?;
            r.skip(36)?;
            Data::Other
        }
        k => return r.err(format!("block type {k:#04x} is not known")),
    };
    Ok(Some(Block { kind, offset, key, next, data }))
}

/// The decoded object stream.
#[derive(Debug, Default)]
pub struct Stream {
    pub blocks: Vec<Block>,
    pub by_key: HashMap<u32, usize>,
    /// Blocks whose type was counted but whose content the reader skipped,
    /// by type.
    pub counts: std::collections::BTreeMap<u8, usize>,
    /// Why the walk stopped early, when it did. Everything read before is kept.
    pub stopped: Option<String>,
    /// Blocks that did not start on a four-byte boundary.
    pub misaligned: usize,
    /// 0x27 blocks whose end the header did not state and the walker found.
    pub recovered_0x27: usize,
}

impl Stream {
    pub fn get(&self, key: u32) -> Option<&Block> {
        if key == 0 {
            return None;
        }
        self.by_key.get(&key).map(|&i| &self.blocks[i])
    }

    /// Follow a `next` chain from `head`, stopping at 0, at a key that is not
    /// a block, or when a key repeats.
    pub fn chain(&self, head: u32) -> Vec<&Block> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut k = head;
        while k != 0 && seen.insert(k) {
            let Some(b) = self.get(k) else { break };
            out.push(b);
            k = b.next;
        }
        out
    }
}

pub fn read_stream(data: &[u8], start: usize, header: &Header) -> Stream {
    let mut s = Stream::default();
    let mut r = R::new(data);
    r.pos = start;
    // The object stream starts at the first four-byte boundary after the
    // string table.
    let _ = r.align4();
    loop {
        if r.pos >= data.len() {
            break;
        }
        if r.pos % 4 != 0 {
            s.misaligned += 1;
        }
        let at = r.pos;
        match read_block(&mut r, header.ver, header.end_0x27 as usize) {
            Ok(Some(b)) => {
                *s.counts.entry(b.kind).or_default() += 1;
                if b.key != 0 {
                    s.by_key.insert(b.key, s.blocks.len());
                }
                s.blocks.push(b);
            }
            Ok(None) => {
                // 18.0 files may leave zero gaps between block groups: skip
                // the zeros and carry on when a block type follows.
                if header.ver >= Ver::V180 {
                    let mut p = at;
                    while p < data.len() && data[p] == 0 {
                        p += 1;
                    }
                    if p < data.len() && (1..=0x3E).contains(&data[p]) {
                        let aligned = p - p % 4;
                        if aligned > at && (1..=0x3E).contains(&data[aligned]) {
                            r.pos = aligned;
                            continue;
                        }
                    }
                }
                break;
            }
            Err(e) if e.what == NO_0X27_END => {
                let (lo, hi) = s
                    .blocks
                    .iter()
                    .filter(|b| b.key != 0)
                    .fold((u32::MAX, 0u32), |(lo, hi), b| (lo.min(b.key), hi.max(b.key)));
                match find_after_0x27(data, at + 1, header.ver, &s.by_key, lo, hi) {
                    Some(p) => {
                        *s.counts.entry(0x27).or_default() += 1;
                        s.recovered_0x27 += 1;
                        r.pos = p;
                    }
                    None if data[at + 1..].iter().filter(|&&b| b != 0).count() * 64 < data.len() - at => {
                        // Nothing but (near-)zero padding follows: the 0x27
                        // block is the last object and runs to the end.
                        *s.counts.entry(0x27).or_default() += 1;
                        s.recovered_0x27 += 1;
                        break;
                    }
                    None => {
                        s.stopped = Some(e.to_string());
                        break;
                    }
                }
            }
            Err(e) => {
                s.stopped = Some(e.to_string());
                break;
            }
        }
    }
    s
}
