//! `.SchLib` and `.PcbLib` — the libraries a design's parts are drawn from.
//!
//! A design does not need them: symbol graphics and footprint primitives are
//! embedded in the documents themselves, which is why the extraction resolves
//! nothing against a library (plan §3.7). What a library file is FOR is a
//! library-level review — checking the parts an organisation ships before a
//! board is drawn with them.
//!
//! Both are compound files holding one storage per item, and both reuse the
//! record readers the document path already has. The framing differs in one way
//! that matters, and it is why this is a reader of its own rather than a loop
//! over [`Doc::records`]: a `.PcbDoc` gives each primitive TYPE its own stream,
//! so one block count describes the whole stream; a `.PcbLib` footprint
//! interleaves every type in one stream, so the block count changes from record
//! to record and is a property of the type byte.

use std::collections::BTreeMap;

use crate::doc::Doc;
use crate::pcb::{self, Arc, Fill, Pad, Region, Text, Track, Via};
use crate::record::{Mode, Raw, TextRecord};
use crate::sch::{self, SchDoc};

/// Storages that belong to the library itself rather than to one of its items.
const RESERVED: [&str; 6] = [
    "FILEHEADER",
    "FILEVERSIONINFO",
    "LIBRARY",
    "STORAGE",
    "SECTIONKEYS",
    "ADDITIONAL",
];

/// Altium's primitive type bytes, as a `.PcbLib` footprint stream writes them.
pub mod pcb_kind {
    pub const ARC: u8 = 1;
    pub const PAD: u8 = 2;
    pub const VIA: u8 = 3;
    pub const TRACK: u8 = 4;
    pub const TEXT: u8 = 5;
    pub const FILL: u8 = 6;
    pub const REGION: u8 = 11;
    pub const COMPONENT_BODY: u8 = 12;
}

/// One symbol in a `.SchLib`.
pub struct Symbol {
    /// The storage name, which is the symbol's library reference.
    pub name: String,
    /// The symbol's own records, read the same way a sheet's are.
    pub doc: SchDoc,
    /// Binary records the symbol's stream carries that this reader could not
    /// decode. A `.SchLib` writes its pins as binary records; the ones that
    /// decode reach `doc`, and these are what is left.
    pub binary_records_unread: usize,
}

/// A `.SchLib`.
pub struct SchLib {
    pub symbols: Vec<Symbol>,
}

impl SchLib {
    /// Binary records no reader here claimed. A symbol that reports any is
    /// missing part of itself, and saying so is the difference between a gap
    /// and a wrong answer.
    pub fn records_unread(&self) -> usize {
        self.symbols.iter().map(|s| s.binary_records_unread).sum()
    }
}

/// One footprint in a `.PcbLib`, in the units and orientation the `.PcbDoc`
/// reader uses: millimetres, Y-up.
#[derive(Default)]
pub struct Footprint {
    pub name: String,
    /// The `Parameters` side stream — description, height and the rest.
    pub params: TextRecord,
    pub pads: Vec<Pad>,
    pub tracks: Vec<Track>,
    pub arcs: Vec<Arc>,
    pub vias: Vec<Via>,
    pub texts: Vec<Text>,
    pub fills: Vec<Fill>,
    pub regions: Vec<Region>,
    /// Type bytes this reader does not know, by type. The walk STOPS at the
    /// first of them rather than guessing a length, so a footprint reporting any
    /// is incomplete — not merely simplified.
    pub unknown: BTreeMap<u8, usize>,
    /// Bytes the walk did not consume. Non-zero means the same thing.
    pub unread_bytes: usize,
}

/// A `.PcbLib`.
pub struct PcbLib {
    pub footprints: Vec<Footprint>,
}

/// How many length-prefixed blocks each primitive type is written in.
///
/// A pad is six blocks in a library exactly as it is in `Pads6`, and a text is
/// two — the record, then its string. Everything else is one. These are not
/// guessed at: the walk must land exactly on the end of the stream, which is
/// the same test [`crate::record::framing_of`] applies, and on the fixture
/// footprint only this table passes it.
fn blocks_of(kind: u8) -> Option<usize> {
    match kind {
        pcb_kind::PAD => Some(6),
        pcb_kind::TEXT => Some(2),
        pcb_kind::ARC
        | pcb_kind::VIA
        | pcb_kind::TRACK
        | pcb_kind::FILL
        | pcb_kind::REGION
        | pcb_kind::COMPONENT_BODY => Some(1),
        _ => None,
    }
}

/// Item storages of a library, in file order and without the library's own.
///
/// A storage is named by the item it holds, so this is also the item list.
/// Reading the storages rather than `Library/Data`'s index means a file whose
/// index has gone stale still yields every item it actually contains.
fn item_names(doc: &Doc) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for path in doc.cfb.paths() {
        let Some((head, _)) = path.split_once('/') else { continue };
        if RESERVED.contains(&head.to_ascii_uppercase().as_str()) {
            continue;
        }
        if !seen.iter().any(|s| s == head) {
            seen.push(head.to_string());
        }
    }
    seen
}

/// Read a `.SchLib`.
pub fn read_schlib(doc: &Doc) -> SchLib {
    let mut symbols = Vec::new();
    for name in item_names(doc) {
        let raw = doc.records(&name);
        if raw.is_empty() {
            continue;
        }
        // A symbol's stream opens with its own `RECORD=1`, where a sheet's opens
        // with a file-header record. `parse_records` reads index 0 as that
        // header, so a blank one is prepended to keep the ownership arithmetic
        // — `OwnerIndex` counts records AFTER the header — identical in both.
        let mut recs = vec![TextRecord::default()];
        recs.extend(raw.iter().map(|r| TextRecord::parse(&r.payload)));
        let mut doc = sch::parse_records(recs);

        // A library writes its pins as BINARY records, which the text path
        // above reads as empty. They are decoded here and given to the symbol's
        // own placement: a `.SchLib` storage holds exactly one component, so
        // there is no owner to resolve and nothing to guess.
        let mut binary_records_unread = 0;
        for r in raw.iter().filter(|r| r.mode == Mode::Binary) {
            match sch::parse_binary_pin(&r.payload) {
                Some(pin) => {
                    if let Some(c) = doc.components.first_mut() {
                        c.pins.push(pin);
                    }
                }
                None => binary_records_unread += 1,
            }
        }
        symbols.push(Symbol { name, doc, binary_records_unread });
    }
    SchLib { symbols }
}

/// Read a `.PcbLib`.
pub fn read_pcblib(doc: &Doc) -> PcbLib {
    let mut footprints = Vec::new();
    for name in item_names(doc) {
        let Some(data) = doc.stream(&name) else { continue };
        let mut fp = read_footprint(&name, data);
        // A side stream is one length-prefixed payload, not a record list.
        if let Some(body) = doc.stream(&format!("{name}/Parameters")).and_then(prefixed) {
            fp.params = TextRecord::parse(body);
        }
        footprints.push(fp);
    }
    PcbLib { footprints }
}

/// One `[u32 len][body]` payload.
fn prefixed(b: &[u8]) -> Option<&[u8]> {
    let len = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
    b.get(4..4 + len)
}

/// Walk one footprint's `Data` stream.
///
/// It opens with the footprint's own name as a length-prefixed Pascal string,
/// then runs `[u8 type][u32 len][block] …` per record, with the block count
/// decided by the type. Every primitive body is byte-for-byte the layout the
/// `.PcbDoc` streams carry, so the readers are shared rather than rewritten — a
/// footprint in a library and the same footprint placed on a board cannot then
/// disagree about a pad.
fn read_footprint(name: &str, data: &[u8]) -> Footprint {
    let mut fp = Footprint { name: name.to_string(), ..Footprint::default() };
    // A stream with no room for the name header is empty, not malformed: it
    // yields no primitives and no complaint.
    let Some(header) = prefixed(data) else { return fp };
    let mut i = 4 + header.len();
    // A library footprint's strings live in its own `WideStrings` side stream,
    // whose encoding differs from the board's. Until that is read a text
    // primitive falls back to the copy in its own second block, which is what
    // `read_text` already does when the index misses.
    let strings = BTreeMap::new();

    while i < data.len() {
        let kind = data[i];
        let Some(n) = blocks_of(kind) else {
            *fp.unknown.entry(kind).or_default() += 1;
            fp.unread_bytes = data.len() - i;
            break;
        };
        let start = i;
        i += 1;
        let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(n);
        let mut short = false;
        for _ in 0..n {
            let Some(len) =
                data.get(i..i + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes)
            else {
                short = true;
                break;
            };
            i += 4;
            let Some(body) = data.get(i..i + len as usize) else {
                short = true;
                break;
            };
            blocks.push(body.to_vec());
            i += len as usize;
        }
        if short {
            fp.unread_bytes = data.len() - start;
            break;
        }
        let payload = blocks.remove(0);
        let r = Raw { mode: Mode::Binary, offset: start, kind: Some(kind), payload, extra: blocks };
        match kind {
            pcb_kind::PAD => fp.pads.extend(pcb::read_pad(&r)),
            pcb_kind::TRACK => fp.tracks.extend(pcb::read_track(&r)),
            pcb_kind::ARC => fp.arcs.extend(pcb::read_arc(&r)),
            pcb_kind::VIA => fp.vias.extend(pcb::read_via(&r)),
            pcb_kind::TEXT => fp.texts.extend(pcb::read_text(&r, &strings)),
            pcb_kind::FILL => fp.fills.extend(pcb::read_fill(&r)),
            pcb_kind::REGION => fp.regions.extend(pcb::read_region(&r)),
            // A 3D body is geometry the board path models from its own stream;
            // in a library it is walked past rather than read.
            _ => {}
        }
    }
    fp
}
