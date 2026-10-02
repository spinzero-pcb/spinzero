//! A bounds-checked little-endian cursor over the board database.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Err {
    pub offset: usize,
    pub what: String,
}

impl fmt::Display for Err {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {:#x}", self.what, self.offset)
    }
}

pub type Res<T> = Result<T, Err>;

pub struct R<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> R<'a> {
    pub fn new(buf: &'a [u8]) -> R<'a> {
        R { buf, pos: 0 }
    }

    pub fn err<T>(&self, what: impl Into<String>) -> Res<T> {
        Err(Err { offset: self.pos, what: what.into() })
    }

    pub fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn skip(&mut self, n: usize) -> Res<()> {
        if self.left() < n {
            return self.err(format!("needs {n} bytes, {} left", self.left()));
        }
        self.pos += n;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Res<&'a [u8]> {
        if self.left() < n {
            return self.err(format!("needs {n} bytes, {} left", self.left()));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Res<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Res<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Res<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Res<i32> {
        Ok(self.u32()? as i32)
    }

    /// An IEEE double stored as two little-endian words, high word first.
    pub fn f64(&mut self) -> Res<f64> {
        let hi = self.u32()? as u64;
        let lo = self.u32()? as u64;
        Ok(f64::from_bits((hi << 32) | lo))
    }

    /// Advance to the next multiple of four.
    pub fn align4(&mut self) -> Res<()> {
        let r = self.pos % 4;
        if r != 0 {
            self.skip(4 - r)?;
        }
        Ok(())
    }

    /// A NUL-terminated string inside a fixed field of `n` bytes, then
    /// optionally to the next four-byte boundary.
    pub fn fixed(&mut self, n: usize, align: bool) -> Res<String> {
        let b = self.bytes(n)?;
        let len = b.iter().position(|&c| c == 0).unwrap_or(n);
        let s = latin1(&b[..len]);
        if align {
            self.align4()?;
        }
        Ok(s)
    }

    pub fn fixed_str(&mut self, n: usize) -> Res<String> {
        self.fixed(n, false)
    }

    /// A NUL-terminated string of any length.
    pub fn cstr(&mut self, align: bool) -> Res<String> {
        let start = self.pos;
        let Some(end) = self.buf[start..].iter().position(|&c| c == 0) else {
            return self.err("unterminated string");
        };
        let s = latin1(&self.buf[start..start + end]);
        self.pos = start + end + 1;
        if align {
            self.align4()?;
        }
        Ok(s)
    }

    /// Read `n` u32 words.
    pub fn words(&mut self, n: usize) -> Res<Vec<u32>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(self.u32()?);
        }
        Ok(v)
    }
}

/// Allegro stores 8-bit text in the Windows code page; Latin-1 is a faithful
/// superset for everything a board names (net, refdes, layer, padstack).
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_is_stored_high_word_first() {
        let bits = 1.5f64.to_bits();
        let mut b = Vec::new();
        b.extend(((bits >> 32) as u32).to_le_bytes());
        b.extend((bits as u32).to_le_bytes());
        assert_eq!(R::new(&b).f64().unwrap(), 1.5);
    }

    #[test]
    fn reads_past_the_end_fail_with_their_offset() {
        let mut r = R::new(&[1, 2, 3]);
        let e = r.u32().unwrap_err();
        assert_eq!(e.offset, 0);
        assert!(e.to_string().contains("needs 4 bytes"));
    }
}
