//! `pcb-extract dump` for OrCAD files: a JSON summary of what the readers
//! decode from a Capture design or library (`.DSN`, `.OLB`) or an Allegro
//! board (`.brd`), for inspecting the format without building a bundle.
//! Development tool, not part of the bundle contract.

use std::collections::BTreeMap;
use std::path::Path;

use eda_parse_orcad::allegro::{self, Data};
use eda_parse_orcad::capture;
use serde_json::{json, Value};

use crate::altium::dump::Level;

pub fn dump(path: &Path, level: Level) -> Result<Value, String> {
    if allegro::sniff_path(path) {
        return dump_board(path, level);
    }
    dump_capture(path, level)
}

fn dump_capture(path: &Path, level: Level) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let cfb = ole_cfb::Cfb::parse(&bytes)?;
    let streams: Vec<Value> = cfb
        .paths()
        .map(|p| json!({ "name": p, "bytes": cfb.stream(p).map(|d| d.len()).unwrap_or(0) }))
        .collect();
    let doc = capture::parse(&bytes)?;
    let folders: Vec<Value> = doc
        .folders
        .iter()
        .map(|f| {
            let pages: Vec<Value> = f
                .pages
                .iter()
                .map(|p| {
                    let mut v = json!({
                        "name": p.name,
                        "stream": p.stream,
                        "size": p.size_name,
                        "parts": p.parts.len(),
                        "wires": p.wires.len(),
                        "blocks": p.blocks.len(),
                        "ports": p.ports.len(),
                        "power": p.globals.len(),
                        "offpage": p.offpages.len(),
                        "graphics": p.graphics.len(),
                    });
                    if let Some(t) = &p.truncated {
                        v["truncated"] = json!(t);
                    }
                    if level == Level::Full {
                        v["part_list"] = json!(p
                            .parts
                            .iter()
                            .map(|q| json!({ "db": q.db_id, "ref": q.reference, "value": q.value, "symbol": q.cache_name, "package": q.package, "at": [q.pos.0, q.pos.1] }))
                            .collect::<Vec<_>>());
                    }
                    v
                })
                .collect();
            json!({ "name": f.name, "visible": f.visible, "pages": pages, "page_errors": f.page_errors })
        })
        .collect();
    Ok(json!({
        "file": path.display().to_string(),
        "format": format!("Capture {}.{}", doc.lib.version.0, doc.lib.version.1),
        "legacy": !doc.lib.modern(),
        "role": format!("{:?}", doc.lib.role),
        "root": doc.root_folder().map(|f| f.name.clone()),
        "strings": doc.lib.strings.len(),
        "fonts": doc.lib.fonts.len(),
        "streams": streams,
        "folders": folders,
        "cache": {
            "symbols": doc.cache.symbol_count(),
            "packages": doc.cache.packages.len(),
            "cells": doc.cache.cells.len(),
        },
        "occurrences": doc.tree.as_ref().map(|t| t.count()),
        "occurrence_tree_error": doc.tree_error,
        "library_symbols": doc.library_symbols.len(),
        "cis_variants": doc.cis.as_ref().map(|c| c.variants.iter().map(|v| v.name.clone()).collect::<Vec<_>>()),
        "notes": doc.notes,
    }))
}

fn dump_board(path: &Path, level: Level) -> Result<Value, String> {
    let db = allegro::open(path)?;
    let h = &db.header;
    let counts: BTreeMap<String, usize> = db.stream.counts.iter().map(|(k, v)| (format!("{k:#04x}"), *v)).collect();
    let mut v = json!({
        "file": path.display().to_string(),
        "format": format!("Allegro {}", h.ver.label()),
        "magic": format!("{:#010x}", h.magic),
        "units": format!("{:?}", h.units),
        "divisor": h.divisor,
        "mm_per_unit": h.scale(),
        "strings": db.strings.len(),
        "objects_stated": h.object_count,
        "blocks_read": db.stream.blocks.len(),
        "blocks_by_type": counts,
        "misaligned": db.stream.misaligned,
        "constraint_xref_recovered": db.stream.recovered_0x27,
        "stopped": db.stream.stopped,
    });
    if level == Level::Full {
        let nets: Vec<&str> = db
            .stream
            .blocks
            .iter()
            .filter_map(|b| match &b.data {
                Data::Net { name, .. } => Some(db.s(*name)),
                _ => None,
            })
            .collect();
        v["nets"] = json!(nets);
    }
    Ok(v)
}
