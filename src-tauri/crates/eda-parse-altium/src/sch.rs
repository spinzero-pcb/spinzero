//! The `.SchDoc` object model.
//!
//! Faithful to the file: objects are a flat list and ownership is by index, so a
//! component is its `RECORD=1` plus every later record whose `OwnerIndex` points
//! back at it. `OwnerIndex` counts records *after* the file-header record, so
//! owner `n` is record `n + 1` in the stream.
//!
//! Coordinates stay in Altium's own space here — integer, Y-up, origin
//! bottom-left — scaled so the optional `*_Frac` companion is exact. Millimetres
//! and the bundle's Y-down orientation are the caller's job.

use std::collections::{BTreeMap, BTreeSet};

use crate::doc::Doc;
use crate::record::TextRecord;
use crate::units;

/// One schematic coordinate unit is 10 mil; `*_Frac` splits it into 100000.
/// Positions are stored in those hundred-thousandths so nothing is lost.
pub const UNIT: i64 = 100_000;

/// Sheet-entry `DistanceFromTop` is in grid steps, not coordinate units — one
/// step is 10 units. Verified against the corpus: with this factor every sheet
/// entry lands exactly on a wire endpoint, without it almost none do.
pub const ENTRY_STEP: i64 = 10;

/// A point in Altium schematic space (Y-up), in hundred-thousandths of a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Pt {
    pub x: i64,
    pub y: i64,
}

impl Pt {
    pub fn mm(self) -> (f64, f64) {
        (units::sch_mm(0, self.x), units::sch_mm(0, self.y))
    }
}

/// BOM inclusion is decided by `ComponentKind`, Altium's analogue of KiCad's
/// `in_bom` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComponentKind {
    #[default]
    Standard,
    Mechanical,
    Graphical,
    NetTieBom,
    NetTieNoBom,
    StandardNoBom,
    Jumper,
}

impl ComponentKind {
    fn from_i(v: i64) -> ComponentKind {
        match v {
            1 => ComponentKind::Mechanical,
            2 => ComponentKind::Graphical,
            3 => ComponentKind::NetTieBom,
            4 => ComponentKind::NetTieNoBom,
            5 => ComponentKind::StandardNoBom,
            6 => ComponentKind::Jumper,
            _ => ComponentKind::Standard,
        }
    }

    /// Whether a part of this kind belongs in the BOM.
    pub fn in_bom(self) -> bool {
        !matches!(
            self,
            ComponentKind::Graphical | ComponentKind::NetTieNoBom | ComponentKind::StandardNoBom
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::Standard => "standard",
            ComponentKind::Mechanical => "mechanical",
            ComponentKind::Graphical => "graphical",
            ComponentKind::NetTieBom => "net_tie_bom",
            ComponentKind::NetTieNoBom => "net_tie_no_bom",
            ComponentKind::StandardNoBom => "standard_no_bom",
            ComponentKind::Jumper => "jumper",
        }
    }
}

/// A component parameter (`RECORD=41`).
#[derive(Debug, Clone, Default)]
pub struct Param {
    pub name: String,
    pub text: String,
    pub hidden: bool,
    pub uuid: String,
    /// Where Altium draws it. A parameter with no `Location` is data only.
    pub at: Pt,
    pub color: String,
    pub font: i64,
    pub justify: i64,
    pub orientation: i64,
    pub mirrored: bool,
}

/// A pin (`RECORD=2`).
///
/// `at` is the record's `Location`, which is the end that touches the symbol
/// BODY — not the connection point. The wire meets the far end, `at` stepped
/// forward by `length` along the rotation. Verified on the corpus: 292 of 324
/// pins on one sheet land on a wire that way and none do from `Location`.
#[derive(Debug, Clone, Default)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub description: String,
    /// Altium's `Electrical` code — see [`pin_type`].
    pub electrical: i64,
    pub conglomerate: i64,
    pub length: i64,
    pub at: Pt,
    pub part_id: i64,
    pub display_mode: i64,
    pub uuid: String,
    /// Net a hidden pin connects to implicitly, when the file names one.
    pub hidden_net_name: String,
    /// Altium's `SymBol_OuterEdge` (1 = the active-low dot) and `SymBol_Inner`
    /// (9 = the clock wedge) pin decorations.
    pub outer_edge: i64,
    pub inner: i64,
    pub color: String,
    /// Font the designer overrode the name and the number with, or 0 for the
    /// sheet's own default.
    pub name_font: i64,
    pub number_font: i64,
}

/// `PinConglomerate` is a bit field: the low two bits are the rotation and the
/// rest are flags.
///
/// The corpus settles which bit is which by how often each is set across its
/// 13480 pins. `0x20` is set on 13450 of them — "not accessible", the flag
/// Altium stamps on library-owned primitives, which appears as
/// `IsNotAccesible` on every graphic record too. `0x04` is set on 66, which is
/// what a design's hidden power pins look like. `0x08` is set on 2232 — a sixth
/// of every pin in the corpus — which is a display flag, not a hidden one.
///
/// The first pass read `0x08` as hidden. That silently made 2232 pins implicit
/// supply connections; only the narrow supply-name rule in `altium::netlist`
/// kept it from re-wiring the netlist outright.
pub const PIN_HIDDEN_BIT: i64 = 0x04;

/// Bit that shows a pin's name beside the stub.
pub const PIN_SHOW_NAME_BIT: i64 = 0x08;

/// Bit that shows a pin's designator (its number) beside the stub.
pub const PIN_SHOW_DESIGNATOR_BIT: i64 = 0x10;

impl Pin {
    /// Rotation in degrees counter-clockwise, from the low two conglomerate bits.
    pub fn rotation(&self) -> i64 {
        (self.conglomerate & 0x3) * 90
    }

    /// Hidden pins are implicitly connected rather than drawn.
    pub fn hidden(&self) -> bool {
        self.conglomerate & PIN_HIDDEN_BIT != 0
    }

    /// Whether Altium draws the pin's name beside its stub.
    pub fn show_name(&self) -> bool {
        self.conglomerate & PIN_SHOW_NAME_BIT != 0
    }

    /// Whether Altium draws the pin's designator beside its stub.
    pub fn show_designator(&self) -> bool {
        self.conglomerate & PIN_SHOW_DESIGNATOR_BIT != 0
    }

    /// The free end of the pin — where a wire connects.
    pub fn connection(&self) -> Pt {
        let l = self.length * UNIT;
        match self.conglomerate & 0x3 {
            0 => Pt { x: self.at.x + l, y: self.at.y },
            1 => Pt { x: self.at.x, y: self.at.y + l },
            2 => Pt { x: self.at.x - l, y: self.at.y },
            _ => Pt { x: self.at.x, y: self.at.y - l },
        }
    }
}

/// A placed component (`RECORD=1`) with its owned children.
#[derive(Debug, Clone, Default)]
pub struct Component {
    pub library_ref: String,
    pub description: String,
    /// Text of the `RECORD=34` designator child.
    pub designator: String,
    pub designator_uuid: String,
    pub part_count: i64,
    pub current_part_id: i64,
    pub display_mode: i64,
    pub kind: ComponentKind,
    pub at: Pt,
    pub uuid: String,
    pub source_library: String,
    pub database_table: String,
    /// `ModelName` of the current `PCBLIB` implementation.
    pub footprint: String,
    pub pins: Vec<Pin>,
    pub parameters: Vec<Param>,
    /// Extent of the component's graphic children, when it has any.
    pub bbox: Option<(Pt, Pt)>,
    /// The symbol's own artwork, already in sheet coordinates.
    pub graphics: Vec<Graphic>,
    /// 0/1/2/3 = 0/90/180/270 degrees counter-clockwise. Altium has already
    /// applied this to every child's coordinates; it is kept because text is
    /// drawn along it.
    pub orientation: i64,
    pub mirrored: bool,
    /// Where the designator is drawn, and whether it is drawn at all.
    pub designator_at: Pt,
    pub designator_hidden: bool,
    pub designator_color: String,
    pub designator_font: i64,
    pub designator_orientation: i64,
}

impl Component {
    /// A parameter's text, matched case-insensitively.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.text.as_str())
    }
}

/// A net label (`RECORD=25`).
#[derive(Debug, Clone, Default)]
pub struct NetLabel {
    pub at: Pt,
    pub text: String,
    pub uuid: String,
    pub color: String,
    pub font: i64,
    pub orientation: i64,
}

/// A power port (`RECORD=17`) — a net namer, not a component.
#[derive(Debug, Clone, Default)]
pub struct PowerPort {
    pub at: Pt,
    pub text: String,
    pub style: i64,
    pub uuid: String,
    pub color: String,
    pub font: i64,
    pub orientation: i64,
    pub show_net_name: bool,
}

/// A port (`RECORD=18`). It has TWO connection points, its left and right edges.
#[derive(Debug, Clone, Default)]
pub struct Port {
    pub at: Pt,
    pub width: i64,
    pub name: String,
    pub io_type: i64,
    pub uuid: String,
    /// Body height in coordinate units; Altium's default is 10 (one grid step).
    pub height: i64,
    /// Altium's port `Style` — which end shapes the body draws.
    pub style: i64,
    /// 1 = left, 2 = right, 3 = centre.
    pub alignment: i64,
    pub color: String,
    pub text_color: String,
    pub area_color: String,
    pub font: i64,
}

impl Port {
    /// Both edges. Taking only `at` misses half of a design's port connections.
    pub fn terminals(&self) -> [Pt; 2] {
        [self.at, Pt { x: self.at.x + self.width * UNIT, y: self.at.y }]
    }
}

/// A sheet entry (`RECORD=16`) on a sheet symbol. It stores no connection point;
/// one is computed from the parent's rectangle, `Side` and `DistanceFromTop`.
#[derive(Debug, Clone, Default)]
pub struct SheetEntry {
    pub name: String,
    pub side: i64,
    pub distance_from_top: i64,
    pub io_type: i64,
    pub uuid: String,
    pub style: i64,
    pub arrow_kind: String,
    pub color: String,
    pub text_color: String,
    pub text_font: i64,
}

/// A sheet symbol (`RECORD=15`) — one placement of a child document.
#[derive(Debug, Clone, Default)]
pub struct SheetSymbol {
    /// Top-left corner, Y-up.
    pub at: Pt,
    pub xsize: i64,
    pub ysize: i64,
    pub uuid: String,
    /// `RECORD=32` display name.
    pub name: String,
    /// `RECORD=33` child document file name.
    pub filename: String,
    pub entries: Vec<SheetEntry>,
    pub color: String,
    pub area_color: String,
    pub solid: bool,
    /// Where the `RECORD=32` name and `RECORD=33` file name are drawn.
    pub name_at: Pt,
    pub filename_at: Pt,
    pub name_color: String,
    pub name_font: i64,
}

impl SheetSymbol {
    /// Connection point of one of this symbol's entries.
    pub fn entry_point(&self, e: &SheetEntry) -> Pt {
        let d = e.distance_from_top * ENTRY_STEP * UNIT;
        match e.side {
            0 => Pt { x: self.at.x, y: self.at.y - d },
            1 => Pt { x: self.at.x + self.xsize * UNIT, y: self.at.y - d },
            2 => Pt { x: self.at.x + d, y: self.at.y },
            _ => Pt { x: self.at.x + d, y: self.at.y - self.ysize * UNIT },
        }
    }
}

/// A wire or bus polyline (`RECORD=27` / `RECORD=26`).
#[derive(Debug, Clone, Default)]
pub struct Wire {
    pub pts: Vec<Pt>,
    pub uuid: String,
    pub color: String,
    /// Altium's `LineWidth` enum — see [`crate::units::sch_line_width_mm`].
    pub width: i64,
}

/// A junction (`RECORD=29`) — makes a mid-span crossing a connection.
#[derive(Debug, Clone, Default)]
pub struct Junction {
    pub at: Pt,
    pub uuid: String,
    pub color: String,
}

/// A parameter set (`RECORD=43`) — a directive placed on a net, e.g. a net class.
#[derive(Debug, Clone)]
pub struct ParamSet {
    pub at: Pt,
    pub name: String,
    pub uuid: String,
    pub parameters: Vec<Param>,
}

/// A rectangular region (`RECORD=211`). See `altium::netlist` for why these are
/// reported rather than applied.
#[derive(Debug, Clone)]
pub struct Region {
    pub min: Pt,
    pub max: Pt,
    pub uuid: String,
}

/// The shape of a drawing primitive, in Altium schematic space.
///
/// Every variant's coordinates are absolute sheet coordinates: Altium stores a
/// symbol's graphics already placed, not in a symbol-local frame, which is why
/// the netlist can compare a pin's location with a wire's endpoint directly and
/// why the renderer needs no per-component transform.
#[derive(Debug, Clone)]
pub enum GShape {
    /// `RECORD=13`.
    Line { a: Pt, b: Pt },
    /// `RECORD=6`. `start_shape`/`end_shape` are Altium's line-end decorations
    /// (arrow, tail, circle, square), sized by `shape_size`.
    Polyline { pts: Vec<Pt>, start_shape: i64, end_shape: i64, shape_size: i64 },
    /// `RECORD=7`.
    Polygon { pts: Vec<Pt> },
    /// `RECORD=14`.
    Rect { min: Pt, max: Pt },
    /// `RECORD=8`. A circle is the case `rx == ry`.
    Ellipse { c: Pt, rx: i64, ry: i64 },
    /// `RECORD=12` (circular, `rx == ry`) and `RECORD=11` (elliptical). Angles
    /// are degrees counter-clockwise from +X, in Altium's Y-up space.
    Arc { c: Pt, rx: i64, ry: i64, start_deg: f64, end_deg: f64 },
}

/// A drawing primitive: a shape plus how Altium paints it.
///
/// `owner` says what the primitive belongs to, which is what decides whether it
/// is part of a symbol, part of the drawing sheet's frame, or free sheet
/// artwork — the three read very differently in a review.
#[derive(Debug, Clone)]
pub struct Graphic {
    pub shape: GShape,
    /// `#RRGGBB` of the pen, already un-BGR'd.
    pub color: String,
    /// Fill colour when the object is solid; `None` for an outline.
    pub fill: Option<String>,
    /// Altium's `LineWidth` enum (0 smallest … 3 large) — see
    /// [`crate::units::sch_line_width_mm`].
    pub width: i64,
    /// Altium's `LineStyle` enum (0 solid, 1 dashed, 2 dotted, 3 dash-dot).
    pub style: i64,
    /// Which part of a multi-part symbol draws this (`OwnerPartId`).
    pub part_id: i64,
    /// Which display mode draws this (`OwnerPartDisplayMode`).
    pub display_mode: i64,
    pub uuid: String,
}

/// What a piece of free-standing text on the sheet is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// `RECORD=4` — a single-line label.
    Label,
    /// `RECORD=28` — a wrapped text frame.
    Frame,
    /// `RECORD=209` — a design note.
    Note,
}

/// Text drawn on the sheet, and not owned by a component.
///
/// Altium's `=Name` special strings (`=SheetNumber`, `=Title`, …) are left
/// verbatim here; resolving them needs the project and the sheet's own
/// parameters, which is the extractor's job, not the parser's.
#[derive(Debug, Clone)]
pub struct SchText {
    pub kind: TextKind,
    pub at: Pt,
    /// Bottom-right corner for a frame or note; `None` for a label.
    pub corner: Option<Pt>,
    pub text: String,
    pub color: String,
    /// Background fill for a solid frame/note.
    pub fill: Option<String>,
    pub font: i64,
    /// Altium's `Justification` (0 bottom-left … 8 top-right); frames and notes
    /// use `Alignment` (1 left, 2 centre, 3 right) which is folded in here.
    pub justify: i64,
    /// 0/1/2/3 = 0/90/180/270 degrees counter-clockwise.
    pub orientation: i64,
    pub mirrored: bool,
    pub show_border: bool,
    /// True when the text belongs to the drawing sheet's template rather than to
    /// the design — Altium's analogue of KiCad's worksheet.
    pub template: bool,
    pub uuid: String,
}

/// A "no ERC" marker (`RECORD=22`) — the designer asserting a pin is
/// deliberately unconnected.
#[derive(Debug, Clone, Default)]
pub struct NoErc {
    pub at: Pt,
    pub color: String,
    /// Altium's marker shape enum; 0 is the plain cross.
    pub symbol: i64,
    pub uuid: String,
}

/// A placed image (`RECORD=30`). The bytes live in the document's `Storage`
/// stream when `embedded` is set; `file_name` is the key into it.
#[derive(Debug, Clone, Default)]
pub struct SchImage {
    pub min: Pt,
    pub max: Pt,
    pub file_name: String,
    pub embedded: bool,
    /// PNG/JPEG bytes resolved from the `Storage` stream, when they were found.
    pub data: Vec<u8>,
    /// Part of the drawing sheet's template rather than of the design.
    pub template: bool,
    pub uuid: String,
}

/// Sheet properties (`RECORD=31`).
#[derive(Debug, Clone, Default)]
pub struct SheetProps {
    /// Altium's own snap tolerance, in coordinate units.
    pub hot_spot_grid: i64,
    pub width: i64,
    pub height: i64,
    pub area_color: String,
    pub fonts: Vec<String>,
    /// Point size of each entry in `fonts`, parallel to it.
    pub font_sizes: Vec<i64>,
    pub font_bold: Vec<bool>,
    pub font_italic: Vec<bool>,
    /// Altium draws its own page border unless the sheet turns it off.
    pub border_on: bool,
    /// The zone ruler round the border: columns, rows, and the band's width in
    /// coordinate units. A reviewer cites these ("C3"), so they are the
    /// design's own numbers, not a drawing convention.
    pub zones_x: i64,
    pub zones_y: i64,
    pub margin: i64,
    /// The template file the drawing sheet came from, when the sheet names one.
    pub template_file: String,
}

/// One parsed `.SchDoc`.
#[derive(Debug, Clone, Default)]
pub struct SchDoc {
    pub uuid: String,
    pub sheet: SheetProps,
    pub components: Vec<Component>,
    pub wires: Vec<Wire>,
    pub buses: Vec<Wire>,
    pub junctions: Vec<Junction>,
    pub net_labels: Vec<NetLabel>,
    pub power_ports: Vec<PowerPort>,
    pub ports: Vec<Port>,
    pub sheet_symbols: Vec<SheetSymbol>,
    pub param_sets: Vec<ParamSet>,
    pub regions: Vec<Region>,
    /// Sheet-level parameters (`RECORD=41` with no owner): title, revision, …
    pub parameters: Vec<Param>,
    /// Free text on the sheet (`RECORD=4` labels and `RECORD=209` notes).
    pub notes: Vec<String>,
    /// Sheet-level artwork: the free graphics a designer drew, plus the drawing
    /// sheet's own frame (`template`).
    pub graphics: Vec<Graphic>,
    pub template_graphics: Vec<Graphic>,
    /// Every placed text object, design and template alike.
    pub texts: Vec<SchText>,
    pub no_ercs: Vec<NoErc>,
    pub images: Vec<SchImage>,
    /// Blankets (`RECORD=225`, in the `Additional` stream) — the closed regions
    /// a designer draws to exclude part of a sheet from compilation. They are
    /// drawn and counted, never applied: see `altium::netlist`.
    pub blankets: Vec<Graphic>,
    /// Record types seen but not modelled, by type and count. Feeds the
    /// extraction diagnostics — a review that silently drops something is worse
    /// than one that says it did.
    pub skipped: BTreeMap<i64, usize>,
}

/// Coordinate of a record's `<key>` plus its optional `<key>_Frac` companion.
fn coord(r: &TextRecord, key: &str) -> i64 {
    r.i(key).unwrap_or(0) * UNIT + r.i(&format!("{key}_Frac")).unwrap_or(0)
}

fn pt(r: &TextRecord, xkey: &str, ykey: &str) -> Pt {
    Pt { x: coord(r, xkey), y: coord(r, ykey) }
}

/// Polyline points from `LocationCount` plus `X1..Xn` / `Y1..Yn`.
///
/// **A key Altium omits means zero, not "absent".** A wire ending on the sheet's
/// left edge is written `X1=20|Y1=740|Y2=740` with no `X2` at all, and requiring
/// both keys drops that endpoint: the wire becomes a single point, disappears
/// from the render, and loses its connection at that end in the netlist. A point
/// is therefore taken whenever EITHER coordinate is written; only an index with
/// neither is missing.
fn polyline(r: &TextRecord) -> Vec<Pt> {
    let n = r.i("LocationCount").unwrap_or(0).max(0);
    (1..=n)
        .filter(|i| r.has(&format!("X{i}")) || r.has(&format!("Y{i}")))
        .map(|i| pt(r, &format!("X{i}"), &format!("Y{i}")))
        .collect()
}

/// A record's pen colour, defaulting to Altium's black when the key is absent.
fn color(r: &TextRecord) -> String {
    units::bgr_hex(r.i("Color").unwrap_or(0))
}

/// A record's fill colour, present only when the object is actually solid.
/// Altium keeps `AreaColor` on outline-only objects too, so reading it without
/// checking `IsSolid` fills every rectangle on the sheet.
fn fill(r: &TextRecord) -> Option<String> {
    r.b("IsSolid").then(|| units::bgr_hex(r.i("AreaColor").unwrap_or(0xFF_FF_FF)))
}

/// The shape of a graphic record, or `None` for a type this does not draw.
fn gshape(r: &TextRecord, t: i64) -> Option<GShape> {
    let loc = pt(r, "Location.X", "Location.Y");
    let corner = pt(r, "Corner.X", "Corner.Y");
    // `Radius`/`SecondaryRadius` carry a `_Frac` companion like any coordinate.
    let radius = |k: &str| coord(r, k);
    Some(match t {
        6 => GShape::Polyline {
            pts: polyline(r),
            start_shape: r.i("StartLineShape").unwrap_or(0),
            end_shape: r.i("EndLineShape").unwrap_or(0),
            shape_size: r.i("LineShapeSize").unwrap_or(0),
        },
        7 => GShape::Polygon { pts: polyline(r) },
        8 => GShape::Ellipse {
            c: loc,
            rx: radius("Radius"),
            ry: if r.has("SecondaryRadius") { radius("SecondaryRadius") } else { radius("Radius") },
        },
        11 | 12 => GShape::Arc {
            c: loc,
            rx: radius("Radius"),
            ry: if r.has("SecondaryRadius") { radius("SecondaryRadius") } else { radius("Radius") },
            start_deg: r.f("StartAngle").unwrap_or(0.0),
            end_deg: r.f("EndAngle").unwrap_or(360.0),
        },
        13 => GShape::Line { a: loc, b: corner },
        14 => GShape::Rect { min: loc, max: corner },
        _ => return None,
    })
}

/// Build the drawing primitive for a graphic record.
fn graphic(r: &TextRecord, t: i64) -> Option<Graphic> {
    Some(Graphic {
        shape: gshape(r, t)?,
        color: color(r),
        fill: fill(r),
        width: r.i("LineWidth").unwrap_or(0),
        style: r.i("LineStyle").unwrap_or(0),
        part_id: r.i("OwnerPartId").unwrap_or(-1),
        display_mode: r.i("OwnerPartDisplayMode").unwrap_or(0),
        uuid: r.s("UniqueID").to_string(),
    })
}

/// Every point a shape touches, for the bounding boxes the design model keeps.
fn shape_points(g: &GShape) -> Vec<Pt> {
    match g {
        GShape::Line { a, b } => vec![*a, *b],
        GShape::Polyline { pts, .. } | GShape::Polygon { pts } => pts.clone(),
        GShape::Rect { min, max } => vec![*min, *max],
        // An arc is bounded by its full ellipse: cheap, never too small, and the
        // bbox feeds placement and hit-testing, not a fabrication output.
        GShape::Ellipse { c, rx, ry } | GShape::Arc { c, rx, ry, .. } => vec![
            Pt { x: c.x - rx, y: c.y - ry },
            Pt { x: c.x + rx, y: c.y + ry },
        ],
    }
}

/// `ComponentKind` with its versioned fallbacks.
///
/// A file written by a newer Altium stores `Standard` in the base field and the
/// real kind in `ComponentKindVersion3`, so reading only the base field puts
/// graphical decorations and no-BOM parts into the BOM.
fn component_kind(r: &TextRecord) -> ComponentKind {
    for key in ["ComponentKindVersion3", "ComponentKindVersion2", "ComponentKind"] {
        if let Some(v) = r.i(key) {
            return ComponentKind::from_i(v);
        }
    }
    ComponentKind::Standard
}

/// Record types that carry component/symbol graphics. They contribute to the
/// component's bbox and are otherwise M3's problem.
const GRAPHIC_RECORDS: [i64; 7] = [6, 7, 8, 11, 12, 13, 14];

/// Parse one `.SchDoc`.
pub fn parse(doc: &Doc) -> SchDoc {
    let raw = doc.records("FileHeader");
    let mut out = parse_records(
        raw.iter()
            .map(|r| TextRecord::parse(&r.payload))
            .collect(),
    );
    // Compile masks are NOT in `FileHeader`. A blanket is `RECORD=225` in the
    // document's `Additional` stream, which is why a sheet whose histogram shows
    // no region record can still have three of them drawn on it.
    out.blankets = parse_blankets(&doc.text_records("Additional"));
    // An image record names its source path; the bytes are in `Storage` under
    // that same path. A logo whose blob is missing keeps its record — the
    // renderer draws the frame it occupies rather than silently losing the box.
    if out.images.iter().any(|i| i.embedded) {
        let store = doc.storage();
        for img in &mut out.images {
            if let Some(bytes) = store.get(&img.file_name) {
                img.data = bytes.clone();
            }
        }
    }
    out
}

/// Blankets from the `Additional` stream. Each is a closed polyline, which is
/// how a non-rectangular mask keeps its shape.
pub fn parse_blankets(recs: &[TextRecord]) -> Vec<Graphic> {
    recs.iter()
        .filter(|r| r.record_type() == Some(225))
        .filter_map(|r| {
            let pts = polyline(r);
            (pts.len() >= 3).then(|| Graphic {
                shape: GShape::Polygon { pts },
                color: color(r),
                fill: None,
                width: r.i("LineWidth").unwrap_or(0),
                style: r.i("LineStyle").unwrap_or(1),
                part_id: -1,
                display_mode: 0,
                uuid: r.s("UniqueID").to_string(),
            })
        })
        .collect()
}

/// Build the document from its already-decoded `FileHeader` records.
///
/// Split out from [`parse`] so a record-level rule can be tested without
/// building a compound file around it.
pub fn parse_records(recs: Vec<TextRecord>) -> SchDoc {
    let mut out = SchDoc::default();
    if let Some(h) = recs.first() {
        out.uuid = h.s("UniqueID").to_string();
    }

    // `OwnerIndex` is 0-based over the records after the header, so owner n is
    // recs[n + 1].
    let owner_of = |r: &TextRecord| r.i("OwnerIndex").map(|n| (n + 1) as usize);

    // Pass 1: the objects that own others, indexed by their own record position.
    let mut comp_at: BTreeMap<usize, usize> = BTreeMap::new(); // rec index -> components[]
    let mut sym_at: BTreeMap<usize, usize> = BTreeMap::new(); // rec index -> sheet_symbols[]
    let mut set_at: BTreeMap<usize, usize> = BTreeMap::new(); // rec index -> param_sets[]
    // Implementation lists (RECORD=44) own the implementations (45); map the
    // list back to its component so a footprint reaches the right part.
    let mut impl_owner: BTreeMap<usize, usize> = BTreeMap::new(); // rec index of 44 -> rec index of 1
    // Components whose footprint came from the implementation Altium marks
    // current, so a later PCBLIB record cannot displace it.
    let mut footprint_is_current: BTreeSet<usize> = BTreeSet::new();
    let mut template_at: Option<usize> = None;
    for (i, r) in recs.iter().enumerate() {
        match r.record_type() {
            Some(1) => {
                comp_at.insert(i, out.components.len());
                out.components.push(Component {
                    library_ref: r.s("LibReference").to_string(),
                    description: r.s("ComponentDescription").to_string(),
                    designator: String::new(),
                    designator_uuid: String::new(),
                    part_count: r.i("PartCount").unwrap_or(1),
                    current_part_id: r.i("CurrentPartId").unwrap_or(1),
                    display_mode: r.i("DisplayMode").unwrap_or(0),
                    kind: component_kind(r),
                    at: pt(r, "Location.X", "Location.Y"),
                    uuid: r.s("UniqueID").to_string(),
                    source_library: r.s("SourceLibraryName").to_string(),
                    database_table: r.s("DatabaseTableName").to_string(),
                    footprint: String::new(),
                    pins: Vec::new(),
                    parameters: Vec::new(),
                    bbox: None,
                    graphics: Vec::new(),
                    orientation: r.i("Orientation").unwrap_or(0),
                    mirrored: r.b("IsMirrored"),
                    designator_at: Pt::default(),
                    designator_hidden: false,
                    designator_color: String::new(),
                    designator_font: 1,
                    designator_orientation: 0,
                });
            }
            Some(15) => {
                sym_at.insert(i, out.sheet_symbols.len());
                out.sheet_symbols.push(SheetSymbol {
                    at: pt(r, "Location.X", "Location.Y"),
                    xsize: r.i("XSize").unwrap_or(0),
                    ysize: r.i("YSize").unwrap_or(0),
                    uuid: r.s("UniqueID").to_string(),
                    name: String::new(),
                    filename: String::new(),
                    entries: Vec::new(),
                    color: color(r),
                    area_color: units::bgr_hex(r.i("AreaColor").unwrap_or(0xFF_FF_FF)),
                    solid: r.b("IsSolid"),
                    name_at: Pt::default(),
                    filename_at: Pt::default(),
                    name_color: String::new(),
                    name_font: 1,
                });
            }
            Some(43) => {
                set_at.insert(i, out.param_sets.len());
                out.param_sets.push(ParamSet {
                    at: pt(r, "Location.X", "Location.Y"),
                    name: r.s("Name").to_string(),
                    uuid: r.s("UniqueID").to_string(),
                    parameters: Vec::new(),
                });
            }
            Some(44) => {
                if let Some(o) = owner_of(r) {
                    impl_owner.insert(i, o);
                }
            }
            // The drawing sheet's template owns the frame and title block, the
            // way KiCad's worksheet does. Its children are recorded separately
            // so the renderer can put the page behind the design instead of
            // treating a title-block rule as a designer's annotation.
            Some(39) => {
                template_at = Some(i);
                out.sheet.template_file = r.s("FileName").to_string();
            }
            _ => {}
        }
    }

    // Pass 2: everything else, attached to its owner where it has one.
    for r in recs.iter() {
        let Some(t) = r.record_type() else {
            continue;
        };
        let owner = owner_of(r);
        match t {
            1 | 15 | 39 | 43 | 44 => {}
            2 => {
                let Some(c) = owner.and_then(|o| comp_at.get(&o)).copied() else {
                    *out.skipped.entry(t).or_default() += 1;
                    continue;
                };
                out.components[c].pins.push(Pin {
                    number: r.s("Designator").to_string(),
                    name: r.s("Name").to_string(),
                    description: r.s("Description").to_string(),
                    electrical: r.i("Electrical").unwrap_or(4),
                    conglomerate: r.i("PinConglomerate").unwrap_or(0),
                    length: r.i("PinLength").unwrap_or(0),
                    at: pt(r, "Location.X", "Location.Y"),
                    part_id: r.i("OwnerPartId").unwrap_or(1),
                    display_mode: r.i("OwnerPartDisplayMode").unwrap_or(0),
                    uuid: r.s("UniqueID").to_string(),
                    hidden_net_name: r.s("HiddenNetName").to_string(),
                    outer_edge: r.i("SymBol_OuterEdge").unwrap_or(0),
                    inner: r.i("SymBol_Inner").unwrap_or(0),
                    color: color(r),
                    name_font: r.i("Name_CustomFontID").unwrap_or(0),
                    number_font: r.i("Designator_CustomFontID").unwrap_or(0),
                });
            }
            34 => {
                if let Some(c) = owner.and_then(|o| comp_at.get(&o)).copied() {
                    let comp = &mut out.components[c];
                    comp.designator = r.s("Text").to_string();
                    comp.designator_uuid = r.s("UniqueID").to_string();
                    comp.designator_at = pt(r, "Location.X", "Location.Y");
                    comp.designator_hidden = r.b("IsHidden");
                    comp.designator_color = color(r);
                    comp.designator_font = r.i("FontID").unwrap_or(1);
                    comp.designator_orientation = r.i("Orientation").unwrap_or(0);
                }
            }
            41 => {
                let p = Param {
                    name: r.s("Name").to_string(),
                    text: r.s("Text").to_string(),
                    hidden: r.b("IsHidden"),
                    uuid: r.s("UniqueID").to_string(),
                    at: pt(r, "Location.X", "Location.Y"),
                    color: color(r),
                    font: r.i("FontID").unwrap_or(1),
                    justify: r.i("Justification").unwrap_or(0),
                    orientation: r.i("Orientation").unwrap_or(0),
                    mirrored: r.b("IsMirrored"),
                };
                match owner {
                    Some(o) if comp_at.contains_key(&o) => {
                        out.components[comp_at[&o]].parameters.push(p)
                    }
                    Some(o) if set_at.contains_key(&o) => {
                        out.param_sets[set_at[&o]].parameters.push(p)
                    }
                    _ => out.parameters.push(p),
                }
            }
            45 => {
                // The current PCBLIB implementation gives the footprint. Altium
                // writes several models per part (PCBLIB, SI, simulation); only
                // the PCB one is a footprint.
                if !r.s("ModelType").eq_ignore_ascii_case("PCBLIB") {
                    continue;
                }
                let Some(c) = owner
                    .and_then(|o| impl_owner.get(&o))
                    .and_then(|o| comp_at.get(o))
                    .copied()
                else {
                    continue;
                };
                // A part carries SEVERAL PCBLIB models and Altium places the
                // one flagged current; taking the first gives a footprint that
                // is plausible, in the right family, and wrong — U2 on the EVAL
                // design is `…-16N-4` first and `…-16N-V` current. Found by the
                // §8.3 differential. An unflagged list keeps the first.
                let current = r.b("IsCurrent");
                if current && !footprint_is_current.contains(&c) {
                    out.components[c].footprint = r.s("ModelName").to_string();
                    footprint_is_current.insert(c);
                } else if out.components[c].footprint.is_empty() {
                    out.components[c].footprint = r.s("ModelName").to_string();
                }
            }
            16 => {
                if let Some(s) = owner.and_then(|o| sym_at.get(&o)).copied() {
                    out.sheet_symbols[s].entries.push(SheetEntry {
                        name: r.s("Name").to_string(),
                        side: r.i("Side").unwrap_or(0),
                        distance_from_top: r.i("DistanceFromTop").unwrap_or(0),
                        io_type: r.i("IOType").unwrap_or(0),
                        uuid: r.s("UniqueID").to_string(),
                        style: r.i("Style").unwrap_or(0),
                        arrow_kind: r.s("ArrowKind").to_string(),
                        color: color(r),
                        text_color: units::bgr_hex(r.i("TextColor").unwrap_or(0)),
                        text_font: r.i("TextFontID").unwrap_or(1),
                    });
                }
            }
            32 => {
                if let Some(s) = owner.and_then(|o| sym_at.get(&o)).copied() {
                    let sym = &mut out.sheet_symbols[s];
                    sym.name = r.s("Text").to_string();
                    sym.name_at = pt(r, "Location.X", "Location.Y");
                    sym.name_color = color(r);
                    sym.name_font = r.i("FontID").unwrap_or(1);
                }
            }
            33 => {
                if let Some(s) = owner.and_then(|o| sym_at.get(&o)).copied() {
                    out.sheet_symbols[s].filename = r.s("Text").to_string();
                    out.sheet_symbols[s].filename_at = pt(r, "Location.X", "Location.Y");
                }
            }
            17 => out.power_ports.push(PowerPort {
                at: pt(r, "Location.X", "Location.Y"),
                text: r.s("Text").to_string(),
                style: r.i("Style").unwrap_or(0),
                uuid: r.s("UniqueID").to_string(),
                color: color(r),
                font: r.i("FontID").unwrap_or(1),
                orientation: r.i("Orientation").unwrap_or(0),
                show_net_name: r.b("ShowNetName"),
            }),
            18 => out.ports.push(Port {
                at: pt(r, "Location.X", "Location.Y"),
                width: r.i("Width").unwrap_or(0),
                name: r.s("Name").to_string(),
                io_type: r.i("IOType").unwrap_or(0),
                uuid: r.s("UniqueID").to_string(),
                height: r.i("Height").unwrap_or(10),
                style: r.i("Style").unwrap_or(0),
                alignment: r.i("Alignment").unwrap_or(0),
                color: color(r),
                text_color: units::bgr_hex(r.i("TextColor").unwrap_or(0)),
                area_color: units::bgr_hex(r.i("AreaColor").unwrap_or(0xFF_FF_FF)),
                font: r.i("FontID").unwrap_or(1),
            }),
            25 => out.net_labels.push(NetLabel {
                at: pt(r, "Location.X", "Location.Y"),
                text: r.s("Text").to_string(),
                uuid: r.s("UniqueID").to_string(),
                color: color(r),
                font: r.i("FontID").unwrap_or(1),
                orientation: r.i("Orientation").unwrap_or(0),
            }),
            26 => out.buses.push(Wire {
                pts: polyline(r),
                uuid: r.s("UniqueID").to_string(),
                color: color(r),
                width: r.i("LineWidth").unwrap_or(1),
            }),
            27 => out.wires.push(Wire {
                pts: polyline(r),
                uuid: r.s("UniqueID").to_string(),
                color: color(r),
                width: r.i("LineWidth").unwrap_or(1),
            }),
            29 => out.junctions.push(Junction {
                at: pt(r, "Location.X", "Location.Y"),
                uuid: r.s("UniqueID").to_string(),
                color: color(r),
            }),
            22 => out.no_ercs.push(NoErc {
                at: pt(r, "Location.X", "Location.Y"),
                color: color(r),
                symbol: r.i("Symbol").unwrap_or(0),
                uuid: r.s("UniqueID").to_string(),
            }),
            30 => out.images.push(SchImage {
                min: pt(r, "Location.X", "Location.Y"),
                max: pt(r, "Corner.X", "Corner.Y"),
                file_name: r.s("FileName").to_string(),
                embedded: r.b("EmbedImage"),
                data: Vec::new(),
                template: owner.is_some() && owner == template_at,
                uuid: r.s("UniqueID").to_string(),
            }),
            31 => {
                let custom = r.b("UseCustomSheet");
                let n_fonts = r.i("FontIdCount").unwrap_or(0);
                // An absent `SheetStyle` is Altium's factory default, which is B
                // and NOT A4 — see `units::DEFAULT_SHEET_STYLE`.
                let style = r.i("SheetStyle").unwrap_or(units::DEFAULT_SHEET_STYLE);
                let (zx, zy, margin) = units::sheet_zones(style);
                // Pass 1 recorded the template file from RECORD=39; the sheet
                // record rebuilds SheetProps wholesale, so carry it across.
                let template_file = std::mem::take(&mut out.sheet.template_file);
                let (w, h) = units::sheet_style_mm(style);
                out.sheet = SheetProps {
                    hot_spot_grid: r.i("HotSpotGridSize").unwrap_or(0) * UNIT,
                    width: if custom {
                        r.i("CustomX").unwrap_or(0) * UNIT
                    } else {
                        (w / 0.254).round() as i64 * UNIT
                    },
                    height: if custom {
                        r.i("CustomY").unwrap_or(0) * UNIT
                    } else {
                        (h / 0.254).round() as i64 * UNIT
                    },
                    area_color: units::bgr_hex(r.i("AreaColor").unwrap_or(0)),
                    fonts: (1..=n_fonts)
                        .map(|i| r.s(&format!("FontName{i}")).to_string())
                        .collect(),
                    // Altium's font sizes are points, and a point is 1/72 inch
                    // against a sheet whose unit is 1/100 inch — the renderer
                    // converts; the table stays the file's own numbers.
                    font_sizes: (1..=n_fonts).map(|i| r.i(&format!("Size{i}")).unwrap_or(10)).collect(),
                    font_bold: (1..=n_fonts).map(|i| r.b(&format!("Bold{i}"))).collect(),
                    font_italic: (1..=n_fonts).map(|i| r.b(&format!("Italic{i}"))).collect(),
                    // `BorderOn` is absent on plenty of sheets that do draw a
                    // border, so its absence must not turn the page frame off.
                    border_on: !r.has("BorderOn") || r.b("BorderOn"),
                    // A sheet that states its own ruler overrides the table for
                    // its style, which is how a custom page keeps its zones.
                    zones_x: r.i("CustomXZones").filter(|v| *v > 0).unwrap_or(zx),
                    zones_y: r.i("CustomYZones").filter(|v| *v > 0).unwrap_or(zy),
                    margin: r.i("CustomMarginWidth").filter(|v| *v > 0).unwrap_or(margin),
                    template_file,
                };
            }
            211 => out.regions.push(Region {
                min: pt(r, "Location.X", "Location.Y"),
                max: pt(r, "Corner.X", "Corner.Y"),
                uuid: r.s("UniqueID").to_string(),
            }),
            4 | 28 | 209 => {
                let text = r.s("Text").to_string();
                // `notes` is the design model's list and only ever meant the
                // designer's own writing; the drawing sheet's title block is
                // not a note about the design.
                let template = owner.is_some() && owner == template_at;
                if !text.is_empty() && owner.is_none() {
                    out.notes.push(text.clone());
                }
                // A text frame or note is a box; `Alignment` is its horizontal
                // setting alone, so it is folded onto Altium's justification
                // grid (0 bottom-left .. 8 top-right) as a TOP-aligned cell,
                // which is where a box lays its first line out from.
                let (corner, justify) = match t {
                    4 => (None, r.i("Justification").unwrap_or(0)),
                    _ => (
                        Some(pt(r, "Corner.X", "Corner.Y")),
                        5 + r.i("Alignment").unwrap_or(1).clamp(1, 3),
                    ),
                };
                out.texts.push(SchText {
                    kind: match t {
                        4 => TextKind::Label,
                        28 => TextKind::Frame,
                        _ => TextKind::Note,
                    },
                    at: pt(r, "Location.X", "Location.Y"),
                    corner,
                    text,
                    color: if t == 4 { color(r) } else { units::bgr_hex(r.i("TextColor").unwrap_or(0)) },
                    fill: fill(r),
                    font: r.i("FontID").unwrap_or(1),
                    justify,
                    orientation: r.i("Orientation").unwrap_or(0),
                    mirrored: r.b("IsMirrored"),
                    show_border: r.b("ShowBorder"),
                    template,
                    uuid: r.s("UniqueID").to_string(),
                });
            }
            t if GRAPHIC_RECORDS.contains(&t) => {
                let Some(g) = graphic(r, t) else {
                    *out.skipped.entry(t).or_default() += 1;
                    continue;
                };
                // Three homes, and the difference matters to a reader: a
                // component's own artwork is the symbol, the template's is the
                // page frame, and anything else is the designer's annotation.
                match owner.and_then(|o| comp_at.get(&o)).copied() {
                    Some(c) => {
                        for p in shape_points(&g.shape) {
                            let b = out.components[c].bbox.get_or_insert((p, p));
                            b.0.x = b.0.x.min(p.x);
                            b.0.y = b.0.y.min(p.y);
                            b.1.x = b.1.x.max(p.x);
                            b.1.y = b.1.y.max(p.y);
                        }
                        out.components[c].graphics.push(g);
                    }
                    None if owner.is_some() && owner == template_at => {
                        out.template_graphics.push(g)
                    }
                    None => out.graphics.push(g),
                }
            }
            // Silent-drop guard: everything not modelled is counted by type.
            // 46/47/48 are implementation-list detail with nothing to draw.
            _ => *out.skipped.entry(t).or_default() += 1,
        }
    }

    // A component's own pin anchors also bound it — a symbol whose graphics are
    // all in one part still has pins on every part.
    for c in &mut out.components {
        for p in &c.pins {
            for q in [p.at, p.connection()] {
                let b = c.bbox.get_or_insert((q, q));
                b.0.x = b.0.x.min(q.x);
                b.0.y = b.0.y.min(q.y);
                b.1.x = b.1.x.max(q.x);
                b.1.y = b.1.y.max(q.y);
            }
        }
    }
    out
}

/// Altium's `Electrical` code as the bundle's `pin_type` vocabulary.
///
/// Altium has no `POWER_OUT`, `UNSPECIFIED`, `NO_CONNECT` or `FREE` at the pin
/// level, so those are never produced — an invented type would be a finding the
/// design does not support.
pub fn pin_type(electrical: i64) -> &'static str {
    match electrical {
        0 => "INPUT",
        1 => "BIDIRECTIONAL",
        2 => "OUTPUT",
        3 => "OPEN_COLLECTOR",
        5 => "TRI_STATE",
        6 => "OPEN_EMITTER",
        7 => "POWER_IN",
        _ => "PASSIVE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Corner case 18: a sheet entry stores no coordinate. `DistanceFromTop` is
    /// in grid steps of 10 units, and getting that wrong silently loses every
    /// hierarchical link.
    #[test]
    fn sheet_entry_hotspot_comes_from_side_and_distance() {
        let s = SheetSymbol {
            at: Pt { x: 190 * UNIT, y: 710 * UNIT },
            xsize: 150,
            ysize: 110,
            uuid: "S".into(),
            name: "Sub".into(),
            filename: "sub.SchDoc".into(),
            ..Default::default()
        };
        let e = |side, d| SheetEntry {
            name: "N".into(),
            side,
            distance_from_top: d,
            ..Default::default()
        };
        assert_eq!(s.entry_point(&e(0, 1)), Pt { x: 190 * UNIT, y: 700 * UNIT });
        assert_eq!(s.entry_point(&e(1, 5)), Pt { x: 340 * UNIT, y: 660 * UNIT });
        assert_eq!(s.entry_point(&e(2, 3)), Pt { x: 220 * UNIT, y: 710 * UNIT });
        assert_eq!(s.entry_point(&e(3, 3)), Pt { x: 220 * UNIT, y: 600 * UNIT });
    }

    /// A part carries several PCBLIB models and Altium places the one flagged
    /// `IsCurrent`. Taking the first gives a footprint in the right family and
    /// wrong — `SOIC127P1030X265-16N-4` where the board has `…-16N-V`. Found by
    /// the plan §8.3 differential on U2 of the EVAL design.
    #[test]
    fn the_footprint_is_the_current_implementation() {
        let rec = |s: &str| TextRecord::parse(format!("{s} ").as_bytes());
        let doc = parse_records(vec![
            rec("|HEADER=Protel for Windows - Schematic Capture|"),
            rec("|RECORD=1|LibReference=GD|PartCount=2|"),
            rec("|RECORD=44|OwnerIndex=0|"),
            rec("|RECORD=45|OwnerIndex=1|ModelName=SOIC127P1030X265-16N-4|ModelType=PCBLIB|"),
            rec("|RECORD=45|OwnerIndex=1|ModelName=SOIC127P1030X265-16N-V|ModelType=PCBLIB|IsCurrent=T|"),
        ]);
        assert_eq!(doc.components.len(), 1);
        assert_eq!(doc.components[0].footprint, "SOIC127P1030X265-16N-V");
    }

    /// With nothing flagged, the first PCBLIB model still wins — the rule adds a
    /// preference, it does not make an unflagged list footprint-less.
    #[test]
    fn an_unflagged_implementation_list_keeps_the_first_footprint() {
        let rec = |s: &str| TextRecord::parse(format!("{s} ").as_bytes());
        let doc = parse_records(vec![
            rec("|HEADER=Protel for Windows - Schematic Capture|"),
            rec("|RECORD=1|LibReference=GD|"),
            rec("|RECORD=44|OwnerIndex=0|"),
            rec("|RECORD=45|OwnerIndex=1|ModelName=FIRST|ModelType=PCBLIB|"),
            rec("|RECORD=45|OwnerIndex=1|ModelName=SECOND|ModelType=PCBLIB|"),
        ]);
        assert_eq!(doc.components[0].footprint, "FIRST");
    }

    /// Corner case 17: a port connects at both edges, not only its origin.
    #[test]
    fn a_port_has_two_terminals() {
        let p = Port {
            at: Pt { x: 60 * UNIT, y: 620 * UNIT },
            width: 70,
            name: "IN_P".into(),
            io_type: 2,
            ..Default::default()
        };
        assert_eq!(
            p.terminals(),
            [Pt { x: 60 * UNIT, y: 620 * UNIT }, Pt { x: 130 * UNIT, y: 620 * UNIT }]
        );
    }

    /// Corner case 13: the kind lives in a versioned field, and the base field
    /// of a newer file lies.
    #[test]
    fn component_kind_prefers_the_newest_version_field() {
        let r = TextRecord::parse(b"|RECORD=1|ComponentKind=0|ComponentKindVersion3=2|\0");
        assert_eq!(component_kind(&r), ComponentKind::Graphical);
        assert!(!component_kind(&r).in_bom());
        let r2 = TextRecord::parse(b"|RECORD=1|ComponentKind=1|\0");
        assert_eq!(component_kind(&r2), ComponentKind::Mechanical);
        assert!(component_kind(&r2).in_bom());
        let r3 = TextRecord::parse(b"|RECORD=1|\0");
        assert_eq!(component_kind(&r3), ComponentKind::Standard);
    }

    /// Altium omits a coordinate key whose value is zero. Requiring both keys
    /// turned a wire ending on the sheet's left edge into a single point: gone
    /// from the render, and disconnected at that end in the netlist.
    #[test]
    fn an_omitted_coordinate_is_zero_not_a_missing_point() {
        let r = TextRecord::parse(b"|RECORD=27|LocationCount=2|X1=20|Y1=740|Y2=740| ");
        let pts = polyline(&r);
        assert_eq!(
            pts,
            vec![Pt { x: 20 * UNIT, y: 740 * UNIT }, Pt { x: 0, y: 740 * UNIT }]
        );
        // An index with neither coordinate is genuinely absent.
        let r2 = TextRecord::parse(b"|RECORD=27|LocationCount=3|X1=1|Y1=2|X2=3|Y2=4| ");
        assert_eq!(polyline(&r2).len(), 2);
    }

    #[test]
    fn pin_types_never_invent_a_missing_altium_type() {
        assert_eq!(pin_type(0), "INPUT");
        assert_eq!(pin_type(7), "POWER_IN");
        assert_eq!(pin_type(4), "PASSIVE");
        assert_eq!(pin_type(99), "PASSIVE");
        for e in 0..8 {
            assert!(!matches!(pin_type(e), "POWER_OUT" | "UNSPECIFIED" | "NO_CONNECT" | "FREE"));
        }
    }

    #[test]
    fn pin_connection_steps_forward_along_the_rotation() {
        let p = |c: i64| Pin {
            number: "1".into(),
            name: "A".into(),
            electrical: 4,
            conglomerate: c,
            length: 20,
            part_id: 1,
            ..Default::default()
        };
        assert_eq!(p(32).connection(), Pt { x: 20 * UNIT, y: 0 });
        assert_eq!(p(33).connection(), Pt { x: 0, y: 20 * UNIT });
        assert_eq!(p(34).connection(), Pt { x: -20 * UNIT, y: 0 });
        assert_eq!(p(35).connection(), Pt { x: 0, y: -20 * UNIT });
        assert_eq!(p(33).rotation(), 90);
    }
}
