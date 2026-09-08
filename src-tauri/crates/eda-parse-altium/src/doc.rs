//! An opened Altium document: the compound container plus decoded records.
//!
//! This layer stays format-faithful — it does not know about the review bundle.
//! It hands out records per stream and resolves the board's string table, which
//! is the one piece of decoding a caller cannot do on its own.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cfb::Cfb;
use crate::record::{self, Framing, Mode, Raw, TextRecord};

/// Which kind of document this is, from its file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Schematic,
    Board,
    SchLib,
    PcbLib,
    Unknown,
}

impl Kind {
    pub fn of(path: &Path) -> Kind {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "schdoc" => Kind::Schematic,
            "pcbdoc" => Kind::Board,
            "schlib" => Kind::SchLib,
            "pcblib" => Kind::PcbLib,
            _ => Kind::Unknown,
        }
    }
}

/// A parsed Altium document.
pub struct Doc {
    pub kind: Kind,
    pub cfb: Cfb,
}

impl Doc {
    pub fn open(path: &Path) -> Result<Doc, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Doc::parse(Kind::of(path), &bytes)
    }

    pub fn parse(kind: Kind, bytes: &[u8]) -> Result<Doc, String> {
        Ok(Doc { kind, cfb: Cfb::parse(bytes)? })
    }

    /// Framed records of a stream. The stream path is matched case-insensitively;
    /// `Foo` also finds `Foo/Data`, which is how Altium nests most of them.
    pub fn records(&self, stream: &str) -> Vec<Raw> {
        let Some(bytes) = self.stream(stream) else {
            return Vec::new();
        };
        match known_framing(stream) {
            Some(f) => record::records_with(bytes, f),
            None => record::records(bytes),
        }
    }

    /// The framing a stream is actually read with — a named layout when it has
    /// one, else the detected one.
    pub fn framing(&self, stream: &str) -> Framing {
        known_framing(stream).unwrap_or_else(|| {
            self.stream(stream).map(record::framing_of).unwrap_or(Framing::Flat)
        })
    }

    /// Stream bytes, trying `<name>` then `<name>/Data`.
    pub fn stream(&self, name: &str) -> Option<&[u8]> {
        self.cfb
            .stream_ci(name)
            .or_else(|| self.cfb.stream_ci(&format!("{name}/Data")))
    }

    /// Decoded text records of a stream, in file order. Binary records are
    /// skipped — a caller that wants them asks for [`Doc::records`].
    pub fn text_records(&self, stream: &str) -> Vec<TextRecord> {
        self.records(stream)
            .iter()
            .filter(|r| r.mode == Mode::Text)
            .map(|r| TextRecord::parse(&r.payload))
            .collect()
    }

    /// The board's `WideStrings6` table: index -> string.
    ///
    /// Each entry is a `u32` index, a `u32` **byte** length (not a character
    /// count) and then UTF-16LE including its NUL. Reading the length as
    /// characters halves every string.
    pub fn wide_strings(&self) -> BTreeMap<u32, String> {
        let mut out = BTreeMap::new();
        let Some(s) = self.stream("WideStrings6") else {
            return out;
        };
        let mut p = 0usize;
        while p + 8 <= s.len() {
            let idx = u32::from_le_bytes(s[p..p + 4].try_into().unwrap());
            let len = u32::from_le_bytes(s[p + 4..p + 8].try_into().unwrap()) as usize;
            p += 8;
            let Some(body) = s.get(p..p + len) else {
                break;
            };
            out.insert(idx, record::decode_utf16(body));
            p += len;
        }
        out
    }
}

/// A stream's record-type histogram: how many records, and how many of each
/// type. Text records are keyed by their `RECORD=` value — a number on a
/// schematic, a name on a board — and binary ones by their type byte. This is the format spike's checkable output — a framing or
/// decoding regression moves a count.
pub fn histogram_text(doc: &Doc) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "kind={:?} wide_strings={}
",
        doc.kind,
        doc.wide_strings().len()
    ));
    for name in doc.cfb.paths().map(str::to_string).collect::<Vec<_>>() {
        let recs = doc.records(&name);
        if recs.is_empty() {
            continue;
        }
        let mut types: BTreeMap<String, usize> = BTreeMap::new();
        for r in &recs {
            let key = match r.mode {
                Mode::Text => match TextRecord::parse(&r.payload).record_kind() {
                    Some(n) => format!("text:{n}"),
                    None => "text:-".to_string(),
                },
                Mode::Binary => match r.kind {
                    Some(b) => format!("bin:{b}"),
                    None => "bin:-".to_string(),
                },
            };
            *types.entry(key).or_default() += 1;
        }
        let types: Vec<String> = types.iter().map(|(k, v)| format!("{k}={v}")).collect();
        out.push_str(&format!(
            "{name}	{:?}	{}	{}
",
            doc.framing(&name),
            recs.len(),
            types.join(",")
        ));
    }
    out
}

/// Framing of the streams whose layout detection cannot pin down on its own.
///
/// A pad is six length-prefixed blocks, and three two-block records walk the
/// same bytes exactly — detection would split every pad in half and read the
/// second half's first byte as a record type. Naming the layout is the fix.
fn known_framing(stream: &str) -> Option<Framing> {
    let base = stream.split('/').next().unwrap_or(stream);
    match base.to_ascii_uppercase().as_str() {
        "PADS6" => Some(Framing::Blocks(6)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_stream_by_bare_name_or_data_child() {
        let doc = Doc::parse(Kind::Unknown, &crate::test_support::tiny_cfb()).unwrap();
        assert_eq!(doc.stream("Storage"), Some(&b"hello altium"[..]));
        assert_eq!(doc.stream("storage/data"), Some(&b"hello altium"[..]));
    }
}
