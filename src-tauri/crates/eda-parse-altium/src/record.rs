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
    /// Record type byte: the leading byte of a binary record's payload under
    /// either framing. `None` for a text record.
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
    /// `u16` index then one `u32`-length-prefixed TEXT payload. `Rules6` is
    /// written this way and nothing else in the corpus is, which is why it is
    /// named rather than detected: a two-byte prefix is not distinguishable
    /// from the other layouts by walking alone.
    Indexed,
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
        Framing::Indexed => walk_indexed(stream).unwrap_or_default(),
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
            // A prefixed binary record leads with its type byte, the same way a
            // block-framed one does; reading it keeps the histogram able to tell
            // one binary record from another.
            kind: match mode {
                Mode::Binary => payload.first().copied(),
                Mode::Text => None,
            },
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

/// Walk the `u16` index + one `u32`-length-prefixed text payload framing.
fn walk_indexed(stream: &[u8]) -> Option<Vec<Raw>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < stream.len() {
        if i + 6 > stream.len() {
            return None;
        }
        let len = u32::from_le_bytes(stream[i + 2..i + 6].try_into().ok()?) as usize;
        let start = i + 6;
        let payload = stream.get(start..start + len)?;
        out.push(Raw {
            mode: Mode::Text,
            offset: start,
            kind: None,
            payload: payload.to_vec(),
            extra: Vec::new(),
        });
        i = start + len;
        if len == 0 {
            return None;
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
            // Un-escaping runs in the domain the pair is written in. A `%UTF8%`
            // pair spells the escape character as its UTF-8 encoding (`C2 A6`),
            // so un-escaping its BYTES writes `0x7C` into the middle of a
            // multi-byte sequence and the decode falls back to a mojibake
            // `Â|`. The pair's own prefix says which domain it is.
            let (key, val) = if is_utf8_pair(chunk) {
                (
                    unescape_pipes_chars(&decode_utf8(&chunk[..eq])),
                    unescape_pipes_chars(&decode_utf8(&chunk[eq + 1..])),
                )
            } else {
                (
                    decode(&unescape_pipes(&chunk[..eq])),
                    decode(&unescape_pipes(&chunk[eq + 1..])),
                )
            };
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
    ///
    /// A twin that holds U+FFFD is broken: Altium wrote a replacement character
    /// where it could not convert. The plain key then wins if it is clean, because
    /// the plain key is Windows-1252 and `decode` reads it as such (`0x99` is the
    /// trade mark sign).
    pub fn get(&self, key: &str) -> Option<&str> {
        let twin = self.raw(&format!("%UTF8%{key}"));
        let plain = self.raw(key);
        match (twin, plain) {
            (Some(t), Some(p)) if t.contains('\u{FFFD}') && !p.contains('\u{FFFD}') => Some(p),
            (Some(t), _) => Some(t),
            (None, p) => p,
        }
    }

    /// Field value, or the empty string.
    pub fn s(&self, key: &str) -> &str {
        self.get(key).unwrap_or("")
    }

    /// Field parsed as an integer. Altium writes plain decimal here.
    pub fn i(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }

    /// Field parsed as a float, tolerating a trailing unit suffix
    /// (`43328.34mil`) and scientific notation.
    ///
    /// A board writes plain decimals with units, but a *rotation* arrives as
    /// ` 1.80000000000000E+0002`. Stopping at the `E` reads that as 1.8 — a
    /// silent 178-degree error on every rotated footprint — so the exponent is
    /// part of the number, while a `mil` suffix still is not.
    pub fn f(&self, key: &str) -> Option<f64> {
        let v = self.get(key)?.trim();
        let mut num = String::new();
        for (i, c) in v.char_indices() {
            let exponent = (c == 'e' || c == 'E')
                && !num.is_empty()
                && matches!(after_exponent(v, i), Some(d) if d.is_ascii_digit());
            if c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || exponent {
                num.push(c);
            } else {
                break;
            }
        }
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

    /// The `RECORD=` value verbatim.
    ///
    /// A schematic numbers its record types (`RECORD=34`); a board NAMES them
    /// (`RECORD=Board`, `RECORD=AdvancedPlacerOptions`). Reading only the number
    /// collapses every board text stream to "untyped", which is what
    /// [`record_type`](Self::record_type) does and why the histogram uses this.
    pub fn record_kind(&self) -> Option<&str> {
        self.raw("RECORD")
    }
}

/// The first character after an exponent marker at byte `at`, skipping its sign
/// — what tells `1.8E+0002` (a number) from a unit suffix.
fn after_exponent(v: &str, at: usize) -> Option<char> {
    v[at + 1..].chars().find(|c| *c != '+' && *c != '-')
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

/// True when a `KEY=VALUE` chunk is written in the `%UTF8%` domain.
fn is_utf8_pair(chunk: &[u8]) -> bool {
    chunk.len() >= 6 && chunk[..6].eq_ignore_ascii_case(b"%UTF8%")
}

/// Undo the same escaping over CHARACTERS, for a `%UTF8%` pair whose escape
/// characters arrive as their UTF-8 encodings rather than as raw bytes.
///
/// The rules are [`unescape_pipes`]'s, one domain up. A field carrying both
/// spellings proves they mean the same thing: the corpus writes
/// `SwapIDPart=<8E>&<8E>` and `%UTF8%SwapIDPart=<C2 A6>&<C2 A6>` on the same
/// pin, and both are the value `|&|`.
pub fn unescape_pipes_chars(text: &str) -> String {
    if !text.contains(['\u{8E}', '\u{A6}']) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{8E}' if chars.peek() == Some(&'\u{8E}') => {
                out.push('\u{8E}');
                chars.next();
            }
            '\u{8E}' | '\u{A6}' => out.push('|'),
            other => out.push(other),
        }
    }
    out
}

/// Decode bytes known to be in the `%UTF8%` domain, lossily rather than
/// erroring: one bad field must not cost the record.
fn decode_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Decode record bytes to text. UTF-8 when the bytes are valid UTF-8 (which is
/// what a `%UTF8%` field carries), else Latin-1 — Altium's 8-bit fallback form.
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

/// One 8-bit byte as CP1252, which is what Altium writes on a Western system.
///
/// Latin-1 and CP1252 agree everywhere except `0x80`-`0x9F`, where Latin-1 has
/// C1 control codes and CP1252 has the punctuation a designer actually types:
/// the motherboard's disclaimer opens with `0x93`, a left double quote, and
/// reading it as Latin-1 put a control character into the SVG and the design
/// JSON. An undefined slot keeps its code point rather than inventing one.
fn cp1252(b: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    match b {
        0x80..=0x9F => char::from_u32(u32::from(HIGH[usize::from(b) - 0x80])).unwrap_or(b as char),
        _ => b as char,
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
        // The doubled escape is what lets a real `0x8E` byte survive, and in
        // CP1252 that byte is a capital Z with caron: the character the
        // designer typed, and the reason Altium had to escape it at all.
        assert_eq!(r.s("TEXT"), "a|b\u{17d}");
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

    /// A text record with no `%UTF8%` twin is Windows-1252: `0x99` is the trade mark
    /// sign, not U+FFFD and not a C1 control.
    #[test]
    fn a_plain_key_without_a_twin_reads_as_windows_1252() {
        let mut payload = b"|ComponentDescription=STripFET".to_vec();
        payload.push(0x99);
        payload.extend_from_slice(b" F7|\0");
        let r = TextRecord::parse(&payload);
        assert_eq!(r.s("ComponentDescription"), "STripFET\u{2122} F7");
    }

    /// A twin that Altium wrote with a replacement character loses to a clean plain key.
    #[test]
    fn a_broken_utf8_twin_does_not_beat_a_clean_plain_key() {
        let mut payload = b"|ComponentDescription=STripFET".to_vec();
        payload.push(0x99);
        payload.extend_from_slice(b"|%UTF8%ComponentDescription=STripFET");
        payload.extend_from_slice(&[0xEF, 0xBF, 0xBD]); // U+FFFD
        payload.extend_from_slice(b"|\0");
        let r = TextRecord::parse(&payload);
        assert_eq!(r.s("ComponentDescription"), "STripFET\u{2122}");
    }

    /// Corner cases 2 and 4 together. The escape character in a `%UTF8%` pair
    /// arrives as `C2 A6`, not as a bare `A6`, so un-escaping its bytes would
    /// split the sequence and leave `Â|`. Found by the §8.3 differential: every
    /// pin on the corpus carries both spellings of the same `SwapIDPart`.
    #[test]
    fn unescapes_a_utf8_pair_in_the_character_domain() {
        let mut payload = b"|SwapIDPart=".to_vec();
        payload.extend_from_slice(&[0x8E, b'&', 0x8E]);
        payload.extend_from_slice(b"|%UTF8%SwapIDPart=");
        payload.extend_from_slice("\u{a6}&\u{a6}".as_bytes()); // C2 A6 & C2 A6
        payload.extend_from_slice(b"|\0");
        let r = TextRecord::parse(&payload);
        assert_eq!(r.raw("SwapIDPart"), Some("|&|"), "the 8-bit spelling");
        assert_eq!(r.get("SwapIDPart"), Some("|&|"), "and the UTF-8 one agrees");
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

    /// A rotation arrives in scientific notation. Stopping at the `E` reads
    /// `1.80000000000000E+0002` as 1.8 and rotates every part 178 degrees wrong.
    #[test]
    fn floats_read_scientific_notation_but_not_unit_suffixes() {
        let r = TextRecord::parse(b"|ROTATION= 1.80000000000000E+0002|H=43.3071mil|SMALL=1.5E-2| ");
        assert_eq!(r.f("ROTATION"), Some(180.0));
        assert_eq!(r.f("H"), Some(43.3071), "`mil` is a suffix, not an exponent");
        assert_eq!(r.f("SMALL"), Some(0.015));
    }

    #[test]
    fn decodes_utf16_runs() {
        let bytes: Vec<u8> = "U6_CH1".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_utf16(&bytes), "U6_CH1");
    }
}

#[cfg(test)]
mod indexed_tests {
    use super::*;

    /// `Rules6` is a `u16` index then one `u32`-length text payload. Walking
    /// cannot tell that from the other layouts, so the stream read as `Flat` and
    /// the board's whole design-rule table went missing — 68 rules on the eval
    /// board, which is what a layout review checks against.
    #[test]
    fn an_indexed_stream_yields_one_text_record_per_entry() {
        let mut b = Vec::new();
        for (i, text) in [b"|RULEKIND=Clearance|".as_slice(), b"|RULEKIND=Width|"].iter().enumerate() {
            b.extend_from_slice(&(i as u16).to_le_bytes());
            b.extend_from_slice(&(text.len() as u32).to_le_bytes());
            b.extend_from_slice(text);
        }
        let recs = records_with(&b, Framing::Indexed);
        assert_eq!(recs.len(), 2);
        let kinds: Vec<String> = recs
            .iter()
            .map(|r| TextRecord::parse(&r.payload).s("RULEKIND").to_string())
            .collect();
        assert_eq!(kinds, vec!["Clearance", "Width"]);

        // A walk that does not land on the end is not this framing.
        assert!(records_with(&b[..b.len() - 1], Framing::Indexed).is_empty());
    }
}
