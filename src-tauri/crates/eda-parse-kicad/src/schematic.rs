//! Faithful in-memory model of a `.kicad_sch` file.
//!
//! The goal is to retain everything useful from the file — symbol placements
//! with all of their properties, the library symbol pin definitions (which carry
//! electrical type and geometry), and the wiring primitives needed later to
//! compile connectivity. Unknown constructs are simply ignored, so the model
//! survives format additions.

use crate::sexpr::{self, Node, ParseError};

/// A 2-D point in schematic millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

/// A position with orientation (degrees), as written by `(at x y [angle])`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct At {
    pub x: f64,
    pub y: f64,
    pub angle: f64,
}

fn read_at(n: &Node) -> At {
    let a = n.child("at");
    match a {
        Some(a) => At {
            x: a.nth(1).and_then(Node::as_f64).unwrap_or(0.0),
            y: a.nth(2).and_then(Node::as_f64).unwrap_or(0.0),
            angle: a.nth(3).and_then(Node::as_f64).unwrap_or(0.0),
        },
        None => At::default(),
    }
}

/// Horizontal text justification (`(justify left|right)`); default centered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HJustify {
    Left,
    #[default]
    Center,
    Right,
}

/// Vertical text justification (`(justify top|bottom)`); default centered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VJustify {
    Top,
    #[default]
    Center,
    Bottom,
}

/// Everything a KiCad `(effects …)` block carries about how text is drawn —
/// captured in full so the renderer can reproduce KiCad's appearance.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEffects {
    /// Glyph height in mm (`(font (size h w))`); KiCad default 1.27.
    pub size: f64,
    /// Stroke thickness in mm, when specified.
    pub thickness: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    /// Explicit font face/family, when specified.
    pub face: Option<String>,
    /// Text color `(r, g, b, a)` from `(font (color …))`, when specified.
    pub color: Option<(u8, u8, u8, f64)>,
    pub h_justify: HJustify,
    pub v_justify: VJustify,
    /// `(justify mirror)` — text is mirrored.
    pub mirror: bool,
    /// `(hide yes)` — the field/text is not drawn.
    pub hidden: bool,
    /// `(effects (href "url"))` — a clickable hyperlink attached to the text (KiCad 7+).
    pub href: Option<String>,
}

impl Default for TextEffects {
    fn default() -> Self {
        TextEffects {
            size: 1.27,
            thickness: None,
            bold: false,
            italic: false,
            face: None,
            color: None,
            h_justify: HJustify::default(),
            v_justify: VJustify::default(),
            mirror: false,
            hidden: false,
            href: None,
        }
    }
}

/// A property field on a symbol (`(property "Key" "Value" …)`), order preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub key: String,
    pub value: String,
    /// Field position in absolute sheet coordinates, when the file records one.
    pub at: Option<At>,
    /// Full text rendering attributes (`(effects …)`).
    pub effects: TextEffects,
}

impl Property {
    /// Whether the field is marked hidden.
    pub fn hidden(&self) -> bool {
        self.effects.hidden
    }
}

/// A pin as defined inside a library symbol body.
#[derive(Debug, Clone, PartialEq)]
pub struct LibPin {
    pub number: String,
    pub name: String,
    /// Raw electrical-type token, e.g. `passive`, `power_in`, `bidirectional`.
    pub etype: String,
    /// Pin graphic style token (`line`, `inverted`, `clock`, `inverted_clock`,
    /// `input_low`, `clock_low`, `output_low`, `edge_clock_high`, `non_logic`).
    pub shape: String,
    /// Unit this pin belongs to; `0` means common to every unit.
    pub unit: u32,
    /// Pin endpoint relative to the symbol origin, before placement transform.
    pub at: At,
    pub length: f64,
    /// `(hide yes)` on the pin — not drawn (but still electrically present).
    pub hidden: bool,
    /// Glyph height (mm) for the pin name, from its `(name (effects (font (size))))`.
    pub name_size: f64,
    /// Glyph height (mm) for the pin number.
    pub number_size: f64,
}

/// A graphic primitive in a library symbol body, in library (Y-up) coordinates.
/// Fill style of a library-symbol graphic, from `(fill (type …))`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Outline only.
    None,
    /// Body background fill — KiCad draws it *behind* the foreground graphics, so
    /// a renderer must emit it first or it paints over the symbol's detail.
    Background,
    /// Filled with the outline (foreground) color (e.g. a solid diode triangle).
    Outline,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Rect { a: Pt, b: Pt, fill: Fill },
    Poly { pts: Vec<Pt>, fill: Fill },
    Circle { center: Pt, radius: f64, fill: Fill },
    /// A three-point arc (start, mid, end) — rendered as a short polyline.
    Arc { start: Pt, mid: Pt, end: Pt },
}

impl Shape {
    /// Fill style of this shape (arcs are never filled).
    pub fn fill(&self) -> Fill {
        match self {
            Shape::Rect { fill, .. } | Shape::Poly { fill, .. } | Shape::Circle { fill, .. } => {
                *fill
            }
            Shape::Arc { .. } => Fill::None,
        }
    }
}

/// One body graphic of a library symbol, tagged with the unit it belongs to
/// (`0` = common to all units).
#[derive(Debug, Clone, PartialEq)]
pub struct LibGraphic {
    pub unit: u32,
    pub shape: Shape,
    /// Explicit stroke width (mm) from `(stroke (width …))`; `0` = use the default.
    /// Some symbols draw deliberately heavy strokes (e.g. the flux-concentrator
    /// "C" arc at 3.048 mm) that must not collapse to a hairline.
    pub width: f64,
}

/// A free-text label drawn inside a library symbol body (`(text "…" (at …))`),
/// e.g. an opamp's `+`/`-` or a gate driver's `Logic Input`. Carried per-unit so
/// the renderer can reproduce the symbol faithfully.
#[derive(Debug, Clone, PartialEq)]
pub struct LibText {
    pub unit: u32,
    pub text: String,
    pub at: At,
    pub effects: TextEffects,
}

/// A library symbol definition (`(symbol "Lib:Name" …)` inside `lib_symbols`).
#[derive(Debug, Clone, PartialEq)]
pub struct LibSymbol {
    pub lib_id: String,
    pub properties: Vec<Property>,
    pub pins: Vec<LibPin>,
    /// Body graphics (rectangles, polylines, circles, arcs).
    pub graphics: Vec<LibGraphic>,
    /// Free-text labels inside the symbol body.
    pub texts: Vec<LibText>,
    /// `(pin_numbers (hide yes))` — pin numbers are not drawn for this symbol.
    pub pin_numbers_hidden: bool,
    /// `(pin_names (hide yes))` — pin names are not drawn for this symbol.
    pub pin_names_hidden: bool,
    /// `(pin_names (offset N))` — gap (mm) between the pin's body end and its
    /// name; KiCad's default when unspecified is 0.508 mm.
    pub pin_name_offset: f64,
    /// Marked `(power)` — its placed instances name a global power net.
    pub power: bool,
    /// Body extent in library (Y-up) space as `(min, max)`, gathered from the
    /// symbol's graphics and pin anchors — the source of a placed component's
    /// bounding box. `None` when the symbol has no geometry.
    pub bbox: Option<(Pt, Pt)>,
}

impl LibSymbol {
    /// Body extent for a placed instance of `unit`: only the geometry actually drawn
    /// for that unit — the unit-common elements (`unit == 0`) plus the unit's own pins,
    /// graphics and body text, mirroring the SVG renderer's `unit == 0 || unit == sym.unit`
    /// filter. Falls back to the whole-symbol [`bbox`](Self::bbox) when the symbol
    /// carries no unit-tagged geometry (so single-unit parts are unaffected).
    ///
    /// A multi-unit symbol's units (U12.A/.B/.C) are placed independently, but the
    /// whole-symbol bbox spans every unit's geometry. Using it per instance means an
    /// edit confined to one unit — moving a pin on U12.A — shifts the placed bbox of
    /// *all* units, so the diff engine reports the untouched units as moved too. Scoping
    /// the extent to the unit's own geometry keeps the change on the unit it belongs to.
    pub fn bbox_for_unit(&self, unit: u32) -> Option<(Pt, Pt)> {
        let on_unit = |u: u32| u == 0 || u == unit;
        let mut pts: Vec<Pt> = Vec::new();
        for p in self.pins.iter().filter(|p| on_unit(p.unit)) {
            pts.push(Pt { x: p.at.x, y: p.at.y });
        }
        for g in self.graphics.iter().filter(|g| on_unit(g.unit)) {
            // Mirror the whole-symbol scan (`collect_points`): a circle contributes its
            // centre only, so a per-unit box never exceeds the whole-symbol box.
            match &g.shape {
                Shape::Rect { a, b, .. } => pts.extend([*a, *b]),
                Shape::Poly { pts: p, .. } => pts.extend(p.iter().copied()),
                Shape::Circle { center, .. } => pts.push(*center),
                Shape::Arc { start, mid, end } => pts.extend([*start, *mid, *end]),
            }
        }
        for t in self.texts.iter().filter(|t| on_unit(t.unit)) {
            pts.push(Pt { x: t.at.x, y: t.at.y });
        }
        bbox_of(&pts).or(self.bbox)
    }
}

/// A placed symbol instance on a sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolInstance {
    pub lib_id: String,
    /// `(lib_name "…")` — when KiCad caches several variants of one `lib_id` it
    /// renames them (`GND_1`, `GND_2`, …) and the instance points at the cached
    /// definition by this name. Resolve the library symbol via this first.
    pub lib_name: Option<String>,
    pub at: At,
    pub mirror: Option<String>, // "x" or "y"
    pub unit: u32,
    pub in_bom: bool,
    pub on_board: bool,
    pub dnp: bool,
    /// The placement uuid — the stable id used to cross-reference SVG groups.
    pub uuid: String,
    pub properties: Vec<Property>,
    /// Per-sheet-instance references: `(path, reference, unit)`.
    pub instances: Vec<InstancePath>,
    /// The placed instance's pin uuids (`(pin "N" (uuid …))`), keyed by pin
    /// number — these are the stable ids SVG pin glyphs carry.
    pub pins: Vec<InstancePin>,
}

/// A placed symbol's pin instance — maps a pin number to its SVG uuid.
#[derive(Debug, Clone, PartialEq)]
pub struct InstancePin {
    pub number: String,
    pub uuid: String,
}

impl SymbolInstance {
    /// The SVG uuid of the placed pin with the given number, if recorded.
    pub fn pin_uuid(&self, number: &str) -> Option<&str> {
        self.pins
            .iter()
            .find(|p| p.number == number)
            .map(|p| p.uuid.as_str())
    }
}

/// One entry of a symbol's `(instances …)` block.
#[derive(Debug, Clone, PartialEq)]
pub struct InstancePath {
    pub project: String,
    pub path: String,
    pub reference: String,
    pub unit: u32,
}

impl SymbolInstance {
    /// Value of a property by key (first match), if present.
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|p| p.key == key)
            .map(|p| p.value.as_str())
    }

    /// Best-effort designator: the placement's `Reference` property.
    pub fn reference(&self) -> Option<&str> {
        self.property("Reference")
    }
}

/// A wire or bus segment (`(wire (pts (xy …) (xy …)))`).
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub a: Pt,
    pub b: Pt,
    pub is_bus: bool,
    /// SVG uuid of this segment.
    pub uuid: String,
    /// Explicit stroke width (mm); `0` means use the default.
    pub width: f64,
    /// Explicit stroke colour, when the wire carries one (`(stroke (color …))`).
    pub color: Option<(u8, u8, u8, f64)>,
}

/// A junction dot (`(junction (at …) … (uuid …))`).
#[derive(Debug, Clone, PartialEq)]
pub struct Junction {
    pub at: Pt,
    pub uuid: String,
    /// Dot diameter (mm); `0` means use the default.
    pub diameter: f64,
    /// Explicit dot colour, when set.
    pub color: Option<(u8, u8, u8, f64)>,
}

/// A bus entry (`(bus_entry (at x y) (size dx dy) …)`) — the short diagonal stub
/// that taps a wire into a bus. Stored as the segment `a`→`b` it draws.
#[derive(Debug, Clone, PartialEq)]
pub struct BusEntry {
    pub a: Pt,
    pub b: Pt,
    pub uuid: String,
}

/// The kind of a net-naming label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelKind {
    Local,
    Global,
    Hierarchical,
}

/// A label that can name a net.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub at: At,
    pub kind: LabelKind,
    /// SVG uuid of the label glyph.
    pub uuid: String,
    /// Full text rendering attributes (`(effects …)`).
    pub effects: TextEffects,
    /// `(shape …)` flag for global/hierarchical labels — `input`, `output`,
    /// `bidirectional`, `tri_state`, `passive` — which selects the drawn glyph.
    pub shape: Option<String>,
}

/// A net-class directive flag (`(netclass_flag …)`): a short stub ending in a
/// shape glyph that assigns a wire to a net class. The class name lives in a
/// `(property "Netclass" …)` that is often hidden.
#[derive(Debug, Clone, PartialEq)]
pub struct NetclassFlag {
    pub at: At,
    /// Length (mm) of the stub line from the attachment point.
    pub length: f64,
    /// Terminal glyph: `round`, `rectangle`, `dot`, `diamond`.
    pub shape: String,
    /// Assigned net-class name (the `(property "Netclass" …)` value).
    pub netclass: String,
    /// Whether the net-class name text is hidden (only the glyph is drawn).
    pub netclass_hidden: bool,
    /// Absolute position of the net-class name text, when recorded.
    pub netclass_at: Option<At>,
    pub uuid: String,
    pub effects: TextEffects,
}

/// A free graphic primitive drawn directly on a sheet (top-level `(polyline)`,
/// `(rectangle)`, `(circle)`, `(arc)`) — annotations and dividers that are part
/// of the drawing and must be reproduced.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetGraphic {
    pub shape: Shape,
    pub uuid: String,
    pub width: f64,
    pub color: Option<(u8, u8, u8, f64)>,
    /// `(stroke (type dash|dash_dot|dot|...))` — anything but solid/default. Drawn
    /// dashed so KiCad's dashed annotation brackets (e.g. "place close to ADC pin")
    /// don't render as solid lines.
    pub dashed: bool,
}

/// A free-text annotation on a sheet (`(text …)` or `(text_box …)`) — designer
/// notes that carry intent (stuffing options, warnings, rationale) and are
/// valuable context for an automated reviewer.
#[derive(Debug, Clone, PartialEq)]
pub struct TextNote {
    pub text: String,
    pub at: At,
    pub uuid: String,
    /// True for `(text_box …)` (a bordered block) vs a plain `(text …)`.
    pub boxed: bool,
    /// Box `(size w h)` for a `(text_box …)`; `None` for plain text. `at` is the
    /// box's top-left corner.
    pub box_size: Option<(f64, f64)>,
    /// Border stroke width (mm) for a text_box whose `(stroke (type …))` is not
    /// `none`; `0` means draw it at the default hairline. `None` = no border.
    pub border_width: Option<f64>,
    /// Box fill colour for a `(text_box … (fill (type color) (color r g b a)))`;
    /// `None` for `(type none)` (or a plain text note). KiCad fills the box behind
    /// the text — the demo boards colour-code call-outs (a yellow GPIO note).
    pub box_fill: Option<(u8, u8, u8, f64)>,
    /// Box inner margins `(left, top, right, bottom)` in mm; KiCad insets the text
    /// from the border and wraps it to the box width minus the horizontal margins.
    pub box_margins: Option<(f64, f64, f64, f64)>,
    /// Full text rendering attributes (`(effects …)`).
    pub effects: TextEffects,
}

/// An embedded raster image (`(image …)`) with its base64 PNG payload.
#[derive(Debug, Clone, PartialEq)]
pub struct SchImage {
    pub at: At,
    pub scale: f64,
    pub uuid: String,
    /// Base64-encoded PNG bytes, exactly as stored in the file.
    pub data: String,
}

/// A pin on a sheet symbol (`(pin "NAME" <dir> (at …))`) — the parent-side
/// endpoint that connects to a same-named hierarchical label in the child sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetPin {
    pub name: String,
    pub at: At,
    pub uuid: String,
}

/// A child sheet placement (`(sheet …)`), for hierarchy walking.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetRef {
    pub uuid: String,
    pub name: String,
    pub file: String,
    pub at: At,
    /// Sheet box size `(w, h)` in mm.
    pub size: (f64, f64),
    /// Background fill `(fill (color r g b a))` when the sheet sets a
    /// non-transparent one; `None` for the default alpha-0 "no fill". KiCad lets
    /// each sheet carry its own colour (the root page tints some sheets white).
    pub fill: Option<(u8, u8, u8, f64)>,
    /// Parent-side connection points to the child sheet.
    pub pins: Vec<SheetPin>,
    /// `(instances (project … (path "<parent uuid-chain>" (page "N"))))` — the
    /// per-placement page label for each instance context this sheet appears in,
    /// keyed by the *parent* sheet's full instance path. KiCad ≥7 stores
    /// hierarchical page numbers here, not in the root's `sheet_instances`.
    pub instances: Vec<SheetPage>,
}

/// A bus alias (`(bus_alias "NAME" (members "A" "B" …))`): a named bundle that is
/// shorthand for its member nets. Captured so the definition isn't lost, but
/// deliberately NOT expanded into connectivity — member names are bus-local
/// templates and routinely reused across different buses (e.g. `SDA`/`SCL` belong
/// to both `I2C` and `HDMI`, `TX0+` to both `DP` and `PCIe`), so a bare-name union
/// would silently short unrelated nets. The real per-member net comes from the
/// tap-wire label, which needs bus-entry geometry to resolve.
#[derive(Debug, Clone, PartialEq)]
pub struct BusAlias {
    pub name: String,
    pub members: Vec<String>,
}

/// One `(sheet_instances (path "<uuid-chain>" (page "N")))` entry — a sheet
/// instance path and the page label KiCad assigns it.
#[derive(Debug, Clone, PartialEq)]
pub struct SheetPage {
    pub path: String,
    pub page: String,
}

/// The parsed schematic.
#[derive(Debug, Clone, PartialEq)]
pub struct Schematic {
    pub version: Option<i64>,
    pub generator: Option<String>,
    /// `(generator_version "9.0")` — the KiCad version, for the title block's
    /// "KiCad E.D.A. …" line.
    pub generator_version: Option<String>,
    pub uuid: Option<String>,
    /// Drawing-sheet paper size token (`A4`, `A3`, `User`, …), if present.
    pub paper: Option<String>,
    /// Explicit page dimensions `(w, h)` in mm for a `User` paper size.
    pub paper_dims: Option<(f64, f64)>,
    /// Title-block title, if present.
    pub title: Option<String>,
    /// Title-block company / revision / date, if present.
    pub company: Option<String>,
    pub rev: Option<String>,
    pub date: Option<String>,
    /// Title-block comment lines (`(comment N "…")`), in ascending number order.
    pub comments: Vec<String>,
    pub lib_symbols: Vec<LibSymbol>,
    pub symbols: Vec<SymbolInstance>,
    pub wires: Vec<Wire>,
    pub junctions: Vec<Junction>,
    pub bus_entries: Vec<BusEntry>,
    pub labels: Vec<Label>,
    pub no_connects: Vec<Pt>,
    pub sheets: Vec<SheetRef>,
    /// Free-text annotations on the sheet.
    pub notes: Vec<TextNote>,
    /// Embedded raster images.
    pub images: Vec<SchImage>,
    /// Free graphic primitives drawn on the sheet (not inside a symbol).
    pub graphics: Vec<SheetGraphic>,
    /// Net-class directive flags.
    pub netclass_flags: Vec<NetclassFlag>,
    /// Bus alias definitions (named bundles → member nets).
    pub bus_aliases: Vec<BusAlias>,
    /// `(sheet_instances …)` page-number map for this file's instances.
    pub sheet_instances: Vec<SheetPage>,
}

impl Schematic {
    /// Parse schematic source text.
    pub fn parse_str(src: &str) -> Result<Schematic, ParseError> {
        let root = sexpr::parse(src)?;
        // Reject a non-schematic root (a renamed `.kicad_sym` / `.kicad_pcb` / netlist)
        // so a wrong file surfaces as an error instead of a silently empty sheet.
        if root.tag() != Some("kicad_sch") {
            return Err(ParseError::WrongRoot {
                expected: "kicad_sch",
                found: root.tag().map(str::to_string),
            });
        }
        Ok(Schematic::from_root(&root))
    }

    /// Look up a library symbol by its `lib_id`.
    pub fn lib_symbol(&self, lib_id: &str) -> Option<&LibSymbol> {
        self.lib_symbols.iter().find(|s| s.lib_id == lib_id)
    }

    /// Resolve the library symbol backing a placed instance. KiCad caches each
    /// used symbol on the sheet; when several variants share a `lib_id` it renames
    /// the cache entries and the instance points at one via `(lib_name …)`. So
    /// match the instance's `lib_name` first, then fall back to its `lib_id`.
    pub fn lib_for(&self, sym: &SymbolInstance) -> Option<&LibSymbol> {
        if let Some(name) = &sym.lib_name {
            if let Some(ls) = self.lib_symbol(name) {
                return Some(ls);
            }
        }
        self.lib_symbol(&sym.lib_id)
    }

    fn from_root(root: &Node) -> Schematic {
        let mut sch = Schematic {
            version: root.field("version").and_then(|s| s.parse().ok()),
            generator: root.field("generator").map(str::to_string),
            generator_version: root.field("generator_version").map(str::to_string),
            uuid: root.field("uuid").map(str::to_string),
            paper: root.field("paper").map(str::to_string),
            paper_dims: root.child("paper").and_then(|p| p.pair_at(2)),
            title: root
                .child("title_block")
                .and_then(|t| t.field("title"))
                .map(str::to_string),
            company: root
                .child("title_block")
                .and_then(|t| t.field("company"))
                .map(str::to_string),
            rev: root
                .child("title_block")
                .and_then(|t| t.field("rev"))
                .map(str::to_string),
            date: root
                .child("title_block")
                .and_then(|t| t.field("date"))
                .map(str::to_string),
            comments: root.child("title_block").map(parse_comments).unwrap_or_default(),
            lib_symbols: Vec::new(),
            symbols: Vec::new(),
            wires: Vec::new(),
            junctions: Vec::new(),
            bus_entries: Vec::new(),
            labels: Vec::new(),
            no_connects: Vec::new(),
            sheets: Vec::new(),
            notes: Vec::new(),
            images: Vec::new(),
            graphics: Vec::new(),
            netclass_flags: Vec::new(),
            bus_aliases: Vec::new(),
            sheet_instances: Vec::new(),
        };

        if let Some(libs) = root.child("lib_symbols") {
            for s in libs.children("symbol") {
                if let Some(ls) = parse_lib_symbol(s) {
                    sch.lib_symbols.push(ls);
                }
            }
        }

        for n in root.as_list().into_iter().flatten() {
            match n.tag() {
                Some("symbol") => {
                    if let Some(si) = parse_symbol_instance(n) {
                        sch.symbols.push(si);
                    }
                }
                Some("wire") | Some("bus") => {
                    let is_bus = n.tag() == Some("bus");
                    if let Some((a, b)) = read_two_pts(n) {
                        let (width, color) = read_stroke(n);
                        sch.wires.push(Wire {
                            a,
                            b,
                            is_bus,
                            uuid: n.field("uuid").unwrap_or("").to_string(),
                            width,
                            color,
                        });
                    }
                }
                Some("junction") => sch.junctions.push(Junction {
                    at: read_at(n).into(),
                    uuid: n.field("uuid").unwrap_or("").to_string(),
                    diameter: n.field_f64("diameter").unwrap_or(0.0),
                    color: read_color(n.child("color")),
                }),
                Some("bus_entry") => {
                    let at = read_at(n);
                    let (dx, dy) = n.pair("size").unwrap_or((0.0, 0.0));
                    sch.bus_entries.push(BusEntry {
                        a: Pt { x: at.x, y: at.y },
                        b: Pt { x: at.x + dx, y: at.y + dy },
                        uuid: n.field("uuid").unwrap_or("").to_string(),
                    });
                }
                Some("no_connect") => sch.no_connects.push(read_at(n).into()),
                Some("label") => sch.labels.push(read_label(n, LabelKind::Local)),
                Some("global_label") => sch.labels.push(read_label(n, LabelKind::Global)),
                Some("hierarchical_label") => {
                    sch.labels.push(read_label(n, LabelKind::Hierarchical))
                }
                Some("sheet") => {
                    if let Some(s) = parse_sheet_ref(n) {
                        sch.sheets.push(s);
                    }
                }
                Some("text") => sch.notes.push(read_note(n, false)),
                Some("text_box") => sch.notes.push(read_note(n, true)),
                Some("image") => {
                    if let Some(img) = parse_image(n) {
                        sch.images.push(img);
                    }
                }
                Some("polyline") | Some("rectangle") | Some("circle") | Some("arc") => {
                    if let Some(shape) = parse_shape(n) {
                        let (width, color) = read_stroke(n);
                        sch.graphics.push(SheetGraphic {
                            shape,
                            uuid: n.field("uuid").unwrap_or("").to_string(),
                            width,
                            color,
                            dashed: stroke_dashed(n),
                        });
                    }
                }
                Some("netclass_flag") => {
                    if let Some(f) = parse_netclass_flag(n) {
                        sch.netclass_flags.push(f);
                    }
                }
                Some("bus_alias") => {
                    if let Some(a) = parse_bus_alias(n) {
                        sch.bus_aliases.push(a);
                    }
                }
                // A rule area is a polygon region; in practice its outline is a
                // dashed `(polyline)`. Surface that outline as a sheet graphic so the
                // annotation box renders. Directive rule areas (carrying a `(name …)`
                // + rule text) are not modelled — only the drawn boundary.
                Some("rule_area") => {
                    for poly in n.children("polyline") {
                        if let Some(shape) = parse_shape(poly) {
                            let (width, color) = read_stroke(poly);
                            sch.graphics.push(SheetGraphic {
                                shape,
                                uuid: poly.field("uuid").unwrap_or("").to_string(),
                                width,
                                color,
                                dashed: stroke_dashed(poly),
                            });
                        }
                    }
                }
                // A schematic table — capture each cell's text as a (boxed) note so
                // the tabulated design intent (e.g. a power-sequencing table) is not
                // dropped, and the grid renders.
                Some("table") => parse_table_cells(n, &mut sch.notes),
                Some("sheet_instances") => {
                    for p in n.children("path") {
                        let path = p.nth(1).and_then(Node::as_str).unwrap_or("").to_string();
                        if path.is_empty() {
                            continue;
                        }
                        let page = p
                            .child("page")
                            .and_then(|pg| pg.nth(1))
                            .and_then(Node::as_str)
                            .unwrap_or("")
                            .to_string();
                        sch.sheet_instances.push(SheetPage { path, page });
                    }
                }
                _ => {}
            }
        }

        sch
    }
}

impl From<At> for Pt {
    fn from(a: At) -> Pt {
        Pt { x: a.x, y: a.y }
    }
}

/// Title-block comments `(comment N "text")`, returned in ascending number order
/// (KiCad numbers them 1..9 and the numbers may be sparse).
fn parse_comments(tb: &Node) -> Vec<String> {
    let mut cs: Vec<(i64, String)> = tb
        .children("comment")
        .filter_map(|c| Some((c.nth(1)?.as_i64()?, c.nth(2)?.as_str()?.to_string())))
        .collect();
    cs.sort_by_key(|(n, _)| *n);
    cs.into_iter().map(|(_, t)| t).collect()
}

fn read_two_pts(n: &Node) -> Option<(Pt, Pt)> {
    let pts = n.child("pts")?;
    let mut it = pts.children("xy");
    let a = it.next()?;
    let b = it.next()?;
    Some((
        Pt {
            x: a.nth(1)?.as_f64()?,
            y: a.nth(2)?.as_f64()?,
        },
        Pt {
            x: b.nth(1)?.as_f64()?,
            y: b.nth(2)?.as_f64()?,
        },
    ))
}

/// Read a full `(effects …)` block (font size/thickness/bold/italic/face/color,
/// horizontal+vertical justify, mirror, hide) from a node that owns one.
fn read_effects(n: &Node) -> TextEffects {
    let mut e = TextEffects::default();
    let Some(eff) = n.child("effects") else { return e };
    if let Some(font) = eff.child("font") {
        if let Some(sz) = font.child("size").and_then(|s| s.nth(1)).and_then(Node::as_f64) {
            e.size = sz;
        }
        e.thickness = font.field_f64("thickness");
        // `(bold yes)` (KiCad 7+) or a bare `bold` token (KiCad 5/6). `has_flag`
        // matches only a bare `Sym`, so a quoted value equal to the tag never counts.
        e.bold = font.has_flag("bold");
        e.italic = font.has_flag("italic");
        e.face = font.field("face").map(str::to_string);
        if let Some(col) = font.child("color").and_then(Node::as_list) {
            let n = |i: usize| col.get(i).and_then(Node::as_f64);
            if let (Some(r), Some(g), Some(b)) = (n(1), n(2), n(3)) {
                let a = n(4).unwrap_or(1.0);
                // KiCad writes `(color 0 0 0 0)` (alpha 0) for "no explicit colour";
                // only keep a real, opaque-enough colour.
                if a > 0.0 {
                    e.color = Some((r as u8, g as u8, b as u8, a));
                }
            }
        }
    }
    if let Some(j) = eff.child("justify").and_then(Node::as_list) {
        for tok in j.iter().skip(1).filter_map(Node::as_str) {
            match tok {
                "left" => e.h_justify = HJustify::Left,
                "right" => e.h_justify = HJustify::Right,
                "top" => e.v_justify = VJustify::Top,
                "bottom" => e.v_justify = VJustify::Bottom,
                "mirror" => e.mirror = true,
                _ => {}
            }
        }
    }
    // The `hide` flag's location moved across KiCad versions:
    //   • KiCad 5/6: a bare `hide` token inside `(effects …)`
    //   • KiCad 7–9: `(hide yes)` inside `(effects …)`
    //   • KiCad 10:  `(hide yes)` as a DIRECT child of the owning node (property /
    //                text / label), no longer inside `(effects …)`
    // Honour all three — otherwise a board's hidden fields (description, MPN, datasheet,
    // manufacturer, …) plot onto the schematic as clutter (the KiCad-10 H1 "Standoff …"
    // description report). The owner check is `n.flag("hide")` (a real `(hide yes)` child
    // list), NOT a bare-token scan — a field whose *value* is the word "hide" must not
    // read as hidden.
    e.hidden = eff.has_flag("hide") || n.flag("hide").unwrap_or(false);
    // `(href "url")` — a clickable hyperlink on the text (KiCad 7+); carried through so
    // the viewer can make the text open the link.
    e.href = eff.field("href").map(str::to_string);
    e
}

fn read_label(n: &Node, kind: LabelKind) -> Label {
    Label {
        text: n.nth(1).and_then(Node::as_str).unwrap_or_default().to_string(),
        at: read_at(n),
        kind,
        uuid: n.field("uuid").unwrap_or("").to_string(),
        effects: read_effects(n),
        shape: n.field("shape").map(str::to_string),
    }
}

/// Read a `(netclass_flag …)` directive: stub length, terminal shape, the
/// `(property "Netclass" …)` value + visibility, and text rendering.
fn parse_netclass_flag(n: &Node) -> Option<NetclassFlag> {
    let nc = n.property_named("Netclass");
    let (netclass, netclass_hidden, netclass_at) = match nc {
        Some(p) => (
            p.nth(2).and_then(Node::as_str).unwrap_or("").to_string(),
            read_effects(p).hidden,
            p.child("at").map(|_| read_at(p)),
        ),
        None => (String::new(), true, None),
    };
    Some(NetclassFlag {
        at: read_at(n),
        length: n.field_f64("length").unwrap_or(0.0),
        shape: n.field("shape").unwrap_or("round").to_string(),
        netclass,
        netclass_hidden,
        netclass_at,
        uuid: n.field("uuid").unwrap_or("").to_string(),
        effects: read_effects(n),
    })
}

/// Read a `(text …)` / `(text_box …)` annotation: the payload is the first arg.
/// For a text_box, capture its `(size …)` and whether a border stroke is drawn
/// (KiCad omits the rectangle only when the stroke type is explicitly `none`).
fn read_note(n: &Node, boxed: bool) -> TextNote {
    let box_size = if boxed { n.pair("size") } else { None };
    let border_width = if boxed {
        match n.child("stroke") {
            Some(st) => (st.field("type") != Some("none")).then(|| st.field_f64("width").unwrap_or(0.0)),
            None => Some(0.0),
        }
    } else {
        None
    };
    // A `(fill (type color) (color …))` paints the box behind the text; `(type none)`
    // carries no colour child, so `read_color` yields `None` for it.
    let box_fill = boxed.then(|| read_color(n.child("fill").and_then(|f| f.child("color")))).flatten();
    let box_margins = boxed
        .then(|| {
            n.child("margins").map(|m| {
                let f = |i: usize| m.nth(i).and_then(Node::as_f64).unwrap_or(0.0);
                (f(1), f(2), f(3), f(4))
            })
        })
        .flatten();
    TextNote {
        text: n.nth(1).and_then(Node::as_str).unwrap_or_default().to_string(),
        at: read_at(n),
        uuid: n.field("uuid").unwrap_or("").to_string(),
        boxed,
        box_size,
        border_width,
        box_fill,
        box_margins,
        effects: read_effects(n),
    }
}

/// Read an `(image …)`: position, scale, uuid and the base64 PNG `(data …)`
/// (which KiCad may split across several quoted chunks — concatenated here).
fn parse_image(n: &Node) -> Option<SchImage> {
    let data: String = n
        .child("data")?
        .tail()
        .iter()
        .filter_map(Node::as_str)
        .collect::<Vec<_>>()
        .join("");
    Some(SchImage {
        at: read_at(n),
        scale: n.field_f64("scale").unwrap_or(1.0),
        uuid: n.field("uuid").unwrap_or("").to_string(),
        data,
    })
}

/// Read a `(bus_alias "NAME" (members "A" "B" …))` definition.
fn parse_bus_alias(n: &Node) -> Option<BusAlias> {
    let name = n.nth(1)?.as_str()?.to_string();
    let members = n
        .child("members")
        .map(|m| m.tail().iter().filter_map(Node::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    Some(BusAlias { name, members })
}

/// Capture a `(table … (cells (table_cell "text" (at …) (size …) (effects …)) …))`
/// as a set of boxed text notes — one per non-empty cell — preserving the
/// tabulated text (e.g. a power-sequencing table) that would otherwise be dropped.
/// Each cell renders with a hairline border so the grid reads as a table.
fn parse_table_cells(table: &Node, notes: &mut Vec<TextNote>) {
    let Some(cells) = table.child("cells") else { return };
    for cell in cells.children("table_cell") {
        let text = cell.nth(1).and_then(Node::as_str).unwrap_or("").to_string();
        if text.is_empty() {
            continue;
        }
        let box_size = cell.pair("size");
        notes.push(TextNote {
            text,
            at: read_at(cell),
            uuid: cell.field("uuid").unwrap_or("").to_string(),
            boxed: true,
            box_size,
            // Draw the cell outline at the default hairline so the table grid shows.
            border_width: Some(0.0),
            box_fill: None,
            box_margins: None,
            effects: read_effects(cell),
        });
    }
}

fn read_properties(n: &Node) -> Vec<Property> {
    n.children("property")
        .filter_map(|p| {
            let key = p.nth(1)?.as_str()?.to_string();
            let value = p.nth(2).and_then(Node::as_str).unwrap_or_default().to_string();
            // A property's `(at …)` is in absolute sheet coordinates.
            let at = p.child("at").map(|_| read_at(p));
            Some(Property { key, value, at, effects: read_effects(p) })
        })
        .collect()
}

/// Split a library sub-symbol name `Base_<unit>_<style>` into its unit number.
fn unit_from_subsymbol(name: &str) -> Option<u32> {
    let mut parts = name.rsplit('_');
    let _style = parts.next()?;
    parts.next()?.parse().ok()
}

/// A point from a `(tag x y …)` child of `n`.
fn read_pt(n: &Node, tag: &str) -> Option<Pt> {
    let c = n.child(tag)?;
    Some(Pt { x: c.nth(1)?.as_f64()?, y: c.nth(2)?.as_f64()? })
}

/// Read a graphic's `(fill (type …))` style.
fn fill_of(n: &Node) -> Fill {
    match n.child("fill").and_then(|f| f.field("type")) {
        Some("background") => Fill::Background,
        Some("outline") => Fill::Outline,
        _ => Fill::None,
    }
}

/// Build a [`Shape`] from a graphic node (`rectangle`/`polyline`/`polygon`/
/// `circle`/`arc`). Shared by library-symbol graphics and free sheet graphics.
fn parse_shape(n: &Node) -> Option<Shape> {
    Some(match n.tag()? {
        "rectangle" => Shape::Rect {
            a: read_pt(n, "start")?,
            b: read_pt(n, "end")?,
            fill: fill_of(n),
        },
        "polyline" | "polygon" => {
            let pts: Vec<Pt> = n
                .child("pts")?
                .children("xy")
                .filter_map(|p| Some(Pt { x: p.nth(1)?.as_f64()?, y: p.nth(2)?.as_f64()? }))
                .collect();
            if pts.len() < 2 {
                return None;
            }
            Shape::Poly { pts, fill: fill_of(n) }
        }
        "circle" => Shape::Circle {
            center: read_pt(n, "center")?,
            radius: n.field_f64("radius").unwrap_or(0.0),
            fill: fill_of(n),
        },
        "arc" => Shape::Arc {
            start: read_pt(n, "start")?,
            mid: read_pt(n, "mid")?,
            end: read_pt(n, "end")?,
        },
        _ => return None,
    })
}

fn parse_graphic(n: &Node, unit: u32) -> Option<LibGraphic> {
    let shape = parse_shape(n)?;
    let (width, _) = read_stroke(n);
    Some(LibGraphic { unit, shape, width })
}

/// Read a `(stroke (width …) (color r g b a))` block: stroke width (mm) and an
/// explicit colour when set to a non-transparent value.
fn read_stroke(n: &Node) -> (f64, Option<(u8, u8, u8, f64)>) {
    match n.child("stroke") {
        Some(s) => (s.field_f64("width").unwrap_or(0.0), read_color(s.child("color"))),
        None => (0.0, None),
    }
}

/// Whether a node's `(stroke (type …))` is a dashed style (anything other than
/// `solid`/`default`). KiCad uses dashed graphic lines for callout brackets.
fn stroke_dashed(n: &Node) -> bool {
    n.child("stroke")
        .and_then(|s| s.child("type"))
        .and_then(|t| t.nth(1))
        .and_then(Node::as_str)
        .map(|ty| matches!(ty, "dash" | "dash_dot" | "dash_dot_dot" | "dot"))
        .unwrap_or(false)
}

/// Read a `(color r g b a)` node, keeping only a non-transparent colour (KiCad
/// writes alpha 0 for "unset").
fn read_color(node: Option<&Node>) -> Option<(u8, u8, u8, f64)> {
    let col = node?.as_list()?;
    let f = |i: usize| col.get(i).and_then(Node::as_f64);
    let (r, g, b) = (f(1)?, f(2)?, f(3)?);
    let a = f(4).unwrap_or(1.0);
    (a > 0.0).then_some((r as u8, g as u8, b as u8, a))
}

fn parse_lib_symbol(n: &Node) -> Option<LibSymbol> {
    let lib_id = n.nth(1)?.as_str()?.to_string();
    let properties = read_properties(n);
    let mut pins = Vec::new();
    let mut graphics = Vec::new();
    let mut texts = Vec::new();
    // Pins, graphics and body text live in nested unit sub-symbols
    // `(symbol "Base_<unit>_<style>" …)`.
    for sub in n.children("symbol") {
        let unit = sub
            .nth(1)
            .and_then(Node::as_str)
            .and_then(unit_from_subsymbol)
            .unwrap_or(0);
        for pin in sub.children("pin") {
            if let Some(p) = parse_lib_pin(pin, unit) {
                pins.push(p);
            }
        }
        for g in sub.as_list().into_iter().flatten() {
            if let Some(gr) = parse_graphic(g, unit) {
                graphics.push(gr);
            } else if g.tag() == Some("text") {
                texts.push(LibText {
                    unit,
                    text: g.nth(1).and_then(Node::as_str).unwrap_or_default().to_string(),
                    at: read_at(g),
                    effects: read_effects(g),
                });
            }
        }
    }
    // Pin name/number visibility + name offset (KiCad default offset 0.508 mm).
    let pin_numbers_hidden = n.child("pin_numbers").map(hide_flag).unwrap_or(false);
    let pin_names = n.child("pin_names");
    let pin_names_hidden = pin_names.map(hide_flag).unwrap_or(false);
    let pin_name_offset = pin_names
        .and_then(|p| p.child("offset"))
        .and_then(|o| o.nth(1))
        .and_then(Node::as_f64)
        .unwrap_or(0.508);
    let mut points = Vec::new();
    collect_points(n, &mut points);
    let bbox = bbox_of(&points);
    Some(LibSymbol {
        lib_id,
        properties,
        pins,
        graphics,
        texts,
        pin_numbers_hidden,
        pin_names_hidden,
        pin_name_offset,
        power: n.child("power").is_some(),
        bbox,
    })
}

/// True if a node carries a `(hide yes)` child or a bare `hide` token (the form
/// varies across KiCad versions for `pin_names`/`pin_numbers`).
fn hide_flag(n: &Node) -> bool {
    n.has_flag("hide")
}

/// Gather coordinate points from a symbol's graphics (rectangles, polylines,
/// circles, arcs) and pin anchors, recursing through nested sub-symbols. This is
/// a heuristic body-extent scan: it reads the coordinate-bearing tags rather
/// than modelling every primitive, which is enough for a bounding box.
fn collect_points(n: &Node, out: &mut Vec<Pt>) {
    let Some(list) = n.as_list() else { return };
    // Don't descend into `(property …)`: its `(at …)` is a text-field anchor, and
    // KiCad parks hidden fields (Datasheet/Footprint/MPN) well away from the body —
    // including them inflates the symbol bbox and the placed-component hit area. Pin
    // `(at …)` anchors ARE wanted, so this skips only the property subtree, not `at`
    // wholesale (the PCB twin can exclude `at` outright; here it can't).
    if n.tag() == Some("property") {
        return;
    }
    if matches!(
        n.tag(),
        Some("xy" | "start" | "end" | "center" | "mid" | "at")
    ) {
        if let (Some(x), Some(y)) = (
            n.nth(1).and_then(Node::as_f64),
            n.nth(2).and_then(Node::as_f64),
        ) {
            out.push(Pt { x, y });
        }
    }
    for c in list {
        collect_points(c, out);
    }
}

/// Axis-aligned bounding box of a point set as `(min, max)`.
fn bbox_of(points: &[Pt]) -> Option<(Pt, Pt)> {
    let first = points.first()?;
    let (mut min, mut max) = (*first, *first);
    for p in &points[1..] {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Some((min, max))
}

fn parse_lib_pin(n: &Node, unit: u32) -> Option<LibPin> {
    // `(pin <etype> <style> (at …) (length …) (name "…" (effects …)) (number "…"))`
    let etype = n.nth(1)?.as_str()?.to_string();
    let shape = n.nth(2).and_then(Node::as_str).unwrap_or("line").to_string();
    let name_node = n.child("name");
    let number_node = n.child("number");
    let name = name_node
        .and_then(|x| x.nth(1))
        .and_then(Node::as_str)
        .unwrap_or("")
        .to_string();
    let number = number_node
        .and_then(|x| x.nth(1))
        .and_then(Node::as_str)
        .unwrap_or("")
        .to_string();
    // Per-pin name/number glyph height; KiCad default 1.27 mm.
    let size_of = |node: Option<&Node>| {
        node.map(read_effects).map(|e| e.size).unwrap_or(1.27)
    };
    Some(LibPin {
        number,
        name,
        etype,
        shape,
        unit,
        at: read_at(n),
        length: n.field_f64("length").unwrap_or(0.0),
        hidden: n.flag("hide").unwrap_or(false),
        name_size: size_of(name_node),
        number_size: size_of(number_node),
    })
}

fn parse_symbol_instance(n: &Node) -> Option<SymbolInstance> {
    let lib_id = n.field("lib_id")?.to_string();
    let uuid = n.field("uuid").unwrap_or("").to_string();
    let mut instances = Vec::new();
    if let Some(ins) = n.child("instances") {
        for proj in ins.children("project") {
            let project = proj.nth(1).and_then(Node::as_str).unwrap_or("").to_string();
            for path in proj.children("path") {
                instances.push(InstancePath {
                    project: project.clone(),
                    path: path.nth(1).and_then(Node::as_str).unwrap_or("").to_string(),
                    reference: path.field("reference").unwrap_or("").to_string(),
                    unit: path.field("unit").and_then(|s| s.parse().ok()).unwrap_or(1),
                });
            }
        }
    }
    // Placed-pin uuids: `(pin "N" (uuid …))` entries directly under the instance.
    let pins = n
        .children("pin")
        .filter_map(|p| {
            let number = p.nth(1)?.as_str()?.to_string();
            Some(InstancePin {
                number,
                uuid: p.field("uuid").unwrap_or("").to_string(),
            })
        })
        .collect();

    Some(SymbolInstance {
        lib_id,
        lib_name: n.field("lib_name").map(str::to_string),
        at: read_at(n),
        mirror: n.child("mirror").and_then(|m| m.nth(1)).and_then(Node::as_str).map(str::to_string),
        unit: n.field("unit").and_then(|s| s.parse().ok()).unwrap_or(1),
        in_bom: n.flag("in_bom").unwrap_or(true),
        on_board: n.flag("on_board").unwrap_or(true),
        dnp: n.flag("dnp").unwrap_or(false),
        uuid,
        properties: read_properties(n),
        instances,
        pins,
    })
}

fn parse_sheet_ref(n: &Node) -> Option<SheetRef> {
    Some(SheetRef {
        uuid: n.field("uuid").unwrap_or("").to_string(),
        name: n.property_value("Sheetname").unwrap_or("").to_string(),
        file: n.property_value("Sheetfile").unwrap_or("").to_string(),
        at: read_at(n),
        size: n.pair("size").unwrap_or((0.0, 0.0)),
        fill: read_color(n.child("fill").and_then(|f| f.child("color"))),
        pins: n
            .children("pin")
            .filter_map(|p| {
                Some(SheetPin {
                    name: p.nth(1)?.as_str()?.to_string(),
                    at: read_at(p),
                    uuid: p.field("uuid").unwrap_or("").to_string(),
                })
            })
            .collect(),
        instances: n
            .child("instances")
            .map(|inst| {
                inst.children("project")
                    .flat_map(|proj| proj.children("path"))
                    .filter_map(|p| {
                        let path = p.nth(1).and_then(Node::as_str)?.to_string();
                        let page = p
                            .child("page")
                            .and_then(|pg| pg.nth(1))
                            .and_then(Node::as_str)
                            .unwrap_or("")
                            .to_string();
                        Some(SheetPage { path, page })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
    (kicad_sch
      (version 20240101)
      (generator "eeschema")
      (uuid "root-uuid")
      (lib_symbols
        (symbol "Device:C"
          (property "Reference" "C")
          (symbol "C_0_1"
            (pin passive line (at -1 0 0) (length 2) (name "~") (number "1")))
          (symbol "C_1_1"
            (pin passive line (at 1 0 180) (length 2) (name "~") (number "2"))))
      )
      (symbol
        (lib_id "Device:C")
        (at 260.35 101.6 0)
        (unit 1)
        (in_bom yes)
        (on_board yes)
        (dnp no)
        (uuid "c12-uuid")
        (property "Reference" "C12")
        (property "Value" "470n")
        (property "Footprint" "Capacitor_SMD:C_0603_1608Metric" (effects (hide yes)))
        (property "LCSC" "C1623" (effects (hide yes)))
        (pin "1" (uuid "p1"))
        (pin "2" (uuid "p2"))
        (instances (project "Demo" (path "/sheet-uuid" (reference "C12") (unit 1)))))
      (wire (pts (xy 10 10) (xy 20 10)))
      (bus_entry (at 20 10) (size 2.54 -2.54) (uuid "be1"))
      (junction (at 20 10))
      (label "VREF-" (at 30 30 0))
      (no_connect (at 5 5)))
    "#;

    #[test]
    fn parses_title_block_version_and_comments() {
        let src = r#"
        (kicad_sch (generator_version "9.0")
          (title_block
            (title "Demo") (company "Acme")
            (comment 3 "(C) 2023 Pat")
            (comment 1 "See LICENSE.txt")
            (comment 2 "Licensed under Apache 2.0")))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        assert_eq!(sch.generator_version.as_deref(), Some("9.0"));
        assert_eq!(sch.company.as_deref(), Some("Acme"));
        // Comments come back ordered by their number, regardless of file order.
        assert_eq!(
            sch.comments,
            vec!["See LICENSE.txt", "Licensed under Apache 2.0", "(C) 2023 Pat"]
        );
    }

    #[test]
    fn parses_lib_symbol_pins_with_type() {
        let sch = Schematic::parse_str(SAMPLE).unwrap();
        let dev_c = sch.lib_symbol("Device:C").unwrap();
        assert_eq!(dev_c.pins.len(), 2);
        assert!(dev_c.pins.iter().all(|p| p.etype == "passive"));
        assert_eq!(dev_c.pins[0].number, "1");
        assert_eq!(dev_c.pins[1].unit, 1);
    }

    #[test]
    fn parses_symbol_instance() {
        let sch = Schematic::parse_str(SAMPLE).unwrap();
        assert_eq!(sch.symbols.len(), 1);
        let c12 = &sch.symbols[0];
        assert_eq!(c12.reference(), Some("C12"));
        assert_eq!(c12.property("Value"), Some("470n"));
        assert_eq!(c12.uuid, "c12-uuid");
        assert!(c12.in_bom && !c12.dnp);
        assert_eq!(c12.property("LCSC"), Some("C1623"));
        assert_eq!(c12.instances[0].reference, "C12");
        assert_eq!(c12.instances[0].path, "/sheet-uuid");
    }

    #[test]
    fn parses_wiring_primitives() {
        let sch = Schematic::parse_str(SAMPLE).unwrap();
        assert_eq!(sch.wires.len(), 1);
        assert_eq!(sch.wires[0].a, Pt { x: 10.0, y: 10.0 });
        assert_eq!(sch.junctions.len(), 1);
        assert_eq!(sch.labels.len(), 1);
        assert_eq!(sch.labels[0].kind, LabelKind::Local);
        assert_eq!(sch.no_connects.len(), 1);
        // Bus entry: (at 20 10) + (size 2.54 -2.54) -> segment (20,10)->(22.54,7.46).
        assert_eq!(sch.bus_entries.len(), 1);
        assert_eq!(sch.bus_entries[0].a, Pt { x: 20.0, y: 10.0 });
        assert_eq!(sch.bus_entries[0].b, Pt { x: 22.54, y: 7.46 });
    }

    #[test]
    fn bbox_for_unit_is_scoped_to_the_unit_geometry() {
        // A two-unit symbol: unit 1's pins sit near the origin, unit 2's far to the +x.
        // A common (unit 0) graphic straddles both. The whole-symbol bbox spans everything;
        // each unit's bbox must cover only that unit plus the common geometry.
        let src = r#"
        (kicad_sch
          (lib_symbols
            (symbol "Lib:DUAL"
              (symbol "DUAL_0_1"
                (rectangle (start -1 -1) (end 1 1)))
              (symbol "DUAL_1_1"
                (pin input line (at -5 5 0) (length 2) (name "A") (number "1")))
              (symbol "DUAL_2_1"
                (pin input line (at 50 -5 0) (length 2) (name "B") (number "2"))))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let lib = &sch.lib_symbols[0];
        let whole = lib.bbox.expect("whole-symbol bbox");
        let u1 = lib.bbox_for_unit(1).expect("unit 1 bbox");
        let u2 = lib.bbox_for_unit(2).expect("unit 2 bbox");

        // Whole spans the -5..50 x range (both units); each unit is scoped.
        assert_eq!(whole.0.x, -5.0);
        assert_eq!(whole.1.x, 50.0);
        // Unit 1: its pin at x=-5 plus the common rect (-1..1) → x in [-5, 1]. Unit 2's
        // pin at x=50 must NOT be included.
        assert_eq!(u1.0.x, -5.0);
        assert_eq!(u1.1.x, 1.0);
        // Unit 2: common rect (-1..1) plus its pin at x=50 → x in [-1, 50]. Unit 1's pin
        // at x=-5 must NOT be included.
        assert_eq!(u2.0.x, -1.0);
        assert_eq!(u2.1.x, 50.0);
    }

    #[test]
    fn bbox_for_unit_matches_whole_for_single_unit_symbol() {
        // When a symbol's geometry all lives on unit 0/1, the placed unit-1 box equals
        // the whole-symbol box — single-unit parts are unaffected by the per-unit path.
        let sch = Schematic::parse_str(SAMPLE).unwrap();
        let lib = sch.lib_symbol("Device:C").unwrap();
        assert_eq!(lib.bbox_for_unit(1), lib.bbox);
    }

    #[test]
    fn parses_full_text_effects() {
        let src = r#"
        (kicad_sch
          (symbol (lib_id "Device:R") (at 10 10 0) (unit 1) (uuid "u1")
            (property "Reference" "R1" (at 5 6 90)
              (effects (font (size 2.0 2.0) (thickness 0.3) bold italic
                (color 255 0 111 1)) (justify right top mirror)))
            (property "Value" "10k" (at 5 8 0)
              (effects (font (size 1.27 1.27)) (hide yes))))
          (hierarchical_label "NET" (at 30 30 180)
            (effects (font (size 1.27 1.27) (color 0 0 0 0)) (justify left))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let r1 = &sch.symbols[0];
        let refp = r1.properties.iter().find(|p| p.key == "Reference").unwrap();
        let e = &refp.effects;
        assert_eq!(e.size, 2.0);
        assert_eq!(e.thickness, Some(0.3));
        assert!(e.bold && e.italic && e.mirror);
        assert_eq!(e.color, Some((255, 0, 111, 1.0)));
        assert_eq!(e.h_justify, HJustify::Right);
        assert_eq!(e.v_justify, VJustify::Top);
        assert!(!e.hidden);
        assert_eq!(refp.at.map(|a| a.angle), Some(90.0));
        // Value field is hidden.
        let valp = r1.properties.iter().find(|p| p.key == "Value").unwrap();
        assert!(valp.effects.hidden);
        // Hierarchical label keeps its justify; alpha-0 colour is treated as unset.
        let lbl = &sch.labels[0];
        assert_eq!(lbl.kind, LabelKind::Hierarchical);
        assert_eq!(lbl.effects.h_justify, HJustify::Left);
        assert_eq!(lbl.effects.color, None);
    }

    #[test]
    fn parses_text_note_hyperlink() {
        // A KiCad `(text …)` note can carry `(effects (href "url"))` — capture it so the
        // viewer can make the text clickable.
        let src = r#"
        (kicad_sch
          (text "Link to design sheet" (at 100 100 0)
            (effects (font (size 1.27 1.27) (color 255 0 111 1)) (href "https://example.com/sheet")))
          (text "plain note" (at 50 50 0) (effects (font (size 1.27 1.27)))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let linked = sch.notes.iter().find(|n| n.text == "Link to design sheet").unwrap();
        assert_eq!(linked.effects.href.as_deref(), Some("https://example.com/sheet"));
        let plain = sch.notes.iter().find(|n| n.text == "plain note").unwrap();
        assert_eq!(plain.effects.href, None);
    }

    #[test]
    fn detects_both_hide_forms_so_old_boards_dont_overprint_fields() {
        // KiCad 7+ `(hide yes)` and the KiCad 5/6 bare `hide` token must BOTH mark a
        // field hidden, else an older board plots every footprint/datasheet/MPN field.
        let src = r#"
        (kicad_sch
          (symbol (lib_id "Device:R") (at 10 10 0) (unit 1) (uuid "u1")
            (property "Reference" "R1" (at 5 6 0) (effects (font (size 1.27 1.27))))
            (property "Footprint" "R_0603" (at 5 8 0)
              (effects (font (size 1.27 1.27)) hide))
            (property "Datasheet" "x.pdf" (at 5 10 0)
              (effects (font (size 1.27 1.27)) (hide yes)))
            (property "MPN" "RC0603" (at 5 12 0)
              (effects (font (size 1.27 1.27)) (hide no)))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let r1 = &sch.symbols[0];
        let by = |k: &str| r1.properties.iter().find(|p| p.key == k).unwrap();
        assert!(!by("Reference").effects.hidden, "Reference stays visible");
        assert!(by("Footprint").effects.hidden, "bare `hide` token must hide the field");
        assert!(by("Datasheet").effects.hidden, "(hide yes) must hide the field");
        assert!(!by("MPN").effects.hidden, "(hide no) must NOT hide the field");
    }

    #[test]
    fn kicad10_property_level_hide_is_honoured() {
        // KiCad 10 moved the field `(hide yes)` OUT of `(effects …)` and onto the
        // `(property …)` node directly. It must still mark the field hidden, else the
        // app overprints hidden descriptions/MPNs (the H1 "Standoff, Steel, M2.5 …"
        // report on the jetson-agx-thor-baseboard demo). A field whose *value* is the
        // literal word "hide" must NOT be treated as hidden.
        let src = r#"
        (kicad_sch
          (symbol (lib_id "Mechanical:MountingHole") (at 10 10 0) (unit 1) (uuid "h1")
            (property "Reference" "H1" (at 5 6 0) (effects (font (size 1.27 1.27))))
            (property "Description" "Standoff, Steel, M2.5"
              (at 15 -25 0) (show_name no) (hide yes)
              (effects (font (size 1.27 1.27)) (justify left bottom)))
            (property "Visible" "hide" (at 5 12 0)
              (effects (font (size 1.27 1.27))))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let h1 = &sch.symbols[0];
        let by = |k: &str| h1.properties.iter().find(|p| p.key == k).unwrap();
        assert!(!by("Reference").effects.hidden, "Reference stays visible");
        assert!(
            by("Description").effects.hidden,
            "KiCad 10 property-level (hide yes) must hide the field"
        );
        assert!(
            !by("Visible").effects.hidden,
            "a field whose value is the word \"hide\" must NOT read as hidden"
        );
    }

    #[test]
    fn parses_bus_alias_rule_area_table_and_pages() {
        let src = r#"
        (kicad_sch
          (bus_alias "I2C" (members "SDA" "SCL" "~{INT}"))
          (rule_area
            (polyline
              (pts (xy 10 10) (xy 30 10) (xy 30 20) (xy 10 20) (xy 10 10))
              (stroke (width 0) (type dash)) (fill (type none))
              (uuid "ra1")))
          (table
            (column_count 2)
            (cells
              (table_cell "Rail" (at 5 5 0) (size 20 5)
                (effects (font (size 1.27 1.27)) (justify left top)))
              (table_cell "+5V" (at 25 5 0) (size 20 5)
                (effects (font (size 1.27 1.27)) (justify left top)))
              (table_cell "" (at 45 5 0) (size 20 5))))
          (sheet_instances (path "/" (page "3")))
          (embedded_fonts no))
        "#;
        let sch = Schematic::parse_str(src).unwrap();

        // Bus alias captured with members (not expanded into nets).
        assert_eq!(sch.bus_aliases.len(), 1);
        assert_eq!(sch.bus_aliases[0].name, "I2C");
        assert_eq!(sch.bus_aliases[0].members, vec!["SDA", "SCL", "~{INT}"]);

        // Rule-area outline becomes a dashed sheet graphic (the boundary polyline).
        assert_eq!(sch.graphics.len(), 1);
        assert!(sch.graphics[0].dashed);
        assert!(matches!(sch.graphics[0].shape, Shape::Poly { .. }));
        assert_eq!(sch.graphics[0].uuid, "ra1");

        // Table cells become boxed notes (empty cell skipped).
        let texts: Vec<&str> = sch.notes.iter().map(|n| n.text.as_str()).collect();
        assert_eq!(texts, vec!["Rail", "+5V"]);
        assert!(sch.notes.iter().all(|n| n.boxed && n.box_size.is_some()));

        // Page label captured.
        assert_eq!(sch.sheet_instances.len(), 1);
        assert_eq!(sch.sheet_instances[0].path, "/");
        assert_eq!(sch.sheet_instances[0].page, "3");
    }

    #[test]
    fn rejects_non_schematic_root() {
        // A renamed board / symbol library must error, not parse as an empty sheet.
        let pcb = r#"(kicad_pcb (version 20240101) (layers (0 "F.Cu" signal)))"#;
        assert!(matches!(
            Schematic::parse_str(pcb),
            Err(ParseError::WrongRoot { expected: "kicad_sch", .. })
        ));
    }

    #[test]
    fn lib_symbol_bbox_excludes_property_anchors() {
        // A tiny body with a hidden Datasheet field parked 50 mm away. The bbox must
        // come from the body graphics only — property `(at …)` anchors would balloon
        // the placed-component hit area.
        let src = r#"
        (kicad_sch
          (lib_symbols
            (symbol "Device:R"
              (property "Reference" "R" (at 0 0 0))
              (property "Datasheet" "~" (at 50 50 0) (effects (hide yes)))
              (symbol "R_0_1"
                (rectangle (start -1.016 -2.54) (end 1.016 2.54)
                  (stroke (width 0.254)) (fill (type none)))))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        let (min, max) = sch.lib_symbol("Device:R").unwrap().bbox.expect("body bbox");
        assert!((max.x - 1.016).abs() < 1e-6 && (max.y - 2.54).abs() < 1e-6, "bbox from body, got {max:?}");
        assert!((min.x + 1.016).abs() < 1e-6 && (min.y + 2.54).abs() < 1e-6, "bbox from body, got {min:?}");
    }

    #[test]
    fn parses_per_placement_page_labels() {
        // A child sheet placement (KiCad ≥7) carries its hierarchical page numbers in
        // its own `(instances …)` block keyed by the parent's instance path — one
        // entry per context a re-used sheet appears in, not in the root sheet_instances.
        let src = r#"
        (kicad_sch
          (uuid "root")
          (sheet
            (at 50 50) (size 30 20)
            (uuid "placement-uuid")
            (property "Sheetname" "bank2")
            (property "Sheetfile" "io.kicad_sch")
            (instances
              (project "demo"
                (path "/root/aaaa" (page "41"))
                (path "/root/bbbb" (page "32"))))))
        "#;
        let sch = Schematic::parse_str(src).unwrap();
        assert_eq!(sch.sheets.len(), 1);
        let inst = &sch.sheets[0].instances;
        assert_eq!(inst.len(), 2);
        assert_eq!((inst[0].path.as_str(), inst[0].page.as_str()), ("/root/aaaa", "41"));
        assert_eq!((inst[1].path.as_str(), inst[1].page.as_str()), ("/root/bbbb", "32"));
    }
}
