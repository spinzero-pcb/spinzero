//! An OrCAD / Allegro board (`.brd`) projected into the geometry IR.
//!
//! The board database is a graph of keyed objects (`eda_parse_orcad::allegro`);
//! this module walks it into the same `ir::Geometry` the KiCad and Altium
//! paths emit, so the GPU renderer, the layer SVGs (`altium::pcb_svg`, which
//! renders from the IR alone) and every review move read an Allegro board with
//! no Allegro-specific code.
//!
//! What was checked, and against what:
//!
//! - Footprint placement (origin, rotation, side) against Allegro's own
//!   `place_txt.txt` / fabmaster exports in the corpus.
//! - Pad placement: a pad's local position taken through its footprint's
//!   rotation (X mirrored first for the bottom side) lands on the centre of
//!   the box the placed-pad object stores, on every pad of the boards tried.
//! - The meaning of the fixed subclass codes, learned by matching decoded
//!   primitives coordinate-for-coordinate against the `CLASS/SUBCLASS` names
//!   in Allegro's extract reports (`sym.txt`) shipped with 16 boards. Codes no
//!   report confirmed keep a hex name and the `user` role rather than a guess.
//!
//! Coordinates: Allegro is Y-up in mils / divisor; the IR is millimetres,
//! Y-down, so every Y is negated and every rotation sense reversed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use eda_parse_orcad::allegro::blocks::{class, FieldValue, LayerName, PadComp, Padstack};
use eda_parse_orcad::allegro::{self, Block, Data, Db, Layer, Ver};
use serde::Serialize;

use crate::design::{Classification, Component, Design, Hierarchy};
use crate::ir::{
    CompDef, GKind, Geometry, GraphicDef, LayerDef, PadDef, TextDef, Tracks, ViaDef, ZoneDef,
    GEOMETRY_SCHEMA,
};
use crate::pipeline::Msg;

fn r4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// Segment and arc block types: the links of every outline chain.
const SEG_KINDS: [u8; 4] = [0x01, 0x15, 0x16, 0x17];

/// Field codes on component and net field chains, as the corpus shows them.
mod field {
    /// Component value (`10u`, `51k`): matches `COMP_VALUE` in Allegro's
    /// fabmaster export of the same board.
    pub const VALUE: u16 = 0x02;
    /// Component description text.
    pub const DESCRIPTION: u16 = 0xC4;
    /// The physical constraint set a net uses, by string id or name.
    pub const PHYSICAL_CSET: u16 = 0x1A0;
}

/// A loaded board: the database plus the indexes every consumer needs.
pub struct Board {
    pub db: Db,
    /// Copper layer names, top first (the ETCH custom layer list).
    pub copper: Vec<String>,
    /// Placed footprints (0x2D), in stream order.
    pub footprints: Vec<Placed>,
    /// Net key -> name, empty names replaced by a stable synthetic one.
    pub net_names: HashMap<u32, String>,
}

/// One placed footprint, with everything resolved that a review reads.
#[derive(Debug, Clone)]
pub struct Placed {
    pub key: u32,
    pub refdes: String,
    /// Footprint (symbol) name, e.g. `R0805`.
    pub symbol: String,
    pub device: String,
    pub value: String,
    pub description: String,
    pub bottom: bool,
    /// Millidegrees, counter-clockwise, Allegro's own sense.
    pub rotation: u32,
    pub at: (i32, i32),
    pub first_pad: u32,
    pub text: u32,
}

pub fn load_board(path: &Path) -> Result<Board, String> {
    let db = allegro::open(path)?;
    Ok(index(db))
}

fn field_text(db: &Db, v: &FieldValue) -> Option<String> {
    match v {
        FieldValue::Text(t) => Some(t.clone()),
        FieldValue::Int(i) => db.strings.get(i).cloned(),
        _ => None,
    }
}

/// The fields on a 0x03 chain, by code. Stops at the first non-field block,
/// which is the chain's owner.
fn fields(db: &Db, head: u32) -> Vec<(u16, &FieldValue)> {
    let mut out = Vec::new();
    let mut k = head;
    let mut seen = HashSet::new();
    while let Some(b) = db.stream.get(k) {
        if !seen.insert(k) {
            break;
        }
        let Data::Field { code, value } = &b.data else { break };
        out.push((*code, value));
        k = b.next;
    }
    out
}

fn index(db: Db) -> Board {
    let copper = layer_list(&db, class::ETCH);
    // Device data: component (0x06) -> its instances (0x07).
    let mut by_inst: HashMap<u32, (String, String, String)> = HashMap::new();
    for b in &db.stream.blocks {
        let Data::Component { device_type, first_inst, fields: f, .. } = &b.data else { continue };
        let mut value = String::new();
        let mut description = String::new();
        for (code, v) in fields(&db, *f) {
            match code {
                field::VALUE => value = field_text(&db, v).unwrap_or_default(),
                field::DESCRIPTION => description = field_text(&db, v).unwrap_or_default(),
                _ => {}
            }
        }
        let mut k = *first_inst;
        let mut seen = HashSet::new();
        while let Some(i) = db.stream.get(k) {
            if i.kind != 0x07 || !seen.insert(k) {
                break;
            }
            by_inst.insert(i.key, (db.s(*device_type).to_string(), value.clone(), description.clone()));
            k = i.next;
        }
    }
    let mut footprints = Vec::new();
    for b in &db.stream.blocks {
        let Data::FootprintDef { name, first_inst, .. } = &b.data else { continue };
        let mut k = *first_inst;
        let mut seen = HashSet::new();
        while let Some(i) = db.stream.get(k) {
            if i.kind != 0x2D || !seen.insert(k) {
                break;
            }
            if let Data::FootprintInst { bottom, rotation, at, inst_ref, first_pad, text, .. } = &i.data {
                let refdes = match db.stream.get(*inst_ref).map(|x| &x.data) {
                    Some(Data::CompInst { refdes, .. }) => db.s(*refdes).to_string(),
                    _ => String::new(),
                };
                let (device, value, description) = by_inst.get(inst_ref).cloned().unwrap_or_default();
                footprints.push(Placed {
                    key: i.key,
                    refdes,
                    symbol: db.s(*name).to_string(),
                    device,
                    value,
                    description,
                    bottom: *bottom,
                    rotation: *rotation,
                    at: *at,
                    first_pad: *first_pad,
                    text: *text,
                });
            }
            k = i.next;
        }
    }
    let mut net_names = HashMap::new();
    for b in &db.stream.blocks {
        if let Data::Net { name, .. } = &b.data {
            let n = db.s(*name);
            // An unnamed net is still a net; naming it keeps it from
            // collapsing into "no net" and so merging with every other one.
            let n = if n.is_empty() { format!("UNNAMED_{:X}", b.key) } else { n.to_string() };
            net_names.insert(b.key, n);
        }
    }
    Board { db, copper, footprints, net_names }
}

/// Names of a class's custom subclasses (the low subclass codes), from the
/// header's per-class layer list.
fn layer_list(db: &Db, class: u8) -> Vec<String> {
    let Some(&k) = db.header.layer_lists.get(class as usize) else { return Vec::new() };
    match db.stream.get(k).map(|b| &b.data) {
        Some(Data::LayerList { names }) => names
            .iter()
            .map(|n| match n {
                LayerName::Text(t) => t.clone(),
                LayerName::Id(i) => db.s(*i).to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn class_name(c: u8) -> &'static str {
    match c {
        0x01 => "BOARD GEOMETRY",
        0x02 => "COMPONENT VALUE",
        0x03 => "DEVICE TYPE",
        0x04 => "DRAWING FORMAT",
        0x05 => "DRC ERROR",
        0x06 => "ETCH",
        0x07 => "MANUFACTURING",
        0x08 => "ANALYSIS",
        0x09 => "PACKAGE GEOMETRY",
        0x0A => "PACKAGE KEEPIN",
        0x0B => "PACKAGE KEEPOUT",
        0x0C => "PIN",
        0x0D => "REF DES",
        0x0E => "ROUTE KEEPIN",
        0x0F => "ROUTE KEEPOUT",
        0x10 => "TOLERANCE",
        0x11 => "USER PART NUMBER",
        0x12 => "VIA CLASS",
        0x13 => "VIA KEEPOUT",
        0x14 => "ANTI ETCH",
        0x15 => "BOUNDARY",
        0x16 => "CONSTRAINT REGION",
        _ => "CLASS",
    }
}

/// Names of the fixed (high) subclass codes that Allegro's extract reports
/// confirmed. The text classes share one table; the two geometry classes
/// each have their own.
fn fixed_subclass(c: u8, s: u8) -> Option<&'static str> {
    let text_class = matches!(c, 0x02 | 0x03 | 0x0D | 0x10 | 0x11);
    Some(match (c, s) {
        (0x01, 0xF0) => "SILKSCREEN_BOTTOM",
        (0x01, 0xF1) => "SILKSCREEN_TOP",
        (0x01, 0xF9) => "DIMENSION",
        (0x01, 0xFD) | (0x01, 0xEA) => "OUTLINE",
        (0x04, 0xFD) => "OUTLINE",
        (0x07, 0xF1) => "NO_PROBE_BOTTOM",
        (0x07, 0xF2) => "NO_PROBE_TOP",
        (0x09, 0xEC) => "PASTEMASK_BOTTOM",
        (0x09, 0xED) => "PASTEMASK_TOP",
        (0x09, 0xEE) => "DFA_BOUND_BOTTOM",
        (0x09, 0xEF) => "DFA_BOUND_TOP",
        (0x09, 0xF2) => "DISPLAY_TOP",
        (0x09, 0xF4) => "SOLDERMASK_TOP",
        (0x09, 0xF5) => "BODY_CENTER",
        (0x09, 0xF6) => "SILKSCREEN_BOTTOM",
        (0x09, 0xF7) => "SILKSCREEN_TOP",
        (0x09, 0xF9) => "PIN_NUMBER",
        (0x09, 0xFA) => "PLACE_BOUND_BOTTOM",
        (0x09, 0xFB) => "PLACE_BOUND_TOP",
        (0x09, 0xFC) => "ASSEMBLY_BOTTOM",
        (0x09, 0xFD) => "ASSEMBLY_TOP",
        (_, 0xF8) if text_class => "DISPLAY_BOTTOM",
        (_, 0xF9) if text_class => "DISPLAY_TOP",
        (_, 0xFA) if text_class => "SILKSCREEN_BOTTOM",
        (_, 0xFB) if text_class => "SILKSCREEN_TOP",
        (_, 0xFC) if text_class => "ASSEMBLY_BOTTOM",
        (_, 0xFD) if text_class => "ASSEMBLY_TOP",
        (0x0B | 0x0F | 0x13, 0xFD) => "ALL",
        _ => return None,
    })
}

/// Where a (class, subclass) pair lands in the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Slot {
    Copper(u8),
    Silk(bool),
    Mask(bool),
    Paste(bool),
    Edge,
    /// Any other layer, kept under its Allegro name with the `user` role.
    Other(u8, u8),
}

fn slot_of(l: Layer, n_copper: usize) -> Slot {
    let top = |s: u8| s == 0xF1 || s == 0xF7 || s == 0xFB;
    match (l.class, l.sub) {
        (class::ETCH, s) if (s as usize) < n_copper.max(1) => Slot::Copper(s),
        (class::BOARD_GEOMETRY, 0xEA | 0xFD) => Slot::Edge,
        (class::BOARD_GEOMETRY, s @ (0xF0 | 0xF1)) => Slot::Silk(top(s)),
        (class::PACKAGE_GEOMETRY, s @ (0xF6 | 0xF7)) => Slot::Silk(top(s)),
        // Only reference designators: value, device type, tolerance and part
        // number texts have silkscreen subclasses too, but whether a board's
        // silkscreen film includes them is film configuration, and by
        // convention it does not. They keep their own named layers.
        (class::REF_DES, s @ (0xFA | 0xFB)) => Slot::Silk(top(s)),
        (class::PACKAGE_GEOMETRY, 0xF4) => Slot::Mask(true),
        (class::PACKAGE_GEOMETRY, 0xED) => Slot::Paste(true),
        (class::PACKAGE_GEOMETRY, 0xEC) => Slot::Paste(false),
        (c, s) => Slot::Other(c, s),
    }
}

/// The IR layer table, built lazily as primitives claim layers.
struct Layers<'a> {
    b: &'a Board,
    defs: Vec<LayerDef>,
    idx: HashMap<Slot, u16>,
}

impl<'a> Layers<'a> {
    fn new(b: &'a Board) -> Self {
        let mut l = Layers { b, defs: Vec::new(), idx: HashMap::new() };
        // Every copper layer exists even when empty: a reviewer expects an
        // inner plane to be selectable. The board edge likewise.
        for i in 0..b.copper.len() {
            l.get(Slot::Copper(i as u8));
        }
        l
    }

    fn n_copper(&self) -> usize {
        self.b.copper.len()
    }

    fn get(&mut self, s: Slot) -> u16 {
        if let Some(&i) = self.idx.get(&s) {
            return i;
        }
        let n = self.n_copper();
        let (name, role, side): (String, &'static str, Option<&'static str>) = match s {
            Slot::Copper(i) => {
                let side = if i == 0 {
                    Some("front")
                } else if i as usize + 1 == n {
                    Some("back")
                } else {
                    Some("inner")
                };
                let name = self.b.copper.get(i as usize).cloned().unwrap_or_else(|| format!("ETCH {i}"));
                (name, "copper", side)
            }
            Slot::Silk(t) => (
                if t { "SILKSCREEN_TOP" } else { "SILKSCREEN_BOTTOM" }.into(),
                "silkscreen",
                Some(if t { "front" } else { "back" }),
            ),
            Slot::Mask(t) => (
                if t { "SOLDERMASK_TOP" } else { "SOLDERMASK_BOTTOM" }.into(),
                "mask",
                Some(if t { "front" } else { "back" }),
            ),
            Slot::Paste(t) => (
                if t { "PASTEMASK_TOP" } else { "PASTEMASK_BOTTOM" }.into(),
                "paste",
                Some(if t { "front" } else { "back" }),
            ),
            Slot::Edge => ("BOARD GEOMETRY/OUTLINE".into(), "edge", None),
            Slot::Other(c, sub) => (other_name(&self.b.db, c, sub), "user", None),
        };
        let i = self.defs.len() as u16;
        self.defs.push(LayerDef { name, role, side, ord: 0, color: None });
        self.idx.insert(s, i);
        i
    }

    fn of(&mut self, l: Layer) -> u16 {
        let s = slot_of(l, self.n_copper());
        self.get(s)
    }

    /// Order the table front to back (silk, paste, mask, copper top..bottom,
    /// then the back technical layers, the edge, and the user layers) and
    /// return the old-index -> new-index map.
    fn finish(self) -> (Vec<LayerDef>, Vec<u16>) {
        let mut slots: Vec<(Slot, u16)> = self.idx.into_iter().collect();
        let rank = |s: &Slot| -> (u8, u16, u8) {
            match *s {
                Slot::Silk(true) => (0, 0, 0),
                Slot::Paste(true) => (1, 0, 0),
                Slot::Mask(true) => (2, 0, 0),
                Slot::Copper(i) => (3, i as u16, 0),
                Slot::Mask(false) => (4, 0, 0),
                Slot::Paste(false) => (5, 0, 0),
                Slot::Silk(false) => (6, 0, 0),
                Slot::Edge => (7, 0, 0),
                Slot::Other(c, sub) => (8, c as u16, sub),
            }
        };
        slots.sort_by_key(|(s, _)| rank(s));
        let mut remap = vec![0u16; self.defs.len()];
        let mut defs: Vec<Option<LayerDef>> = self.defs.into_iter().map(Some).collect();
        let mut out = Vec::with_capacity(defs.len());
        for (ord, (_, old)) in slots.iter().enumerate() {
            remap[*old as usize] = ord as u16;
            let mut d = defs[*old as usize].take().expect("each layer once");
            d.ord = ord as i64;
            out.push(d);
        }
        (out, remap)
    }
}

fn other_name(db: &Db, c: u8, s: u8) -> String {
    let sub = if s < 0xE0 {
        layer_list(db, c).get(s as usize).cloned().unwrap_or_else(|| format!("{s:#04X}"))
    } else {
        fixed_subclass(c, s).map(str::to_string).unwrap_or_else(|| format!("{s:#04X}"))
    };
    format!("{}/{}", class_name(c), sub)
}

/// Board-unit to IR coordinate transform.
#[derive(Clone, Copy)]
struct Xf {
    s: f64,
}

impl Xf {
    fn len(&self, v: f64) -> f64 {
        r4(v * self.s)
    }
    fn pt(&self, p: (f64, f64)) -> (f64, f64) {
        (r4(p.0 * self.s), r4(-p.1 * self.s))
    }
    fn ipt(&self, p: (i32, i32)) -> (f64, f64) {
        self.pt((p.0 as f64, p.1 as f64))
    }
}

/// A point on an Allegro arc halfway along its sweep, in board units.
fn arc_mid(start: (i32, i32), end: (i32, i32), c: (f64, f64), r: f64, cw: bool) -> (f64, f64) {
    let a0 = (start.1 as f64 - c.1).atan2(start.0 as f64 - c.0);
    let a1 = (end.1 as f64 - c.1).atan2(end.0 as f64 - c.0);
    let tau = std::f64::consts::TAU;
    let mut sweep = if cw { a0 - a1 } else { a1 - a0 };
    sweep = sweep.rem_euclid(tau);
    if sweep < 1e-9 {
        sweep = tau; // start == end: a full circle
    }
    let m = if cw { a0 - sweep / 2.0 } else { a0 + sweep / 2.0 };
    (c.0 + r * m.cos(), c.1 + r * m.sin())
}

/// One link of an outline chain.
enum Link {
    Seg { a: (i32, i32), b: (i32, i32), width: u32 },
    Arc { a: (i32, i32), b: (i32, i32), c: (f64, f64), r: f64, cw: bool, width: u32 },
}

fn links(db: &Db, head: u32) -> Vec<Link> {
    let mut out = Vec::new();
    let mut k = head;
    let mut seen = HashSet::new();
    while let Some(b) = db.stream.get(k) {
        if !SEG_KINDS.contains(&b.kind) || !seen.insert(k) {
            break;
        }
        match &b.data {
            Data::Seg { a, b: e, width, .. } => out.push(Link::Seg { a: *a, b: *e, width: *width }),
            Data::Arc { start, end, center, radius, clockwise, width, .. } => out.push(Link::Arc {
                a: *start,
                b: *end,
                c: *center,
                r: *radius,
                cw: *clockwise,
                width: *width,
            }),
            _ => break,
        }
        k = b.next;
    }
    out
}

/// A closed outline as a polygon ring in board units, arcs flattened.
fn ring(ls: &[Link]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let push = |pts: &mut Vec<(f64, f64)>, p: (f64, f64)| {
        if pts.last().map(|q| (q.0 - p.0).abs() > 1e-6 || (q.1 - p.1).abs() > 1e-6).unwrap_or(true) {
            pts.push(p);
        }
    };
    for l in ls {
        match *l {
            Link::Seg { a, b, .. } => {
                push(&mut pts, (a.0 as f64, a.1 as f64));
                push(&mut pts, (b.0 as f64, b.1 as f64));
            }
            Link::Arc { a, b, c, r, cw, .. } => {
                let a0 = (a.1 as f64 - c.1).atan2(a.0 as f64 - c.0);
                let a1 = (b.1 as f64 - c.1).atan2(b.0 as f64 - c.0);
                let tau = std::f64::consts::TAU;
                let mut sweep = if cw { a0 - a1 } else { a1 - a0 }.rem_euclid(tau);
                if sweep < 1e-9 {
                    sweep = tau;
                }
                // About 5 degrees a step: smooth at review zoom, small in JSON.
                let n = ((sweep / 0.087).ceil() as usize).clamp(2, 72);
                push(&mut pts, (a.0 as f64, a.1 as f64));
                for i in 1..n {
                    let t = sweep * i as f64 / n as f64;
                    let ang = if cw { a0 - t } else { a0 + t };
                    push(&mut pts, (c.0 + r * ang.cos(), c.1 + r * ang.sin()));
                }
                push(&mut pts, (b.0 as f64, b.1 as f64));
            }
        }
    }
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

fn signed_area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>() / 2.0
}

/// Fold holes into an outer ring with zero-width bridges, so a fill with
/// voids is still one ring the IR can carry. Holes run against the outer
/// ring's winding, so either fill rule leaves them empty. Each hole bridges
/// to the nearest vertex of the ORIGINAL outer ring and the result is built
/// once: a plane with thousands of voids stays linear in its size.
fn bridge(outer: Vec<(f64, f64)>, holes: Vec<Vec<(f64, f64)>>) -> Vec<(f64, f64)> {
    if outer.is_empty() {
        return outer;
    }
    let outer_ccw = signed_area(&outer) > 0.0;
    let mut at: Vec<Vec<Vec<(f64, f64)>>> = vec![Vec::new(); outer.len()];
    for mut h in holes {
        if h.len() < 3 {
            continue;
        }
        if (signed_area(&h) > 0.0) == outer_ccw {
            h.reverse();
        }
        let p = h[0];
        let mut best = (f64::MAX, 0usize);
        for (i, q) in outer.iter().enumerate() {
            let d = (q.0 - p.0) * (q.0 - p.0) + (q.1 - p.1) * (q.1 - p.1);
            if d < best.0 {
                best = (d, i);
            }
        }
        at[best.1].push(h);
    }
    let mut out = Vec::with_capacity(outer.len() * 2);
    for (i, q) in outer.iter().enumerate() {
        out.push(*q);
        for h in &at[i] {
            out.extend_from_slice(h);
            out.push(h[0]);
            out.push(*q);
        }
    }
    out
}

/// The IR pad shape code for a padstack component, with the corner ratio.
/// Shapes the IR cannot draw become their nearest drawable kin; the count is
/// reported.
fn pad_shape(c: &PadComp, approx: &mut usize) -> (u8, f64) {
    let short = c.w.min(c.h).max(1) as f64;
    match c.kind {
        0x02 => (if c.w == c.h { 0 } else { 3 }, 0.0),
        0x05 | 0x06 => (1, 0.0),
        0x0B | 0x0C => (3, 0.0),
        0x1B => (2, (c.corner as f64 / short).clamp(0.0, 0.5)),
        // Octagon and chamfered rectangle: a rounded rectangle of about the
        // chamfer's size is the closest drawable shape.
        0x03 => {
            *approx += 1;
            (2, 0.29)
        }
        0x1C => {
            *approx += 1;
            (2, (c.corner as f64 / short).clamp(0.0, 0.5))
        }
        0x16 => (5, 0.0),
        _ => {
            *approx += 1;
            (1, 0.0)
        }
    }
}

/// The copper pad of a padstack on the board layer `cu`, accounting for the
/// footprint side (a bottom footprint's padstack layer 0 is the bottom).
fn pad_on_board(ps: &Padstack, cu: usize, n: usize, bottom: bool) -> Option<&PadComp> {
    let cu = if bottom { n.checked_sub(1)?.checked_sub(cu)? } else { cu };
    let i = cu.checked_sub(ps.start_layer as usize)?;
    if i >= ps.layer_count as usize {
        return None;
    }
    ps.pad_on(i)
}

/// Board copper layers a padstack spans, top first.
fn span(ps: &Padstack, n: usize, bottom: bool) -> Vec<usize> {
    let start = ps.start_layer as usize;
    let count = (ps.layer_count as usize).max(1);
    let mut v: Vec<usize> = (start..(start + count).min(n.max(1))).collect();
    if bottom {
        v = v.into_iter().map(|i| n - 1 - i).collect();
        v.sort();
    }
    v
}

/// Technical-layer slots of a padstack: top/bottom solder mask and paste.
/// 17.2+ keeps 21 fixed slots with the masks at 14/15 and the pastes at
/// 16/17; earlier formats keep 10 or 11 with the top mask at 1 and the top
/// paste at 6, and no bottom slots this reader can name.
fn tech(ps: &Padstack, ver: Ver) -> [Option<&PadComp>; 4] {
    let g = |i: usize| ps.comps.get(i).filter(|c| c.kind != 0);
    if ver >= Ver::V172 {
        [g(14), g(15), g(16), g(17)]
    } else {
        [g(1), None, g(6), None]
    }
}

/// Per-board diagnostics the bundle reports.
#[derive(Debug, Default, Clone, Serialize)]
pub struct BoardStats {
    pub format: String,
    /// Blocks read, and the reason the walk stopped early (if it did).
    pub blocks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// Footprint-library templates skipped (objects owned by a definition,
    /// not placed on the board).
    pub template_objects: usize,
    /// Pads drawn with an approximated shape.
    pub pads_approximated: usize,
    /// Primitives on a layer whose meaning no extract report confirmed.
    pub unnamed_layers: Vec<String>,
}

/// The geometry document plus what building it could not resolve.
pub struct Built {
    pub geometry: Geometry,
    pub stats: BoardStats,
}

/// Owner type of a text block: the type of the first block its `next` chain
/// reaches that is not another text. Allegro closes every list on its owner.
fn chain_owner<'a>(db: &'a Db, b: &Block) -> Option<&'a Block> {
    let mut k = b.next;
    let mut seen = HashSet::new();
    while let Some(x) = db.stream.get(k) {
        if x.kind != b.kind {
            return Some(x);
        }
        if !seen.insert(k) {
            return None;
        }
        k = x.next;
    }
    None
}

pub fn build(b: &Board, source: &str) -> Built {
    let db = &b.db;
    let xf = Xf { s: db.header.scale() };
    let n = b.copper.len().max(1);
    let mut layers = Layers::new(b);
    let mut stats = BoardStats {
        format: format!("Allegro {}", db.header.ver.label()),
        blocks: db.stream.blocks.len(),
        stopped: db.stream.stopped.clone(),
        ..Default::default()
    };

    // ---- nets ---------------------------------------------------------------
    let mut nets: Vec<String> = vec![String::new()];
    let mut net_idx: HashMap<u32, u32> = HashMap::new();
    for blk in &db.stream.blocks {
        if blk.kind == 0x1B {
            net_idx.insert(blk.key, nets.len() as u32);
            nets.push(b.net_names.get(&blk.key).cloned().unwrap_or_default());
        }
    }
    // A primitive names its net through an assignment (0x04) or directly.
    let net_of = |k: u32| -> u32 {
        match db.stream.get(k) {
            Some(x) if x.kind == 0x1B => net_idx.get(&x.key).copied().unwrap_or(0),
            Some(Block { data: Data::NetAssign { net, .. }, .. }) => net_idx.get(net).copied().unwrap_or(0),
            _ => 0,
        }
    };
    let is_template = |k: u32| db.stream.get(k).map(|x| x.kind == 0x2B).unwrap_or(false);

    // ---- components ---------------------------------------------------------
    let comp_idx: HashMap<u32, usize> = b.footprints.iter().enumerate().map(|(i, f)| (f.key, i)).collect();
    let front = layers.get(Slot::Copper(0));
    let back = layers.get(Slot::Copper((n - 1) as u8));
    let mut components: Vec<CompDef> = b
        .footprints
        .iter()
        .map(|f| {
            let (x, y) = xf.ipt(f.at);
            CompDef {
                reference: f.refdes.clone(),
                fp: f.symbol.clone(),
                layer: if f.bottom { back as i32 } else { front as i32 },
                x,
                y,
                angle: r4(-(f.rotation as f64) / 1000.0),
                dnp: false,
                bbox: None,
                uuid: String::new(),
            }
        })
        .collect();

    let padstack = |k: u32| match db.stream.get(k).map(|x| &x.data) {
        Some(Data::Padstack(p)) => Some(p.as_ref()),
        _ => None,
    };

    // ---- pads -----------------------------------------------------------------
    let mut pads: Vec<PadDef> = Vec::new();
    let mut comp_box: HashMap<usize, [f64; 4]> = HashMap::new();
    for blk in &db.stream.blocks {
        let Data::PlacedPad { net, parent_fp, pad, .. } = &blk.data else { continue };
        let Some(&ci) = comp_idx.get(parent_fp) else { continue };
        let f = &b.footprints[ci];
        let Some(Data::Pad { name, at: lp, padstack: psk, rotation: pr, .. }) = db.stream.get(*pad).map(|x| &x.data) else {
            continue;
        };
        let Some(ps) = padstack(*psk) else { continue };
        let a = (f.rotation as f64 / 1000.0).to_radians();
        let place = |p: (f64, f64)| {
            let lx = if f.bottom { -p.0 } else { p.0 };
            (f.at.0 as f64 + lx * a.cos() - p.1 * a.sin(), f.at.1 as f64 + lx * a.sin() + p.1 * a.cos())
        };
        let drill = ps.drill_w.max(ps.drill_h) > 0;
        let cu_span: Vec<usize> = if drill {
            (0..n).collect()
        } else {
            span(ps, n, f.bottom)
        };
        // The pad on its first copper layer gives the drawn shape; through
        // pads are the same on every layer far more often than not.
        let Some((cu0, comp)) = cu_span.iter().find_map(|&cu| pad_on_board(ps, cu, n, f.bottom).map(|c| (cu, c))) else {
            continue;
        };
        let _ = cu0;
        let (shape, rratio) = pad_shape(comp, &mut stats.pads_approximated);
        let pad_rot = *pr as f64 / 1000.0;
        let total = f.rotation as f64 / 1000.0 + if f.bottom { -pad_rot } else { pad_rot };
        // The component's shape offset turns with the pad.
        let pr_rad = pad_rot.to_radians();
        let (ox, oy) = (comp.off_x as f64, comp.off_y as f64);
        let off = (ox * pr_rad.cos() - oy * pr_rad.sin(), ox * pr_rad.sin() + oy * pr_rad.cos());
        let centre = place((lp.0 as f64 + off.0, lp.1 as f64 + off.1));
        let (x, y) = xf.pt(centre);
        let mut on: Vec<u16> = cu_span
            .iter()
            .filter(|&&cu| pad_on_board(ps, cu, n, f.bottom).is_some())
            .map(|&cu| layers.get(Slot::Copper(cu as u8)))
            .collect();
        let [tsm, bsm, tpm, bpm] = tech(ps, db.header.ver);
        let (tsm, bsm, tpm, bpm) = if f.bottom { (bsm, tsm, bpm, tpm) } else { (tsm, bsm, tpm, bpm) };
        let top_cu = cu_span.contains(&0);
        let bot_cu = cu_span.contains(&(n - 1));
        let mut mask = None;
        // A through pad on a pre-17.2 board names only its top mask; Allegro
        // opens both sides of a plated hole, so the bottom follows the top.
        let bsm = bsm.or(if drill && db.header.ver < Ver::V172 { tsm } else { None });
        if let Some(m) = tsm.filter(|_| top_cu) {
            on.push(layers.get(Slot::Mask(true)));
            mask = Some(m);
        }
        if let Some(m) = bsm.filter(|_| bot_cu) {
            on.push(layers.get(Slot::Mask(false)));
            mask = mask.or(Some(m));
        }
        if tpm.is_some() && top_cu {
            on.push(layers.get(Slot::Paste(true)));
        }
        if bpm.is_some() && bot_cu {
            on.push(layers.get(Slot::Paste(false)));
        }
        let expansion = mask.map(|m| xf.len((m.w.min(m.h) - comp.w.min(comp.h)) as f64 / 2.0)).filter(|e| *e != 0.0);
        let (mut dw, mut dh) = (ps.drill_w as f64, ps.drill_h as f64);
        // A slot is stored long side first; turn it to follow the pad.
        if dh > 0.0 && (comp.h > comp.w) != (dh > dw) {
            std::mem::swap(&mut dw, &mut dh);
        }
        let (w, h) = (xf.len(comp.w as f64), xf.len(comp.h as f64));
        let e = comp_box.entry(ci).or_insert([f64::MAX, f64::MAX, f64::MIN, f64::MIN]);
        let rr = w.max(h) / 2.0;
        e[0] = e[0].min(x - rr);
        e[1] = e[1].min(y - rr);
        e[2] = e[2].max(x + rr);
        e[3] = e[3].max(y + rr);
        pads.push(PadDef {
            x,
            y,
            w,
            h,
            angle: r4(-total),
            shape,
            rratio,
            drill: if drill { xf.len(dw) } else { 0.0 },
            drillh: if dh > 0.0 { xf.len(dh) } else { 0.0 },
            net: net_of(*net),
            comp: ci as i32,
            num: db.s(*name).to_string(),
            layers: on,
            mask: expansion,
            paste: None,
            npth: drill && !ps.plated,
        });
    }

    // ---- vias ---------------------------------------------------------------
    let mut vias: Vec<ViaDef> = Vec::new();
    for blk in &db.stream.blocks {
        let Data::Via { net, at, padstack: psk, .. } = &blk.data else { continue };
        if is_template(*net) {
            stats.template_objects += 1;
            continue;
        }
        let Some(ps) = padstack(*psk) else { continue };
        let sp = span(ps, n, false);
        let sp = if sp.len() <= 1 { (0..n).collect() } else { sp };
        let size = sp.iter().find_map(|&cu| pad_on_board(ps, cu, n, false)).map(|c| c.w.max(c.h)).unwrap_or(0);
        let (x, y) = xf.ipt(*at);
        vias.push(ViaDef {
            x,
            y,
            size: xf.len(size as f64),
            drill: xf.len(ps.drill_w as f64),
            net: net_of(*net),
            layers: sp.iter().map(|&cu| layers.get(Slot::Copper(cu as u8))).collect(),
            ring: None,
        });
    }

    // ---- tracks, graphics, shapes, zones -------------------------------------
    let mut tracks = Tracks::default();
    let mut graphics: Vec<GraphicDef> = Vec::new();
    let mut zones: Vec<ZoneDef> = Vec::new();
    let mut unnamed: BTreeMap<String, usize> = BTreeMap::new();
    let mut note_layer = |l: Layer, layers: &Layers| {
        let s = slot_of(l, layers.n_copper());
        if let Slot::Other(c, sub) = s {
            if sub >= 0xE0 && fixed_subclass(c, sub).is_none() {
                *unnamed.entry(other_name(db, c, sub)).or_default() += 1;
            }
        }
    };
    let comp_of = |k: u32| comp_idx.get(&k).map(|&i| i as u32);
    let emit_links = |ls: &[Link], layer: u16, comp: Option<u32>, graphics: &mut Vec<GraphicDef>| {
        for l in ls {
            match *l {
                Link::Seg { a, b: e, width } => {
                    let (x1, y1) = xf.ipt(a);
                    let (x2, y2) = xf.ipt(e);
                    graphics.push(GraphicDef { layer, width: xf.len(width as f64), kind: GKind::Seg, data: vec![x1, y1, x2, y2], filled: false, comp });
                }
                Link::Arc { a, b: e, c, r, cw, width } => {
                    if a == e {
                        let (cx, cy) = xf.pt(c);
                        graphics.push(GraphicDef { layer, width: xf.len(width as f64), kind: GKind::Circle, data: vec![cx, cy, xf.len(r)], filled: false, comp });
                    } else {
                        let (sx, sy) = xf.ipt(a);
                        let (mx, my) = xf.pt(arc_mid(a, e, c, r, cw));
                        let (ex, ey) = xf.ipt(e);
                        graphics.push(GraphicDef { layer, width: xf.len(width as f64), kind: GKind::Arc, data: vec![sx, sy, mx, my, ex, ey], filled: false, comp });
                    }
                }
            }
        }
    };
    let poly = |ring: &[(f64, f64)]| -> Vec<f64> {
        ring.iter().flat_map(|&p| {
            let (x, y) = xf.pt(p);
            [x, y]
        }).collect()
    };

    for blk in &db.stream.blocks {
        match &blk.data {
            Data::Track { layer, net_assign, first_seg } => {
                if is_template(*net_assign) {
                    stats.template_objects += 1;
                    continue;
                }
                let li = layers.of(*layer);
                let net = net_of(*net_assign);
                for l in links(db, *first_seg) {
                    match l {
                        Link::Seg { a, b: e, width } => {
                            let (x1, y1) = xf.ipt(a);
                            let (x2, y2) = xf.ipt(e);
                            tracks.seg.xy.extend([x1, y1, x2, y2]);
                            tracks.seg.w.push(xf.len(width as f64));
                            tracks.seg.layer.push(li);
                            tracks.seg.net.push(net);
                        }
                        Link::Arc { a, b: e, c, r, cw, width } => {
                            let (sx, sy) = xf.ipt(a);
                            let (mx, my) = xf.pt(arc_mid(a, e, c, r, cw));
                            let (ex, ey) = xf.ipt(e);
                            tracks.arc.xy.extend([sx, sy, mx, my, ex, ey]);
                            tracks.arc.w.push(xf.len(width as f64));
                            tracks.arc.layer.push(li);
                            tracks.arc.net.push(net);
                        }
                    }
                }
            }
            Data::Graphic { layer, parent, first_seg } => {
                if is_template(*parent) {
                    stats.template_objects += 1;
                    continue;
                }
                note_layer(*layer, &layers);
                let li = layers.of(*layer);
                emit_links(&links(db, *first_seg), li, comp_of(*parent), &mut graphics);
            }
            Data::Rect { layer, parent, coords, rotation } => {
                if is_template(*parent) {
                    stats.template_objects += 1;
                    continue;
                }
                // Corners about the rectangle's centre, turned by its rotation.
                let (cx, cy) = ((coords[0] as f64 + coords[2] as f64) / 2.0, (coords[1] as f64 + coords[3] as f64) / 2.0);
                let (hw, hh) = ((coords[2] - coords[0]) as f64 / 2.0, (coords[3] - coords[1]) as f64 / 2.0);
                let a = (*rotation as f64 / 1000.0).to_radians();
                let corners: Vec<(f64, f64)> = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)]
                    .iter()
                    .map(|&(dx, dy)| (cx + dx * a.cos() - dy * a.sin(), cy + dx * a.sin() + dy * a.cos()))
                    .collect();
                // 0x0E is a filled rectangle (an area: keepouts, place and
                // DFA bounds, paste), 0x24 a drawn one (assembly outlines,
                // keepins, the outline) — by what each carries across the corpus.
                let area = blk.kind == 0x0E;
                match layer.class {
                    0x0F | 0x13 | 0x0B if area => {
                        let cus: Vec<usize> = if (layer.sub as usize) < n { vec![layer.sub as usize] } else { (0..n).collect() };
                        for cu in cus {
                            let li = layers.get(Slot::Copper(cu as u8));
                            zones.push(ZoneDef { layer: li, net: 0, filled: false, keepout: true, pts: poly(&corners) });
                        }
                    }
                    class::ETCH if area => {
                        let li = layers.of(*layer);
                        zones.push(ZoneDef { layer: li, net: 0, filled: true, keepout: false, pts: poly(&corners) });
                    }
                    _ => {
                        note_layer(*layer, &layers);
                        let li = layers.of(*layer);
                        graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Poly, data: poly(&corners), filled: area, comp: comp_of(*parent) });
                    }
                }
            }
            Data::DrillMark { layer, shape, at, size, .. } => {
                note_layer(*layer, &layers);
                let li = layers.of(*layer);
                let (cx, cy) = xf.ipt(*at);
                let (w, h) = (xf.len(size.0 as f64), xf.len(size.1 as f64));
                match shape {
                    0x05 | 0x06 => {
                        let d = vec![cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0, cx - w / 2.0, cy + h / 2.0];
                        graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Poly, data: d, filled: false, comp: None });
                    }
                    0x04 => {
                        graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Seg, data: vec![cx - w / 2.0, cy, cx + w / 2.0, cy], filled: false, comp: None });
                        graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Seg, data: vec![cx, cy - h / 2.0, cx, cy + h / 2.0], filled: false, comp: None });
                    }
                    // Circles, and the polygons a drill chart rarely uses,
                    // drawn as their circumscribed circle.
                    _ => graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Circle, data: vec![cx, cy, r4(w.max(h) / 2.0)], filled: false, comp: None }),
                }
            }
            Data::Shape { layer, ptr1, first_seg, first_keepout, .. } => {
                if is_template(*ptr1) {
                    stats.template_objects += 1;
                    continue;
                }
                let outer = ring(&links(db, *first_seg));
                if outer.len() < 3 {
                    continue;
                }
                match layer.class {
                    class::ETCH => {
                        // Voids are keepout (0x34) outlines chained from the shape.
                        let mut holes = Vec::new();
                        let mut k = *first_keepout;
                        let mut seen = HashSet::new();
                        while let Some(x) = db.stream.get(k) {
                            let Data::Keepout { first_seg, .. } = &x.data else { break };
                            if !seen.insert(k) {
                                break;
                            }
                            holes.push(ring(&links(db, *first_seg)));
                            k = x.next;
                        }
                        let li = layers.of(*layer);
                        zones.push(ZoneDef { layer: li, net: net_of(*ptr1), filled: true, keepout: false, pts: poly(&bridge(outer, holes)) });
                    }
                    0x15 => {
                        // A BOUNDARY shape is the zone the designer drew; its
                        // fill is the ETCH shape above. Its net hangs off a
                        // table -> pointer array -> net chain.
                        let li = layers.get(Slot::Copper(layer.sub.min((n - 1) as u8)));
                        zones.push(ZoneDef { layer: li, net: boundary_net(db, blk, &net_idx), filled: false, keepout: false, pts: poly(&outer) });
                    }
                    0x0F | 0x13 | 0x0B => {
                        // Keepouts: per copper layer, or every layer for ALL.
                        let cus: Vec<usize> = if (layer.sub as usize) < n { vec![layer.sub as usize] } else { (0..n).collect() };
                        for cu in cus {
                            let li = layers.get(Slot::Copper(cu as u8));
                            zones.push(ZoneDef { layer: li, net: 0, filled: false, keepout: true, pts: poly(&outer) });
                        }
                    }
                    _ => {
                        note_layer(*layer, &layers);
                        let li = layers.of(*layer);
                        let slot = slot_of(*layer, n);
                        let filled = matches!(slot, Slot::Silk(_));
                        graphics.push(GraphicDef { layer: li, width: 0.0, kind: GKind::Poly, data: poly(&outer), filled, comp: comp_of(*ptr1) });
                    }
                }
            }
            _ => {}
        }
    }

    // ---- texts ----------------------------------------------------------------
    let fonts: Vec<allegro::blocks::Font> = db
        .stream
        .blocks
        .iter()
        .find_map(|x| match &x.data {
            Data::Fonts(f) => Some(f.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut fp_text: HashMap<u32, usize> = HashMap::new();
    for (i, f) in b.footprints.iter().enumerate() {
        let mut k = f.text;
        let mut seen = HashSet::new();
        while let Some(x) = db.stream.get(k) {
            if x.kind != 0x30 || !seen.insert(k) {
                break;
            }
            fp_text.insert(x.key, i);
            k = x.next;
        }
    }
    let mut texts: Vec<TextDef> = Vec::new();
    for blk in &db.stream.blocks {
        let Data::Text { layer, sgraphic, at, rotation, font, align, reversal, .. } = &blk.data else { continue };
        let comp = fp_text.get(&blk.key).copied();
        if comp.is_none() {
            // A text not on a placed footprint's chain is board text unless
            // its chain closes on a footprint definition (a library template).
            if let Some(o) = chain_owner(db, blk) {
                if o.kind == 0x2B {
                    stats.template_objects += 1;
                    continue;
                }
            }
        }
        let Some(Data::StrGraphic { text, .. }) = db.stream.get(*sgraphic).map(|x| &x.data) else { continue };
        if text.is_empty() {
            continue;
        }
        note_layer(*layer, &layers);
        let li = layers.of(*layer);
        let f = fonts.get((*font as usize).wrapping_sub(1)).cloned().unwrap_or_default();
        let size = if f.height > 0 { xf.len(f.height as f64) } else { 1.0 };
        let width = if f.width > 0 { xf.len(f.width as f64) } else { size };
        let (x, y) = xf.ipt(*at);
        let role = match (comp, layer.class) {
            (Some(_), class::REF_DES) => "reference",
            (Some(_), 0x02) => "value",
            (Some(_), _) => "user",
            _ => "",
        };
        texts.push(TextDef {
            layer: li,
            text: text.clone(),
            x,
            y,
            angle: r4(-(*rotation as f64) / 1000.0),
            size,
            width: (r4(width) != r4(size)).then_some(r4(width)),
            thickness: (f.stroke > 0).then(|| xf.len(f.stroke as f64)),
            // Allegro anchors text on its baseline: 1 left, 2 right, 3 centre.
            justify: [
                match align {
                    2 => 1,
                    3 => 0,
                    _ => -1,
                },
                1,
            ],
            mirror: matches!(reversal, 1 | 3),
            bold: false,
            italic: false,
            knockout: false,
            upright: false,
            font: None,
            comp: comp.map(|c| c as u32),
            role: role.to_string(),
        });
    }

    // ---- component boxes: place bound when drawn, else the pads -------------
    let mut place: HashMap<usize, [f64; 4]> = HashMap::new();
    for blk in &db.stream.blocks {
        let Data::Shape { layer, ptr1, first_seg, .. } = &blk.data else { continue };
        if layer.class != class::PACKAGE_GEOMETRY || !matches!(layer.sub, 0xFA | 0xFB) {
            continue;
        }
        let Some(&ci) = comp_idx.get(ptr1) else { continue };
        let e = place.entry(ci).or_insert([f64::MAX, f64::MAX, f64::MIN, f64::MIN]);
        for p in ring(&links(db, *first_seg)) {
            let (x, y) = xf.pt(p);
            e[0] = e[0].min(x);
            e[1] = e[1].min(y);
            e[2] = e[2].max(x);
            e[3] = e[3].max(y);
        }
    }
    for (i, c) in components.iter_mut().enumerate() {
        if let Some(e) = place.get(&i).or_else(|| comp_box.get(&i)) {
            if e[0] <= e[2] {
                c.bbox = Some([r4(e[0]), r4(e[1]), r4(e[2] - e[0]), r4(e[3] - e[1])]);
            }
        }
    }

    // ---- final layer order ----------------------------------------------------
    stats.unnamed_layers = unnamed.into_iter().map(|(k, v)| format!("{k} ({v})")).collect();
    let (layer_defs, remap) = layers.finish();
    let m = |l: u16| remap[l as usize];
    for v in tracks.seg.layer.iter_mut().chain(tracks.arc.layer.iter_mut()) {
        *v = m(*v);
    }
    for p in &mut pads {
        for l in &mut p.layers {
            *l = m(*l);
        }
    }
    for v in &mut vias {
        for l in &mut v.layers {
            *l = m(*l);
        }
    }
    for z in &mut zones {
        z.layer = m(z.layer);
    }
    for g in &mut graphics {
        g.layer = m(g.layer);
    }
    for t in &mut texts {
        t.layer = m(t.layer);
    }
    for c in &mut components {
        if c.layer >= 0 {
            c.layer = m(c.layer as u16) as i32;
        }
    }

    let bbox = extent(&layer_defs, &tracks, &pads, &vias, &graphics, &zones);
    let _ = source;
    Built {
        geometry: Geometry {
            schema: GEOMETRY_SCHEMA,
            units: "mm",
            bbox,
            page: None,
            frame: None,
            layers: layer_defs,
            nets,
            components,
            tracks,
            vias,
            pads,
            zones,
            graphics,
            texts,
        },
        stats,
    }
}

/// Net of a BOUNDARY shape: table (0x2C) -> pointer array (0x37) -> the first
/// entry that is a net.
fn boundary_net(db: &Db, shape: &Block, net_idx: &HashMap<u32, u32>) -> u32 {
    let Data::Shape { table, .. } = &shape.data else { return 0 };
    let Some(Data::Table { p1, .. }) = db.stream.get(*table).map(|x| &x.data) else { return 0 };
    let Some(Data::PtrArray { ptrs, .. }) = db.stream.get(*p1).map(|x| &x.data) else { return 0 };
    ptrs.iter().find_map(|k| net_idx.get(k).copied()).unwrap_or(0)
}

/// Content extent: the board outline when there is one (so off-board notes
/// and drawing formats do not shrink the board in the view), else everything.
fn extent(
    layers: &[LayerDef],
    tracks: &Tracks,
    pads: &[PadDef],
    vias: &[ViaDef],
    graphics: &[GraphicDef],
    zones: &[ZoneDef],
) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut add = |x: f64, y: f64| {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    };
    for c in tracks.seg.xy.chunks_exact(2).chain(tracks.arc.xy.chunks_exact(2)) {
        add(c[0], c[1]);
    }
    for p in pads {
        let r = p.w.max(p.h) / 2.0;
        add(p.x - r, p.y - r);
        add(p.x + r, p.y + r);
    }
    for v in vias {
        add(v.x - v.size / 2.0, v.y - v.size / 2.0);
        add(v.x + v.size / 2.0, v.y + v.size / 2.0);
    }
    for z in zones {
        for c in z.pts.chunks_exact(2) {
            add(c[0], c[1]);
        }
    }
    for g in graphics {
        // Drawing formats and user layers can sit far off the board; the
        // view is the board's, so only placed technical layers count.
        let role = layers.get(g.layer as usize).map(|l| l.role).unwrap_or("user");
        if role == "user" {
            continue;
        }
        match g.kind {
            GKind::Circle => {
                if let [cx, cy, r] = g.data[..] {
                    add(cx - r, cy - r);
                    add(cx + r, cy + r);
                }
            }
            _ => {
                for c in g.data.chunks_exact(2) {
                    add(c[0], c[1]);
                }
            }
        }
    }
    if x0 > x1 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    // A margin so edge strokes are not clipped.
    let m = 1.0;
    [r4(x0 - m), r4(y0 - m), r4(x1 - x0 + 2.0 * m), r4(y1 - y0 + 2.0 * m)]
}

/// Net classes from the board's constraint sets: each physical constraint
/// set a net names (field 0x1A0) is a class; nets naming none are DEFAULT's.
pub fn net_classes(b: &Board) -> Vec<(String, Vec<String>)> {
    let db = &b.db;
    let mut set_names: HashMap<u32, String> = HashMap::new();
    for blk in &db.stream.blocks {
        if let Data::ConstraintSet { name, .. } = &blk.data {
            let n = db.s(*name);
            if !n.is_empty() {
                set_names.insert(*name, n.to_string());
            }
        }
    }
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for blk in &db.stream.blocks {
        let Data::Net { fields: f, name: sid, .. } = &blk.data else { continue };
        // A net Allegro left unnamed has no identity a class rule could cite.
        if db.s(*sid).is_empty() {
            continue;
        }
        let Some(name) = b.net_names.get(&blk.key) else { continue };
        let set = fields(db, *f).into_iter().find(|(c, _)| *c == field::PHYSICAL_CSET).and_then(|(_, v)| match v {
            FieldValue::Int(i) => set_names.get(i).cloned().or_else(|| db.strings.get(i).cloned()),
            FieldValue::Text(t) => Some(t.clone()),
            _ => None,
        });
        let class = match set {
            Some(s) if !s.eq_ignore_ascii_case("DEFAULT") => s,
            _ => "Default".to_string(),
        };
        out.entry(class).or_default().push(name.clone());
        // Match groups and differential pairs are classes too, under the
        // name Allegro gives the group.
        if let Some(g) = match_group(db, blk) {
            out.entry(g).or_default().push(name.clone());
        }
    }
    out.into_iter()
        .map(|(k, mut v)| {
            v.sort();
            v.dedup();
            (k, v)
        })
        .collect()
}

/// The match group (or differential pair) a net belongs to: through a 0x26
/// indirection on 17.2+, straight to the naming table before.
fn match_group(db: &Db, net: &Block) -> Option<String> {
    let Data::Net { match_group, .. } = &net.data else { return None };
    let mut k = *match_group;
    for _ in 0..4 {
        match db.stream.get(k).map(|x| &x.data) {
            Some(Data::MatchGroup { group, .. }) => k = *group,
            Some(Data::Table { text, .. }) => {
                let n = db.s(*text);
                return (!n.is_empty()).then(|| n.to_string());
            }
            _ => return None,
        }
    }
    None
}

/// One physical constraint set, as its first copper layer states it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintSet {
    pub name: String,
    pub line_width_mm: f64,
    pub spacing_mm: f64,
    /// 17.2 and later state a clearance of their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clearance_mm: Option<f64>,
    /// Differential-pair gap, when the set defines one (17.2 and later).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_pair_gap_mm: Option<f64>,
    /// Nets that use the set.
    pub nets: usize,
}

/// The board's physical constraint sets. Record fields by format, checked on
/// BeagleBone Black (17.2) and TRS-80 (16.6) against their known rules:
/// 17.2+ keeps width, spacing, clearance and pair gap at words 1, 2, 4 and 7;
/// earlier boards keep width and spacing at words 0 and 1.
pub fn constraint_sets(b: &Board) -> Vec<ConstraintSet> {
    let db = &b.db;
    let classes = net_classes(b);
    let mm = |v: i32| r4(v as f64 * db.header.scale());
    let mut out = Vec::new();
    for blk in &db.stream.blocks {
        let Data::ConstraintSet { name, field: fptr, records } = &blk.data else { continue };
        let Some(r) = records.first() else { continue };
        let mut n = db.s(*name).to_string();
        if n.is_empty() {
            // The set's field names it in a schematic cross-reference,
            // `@lib.xxx(view):\NAME\`.
            n = fields(db, *fptr)
                .into_iter()
                .find_map(|(_, v)| field_text(db, v))
                .and_then(|t| t.rsplit(":\\").next().map(|s| s.trim_end_matches('\\').to_string()))
                .unwrap_or_default();
        }
        if n.is_empty() {
            // Some 16.x boards key the name by a table index this reader does
            // not resolve (0x03000001); the set keeps a positional name.
            n = format!("UNNAMED_{}", out.len());
        }
        let modern = db.header.ver >= Ver::V172;
        let class = if n.eq_ignore_ascii_case("DEFAULT") { "Default".to_string() } else { n.clone() };
        out.push(ConstraintSet {
            nets: classes.iter().find(|(c, _)| *c == class).map(|(_, m)| m.len()).unwrap_or(0),
            name: n,
            line_width_mm: mm(if modern { r[1] } else { r[0] }),
            spacing_mm: mm(if modern { r[2] } else { r[1] }),
            clearance_mm: modern.then(|| mm(r[4])),
            diff_pair_gap_mm: (modern && r[7] != 0).then(|| mm(r[7])),
        });
    }
    out
}

pub struct BoardSummary {
    pub layers: usize,
    pub components: usize,
    pub nets: usize,
}

/// What a board contributed to the bundle.
pub struct BoardArtifacts {
    /// Cache-relative path of `pcb/geometry.json`.
    pub geometry: String,
    pub svgs: Vec<serde_json::Value>,
    pub theme: BTreeMap<String, String>,
    pub net_classes: Vec<(String, Vec<String>)>,
    /// `Allegro 17.4`, for the source block.
    pub format: String,
    pub summary: BoardSummary,
    pub stats: BoardStats,
    pub constraint_sets: Vec<ConstraintSet>,
}

/// Read a board and write `pcb/geometry.json` plus the per-layer SVGs.
pub fn extract_board(path: &Path, out_dir: &Path, emit: &mut dyn FnMut(Msg)) -> Result<BoardArtifacts, String> {
    let b = load_board(path)?;
    let source = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let built = build(&b, &source);
    let g = &built.geometry;
    if let Some(s) = &built.stats.stopped {
        emit(Msg::Progress(format!("pcb: board read stopped early ({s}); what was read is kept")));
    }
    let pcb_dir = out_dir.join("pcb");
    std::fs::create_dir_all(&pcb_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string(g).map_err(|e| e.to_string())?;
    std::fs::write(pcb_dir.join("geometry.json"), json).map_err(|e| e.to_string())?;
    emit(Msg::Progress(format!(
        "pcb geometry: {} layers, {} nets, {} seg + {} arc tracks, {} vias, {} pads, {} zones, {} graphics, {} texts",
        g.layers.len(),
        g.nets.len().saturating_sub(1),
        g.tracks.seg.w.len(),
        g.tracks.arc.w.len(),
        g.vias.len(),
        g.pads.len(),
        g.zones.len(),
        g.graphics.len(),
        g.texts.len(),
    )));
    let rel = "pcb/geometry.json".to_string();
    emit(Msg::Artifact(rel.clone()));
    let svgs = match crate::altium::pcb_svg::write_layer_svgs(g, &source, out_dir, emit) {
        Ok(v) => v,
        Err(e) => {
            emit(Msg::Progress(format!("pcb layer svgs skipped: {e}")));
            Vec::new()
        }
    };
    emit(Msg::Progress(format!("pcb: {} layer svgs", svgs.len())));
    // A 3D artifact is worth having and never worth losing the board over.
    if let Err(e) = write_models(&b, g, path, out_dir, emit) {
        emit(Msg::Progress(format!("models skipped: {e}")));
    }
    Ok(BoardArtifacts {
        geometry: rel,
        svgs,
        theme: board_theme(g),
        net_classes: net_classes(&b),
        constraint_sets: constraint_sets(&b),
        format: built.stats.format.clone(),
        summary: BoardSummary {
            layers: g.layers.len(),
            components: g.components.len(),
            nets: g.nets.len().saturating_sub(1),
        },
        stats: built.stats,
    })
}

/// Field codes on a footprint definition naming its 3D model.
const MODEL_FILE: u16 = 0x345;
const MODEL_PLACE: u16 = 0x346;

/// One footprint definition's 3D model: the file name and its placement.
struct ModelRef {
    file: String,
    offset: (f64, f64, f64),
    rotate: (f64, f64, f64),
}

/// The model a footprint definition names. `0x345` is `file, bytes, mtime,
/// colour, r, g, b, triangles`; `0x346` is `units, dx, dy, dz, rx, ry, rz`
/// relative to the symbol origin in a Z-up, Y-up frame.
fn model_of(db: &Db, def_fields: u32) -> Option<ModelRef> {
    let fs = fields(db, def_fields);
    let text = |code| fs.iter().find(|(c, _)| *c == code).and_then(|(_, v)| field_text(db, v));
    let file = text(MODEL_FILE)?.split(',').next()?.trim().to_string();
    if file.is_empty() {
        return None;
    }
    let mut offset = (0.0, 0.0, 0.0);
    let mut rotate = (0.0, 0.0, 0.0);
    if let Some(p) = text(MODEL_PLACE) {
        let v: Vec<&str> = p.split(',').map(str::trim).collect();
        let unit = match v.first().map(|u| u.to_ascii_uppercase()).as_deref() {
            Some("CM") => 10.0,
            Some("MICRONS") => 0.001,
            Some("MILS") => 0.0254,
            Some("INCH") | Some("INCHES") => 25.4,
            _ => 1.0,
        };
        let f = |i: usize| v.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        offset = (r4(f(1) * unit), r4(f(2) * unit), r4(f(3) * unit));
        // Stored the way the KiCad path stores a model's rotation: negated.
        rotate = (r4(-f(4)), r4(-f(5)), r4(-f(6)));
    }
    Some(ModelRef { file, offset, rotate })
}

/// A model file named by the board, looked for beside it (and in the usual
/// model folders there), case-insensitively. Allegro keeps only the name.
fn find_model(board_dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let want = name.to_ascii_lowercase();
    for d in [board_dir.to_path_buf(), board_dir.join("step"), board_dir.join("3d"), board_dir.join("models"), board_dir.join("..").join("step")] {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().to_ascii_lowercase() == want && e.path().is_file() {
                return Some(e.path());
            }
        }
    }
    None
}

/// Write `models/models.json` (the shared `extract.models.a0`) for the placed
/// footprints whose definition names a 3D model, copying any model file found
/// beside the board.
fn write_models(b: &Board, g: &Geometry, board_path: &Path, out_dir: &Path, emit: &mut dyn FnMut(Msg)) -> Result<usize, String> {
    let db = &b.db;
    let mut by_def: HashMap<u32, ModelRef> = HashMap::new();
    let mut inst_def: HashMap<u32, u32> = HashMap::new();
    for blk in &db.stream.blocks {
        let Data::FootprintDef { fields: f, first_inst, .. } = &blk.data else { continue };
        if let Some(m) = model_of(db, *f) {
            by_def.insert(blk.key, m);
        }
        let mut k = *first_inst;
        let mut seen = HashSet::new();
        while let Some(i) = db.stream.get(k) {
            if i.kind != 0x2D || !seen.insert(k) {
                break;
            }
            inst_def.insert(i.key, blk.key);
            k = i.next;
        }
    }
    if by_def.is_empty() {
        return Ok(0);
    }
    let models_dir = out_dir.join("models");
    let files_dir = models_dir.join("files");
    let board_dir = board_path.parent().unwrap_or(Path::new("."));
    let mut copied: HashMap<String, String> = HashMap::new();
    let (mut refs, mut unresolved) = (0usize, 0usize);
    let mut entries = Vec::new();
    for (i, f) in b.footprints.iter().enumerate() {
        let Some(m) = inst_def.get(&f.key).and_then(|d| by_def.get(d)) else { continue };
        let Some(c) = g.components.get(i) else { continue };
        refs += 1;
        let mut entry = serde_json::json!({
            "path": m.file,
            "offset": { "x": m.offset.0, "y": m.offset.1, "z": m.offset.2 },
            "scale": { "x": 1.0, "y": 1.0, "z": 1.0 },
            "rotate": { "x": m.rotate.0, "y": m.rotate.1, "z": m.rotate.2 },
        });
        let key = m.file.to_ascii_lowercase();
        let rel = match copied.get(&key) {
            Some(r) => Some(r.clone()),
            None => match find_model(board_dir, &m.file) {
                Some(src) => {
                    std::fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;
                    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    std::fs::copy(&src, files_dir.join(&name)).map_err(|e| e.to_string())?;
                    let r = format!("files/{name}");
                    copied.insert(key, r.clone());
                    Some(r)
                }
                None => None,
            },
        };
        match rel {
            Some(r) => {
                let ext = Path::new(&r).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                entry["file"] = serde_json::json!(r);
                entry["format"] = serde_json::json!(ext);
            }
            None => unresolved += 1,
        }
        entries.push(serde_json::json!({
            "reference": c.reference,
            "footprint": c.fp,
            "layer": c.layer,
            "uuid": c.uuid,
            "at": { "x": c.x, "y": c.y, "angle": c.angle },
            "models": [entry],
        }));
    }
    std::fs::create_dir_all(&models_dir).map_err(|e| e.to_string())?;
    let doc = serde_json::json!({
        "schema": "extract.models.a0",
        "count": entries.len(),
        "refs": refs,
        "files": copied.len(),
        "unresolved": unresolved,
        "models": entries,
    });
    std::fs::write(models_dir.join("models.json"), serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    emit(Msg::Artifact("models/models.json".to_string()));
    emit(Msg::Progress(format!(
        "models: {refs} refs on {} footprints, {} files found beside the board, {unresolved} unresolved",
        entries.len(),
        copied.len()
    )));
    Ok(entries.len())
}

/// Board colours: Allegro's default film colours are user configuration, not
/// board data, so these follow its factory display — top copper green-ish
/// cyan is avoided in favour of the conventional red/blue the other paths use,
/// keeping one look across tools for the same roles.
pub fn board_theme(g: &Geometry) -> BTreeMap<String, String> {
    crate::altium::pcb_svg::board_theme(g)
}

/// The board's components as design-model components, for a board reviewed
/// without its schematic (and for the BOM of a loose `.brd`).
pub fn board_components(b: &Board) -> Vec<Component> {
    let mut pins: HashMap<u32, Vec<String>> = HashMap::new();
    for blk in &b.db.stream.blocks {
        let Data::PlacedPad { parent_fp, pad, .. } = &blk.data else { continue };
        if let Some(Data::Pad { name, .. }) = b.db.stream.get(*pad).map(|x| &x.data) {
            pins.entry(*parent_fp).or_default().push(b.db.s(*name).to_string());
        }
    }
    b.footprints
        .iter()
        .filter(|f| !f.refdes.is_empty())
        .map(|f| {
            let mut p = pins.get(&f.key).cloned().unwrap_or_default();
            p.sort();
            p.dedup();
            let prefix = crate::design::prefix_of(&f.refdes);
            let mut parameters = BTreeMap::new();
            parameters.insert("Value".to_string(), f.value.clone());
            parameters.insert("Footprint".to_string(), f.symbol.clone());
            if !f.device.is_empty() {
                parameters.insert("Device".to_string(), f.device.clone());
            }
            parameters.insert("kicad_in_bom".into(), "true".into());
            parameters.insert("kicad_dnp".into(), "false".into());
            parameters.insert("kicad_on_board".into(), "true".into());
            Component {
                designator: f.refdes.clone(),
                svg_id: format!("brd:{}", f.key),
                value: f.value.clone(),
                footprint: f.symbol.clone(),
                library_ref: if f.device.is_empty() { f.symbol.clone() } else { f.device.clone() },
                description: if f.description.is_empty() { f.device.clone() } else { f.description.clone() },
                hierarchy: Hierarchy {
                    base_designator: f.refdes.clone(),
                    channel: None,
                    channel_index: None,
                    sheet: String::new(),
                    sheet_path: String::new(),
                    sheet_path_uuids: String::new(),
                },
                classification: Classification {
                    prefix: prefix.clone(),
                    kind: crate::design::classify(&prefix, p.len() as u32).to_string(),
                    pin_count: p.len() as u32,
                },
                parameters,
                bbox: None,
            }
        })
        .collect()
}

/// Pad connectivity: net name -> (designator, pin number) terminals.
pub fn board_netlist(b: &Board) -> BTreeMap<String, Vec<(String, String)>> {
    let db = &b.db;
    let comp: HashMap<u32, &Placed> = b.footprints.iter().map(|f| (f.key, f)).collect();
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for blk in &db.stream.blocks {
        let Data::PlacedPad { net, parent_fp, pad, .. } = &blk.data else { continue };
        let Some(f) = comp.get(parent_fp) else { continue };
        let net_key = match db.stream.get(*net).map(|x| &x.data) {
            Some(Data::NetAssign { net, .. }) => *net,
            _ => continue,
        };
        let Some(name) = b.net_names.get(&net_key) else { continue };
        let pin = match db.stream.get(*pad).map(|x| &x.data) {
            Some(Data::Pad { name, .. }) => db.s(*name).to_string(),
            _ => continue,
        };
        out.entry(name.clone()).or_default().push((f.refdes.clone(), pin));
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    out
}

/// A design model built from the board alone.
pub fn board_design(b: &Board, name: &str, path: &str, filename: &str) -> Design {
    let components = board_components(b);
    let mut frags = Vec::new();
    for (net, terms) in board_netlist(b) {
        frags.push(crate::netlist::Frag {
            terminals: terms
                .into_iter()
                .map(|(d, p)| crate::netlist::Terminal { designator: d, pin: p, pin_name: String::new(), pin_type: String::new() })
                .collect(),
            graphical: Default::default(),
            keys: vec![format!("brd:{net}")],
            name: net,
            driver_kind: "board".to_string(),
            rank: 1,
            classes: Vec::new(),
            sheet: "/".to_string(),
        });
    }
    let nets = crate::netlist::merge_frags(frags);
    crate::design::assemble(name, path, filename, Vec::new(), components, nets, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
    }

    #[test]
    fn an_arc_midpoint_follows_its_sweep() {
        let r = 1000.0;
        let half = r / std::f64::consts::SQRT_2;
        // Counter-clockwise (Y up) from +X to +Y passes through the first quadrant;
        // clockwise between the same ends goes the long way round.
        assert!(close(arc_mid((1000, 0), (0, 1000), (0.0, 0.0), r, false), (half, half)));
        assert!(close(arc_mid((1000, 0), (0, 1000), (0.0, 0.0), r, true), (-half, -half)));
        // Equal ends are a full circle: the midpoint is diametrically opposite.
        assert!(close(arc_mid((1000, 0), (1000, 0), (0.0, 0.0), r, false), (-r, 0.0)));
    }

    #[test]
    fn a_void_is_bridged_into_its_outline_against_its_winding() {
        let outer = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let hole = vec![(4.0, 4.0), (6.0, 4.0), (6.0, 6.0), (4.0, 6.0)];
        let ring = bridge(outer.clone(), vec![hole.clone()]);
        assert_eq!(ring.len(), outer.len() + hole.len() + 2);
        for p in &hole {
            assert!(ring.contains(p), "every void vertex is on the ring");
        }
        // The outline and the void wind opposite ways, so the void stays empty.
        let at = ring.iter().position(|p| hole.contains(p)).unwrap();
        let v: Vec<(f64, f64)> = ring[at..at + hole.len()].to_vec();
        assert!(signed_area(&v) * signed_area(&outer) < 0.0);
    }

    #[test]
    fn layers_take_the_roles_the_extract_reports_confirmed() {
        let l = |class, sub| Layer { class, sub };
        assert_eq!(slot_of(l(class::ETCH, 1), 4), Slot::Copper(1));
        assert_eq!(slot_of(l(class::BOARD_GEOMETRY, 0xEA), 4), Slot::Edge);
        assert_eq!(slot_of(l(class::BOARD_GEOMETRY, 0xFD), 4), Slot::Edge);
        assert_eq!(slot_of(l(class::PACKAGE_GEOMETRY, 0xF7), 4), Slot::Silk(true));
        assert_eq!(slot_of(l(class::PACKAGE_GEOMETRY, 0xF6), 4), Slot::Silk(false));
        assert_eq!(slot_of(l(class::REF_DES, 0xFB), 4), Slot::Silk(true));
        assert_eq!(slot_of(l(class::BOARD_GEOMETRY, 0xF1), 4), Slot::Silk(true));
        // A device type on its silkscreen subclass is film configuration, not
        // silkscreen by convention.
        assert_eq!(slot_of(l(class::DEVICE_TYPE, 0xFB), 4), Slot::Other(class::DEVICE_TYPE, 0xFB));
        // The drawing format's outline is the sheet's, not the board's.
        assert_eq!(slot_of(l(class::DRAWING_FORMAT, 0xFD), 4), Slot::Other(class::DRAWING_FORMAT, 0xFD));
        assert_eq!(fixed_subclass(class::PACKAGE_GEOMETRY, 0xF9), Some("PIN_NUMBER"));
        assert_eq!(fixed_subclass(class::PACKAGE_GEOMETRY, 0xE5), None);
    }

    fn padstack(start: u8, count: u16) -> Padstack {
        let pad = PadComp { kind: 0x06, w: 10, h: 20, ..Default::default() };
        let mut comps = vec![PadComp::default(); 21];
        for _ in 0..count {
            comps.extend([PadComp::default(), PadComp::default(), pad, PadComp::default()]);
        }
        Padstack {
            name: 0,
            start_layer: start,
            layer_count: count,
            drill_w: 0,
            drill_h: 0,
            plated: false,
            kind: Some(0),
            comps,
            fixed: 21,
            per_layer: 4,
        }
    }

    #[test]
    fn a_bottom_footprint_puts_its_padstack_top_on_the_board_bottom() {
        let smd = padstack(0, 1);
        assert!(pad_on_board(&smd, 0, 4, false).is_some());
        assert!(pad_on_board(&smd, 3, 4, false).is_none());
        assert!(pad_on_board(&smd, 3, 4, true).is_some());
        assert!(pad_on_board(&smd, 0, 4, true).is_none());
        assert_eq!(span(&smd, 4, true), vec![3]);
        assert_eq!(span(&padstack(1, 2), 4, false), vec![1, 2]);
    }

    #[test]
    fn pad_shapes_map_to_the_ir_codes() {
        let mut approx = 0;
        let comp = |kind, w, h| PadComp { kind, w, h, corner: 5, ..Default::default() };
        assert_eq!(pad_shape(&comp(0x02, 10, 10), &mut approx), (0, 0.0));
        assert_eq!(pad_shape(&comp(0x02, 10, 20), &mut approx).0, 3);
        assert_eq!(pad_shape(&comp(0x06, 10, 20), &mut approx), (1, 0.0));
        assert_eq!(pad_shape(&comp(0x0B, 30, 10), &mut approx), (3, 0.0));
        assert_eq!(pad_shape(&comp(0x1B, 20, 10), &mut approx), (2, 0.5));
        assert_eq!(approx, 0);
        let _ = pad_shape(&comp(0x03, 10, 10), &mut approx);
        assert_eq!(approx, 1, "an octagon is drawn approximately and counted");
    }
}
