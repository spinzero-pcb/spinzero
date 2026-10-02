//! Per-element Capture schematic geometry: the same `schematics/geometry.json`
//! the KiCad and Altium paths emit (`crate::sch_geom`), built from the pages of
//! a Capture design.
//!
//! The diff engine needs, per drawn object, the id the sheet SVG stamps as
//! `data-uuid`, its extent in sheet millimetres, and a POSITION-FREE content
//! signature, so a dragged part reads as moved and a changed one as edited.
//! Capture stores each part's artwork in symbol space and its placement as a
//! corner plus an orientation, so a part's signature is its library identity,
//! orientation and properties — none of which move with it. A wire's is its
//! shape relative to its first end.
//!
//! A folder placed by several blocks is several sheet instances of one page;
//! the page (`folder/page`, the sheet's `filename`) is the dedupe key, and the
//! diff maps it back to sheet numbers.

use std::collections::BTreeMap;

use eda_parse_orcad::capture::page::Graphic;
use eda_parse_orcad::capture::CaptureDoc;

use super::design::part_id;
use super::{mm, SheetInstance};
use crate::sch_geom::{r4, SchElem, SchGeometry, SheetGeom, SCH_GEOMETRY_SCHEMA};

fn bbox(pts: &[(f64, f64)]) -> [f64; 4] {
    if pts.is_empty() {
        return [0.0; 4];
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    [r4(x0), r4(y0), r4(x1 - x0), r4(y1 - y0)]
}

fn corners(b: (i32, i32, i32, i32)) -> Vec<(f64, f64)> {
    vec![(mm(b.0), mm(b.1)), (mm(b.2), mm(b.3))]
}

fn props_sig(props: &[(String, String)]) -> String {
    let mut v: Vec<String> = props.iter().map(|(k, v)| format!("{k}={v}")).collect();
    v.sort();
    v.join(";")
}

fn graphic_sig(kind: &str, g: &Graphic) -> String {
    format!(
        "{kind}|{}|{}|t{}m{}|{}",
        g.name,
        g.cache_name,
        g.orient.turns,
        g.orient.mirror as u8,
        props_sig(&g.props)
    )
}

pub fn build(doc: &CaptureDoc, sheets: &[SheetInstance]) -> SchGeometry {
    let mut files: BTreeMap<String, Vec<SchElem>> = BTreeMap::new();
    for inst in sheets {
        if files.contains_key(&inst.info.filename) {
            continue;
        }
        let page = &doc.folders[inst.folder].pages[inst.page];
        let mut out: Vec<SchElem> = Vec::new();
        let mut push = |uuid: String, kind: &'static str, pts: &[(f64, f64)], sig: String| {
            out.push(SchElem { uuid, kind, bbox: bbox(pts), sig });
        };

        for p in &page.parts {
            let mut pins: Vec<String> = p
                .pins
                .iter()
                .map(|q| format!("{}@{},{}", q.index, q.pos.0 - p.pos.0, q.pos.1 - p.pos.1))
                .collect();
            pins.sort();
            let mut disp: Vec<String> = p
                .display
                .iter()
                .map(|d| format!("{}@{},{}f{}r{}m{}", d.name, d.x, d.y, d.font, d.rotation, d.mode))
                .collect();
            disp.sort();
            let sig = format!(
                "symbol|{}|{}|{}|{}|u{}|t{}m{}|props[{}]|pins[{}]|disp[{}]",
                p.cache_name,
                p.package,
                p.reference,
                p.value,
                p.unit_index,
                p.orient.turns,
                p.orient.mirror as u8,
                props_sig(&p.props),
                pins.join(";"),
                disp.join(";"),
            );
            push(part_id(p.db_id), "symbol", &corners(p.bbox), sig);
        }

        for w in &page.wires {
            let kind = if w.bus { "bus" } else { "wire" };
            let sig = format!("{kind}|{},{}|w{}|s{}", w.b.0 - w.a.0, w.b.1 - w.a.1, w.width, w.style);
            push(part_id(w.db_id), kind, &[(mm(w.a.0), mm(w.a.1)), (mm(w.b.0), mm(w.b.1))], sig);
            for (i, al) in w.aliases.iter().enumerate() {
                let sig = format!("label|{}|f{}|r{}", al.name, al.font, al.rotation);
                push(format!("{}:a{i}", part_id(w.db_id)), "label", &[(mm(al.pos.0), mm(al.pos.1))], sig);
            }
        }

        for (kind, list) in [("power", &page.globals), ("label", &page.offpages), ("port", &page.ports)] {
            for g in list {
                push(part_id(g.db_id), kind, &corners(g.bbox), graphic_sig(kind, g));
            }
        }
        for g in &page.erc {
            push(part_id(g.db_id), "no_connect", &corners(g.bbox), graphic_sig("no_connect", g));
        }

        for b in &page.blocks {
            let mut pins: Vec<String> = b
                .pins
                .iter()
                .map(|q| format!("{}@{},{}b{}", q.name, q.pos.0 - b.rect.0, q.pos.1 - b.rect.1, q.bus as u8))
                .collect();
            pins.sort();
            let sig = format!(
                "sheet_symbol|{}|{}|{}|{}x{}|props[{}]|entries[{}]",
                b.name,
                b.reference,
                b.implementation,
                b.rect.2 - b.rect.0,
                b.rect.3 - b.rect.1,
                props_sig(&b.props),
                pins.join(";")
            );
            push(part_id(b.db_id), "sheet_symbol", &corners(b.rect), sig);
        }

        for g in &page.graphics {
            let body = g.body.as_ref().map(|b| format!("{:?}", b.prims)).unwrap_or_default();
            // The body is in its own space; hashing it keeps the signature
            // small while still telling an edited drawing from a moved one.
            let sig = format!("graphic|k{}|t{}m{}|{:016x}", g.kind, g.orient.turns, g.orient.mirror as u8, fnv(&body));
            push(part_id(g.db_id), "graphic", &corners(g.bbox), sig);
        }

        out.sort_by(|a, b| (a.kind, &a.uuid).cmp(&(b.kind, &b.uuid)));
        files.insert(inst.info.filename.clone(), out);
    }
    SchGeometry {
        schema: SCH_GEOMETRY_SCHEMA,
        units: "mm",
        sheets: files.into_iter().map(|(file, elements)| SheetGeom { file, elements }).collect(),
    }
}

/// FNV-1a, for a stable short digest of a drawing.
fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
