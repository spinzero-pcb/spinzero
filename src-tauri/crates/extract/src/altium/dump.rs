//! `pcb-extract dump` — the Altium format spike's debug output.
//!
//! Prints an Altium document's streams and records as JSON so the format can be
//! inspected without a bundle build. Development tool, not part of the bundle
//! contract.

use std::path::Path;

use eda_parse_altium::record::{Mode, TextRecord};
use eda_parse_altium::{Doc, Kind};
use serde_json::{json, Value};

/// How much of a document to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Stream inventory plus per-type record counts. The M0 differential input.
    Summary,
    /// Every record, decoded.
    Full,
}

/// Dump one document. `only` limits the output to one stream when set.
pub fn dump(path: &Path, level: Level, only: Option<&str>) -> Result<Value, String> {
    dump_head(path, level, only, 48)
}

/// As [`dump`], with the raw-byte preview length under the caller's control.
pub fn dump_head(
    path: &Path,
    level: Level,
    only: Option<&str>,
    head: usize,
) -> Result<Value, String> {
    let kind = Kind::of(path);
    if kind == Kind::Unknown {
        return Err(format!("not an Altium document: {}", path.display()));
    }
    let doc = Doc::open(path)?;
    let mut streams = Vec::new();
    for name in doc.cfb.paths().map(str::to_string).collect::<Vec<_>>() {
        if only.map(|o| !name.eq_ignore_ascii_case(o)).unwrap_or(false) {
            continue;
        }
        streams.push(dump_stream(&doc, &name, level, head));
    }
    let wide = doc.wide_strings();
    Ok(json!({
        "file": path.display().to_string(),
        "kind": format!("{kind:?}"),
        // The board's string table: `Texts6` references it by index, and it is
        // where the placed (channel-suffixed) designators actually live.
        "wide_strings": wide.len(),
        "wide_strings_sample": wide.iter().take(8).map(|(i, s)| json!([i, s])).collect::<Vec<_>>(),
        "streams": streams,
    }))
}

fn dump_stream(doc: &Doc, name: &str, level: Level, head: usize) -> Value {
    let bytes = doc.cfb.stream_ci(name).unwrap_or_default();
    let raws = doc.records(name);
    let mut text = 0usize;
    let mut binary = 0usize;
    // Record-type histogram: `RECORD=n` for text records, the leading type byte
    // for binary ones. This is the fixture the format spike checks in.
    let mut types: std::collections::BTreeMap<String, usize> = Default::default();
    let mut records = Vec::new();
    for r in &raws {
        match r.mode {
            Mode::Text => {
                text += 1;
                let t = TextRecord::parse(&r.payload);
                let key = match t.record_type() {
                    Some(n) => format!("text:{n}"),
                    None => "text:-".to_string(),
                };
                *types.entry(key).or_default() += 1;
                if level == Level::Full {
                    records.push(json!({
                        "mode": "text",
                        "offset": r.offset,
                        "fields": t
                            .fields
                            .iter()
                            .map(|(k, v)| json!([k, v]))
                            .collect::<Vec<_>>(),
                    }));
                }
            }
            Mode::Binary => {
                binary += 1;
                let key = match r.kind {
                    Some(b) => format!("bin:{b}"),
                    None => "bin:-".to_string(),
                };
                *types.entry(key).or_default() += 1;
                if level == Level::Full {
                    records.push(json!({
                        "mode": "binary",
                        "offset": r.offset,
                        "kind": r.kind,
                        "len": r.payload.len(),
                        "blocks": r.extra.len() + 1,
                        "head": hex(&r.payload[..r.payload.len().min(64)]),
                        "tail_blocks": r
                            .extra
                            .iter()
                            .map(|b| hex(&b[..b.len().min(32)]))
                            .collect::<Vec<_>>(),
                    }));
                }
            }
        }
    }
    let mut v = json!({
        "stream": name,
        "bytes": bytes.len(),
        "framing": format!("{:?}", doc.framing(name)),
        "raw_head": hex(&bytes[..bytes.len().min(head)]),
        "records": raws.len(),
        "text_records": text,
        "binary_records": binary,
        "types": types,
    });
    if level == Level::Full {
        v["decoded"] = Value::Array(records);
    }
    v
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
