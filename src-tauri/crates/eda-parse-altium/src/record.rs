//! Altium record framing and the text-record key/value model.
//!
//! Three framings occur, and picking the wrong one reads garbage without
//! erroring, so the framing is DETECTED per stream rather than assumed:
//!
//! - **Prefixed** — a `u32` prefix whose low 3 bytes are the payload length and
//!   whose high byte is the mode (0 text, non-zero binary). Every `.SchDoc`
//!   stream and the board's text streams (`Board6`, `Components6`, `Nets6`, …).
//! - **Blocks(n)** — a `u8` record type followed by `n` length-prefixed blocks.
//!   The board's primitive streams: tracks and arcs carry one block, texts two
//!   (the second is the string), pads six.
//! - **Flat** — not record-framed at all, e.g. `WideStrings6`.
//!
//! Masking the prefix length and ignoring the high byte happens to read the
//! right bytes for a text stream, but only the mode byte says which parser to
//! apply, so it is kept.

use std::collections::BTreeMap;

/// The mode byte of a record prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text,
    Binary,
}

/// One framed record, still undecoded.
#[derive(Debug, Clone)]
pub struct Raw {
    pub mode: Mode,
    /// Byte offset of the payload inside the stream — useful for diagnostics.
    pub offset: usize,
    /// Record type byte, for block-framed records only.
    pub kind: Option<u8>,
    /// The record body. Under block framing this is the FIRST block, which is
    /// the fixed-layout part every primitive has.
    pub payload: Vec<u8>,
    /// Blocks after the first: a text primitive's string, a pad's shape
    /// overrides. Empty under prefixed framing.
    pub extra: Vec<Vec<u8>>,
}

/// How a stream frames its records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `u32` prefix, low 3 bytes length, high byte mode.
    Prefixed,
    /// `u8` record type then `n` length-prefixed blocks.
    Blocks(u8),
    /// Not record-framed.
    Flat,
}

/// Highest block count tried when detecting. Pads use six.
const MAX_BLOCKS: u8 = 8;

/// Pick a stream's framing by walking it every way and keeping the one that
/// consumes the stream EXACTLY. Landing on the end is what makes this a real
/// test rather than a guess; a partial walk is rejected.
pub fn framing_of(stream: &[u8]) -> Framing {
    let text_ok = walk_prefixed(stream).map(|r| !r.is_empty()).unwrap_or(false);
    let looks_text = stream.len() > 4 && stream[3] == 0 && stream.get(4) == Some(&b'|');
    if text_ok && looks_text {
        return Framing::Prefixed;
    }
    for n in 1..=MAX_BLOCKS {
        if walk_blocks(stream, n).map(|r| !r.is_empty()).unwrap_or(false) {
            return Framing::Blocks(n);
        }
    }
    if text_ok {
        return Framing::Prefixed;
    }
    Framing::Flat
}

/// Split a stream into records using its detected framing. A stream that fits no
/// framing yields nothing rather than erroring: one opaque stream must not cost
/// the document.
pub fn records(stream: &[u8]) -> Vec<Raw> {
    records_with(stream, framing_of(stream))
}

/// Split a stream using a framing the caller already knows. Detection cannot
/// always tell block counts apart — a six-block pad also walks cleanly as three
/// two-block records — so streams with a known layout say so by name.
pub fn records_with(stream: &[u8], framing: Framing) -> Vec<Raw> {
    match framing {
        Framing::Prefixed => walk_prefixed(stream).unwrap_or_default(),
        Framing::Blocks(n) => walk_blocks(stream, n).unwrap_or_default(),
        Framing::Flat => Vec::new(),
    }
}

/// Walk the `u32` prefix framing. `None` when the walk does not land exactly on
/// the end of the stream.
fn walk_prefixed(stream: &[u8]) -> Option<Vec<Raw>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < stream.len() {
        if i + 4 > stream.len() {
            return None;
        }
        let len = u32::from_le_bytes([stream[i], stream[i + 1], stream[i + 2], 0]) as usize;
        let mode = if stream[i + 3] == 0 { Mode::Text } else { Mode::Binary };
        let start = i + 4;
        let payload = stream.get(start..start + len)?;
        out.push(Raw {
            mode,
            offset: start,
            kind: None,
            payload: payload.to_vec(),
            extra: Vec::new(),
        });
        i = start + len;
        if len == 0 {
            return None; // would not advance
        }
    }
    Some(out)
}

/// Walk the `u8` type + `n` length-prefixed blocks framing.
fn walk_blocks(stream: &[u8], n: u8) -> Option<Vec<Raw>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < stream.len() {
        let kind = *stream.get(i)?;
        i += 1;
        let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(n as usize);
        let start = i;
        for _ in 0..n {
            if i + 4 > stream.len() {
                return None;
            }
            let len = u32::from_le_bytes(stream[i..i + 4].try_into().ok()?) as usize;
            i += 4;
            blocks.push(stream.get(i..i + len)?.to_vec());
            i += len;
        }
        let payload = blocks.remove(0);
        out.push(Raw { mode: Mode::Binary, offset: start, kind: Some(kind), payload, extra: blocks });
    }
    Some(out)
}

/// A decoded text record: `|KEY=VALUE|KEY=VALUE|`.
///
/// Keys are looked up case-insensitively (the same field is `ComponentDescription`
/// in a `.SchDoc` and `COMPONENTDESCRIPTION` in a `.PcbDoc`), and a `%UTF8%`-prefixed
/// variant of a key wins over the plain one, which carries a lossy 8-bit form.
#[derive(Debug, Clone, Default)]
pub struct TextRecord {
    /// Fields in file order, with their original key casing.
    pub fields: Vec<(String, String)>,
    /// Upper-cased key -> index of its first occurrence in `fields`.
    index: BTreeMap<String, usize>,
}

impl TextRecord {
    /// Decode a text-record payload.
    pub fn parse(payload: &[u8]) -> TextRecord {
        let mut rec = TextRecord::default();
        // Trim the NUL terminator (and any padding) before splitting.
        let end = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
        for chunk in payload[..end].split(|&b| b == b'|') {
            if chunk.is_empty() {
                continue;
            }
            let Some(eq) = chunk.iter().position(|&b| b == b'=') else {
                continue;
            };
            let key = decode(&unescape_pipes(&chunk[..eq]));
            let val = decode(&unescape_pipes(&chunk[eq + 1..]));
            let up = key.to_ascii_uppercase();
            rec.index.entry(up).or_insert(rec.fields.len());
            rec.fields.push((key, val));
        }
        rec
    }

    /// Raw case-insensitive lookup — no `%UTF8%` preference.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.index
            .get(&key.to_ascii_uppercase())
            .map(|&i| self.fields[i].1.as_str())
    }

    /// Field value, preferring the `%UTF8%` variant when the file carries one.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.raw(&format!("%UTF8%{key}")).or_else(|| self.raw(key))
    }

    /// Field value, or the empty string.
    pub fn s(&self, key: &str) -> &str {
        self.get(key).unwrap_or("")
    }

    /// Field parsed as an integer. Altium writes plain decimal here.
    pub fn i(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Field parsed as a float, tolerating a trailing unit suffix (`43328.34mil`).
    pub fn f(&self, key: &str) -> Option<f64> {
        let v = self.get(key)?.trim();
        let num: String = v
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
            .collect();
        num.parse().ok()
    }

    /// Field read as a boolean. Altium writes `T`/`F` on documents and `TRUE`/`FALSE`
    /// or `1`/`0` in project files, so all three forms are accepted.
    pub fn b(&self, key: &str) -> bool {
        matches!(
            self.get(key).unwrap_or("").trim().to_ascii_uppercase().as_str(),
            "T" | "TRUE" | "1" | "Y" | "YES"
        )
    }

    /// True when the record carries the key at all (in either variant).
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// The `RECORD=` type number, when the record declares one.
    pub fn record_type(&self) -> Option<i64> {
        self.i("RECORD")
    }
}

/// Undo Altium's byte-level pipe escaping. This runs before character decoding:
/// the escape bytes are not valid text in either encoding.
///
/// - `0x8E 0x8E` -> a literal `0x8E` byte
/// - a lone `0x8E` -> `|`
/// - `0xA6` (broken bar) -> `|`
pub fn unescape_pipes(bytes: &[u8]) -> Vec<u8> {
    if !bytes.iter().any(|&b| b == 0x8E || b == 0xA6) {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0x8E if bytes.get(i + 1) == Some(&0x8E) => {
                out.push(0x8E);
                i += 2;
            }
            0x8E | 0xA6 => {
                out.push(b'|');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Decode record bytes to text. UTF-8 when the bytes are valid UTF-8 (which is
/// what a `%UTF8%` field carries), else Latin-1 — Altium's 8-bit fallback form.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Decode a UTF-16LE run (used by the board's string table).
pub fn decode_utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_record(payload: &[u8]) -> Vec<u8> {
        let mut v = (payload.len() as u32).to_le_bytes().to_vec();
        v[3] = 0; // text mode
        v.extend_from_slice(payload);
        v
    }

    /// Corner case 1: the high byte is the mode, not part of the length. A binary
    /// record must not be handed to the text parser.
    #[test]
    fn mode_byte_selects_the_parser() {
        let mut s = text_record(b"|RECORD=1|\0");
        s.extend_from_slice(&[3, 0, 0, 1, 0xAA, 0xBB, 0xCC]);
        let recs = records(&s);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].mode, Mode::Text);
        assert_eq!(recs[1].mode, Mode::Binary);
        assert_eq!(recs[1].payload, vec![0xAA, 0xBB, 0xCC]);
    }

    /// Corner case 2: a value containing a pipe survives.
    #[test]
    fn unescapes_pipes_in_values() {
        let mut payload = b"|TEXT=a".to_vec();
        payload.push(0x8E); // an escaped '|'
        payload.extend_from_slice(b"b");
        payload.extend_from_slice(&[0x8E, 0x8E]); // a literal 0x8E byte
        payload.push(0);
        let r = TextRecord::parse(&payload);
        assert_eq!(r.s("TEXT"), "a|b\u{8e}");
    }

    /// Corner case 3: the same field is spelled differently per document kind.
    #[test]
    fn keys_are_case_insensitive() {
        let r = TextRecord::parse(b"|COMPONENTDESCRIPTION=cap|\0");
        assert_eq!(r.s("ComponentDescription"), "cap");
        assert_eq!(r.s("componentdescription"), "cap");
    }

    /// Corner case 4: the `%UTF8%` variant wins, so `°C` does not become mojibake.
    #[test]
    fn prefers_the_utf8_variant() {
        let mut payload = b"|DESCRIPTION=NTC 100".to_vec();
        payload.extend_from_slice(&[0xB0]); // Latin-1 degree sign
        payload.extend_from_slice("C|%UTF8%DESCRIPTION=NTC 100\u{b0}C|\0".as_bytes());
        let r = TextRecord::parse(&payload);
        assert_eq!(r.s("DESCRIPTION"), "NTC 100°C");
        assert_eq!(r.raw("DESCRIPTION"), Some("NTC 100°C"), "Latin-1 fallback decodes too");
    }

    #[test]
    fn reads_numbers_units_and_booleans() {
        let r = TextRecord::parse(b"|X=43328.3462mil|N=-7|ON=T|OFF=F|\0");
        assert_eq!(r.f("X"), Some(43328.3462));
        assert_eq!(r.i("N"), Some(-7));
        assert!(r.b("ON"));
        assert!(!r.b("OFF"));
        assert!(!r.b("MISSING"));
    }

    #[test]
    fn decodes_utf16_runs() {
        let bytes: Vec<u8> = "U6_CH1".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_utf16(&bytes), "U6_CH1");
    }
}
