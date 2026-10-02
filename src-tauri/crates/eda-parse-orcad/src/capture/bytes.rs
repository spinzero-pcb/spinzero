//! A bounds-checked little-endian cursor over one Capture stream.
//!
//! Every read either succeeds or returns an error naming the offset; nothing in
//! this crate indexes a stream directly, so a mis-framed record fails where it
//! goes wrong instead of panicking three records later.

use std::fmt;

/// A decoding failure at a byte offset inside one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub offset: usize,
    pub what: String,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.offset)
    }
}

pub type Res<T> = Result<T, DecodeError>;

/// Cursor over a byte slice. `end` may be pulled in below the slice length to
/// confine a reader to one record.
#[derive(Clone)]
pub struct Cur<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
    pub end: usize,
}

impl<'a> Cur<'a> {
    pub fn new(buf: &'a [u8]) -> Cur<'a> {
        Cur { buf, pos: 0, end: buf.len() }
    }

    pub fn at(buf: &'a [u8], pos: usize, end: usize) -> Cur<'a> {
        Cur { buf, pos, end: end.min(buf.len()) }
    }

    pub fn err<T>(&self, what: impl Into<String>) -> Res<T> {
        Err(DecodeError { offset: self.pos, what: what.into() })
    }

    pub fn left(&self) -> usize {
        self.end.saturating_sub(self.pos)
    }

    pub fn is_done(&self) -> bool {
        self.pos >= self.end
    }

    pub fn need(&self, n: usize) -> Res<()> {
        if self.left() < n {
            return self.err(format!("needs {n} bytes, {} left", self.left()));
        }
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Res<()> {
        self.need(n)?;
        self.pos += n;
        Ok(())
    }

    pub fn seek(&mut self, pos: usize) -> Res<()> {
        if pos > self.end {
            return self.err(format!("seek to {pos} beyond record end {}", self.end));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Res<&'a [u8]> {
        self.need(n)?;
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn peek_u8(&self) -> Option<u8> {
        (self.pos < self.end).then(|| self.buf[self.pos])
    }

    pub fn peek_at(&self, off: usize) -> Option<u8> {
        let p = self.pos + off;
        (p < self.end).then(|| self.buf[p])
    }

    pub fn u8(&mut self) -> Res<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Res<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Res<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Res<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Res<i32> {
        Ok(self.u32()? as i32)
    }

    /// Capture's counted string: u16 length, that many bytes, then a NUL the
    /// length does not count. A non-zero terminator means the reader is not
    /// where it thinks it is, which is the main guard against a mis-frame
    /// silently swallowing the rest of a stream.
    pub fn lstr(&mut self) -> Res<String> {
        let start = self.pos;
        let n = self.u16()? as usize;
        let b = self.bytes(n)?;
        if self.u8()? != 0 {
            self.pos = start;
            return self.err("string terminator is not NUL");
        }
        Ok(decode_1252(b))
    }

    /// A NUL-terminated string inside a fixed-size field.
    pub fn fixed_str(&mut self, n: usize) -> Res<String> {
        let b = self.bytes(n)?;
        let len = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(decode_1252(&b[..len]))
    }
}

/// Windows-1252 to UTF-8. Capture writes the ANSI code page; the 0x80..0x9F
/// block is where 1252 differs from Latin-1 (smart quotes, the euro sign, µ is
/// already Latin-1), so it is mapped explicitly rather than read as control
/// characters.
pub fn decode_1252(b: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| match c {
            0x80..=0x9F => HIGH[(c - 0x80) as usize],
            _ => c as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counted_string_requires_its_terminator() {
        let mut c = Cur::new(&[3, 0, b'L', b'E', b'D', 0, 9]);
        assert_eq!(c.lstr().unwrap(), "LED");
        assert_eq!(c.pos, 6);
        let mut bad = Cur::new(&[2, 0, b'A', b'B', b'C']);
        assert!(bad.lstr().is_err());
        assert_eq!(bad.pos, 0, "a failed string leaves the cursor where it was");
    }

    #[test]
    fn code_page_1252_high_block() {
        assert_eq!(decode_1252(&[0x93, b'x', 0x94, 0xB5]), "“x”µ");
    }

    #[test]
    fn reads_are_bounded_by_the_record_end() {
        let data = [1u8, 2, 3, 4, 5, 6];
        let mut c = Cur::at(&data, 0, 3);
        assert!(c.u32().is_err());
        assert_eq!(c.u16().unwrap(), 0x0201);
    }
}
