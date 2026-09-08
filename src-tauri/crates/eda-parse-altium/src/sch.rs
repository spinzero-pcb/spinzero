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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
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
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub text: String,
    pub hidden: bool,
    pub uuid: String,
}

/// A pin (`RECORD=2`).
///
/// `at` is the record's `Location`, which is the end that touches the symbol
/// BODY — not the connection point. The wire meets the far end, `at` stepped
/// forward by `length` along the rotation. Verified on the corpus: 292 of 324
/// pins on one sheet land on a wire that way and none do from `Location`.
#[derive(Debug, Clone)]
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
}

/// Bit in `PinConglomerate` that marks a pin hidden.
///
/// UNVERIFIED against the corpus: none of the four reference designs contains a
/// pin that is demonstrably hidden, so this bit could not be confirmed the way
/// the sheet-entry step was. The implicit-supply rule it feeds is therefore
/// narrowed (see `altium::netlist`) and the count is reported in diagnostics,
/// so a wrong bit shows up as a number rather than silently re-wiring a design.
pub const PIN_HIDDEN_BIT: i64 = 0x08;

impl Pin {
    /// Rotation in degrees counter-clockwise, from the low two conglomerate bits.
    pub fn rotation(&self) -> i64 {
        (self.conglomerate & 0x3) * 90
    }

    /// Hidden pins are implicitly connected rather than drawn.
    pub fn hidden(&self) -> bool {
        self.conglomerate & PIN_HIDDEN_BIT != 0
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct NetLabel {
    pub at: Pt,
    pub text: String,
    pub uuid: String,
}

/// A power port (`RECORD=17`) — a net namer, not a component.
#[derive(Debug, Clone)]
pub struct PowerPort {
    pub at: Pt,
    pub text: String,
    pub style: i64,
    pub uuid: String,
}

/// A port (`RECORD=18`). It has TWO connection points, its left and right edges.
#[derive(Debug, Clone)]
pub struct Port {
    pub at: Pt,
    pub width: i64,
    pub name: String,
    pub io_type: i64,
    pub uuid: String,
}

impl Port {
    /// Both edges. Taking only `at` misses half of a design's port connections.
    pub fn terminals(&self) -> [Pt; 2] {
        [self.at, Pt { x: self.at.x + self.width * UNIT, y: self.at.y }]
    }
}

/// A sheet entry (`RECORD=16`) on a sheet symbol. It stores no connection point;
/// one is computed from the parent's rectangle, `Side` and `DistanceFromTop`.
#[derive(Debug, Clone)]
pub struct SheetEntry {
    pub name: String,
    pub side: i64,
    pub distance_from_top: i64,
    pub io_type: i64,
    pub uuid: String,
}

/// A sheet symbol (`RECORD=15`) — one placement of a child document.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct Wire {
    pub pts: Vec<Pt>,
    pub uuid: String,
}

/// A junction (`RECORD=29`) — makes a mid-span crossing a connection.
#[derive(Debug, Clone)]
pub struct Junction {
    pub at: Pt,
    pub uuid: String,
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

/// Sheet properties (`RECORD=31`).
#[derive(Debug, Clone, Default)]
pub struct SheetProps {
    /// Altium's own snap tolerance, in coordinate units.
    pub hot_spot_grid: i64,
    pub width: i64,
    pub height: i64,
    pub area_color: String,
    pub fonts: Vec<String>,
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
fn polyline(r: &TextRecord) -> Vec<Pt> {
    let n = r.i("LocationCount").unwrap_or(0).max(0);
    (1..=n)
        .filter(|i| r.has(&format!("X{i}")))
        .map(|i| pt(r, &format!("X{i}"), &format!("Y{i}")))
        .collect()
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
    parse_records(
        raw.iter()
            .map(|r| TextRecord::parse(&r.payload))
            .collect(),
    )
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
            _ => {}
        }
    }

    // Pass 2: everything else, attached to its owner where it has one.
    for (_i, r) in recs.iter().enumerate() {
        let Some(t) = r.record_type() else {
            continue;
        };
        let owner = owner_of(r);
        match t {
            1 | 15 | 43 | 44 => {}
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
                });
            }
            34 => {
                if let Some(c) = owner.and_then(|o| comp_at.get(&o)).copied() {
                    out.components[c].designator = r.s("Text").to_string();
                    out.components[c].designator_uuid = r.s("UniqueID").to_string();
                }
            }
            41 => {
                let p = Param {
                    name: r.s("Name").to_string(),
                    text: r.s("Text").to_string(),
                    hidden: r.b("IsHidden"),
                    uuid: r.s("UniqueID").to_string(),
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
                    });
                }
            }
            32 => {
                if let Some(s) = owner.and_then(|o| sym_at.get(&o)).copied() {
                    out.sheet_symbols[s].name = r.s("Text").to_string();
                }
            }
            33 => {
                if let Some(s) = owner.and_then(|o| sym_at.get(&o)).copied() {
                    out.sheet_symbols[s].filename = r.s("Text").to_string();
                }
            }
            17 => out.power_ports.push(PowerPort {
                at: pt(r, "Location.X", "Location.Y"),
                text: r.s("Text").to_string(),
                style: r.i("Style").unwrap_or(0),
                uuid: r.s("UniqueID").to_string(),
            }),
            18 => out.ports.push(Port {
                at: pt(r, "Location.X", "Location.Y"),
                width: r.i("Width").unwrap_or(0),
                name: r.s("Name").to_string(),
                io_type: r.i("IOType").unwrap_or(0),
                uuid: r.s("UniqueID").to_string(),
            }),
            25 => out.net_labels.push(NetLabel {
                at: pt(r, "Location.X", "Location.Y"),
                text: r.s("Text").to_string(),
                uuid: r.s("UniqueID").to_string(),
            }),
            26 => out.buses.push(Wire {
                pts: polyline(r),
                uuid: r.s("UniqueID").to_string(),
            }),
            27 => out.wires.push(Wire {
                pts: polyline(r),
                uuid: r.s("UniqueID").to_string(),
            }),
            29 => out.junctions.push(Junction {
                at: pt(r, "Location.X", "Location.Y"),
                uuid: r.s("UniqueID").to_string(),
            }),
            31 => {
                let custom = r.b("UseCustomSheet");
                let (w, h) = units::sheet_style_mm(r.i("SheetStyle").unwrap_or(0));
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
                    fonts: (1..=r.i("FontIdCount").unwrap_or(0))
                        .map(|i| r.s(&format!("FontName{i}")).to_string())
                        .collect(),
                };
            }
            211 => out.regions.push(Region {
                min: pt(r, "Location.X", "Location.Y"),
                max: pt(r, "Corner.X", "Corner.Y"),
                uuid: r.s("UniqueID").to_string(),
            }),
            4 | 28 | 209 => {
                let text = r.s("Text").to_string();
                if !text.is_empty() && owner.is_none() {
                    out.notes.push(text);
                }
            }
            t if GRAPHIC_RECORDS.contains(&t) => {
                // Symbol graphics are children of their component; use them for
                // the component's extent. Sheet-level graphics have no owner.
                let Some(c) = owner.and_then(|o| comp_at.get(&o)).copied() else {
                    continue;
                };
                let mut pts = polyline(r);
                if pts.is_empty() {
                    pts.push(pt(r, "Location.X", "Location.Y"));
                    if r.has("Corner.X") {
                        pts.push(pt(r, "Corner.X", "Corner.Y"));
                    }
                }
                for p in pts {
                    let b = out.components[c].bbox.get_or_insert((p, p));
                    b.0.x = b.0.x.min(p.x);
                    b.0.y = b.0.y.min(p.y);
                    b.1.x = b.1.x.max(p.x);
                    b.1.y = b.1.y.max(p.y);
                }
            }
            // Silent-drop guard: everything not modelled is counted by type.
            // 22 no-ERC, 30 image, 39 template, 46/47/48 implementation detail.
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
            entries: Vec::new(),
        };
        let e = |side, d| SheetEntry {
            name: "N".into(),
            side,
            distance_from_top: d,
            io_type: 0,
            uuid: String::new(),
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
            uuid: String::new(),
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
            description: String::new(),
            electrical: 4,
            conglomerate: c,
            length: 20,
            at: Pt { x: 0, y: 0 },
            part_id: 1,
            display_mode: 0,
            uuid: String::new(),
            hidden_net_name: String::new(),
        };
        assert_eq!(p(32).connection(), Pt { x: 20 * UNIT, y: 0 });
        assert_eq!(p(33).connection(), Pt { x: 0, y: 20 * UNIT });
        assert_eq!(p(34).connection(), Pt { x: -20 * UNIT, y: 0 });
        assert_eq!(p(35).connection(), Pt { x: 0, y: -20 * UNIT });
        assert_eq!(p(33).rotation(), 90);
    }
}
