//! Structure framing for the modern (Library version 3.x) Capture grammar.
//!
//! Every structure in a page, cache, package or hierarchy stream opens with a
//! chain of one or more LONG prefixes, then one SHORT prefix, then a fixed
//! four-byte marker and a counted trail, then the type's own body:
//!
//! ```text
//! long  prefix × N   u8 type, u32 length, u32 zero
//! short prefix       u8 type, i16 pair count, count × (u32 name, u32 value)
//! marker             FF E4 5C 39, u32 trail length, trail bytes
//! body
//! ```
//!
//! Each long prefix's length runs from the end of its own nine bytes, so it
//! names an offset ("stop") inside the structure; the outermost stop is the end
//! of the whole structure. The short prefix carries the structure's property
//! pairs, both halves of which index the `Library` string table.
//!
//! How many long prefixes a structure has depends on its type, and the readers
//! below do not carry a table of it. The chain is DETECTED: every candidate
//! depth is checked for a well-formed chain (the same type byte at every link,
//! zero pads, stops that nest inside each other and inside the enclosing
//! record) that ends in a short prefix followed by the marker. The deepest
//! chain that satisfies all of that is the structure. A shallower reading of a
//! deeper chain fails the marker test, because it lands on a long prefix where
//! the short one should be; a deeper reading of a shallower chain would need a
//! short prefix whose bytes also read as a zero-padded length that fits, which
//! needs a property pair naming string 0 with value string 0. The corpus counts
//! of each type's detected depth are in the dump histogram, so a change shows.

use super::bytes::{Cur, Res};

/// The marker that ends a short prefix.
pub const MARKER: [u8; 4] = [0xFF, 0xE4, 0x5C, 0x39];

/// Deepest chain looked for. The deepest seen in the corpus is 5 (a cached
/// library part); the cap only bounds the search.
const MAX_DEPTH: usize = 8;

/// One framed structure, located but not yet decoded.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Structure type byte.
    pub kind: u8,
    /// Offset of the first long prefix.
    pub start: usize,
    /// Stop offsets of the long prefixes, outermost first. `stops[0]` is the
    /// end of the whole structure.
    pub stops: Vec<usize>,
    /// Short-prefix property pairs, as string-table indices.
    pub props: Vec<(u32, u32)>,
    /// First byte of the body (just past the marker's trail).
    pub body: usize,
}

impl Frame {
    /// End of the whole structure.
    pub fn end(&self) -> usize {
        self.stops[0]
    }

    pub fn depth(&self) -> usize {
        self.stops.len()
    }

    /// The smallest stop strictly above `pos`, or the end. Some bodies keep
    /// undecoded bytes before a field that sits at a known stop.
    pub fn next_stop_above(&self, pos: usize) -> usize {
        self.stops
            .iter()
            .copied()
            .filter(|&s| s > pos)
            .min()
            .unwrap_or(self.end())
    }
}

/// Try to read a long-prefix chain of exactly `depth` links at `start`,
/// followed by a short prefix and the marker. Returns the frame or `None`.
fn try_depth(buf: &[u8], start: usize, limit: usize, depth: usize) -> Option<Frame> {
    let kind = *buf.get(start)?;
    let mut stops = Vec::with_capacity(depth);
    let mut prev = limit;
    for i in 0..depth {
        let p = start + 9 * i;
        if p + 9 > limit || buf[p] != kind || buf[p + 5..p + 9] != [0, 0, 0, 0] {
            return None;
        }
        let len = u32::from_le_bytes([buf[p + 1], buf[p + 2], buf[p + 3], buf[p + 4]]) as usize;
        let stop = (p + 9).checked_add(len)?;
        if stop > prev {
            return None;
        }
        stops.push(stop);
        prev = stop;
    }
    let mut c = Cur::at(buf, start + 9 * depth, limit);
    if c.u8().ok()? != kind {
        return None;
    }
    let n = c.i16().ok()?;
    let mut props = Vec::new();
    for _ in 0..n.max(0) {
        let a = c.u32().ok()?;
        let b = c.u32().ok()?;
        props.push((a, b));
    }
    if c.bytes(4).ok()? != MARKER {
        return None;
    }
    let trail = c.u32().ok()? as usize;
    c.skip(trail).ok()?;
    // Every stop lies at or beyond the header.
    if stops.iter().any(|&s| s < c.pos) {
        return None;
    }
    Some(Frame { kind, start, stops, props, body: c.pos })
}

/// Locate the structure at the cursor, bounded by the cursor's end, and leave
/// the cursor at the start of its body.
pub fn read_frame(c: &mut Cur) -> Res<Frame> {
    for depth in (1..=MAX_DEPTH).rev() {
        if let Some(f) = try_depth(c.buf, c.pos, c.end, depth) {
            c.pos = f.body;
            return Ok(f);
        }
    }
    c.err(format!(
        "no well-formed structure frame (type byte {:?})",
        c.peek_u8()
    ))
}

/// As [`read_frame`], but the structure must be of type `kind`.
pub fn expect_frame(c: &mut Cur, kinds: &[u8]) -> Res<Frame> {
    let at = c.pos;
    let f = read_frame(c)?;
    if !kinds.contains(&f.kind) {
        c.pos = at;
        return c.err(format!("expected structure type {kinds:?}, found {}", f.kind));
    }
    Ok(f)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a framed structure the way Capture writes one.
    pub fn frame(kind: u8, depth: usize, props: &[(u32, u32)], body: &[u8]) -> Vec<u8> {
        let mut short = vec![kind];
        short.extend_from_slice(&(props.len() as i16).to_le_bytes());
        for (a, b) in props {
            short.extend_from_slice(&a.to_le_bytes());
            short.extend_from_slice(&b.to_le_bytes());
        }
        short.extend_from_slice(&MARKER);
        short.extend_from_slice(&0u32.to_le_bytes());
        // The innermost long prefix stops at the body; outer ones at the end.
        let mut out = Vec::new();
        let total_after_chain = short.len() + body.len();
        for i in 0..depth {
            let remaining_links = depth - i - 1;
            let len = if i + 1 == depth && depth > 1 {
                remaining_links * 9 + short.len()
            } else {
                remaining_links * 9 + total_after_chain
            };
            out.push(kind);
            out.extend_from_slice(&(len as u32).to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0]);
        }
        out.extend_from_slice(&short);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn detects_chain_depth() {
        for depth in 1..=5 {
            let bytes = frame(24, depth, &[(3, 4)], b"BODY");
            let mut c = Cur::new(&bytes);
            let f = read_frame(&mut c).unwrap();
            assert_eq!(f.depth(), depth);
            assert_eq!(f.kind, 24);
            assert_eq!(f.props, vec![(3, 4)]);
            assert_eq!(&bytes[f.body..f.end()], b"BODY");
        }
    }

    #[test]
    fn rejects_a_missing_marker() {
        let mut bytes = frame(13, 2, &[], b"xx");
        let m = bytes.iter().position(|&b| b == 0xFF).unwrap();
        bytes[m] = 0;
        assert!(read_frame(&mut Cur::new(&bytes)).is_err());
    }

    #[test]
    fn a_frame_cannot_overrun_its_parent() {
        let bytes = frame(20, 2, &[], b"0123456789");
        let mut c = Cur::at(&bytes, 0, bytes.len() - 1);
        assert!(read_frame(&mut c).is_err());
    }
}
