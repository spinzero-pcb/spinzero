//! The `.PcbDoc` object model.
//!
//! Faithful to the file: primitives keep Altium's own space — millimetres,
//! **Y-up**, absolute, with `Board6`'s `ORIGINX`/`ORIGINY` left alone because it
//! is a *display* origin and the stored coordinates are already absolute. The
//! bundle's Y-down flip is the caller's job (`extract::altium::pcb`), the same
//! split [`sch`](crate::sch) keeps.
//!
//! Two framings meet here. `Board6`, `Nets6`, `Components6`, `Classes6` and
//! `Polygons6` are text records; the primitives — tracks, arcs, pads, vias,
//! texts, fills, regions — are binary, and every one of them opens with the same
//! 13-byte header ([`Common`]) carrying its layer, net, polygon and component.
//!
//! Record lengths vary by Altium vintage (a pad's main block is 180 bytes in the
//! oldest corpus design and 194 in the newest; a via is 316, 321 or 330). Every
//! read here is therefore bounds-checked, and a short record simply stops
//! yielding fields rather than costing the document.

use std::collections::BTreeMap;

use crate::doc::Doc;
use crate::record::{decode_utf16, Raw, TextRecord};
use crate::units;

/// Legacy layer ids a primitive can reference. `Board6` describes 1..=82.
pub const LEGACY_LAYERS: u8 = 82;
/// Legacy id of the top copper layer.
pub const TOP: u8 = 1;
/// Legacy id of the bottom copper layer.
pub const BOTTOM: u8 = 32;
/// Legacy id of the multi-layer pseudo-layer (through-hole pads and vias).
pub const MULTI_LAYER: u8 = 74;
/// Legacy id of the keep-out layer, which is where Altium's board shape lives.
pub const KEEP_OUT: u8 = 56;

/// The 13-byte header every binary primitive opens with.
///
/// `net`, `polygon` and `component` are `u16` indexes into `Nets6`,
/// `Polygons6` and `Components6` in file order, with `0xFFFF` meaning "none".
#[derive(Debug, Clone, Copy, Default)]
pub struct Common {
    pub layer: u8,
    pub net: Option<u16>,
    pub polygon: Option<u16>,
    pub component: Option<u16>,
}

impl Common {
    fn parse(b: &[u8]) -> Option<Common> {
        if b.len() < 13 {
            return None;
        }
        let idx = |o: usize| match u16::from_le_bytes([b[o], b[o + 1]]) {
            0xFFFF => None,
            v => Some(v),
        };
        Some(Common { layer: b[0], net: idx(3), polygon: idx(5), component: idx(7) })
    }
}

/// A copper or graphic line segment (`Tracks6`).
#[derive(Debug, Clone)]
pub struct Track {
    pub c: Common,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub width: f64,
}

/// A circular arc (`Arcs6`). Angles are degrees counter-clockwise from +X in
/// Altium's Y-up space.
#[derive(Debug, Clone)]
pub struct Arc {
    pub c: Common,
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub width: f64,
}

/// Altium's pad shape codes, as stored.
pub const SHAPE_ROUND: u8 = 1;
pub const SHAPE_RECT: u8 = 2;
pub const SHAPE_OCTAGONAL: u8 = 3;
pub const SHAPE_ROUNDED_RECT: u8 = 9;

/// A pad (`Pads6`).
///
/// Size and shape come from the *top* entry of Altium's stack, taken from the
/// extended per-layer table when the record carries one — that is the block
/// Altium itself honours, and a rounded-rectangle pad reads as a plain
/// rectangle from the base fields alone (corner case 24).
#[derive(Debug, Clone)]
pub struct Pad {
    pub c: Common,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub shape: u8,
    /// Corner radius as a fraction of the short side, for a rounded rectangle.
    pub corner_ratio: f64,
    pub hole: f64,
    pub rotation: f64,
    pub plated: bool,
}

/// A via (`Vias6`). `from_layer`/`to_layer` are legacy copper ids.
#[derive(Debug, Clone)]
pub struct Via {
    pub c: Common,
    pub x: f64,
    pub y: f64,
    pub diameter: f64,
    pub hole: f64,
    pub from_layer: u8,
    pub to_layer: u8,
}

/// A text object (`Texts6`).
///
/// The string comes from `WideStrings6` by index, which is the only spelling
/// that survives a non-ASCII designator; the record's own 8-bit copy is the
/// fallback. Altium's special strings (`.DESIGNATOR`, `.COMMENT`) arrive
/// unresolved and are the caller's to resolve against the owning component.
#[derive(Debug, Clone)]
pub struct Text {
    pub c: Common,
    pub x: f64,
    pub y: f64,
    pub height: f64,
    pub width: f64,
    pub rotation: f64,
    pub mirror: bool,
    pub font: String,
    pub text: String,
    /// True for the object that IS the component's designator, as opposed to
    /// free silkscreen text the footprint happens to own. This is the flag that
    /// makes the designator exact rather than guessed (corner case 7).
    pub is_designator: bool,
    /// True for the object that is the component's comment.
    pub is_comment: bool,
}

/// A filled rectangle (`Fills6`).
#[derive(Debug, Clone)]
pub struct Fill {
    pub c: Common,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub rotation: f64,
}

/// A filled region (`Regions6`) — polygon-pour copper, shape-based pad
/// apertures, keep-outs and the board shape all share this record.
#[derive(Debug, Clone)]
pub struct Region {
    pub c: Common,
    /// The record's own `|KEY=VALUE|` parameter string.
    pub params: TextRecord,
    /// Outline ring, millimetres, in file order.
    pub outline: Vec<(f64, f64)>,
}

impl Region {
    /// True for a region that cuts the board shape rather than adding copper.
    pub fn is_board_cutout(&self) -> bool {
        self.params.b("ISBOARDCUTOUT")
    }
    pub fn is_keepout(&self) -> bool {
        self.params.b("KEEPOUT")
    }
}

/// A placed footprint (`Components6`).
#[derive(Debug, Clone, Default)]
pub struct Component {
    /// `SOURCEDESIGNATOR` — the *base* designator. The displayed one, channel
    /// suffix included, is resolved through `Texts6` ([`designators`]).
    pub designator: String,
    pub comment: String,
    pub pattern: String,
    pub description: String,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    /// Legacy id of the mount side ([`TOP`] or [`BOTTOM`]).
    pub layer: u8,
    /// `SOURCEUNIQUEID` — the schematic instance this footprint came from, and
    /// the identity the diff engine pairs on.
    pub source_unique_id: String,
    pub unique_id: String,
    pub source_hierarchical_path: String,
}

/// A net (`Nets6`). Its position in the stream is the id primitives reference.
#[derive(Debug, Clone, Default)]
pub struct Net {
    pub name: String,
    /// Altium's per-net colour, already converted from BGR.
    pub color: String,
}

/// One layer of the board, keyed by its legacy id.
#[derive(Debug, Clone, Default)]
pub struct Layer {
    /// Legacy id, 1..=82.
    pub id: u8,
    /// The name the design gives it (`L2_PWR`, `M15 (CMP_Courtyard_Top)`), which
    /// is what Altium shows the user.
    pub name: String,
    /// Copper thickness, when the stack declares one.
    pub thickness: String,
    /// True for a mechanical layer the design has enabled.
    pub enabled: bool,
}

/// A whole `.PcbDoc`.
#[derive(Debug, Clone, Default)]
pub struct PcbDoc {
    /// Layer table keyed by legacy id.
    pub layers: BTreeMap<u8, Layer>,
    /// Copper layers in stack order, walked from `LAYER1NEXT`.
    pub stack: Vec<u8>,
    pub nets: Vec<Net>,
    pub components: Vec<Component>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub pads: Vec<Pad>,
    pub vias: Vec<Via>,
    pub texts: Vec<Text>,
    pub fills: Vec<Fill>,
    pub regions: Vec<Region>,
    /// Board shape rings from `BoardRegions`, millimetres.
    pub outline: Vec<Vec<(f64, f64)>>,
    /// Net classes (`Classes6` `KIND=0`): class name -> member net names.
    pub net_classes: Vec<(String, Vec<String>)>,
    /// Layer-stack regions beyond the first — the rigid-flex model, detected and
    /// reported rather than modelled (plan §4.5).
    pub substacks: Vec<String>,
    /// Streams read but not modelled, by stream and record count.
    pub skipped: BTreeMap<String, usize>,
}

/// Parse one `.PcbDoc`.
pub fn parse(doc: &Doc) -> PcbDoc {
    let mut out = PcbDoc::default();
    let strings = doc.wide_strings();

    if let Some(board) = doc.text_records("Board6").first() {
        read_layers(board, &mut out);
    }
    for r in doc.text_records("Nets6") {
        out.nets.push(Net {
            name: r.s("NAME").to_string(),
            color: units::bgr_hex(r.i("COLOR").unwrap_or(0)),
        });
    }
    for r in doc.text_records("Components6") {
        out.components.push(read_component(&r));
    }
    for r in doc.text_records("Classes6") {
        // KIND=0 is a net class; 1 is a component class, 2 a layer class.
        if r.i("KIND") != Some(0) {
            continue;
        }
        let mut members = Vec::new();
        for i in 0.. {
            let Some(m) = r.get(&format!("M{i}")) else { break };
            members.push(m.to_string());
        }
        out.net_classes.push((r.s("NAME").to_string(), members));
    }

    for r in doc.records("Tracks6") {
        if let Some(t) = read_track(&r) {
            out.tracks.push(t);
        }
    }
    for r in doc.records("Arcs6") {
        if let Some(a) = read_arc(&r) {
            out.arcs.push(a);
        }
    }
    for r in doc.records("Pads6") {
        if let Some(p) = read_pad(&r) {
            out.pads.push(p);
        }
    }
    for r in doc.records("Vias6") {
        if let Some(v) = read_via(&r) {
            out.vias.push(v);
        }
    }
    for r in doc.records("Texts6") {
        if let Some(t) = read_text(&r, &strings) {
            out.texts.push(t);
        }
    }
    for r in doc.records("Fills6") {
        if let Some(f) = read_fill(&r) {
            out.fills.push(f);
        }
    }
    for r in doc.records("Regions6") {
        if let Some(g) = read_region(&r) {
            out.regions.push(g);
        }
    }
    // The board shape. Altium keeps it in its own region stream; a design that
    // predates layer-stack regions has none, and the outline is then the
    // keep-out line work the caller already emits on the `edge` role.
    for r in doc.records("BoardRegions") {
        let Some(g) = read_region(&r) else { continue };
        let name = g.params.s("NAME").to_string();
        if !out.outline.is_empty() && !name.is_empty() {
            out.substacks.push(name);
        }
        out.outline.push(g.outline);
    }

    // Geometry we do not model yet, recorded so the bundle's `unresolved` block
    // can say what was skipped rather than stay silent.
    for stream in ["ComponentBodies6", "Dimensions6", "Coordinates6", "EmbeddedBoards6"] {
        let n = doc.records(stream).len();
        if n > 0 {
            out.skipped.insert(stream.to_string(), n);
        }
    }
    out
}

/// The displayed designator of every component, resolved through `Texts6`.
///
/// `Components6` carries only the *base* designator, so taking
/// `SOURCEDESIGNATOR` at face value collapses every channel of a repeated block
/// onto one name and keeps a stale spelling when the board was re-annotated —
/// the corpus places `RD1` on a part whose `SOURCEDESIGNATOR` still reads `Rd1`
/// (corner case 7). The authoritative one is the text object the file MARKS as
/// the designator, which every placed component has exactly one of; the base is
/// the fallback for a component that somehow has none.
pub fn designators(pcb: &PcbDoc) -> Vec<String> {
    let mut out: Vec<String> = pcb.components.iter().map(|c| c.designator.clone()).collect();
    for t in &pcb.texts {
        let Some(i) = t.c.component.map(usize::from) else { continue };
        if !t.is_designator || t.text.is_empty() || i >= out.len() {
            continue;
        }
        out[i] = t.text.clone();
    }
    out
}

/// The layer stack and the legacy id -> name table.
///
/// Three generations describe the same layers and a primitive names one by its
/// **legacy** id, so all three are read and the newest name wins: the flat
/// `LAYER<n>` table is indexed by legacy id directly, then V7 and V9 are keyed
/// by species id and mapped back. A pass that reads only one generation cannot
/// label every primitive (corner case 22).
fn read_layers(b: &TextRecord, out: &mut PcbDoc) {
    for id in 1..=LEGACY_LAYERS {
        out.layers.insert(
            id,
            Layer {
                id,
                name: b.get(&format!("LAYER{id}NAME")).unwrap_or("").to_string(),
                thickness: b.get(&format!("LAYER{id}COPTHICK")).unwrap_or("").to_string(),
                enabled: b.b(&format!("LAYER{id}MECHENABLED")),
            },
        );
    }
    // V7 then V9, newest last so it wins. Altium writes the V7 table under two
    // spellings depending on vintage.
    for (prefix, id_key, name_key) in [
        ("LAYERV7_", "LAYERID", "NAME"),
        ("LAYER_V7_", "LAYERID", "NAME"),
        ("V9_CACHE_LAYER", "_LAYERID", "_NAME"),
    ] {
        for i in 0..=512 {
            let Some(raw) = b.i(&format!("{prefix}{i}{id_key}")) else { continue };
            let Some(legacy) = legacy_of_v9(raw as u32) else { continue };
            let Some(name) = b.get(&format!("{prefix}{i}{name_key}")) else { continue };
            if name.is_empty() {
                continue;
            }
            out.layers
                .entry(legacy)
                .or_insert(Layer { id: legacy, ..Layer::default() })
                .name = name.to_string();
        }
    }
    // A layer with no name of its own gets Altium's default, so a primitive is
    // never unlabelled.
    for (id, l) in out.layers.iter_mut() {
        if l.name.is_empty() {
            l.name = default_layer_name(*id);
        }
    }
    // The copper stack, walked along `LAYER<n>NEXT` from the top. The chain is
    // the only place the *used* copper layers are distinguishable from the 32
    // the file always declares.
    let mut id = TOP;
    for _ in 0..=32 {
        out.stack.push(id);
        let next = b.i(&format!("LAYER{id}NEXT")).unwrap_or(0);
        if next <= 0 || next > i64::from(LEGACY_LAYERS) {
            break;
        }
        id = next as u8;
    }
    if !out.stack.contains(&BOTTOM) {
        out.stack.push(BOTTOM);
    }
}

/// Decode a V7/V9 layer id into the legacy id primitives use, when it has one.
///
/// The id is `0x01_CC_IIII`: class `0x00` is signal copper (`0xFFFF` is the
/// bottom layer, not 32), `0x02` mechanical, `0x03` the fixed layers.
/// Mechanical 17 and up, and dielectrics, have no legacy id at all — they are
/// skipped rather than folded onto a neighbour.
fn legacy_of_v9(id: u32) -> Option<u8> {
    let class = (id >> 16) & 0xFF;
    let index = id & 0xFFFF;
    match class {
        0x00 if index == 0xFFFF => Some(BOTTOM),
        0x00 if (1..=31).contains(&index) => Some(index as u8),
        0x02 if (1..=16).contains(&index) => Some(56 + index as u8),
        0x03 => match index {
            0x06 => Some(33), // Top Overlay
            0x07 => Some(34), // Bottom Overlay
            0x08 => Some(35), // Top Paste
            0x09 => Some(36), // Bottom Paste
            0x0A => Some(37), // Top Solder
            0x0B => Some(38), // Bottom Solder
            0x0F => Some(MULTI_LAYER),
            _ => None,
        },
        _ => None,
    }
}

/// Altium's own name for a legacy layer id, for a file that names none.
pub fn default_layer_name(id: u8) -> String {
    match id {
        1 => "Top Layer".into(),
        2..=31 => format!("Mid-Layer {}", id - 1),
        32 => "Bottom Layer".into(),
        33 => "Top Overlay".into(),
        34 => "Bottom Overlay".into(),
        35 => "Top Paste".into(),
        36 => "Bottom Paste".into(),
        37 => "Top Solder".into(),
        38 => "Bottom Solder".into(),
        39..=54 => format!("Internal Plane {}", id - 38),
        55 => "Drill Guide".into(),
        56 => "Keep-Out Layer".into(),
        57..=72 => format!("Mechanical {}", id - 56),
        73 => "Drill Drawing".into(),
        74 => "Multi-Layer".into(),
        _ => format!("Layer {id}"),
    }
}

fn read_component(r: &TextRecord) -> Component {
    Component {
        designator: r.s("SOURCEDESIGNATOR").to_string(),
        comment: r.s("COMMENT").to_string(),
        pattern: r.s("PATTERN").to_string(),
        description: r.s("SOURCEDESCRIPTION").to_string(),
        // The board's text records write coordinates as decimals with an
        // explicit unit suffix, not in the primitives' integer units.
        x: units::mil_mm(r.f("X").unwrap_or(0.0)),
        y: units::mil_mm(r.f("Y").unwrap_or(0.0)),
        rotation: r.f("ROTATION").unwrap_or(0.0),
        layer: if r.s("LAYER").eq_ignore_ascii_case("BOTTOM") { BOTTOM } else { TOP },
        source_unique_id: r.s("SOURCEUNIQUEID").to_string(),
        unique_id: r.s("UNIQUEID").to_string(),
        source_hierarchical_path: r.s("SOURCEHIERARCHICALPATH").to_string(),
    }
}

/// Board coordinate at a byte offset, in millimetres. `None` past the record.
fn coord(b: &[u8], o: usize) -> Option<f64> {
    Some(units::board_mm(i64::from(i32_at(b, o)?)))
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn f64_at(b: &[u8], o: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

fn read_track(r: &Raw) -> Option<Track> {
    let b = &r.payload;
    Some(Track {
        c: Common::parse(b)?,
        x1: coord(b, 13)?,
        y1: coord(b, 17)?,
        x2: coord(b, 21)?,
        y2: coord(b, 25)?,
        width: coord(b, 29)?,
    })
}

fn read_arc(r: &Raw) -> Option<Arc> {
    let b = &r.payload;
    Some(Arc {
        c: Common::parse(b)?,
        cx: coord(b, 13)?,
        cy: coord(b, 17)?,
        radius: coord(b, 21)?,
        start_angle: f64_at(b, 25)?,
        end_angle: f64_at(b, 33)?,
        width: coord(b, 41)?,
    })
}

fn read_via(r: &Raw) -> Option<Via> {
    let b = &r.payload;
    Some(Via {
        c: Common::parse(b)?,
        x: coord(b, 13)?,
        y: coord(b, 17)?,
        diameter: coord(b, 21)?,
        hole: coord(b, 25)?,
        from_layer: *b.get(29)?,
        to_layer: *b.get(30)?,
    })
}

/// A pad is six length-prefixed blocks: the name, three small ones, the main
/// record, and — when the pad is not a plain stack — the per-layer size and
/// shape table that supersedes the main record's own (corner case 24).
fn read_pad(r: &Raw) -> Option<Pad> {
    let name = pascal(&r.payload);
    // Blocks after the first arrive in `extra`, so the fifth sub-record — the
    // main one — is `extra[3]` and the size/shape override is `extra[4]`.
    let main = r.extra.get(3)?;
    let c = Common::parse(main)?;
    let (w, h) = (coord(main, 21)?, coord(main, 25)?);
    let mut shape = *main.get(49)?;
    let rotation = f64_at(main, 52).unwrap_or(0.0);
    // A drilled hole with no plating is a bare hole, not a pad; Altium keeps the
    // flag set on the SMD pads it does not apply to, so the hole gates it.
    let plated = main.get(60).copied().unwrap_or(1) != 0;
    let hole = coord(main, 45).unwrap_or(0.0);

    // A rounded rectangle has NO code of its own in the base record: Altium
    // stores it as a plain `Round` pad plus a per-layer override in the sixth
    // sub-record, so reading the base alone draws 222 of the corpus's pads as
    // stadiums (corner case 24). The override only ever means anything when it
    // names the shape the base enum cannot express.
    let mut corner_ratio = 0.0;
    if let Some(ext) = r.extra.get(4).filter(|e| e.len() >= EXT_CORNERS + EXT_STACK) {
        if ext.get(EXT_ALT_SHAPES) == Some(&SHAPE_ROUNDED_RECT) {
            shape = SHAPE_ROUNDED_RECT;
            // Altium's corner radius is a percentage where 100% is a full
            // stadium — half the short side — so the fraction of the short side
            // the renderer wants is half of it.
            corner_ratio = f64::from(ext[EXT_CORNERS].min(100)) / 200.0;
        }
    }
    Some(Pad {
        c,
        name,
        x: coord(main, 13)?,
        y: coord(main, 17)?,
        w,
        h,
        shape,
        corner_ratio,
        hole,
        rotation,
        plated,
    })
}

/// Offsets inside a pad's sixth sub-record. It opens with 29 layers of x sizes,
/// 29 of y sizes and 29 inner-shape bytes, and later carries the two arrays that
/// matter here: the per-layer shape override and its corner radius, 32 entries
/// each with the top layer first.
const EXT_ALT_SHAPES: usize = 532;
const EXT_CORNERS: usize = 564;
/// Layers those two arrays describe.
const EXT_STACK: usize = 32;

fn read_text(r: &Raw, strings: &BTreeMap<u32, String>) -> Option<Text> {
    let b = &r.payload;
    let c = Common::parse(b)?;
    // `WideStrings6` is indexed from the record and is the only spelling that
    // survives a non-ASCII string; the record's own 8-bit copy is the fallback.
    let idx = b
        .get(115..119)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes);
    let text = idx
        .and_then(|i| strings.get(&i).cloned())
        .unwrap_or_else(|| pascal(r.extra.first().map(Vec::as_slice).unwrap_or_default()));
    let font = b
        .get(46..110)
        .map(decode_utf16)
        .unwrap_or_default()
        .trim_end_matches('\0')
        .to_string();
    Some(Text {
        c,
        x: coord(b, 13)?,
        y: coord(b, 17)?,
        height: coord(b, 21)?,
        rotation: f64_at(b, 27).unwrap_or(0.0),
        mirror: b.get(35).copied().unwrap_or(0) != 0,
        width: coord(b, 36).unwrap_or(0.0),
        font,
        text,
        is_comment: b.get(40).copied().unwrap_or(0) != 0,
        is_designator: b.get(41).copied().unwrap_or(0) != 0,
    })
}

fn read_fill(r: &Raw) -> Option<Fill> {
    let b = &r.payload;
    Some(Fill {
        c: Common::parse(b)?,
        x1: coord(b, 13)?,
        y1: coord(b, 17)?,
        x2: coord(b, 21)?,
        y2: coord(b, 25)?,
        rotation: f64_at(b, 29).unwrap_or(0.0),
    })
}

/// A region is the common header, five bytes, a length-prefixed parameter
/// string, then a vertex count and that many `f64` pairs — the one primitive
/// whose coordinates are floating point rather than integer units.
fn read_region(r: &Raw) -> Option<Region> {
    let b = &r.payload;
    let c = Common::parse(b)?;
    let len = i32_at(b, 18)?.max(0) as usize;
    let params = TextRecord::parse(b.get(22..22 + len)?);
    let at = 22 + len;
    let n = i32_at(b, at)?.max(0) as usize;
    let mut outline = Vec::with_capacity(n.min(4096));
    for i in 0..n {
        let o = at + 4 + i * 16;
        let (Some(x), Some(y)) = (f64_at(b, o), f64_at(b, o + 8)) else { break };
        outline.push((units::board_mm(x as i64), units::board_mm(y as i64)));
    }
    Some(Region { c, params, outline })
}

/// A `u8`-length-prefixed 8-bit string, as the small pad and text blocks carry.
fn pascal(b: &[u8]) -> String {
    let Some(&len) = b.first() else { return String::new() };
    crate::record::decode(b.get(1..1 + usize::from(len)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Mode;

    /// Corner case 22: a primitive names its layer by the LEGACY id while the
    /// stack is described in the V9 form. Without the mapping, a design whose
    /// names live only in the V9 cache labels nothing.
    #[test]
    fn v9_layer_ids_map_back_to_legacy_ids() {
        assert_eq!(legacy_of_v9(0x0100_0001), Some(TOP));
        assert_eq!(legacy_of_v9(0x0100_0002), Some(2), "Mid-Layer 1");
        assert_eq!(legacy_of_v9(0x0100_FFFF), Some(BOTTOM), "the bottom layer is 0xFFFF, not 32");
        assert_eq!(legacy_of_v9(0x0103_0006), Some(33), "Top Overlay");
        assert_eq!(legacy_of_v9(0x0102_000F), Some(71), "Mechanical 15");
        assert_eq!(legacy_of_v9(0x0103_000F), Some(MULTI_LAYER));
        assert_eq!(legacy_of_v9(0x0102_0012), None, "Mechanical 18 has no legacy id");
    }

    #[test]
    fn default_names_follow_altium() {
        assert_eq!(default_layer_name(1), "Top Layer");
        assert_eq!(default_layer_name(9), "Mid-Layer 8");
        assert_eq!(default_layer_name(32), "Bottom Layer");
        assert_eq!(default_layer_name(71), "Mechanical 15");
    }

    fn common_header(layer: u8, net: u16, comp: u16) -> Vec<u8> {
        let mut b = vec![layer, 0x0C, 0x00];
        b.extend_from_slice(&net.to_le_bytes());
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        b.extend_from_slice(&comp.to_le_bytes());
        b.extend_from_slice(&[0xFF; 4]);
        b
    }

    fn raw(payload: Vec<u8>) -> Raw {
        Raw { mode: Mode::Binary, offset: 0, kind: Some(4), payload, extra: Vec::new() }
    }

    #[test]
    fn a_track_reads_its_layer_net_and_geometry() {
        let mut b = common_header(TOP, 12, 0xFFFF);
        for v in [100_000i32, 200_000, 300_000, 200_000, 39_370] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let t = read_track(&raw(b)).expect("track");
        assert_eq!(t.c.layer, TOP);
        assert_eq!(t.c.net, Some(12));
        assert_eq!(t.c.component, None, "0xFFFF is 'none', not component 65535");
        assert!((t.x1 - 0.254).abs() < 1e-9);
        assert!((t.width - 0.1).abs() < 1e-4, "39370 units is a 0.1 mm track");
    }

    /// A short record — an older Altium vintage, or a stream we mis-framed —
    /// stops yielding fields instead of panicking or reading past its end.
    #[test]
    fn a_truncated_record_is_refused_not_panicked() {
        assert!(read_track(&raw(common_header(TOP, 0, 0))).is_none());
        assert!(read_track(&raw(vec![1, 2, 3])).is_none());
    }

    /// Corner case 24: the extended table supersedes the base shape, so a
    /// rounded-rectangle pad does not read as a plain rectangle.
    #[test]
    fn the_extended_table_supersedes_the_base_pad_shape() {
        let mut main = common_header(TOP, 0, 0);
        main.extend_from_slice(&100_000i32.to_le_bytes()); // x
        main.extend_from_slice(&100_000i32.to_le_bytes()); // y
        for _ in 0..6 {
            main.extend_from_slice(&39_370i32.to_le_bytes()); // top/mid/bot sizes
        }
        main.extend_from_slice(&0i32.to_le_bytes()); // hole
        main.extend_from_slice(&[SHAPE_RECT, SHAPE_RECT, SHAPE_RECT]);
        main.extend_from_slice(&0f64.to_le_bytes()); // rotation
        let mut ext = vec![0u8; EXT_CORNERS + EXT_STACK];
        // Altium writes the rounded rectangle here, not in the base record.
        ext[EXT_ALT_SHAPES..EXT_ALT_SHAPES + EXT_STACK].fill(SHAPE_ROUNDED_RECT);
        ext[EXT_CORNERS..EXT_CORNERS + EXT_STACK].fill(50); // 50% of a stadium

        let r = Raw {
            mode: Mode::Binary,
            offset: 0,
            kind: Some(2),
            payload: vec![1, b'1'],
            extra: vec![vec![0], vec![0, 0, 0, 0, 0], vec![0], main, ext],
        };
        let p = read_pad(&r).expect("pad");
        assert_eq!(p.name, "1");
        assert_eq!(p.shape, SHAPE_ROUNDED_RECT, "the base record says plain rectangle");
        assert!((p.w - 0.1).abs() < 1e-4, "the size still comes from the base record");
        assert!((p.corner_ratio - 0.25).abs() < 1e-9, "50% is half a stadium");
    }

    /// Corner case 7: the placed designator is the one the file marks as the
    /// designator — it carries the channel suffix, and it survives a
    /// re-annotation the base designator did not follow.
    #[test]
    fn designators_come_from_the_text_object_the_file_marks() {
        let mut pcb = PcbDoc::default();
        pcb.components.push(Component { designator: "U6".into(), ..Component::default() });
        pcb.components.push(Component { designator: "U6".into(), ..Component::default() });
        pcb.components.push(Component { designator: "Rd1".into(), ..Component::default() });
        let text = |i: u16, s: &str, is_designator: bool| Text {
            c: Common { layer: 33, net: None, polygon: None, component: Some(i) },
            x: 0.0,
            y: 0.0,
            height: 1.0,
            width: 0.0,
            rotation: 0.0,
            mirror: false,
            font: String::new(),
            text: s.to_string(),
            is_designator,
            is_comment: false,
        };
        pcb.texts.push(text(0, "U6_CH1", true));
        pcb.texts.push(text(1, "U6_CH2", true));
        pcb.texts.push(text(1, "GND", false)); // free silkscreen text on the same part
        // Re-annotation: the placed designator is not even a suffix of the base.
        pcb.texts.push(text(2, "RD1", true));
        assert_eq!(designators(&pcb), vec!["U6_CH1", "U6_CH2", "RD1"]);
    }
}
