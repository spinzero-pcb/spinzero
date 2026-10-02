//! Read-only MS-CFB (OLE compound file) reader.
//!
//! Altium's `.SchDoc`, `.PcbDoc`, `.SchLib` and `.PcbLib` and OrCAD Capture's
//! `.DSN` and `.OLB` are compound files: a directory tree of storages and streams
//! inside one file. We only ever read, so
//! this covers the header, the FAT/DIFAT chains, the mini-FAT, the directory
//! tree and stream extraction — nothing else.
//!
//! Both sector sizes occur in the wild (512-byte v3, 4096-byte v4), so neither
//! is hard-coded.

use std::collections::BTreeMap;

/// CFB file magic.
pub const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const FREE_SECT: u32 = 0xFFFF_FFFF;
/// Directory entry size, fixed by the spec.
const DIR_ENTRY: usize = 128;
/// Chain-walk cap. A corrupt FAT can loop; bail instead of hanging.
const MAX_CHAIN: usize = 1 << 22;

/// One stream in the compound file, addressed by its full path.
#[derive(Debug, Clone)]
pub struct Stream {
    /// `/`-joined path below the root, e.g. `Board6/Data`.
    pub path: String,
    pub data: Vec<u8>,
}

/// A parsed compound file: every stream, keyed by path.
#[derive(Debug, Clone)]
pub struct Cfb {
    streams: BTreeMap<String, Vec<u8>>,
    /// CLSID of the root entry. Altium stamps its own; a cheap identity check.
    pub root_clsid: [u8; 16],
}

/// True when the bytes start with the CFB magic.
pub fn is_cfb(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..8] == MAGIC
}

fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

impl Cfb {
    /// Parse a whole compound file held in memory.
    pub fn parse(bytes: &[u8]) -> Result<Cfb, String> {
        if !is_cfb(bytes) {
            return Err("not a compound file (bad magic)".into());
        }
        if bytes.len() < 512 {
            return Err("compound file truncated (header)".into());
        }
        let sector_shift = u16le(bytes, 30) as u32;
        let mini_shift = u16le(bytes, 32) as u32;
        if !(7..=20).contains(&sector_shift) || !(2..=20).contains(&mini_shift) {
            return Err(format!("unsupported sector shift {sector_shift}/{mini_shift}"));
        }
        let sec_size = 1usize << sector_shift;
        let mini_size = 1usize << mini_shift;
        let num_fat = u32le(bytes, 44) as usize;
        let first_dir = u32le(bytes, 48);
        let mini_cutoff = u32le(bytes, 56) as usize;
        let first_mini_fat = u32le(bytes, 60);
        let num_mini_fat = u32le(bytes, 64) as usize;
        let first_difat = u32le(bytes, 68);
        let num_difat = u32le(bytes, 72) as usize;

        let file = Sectors { bytes, sec_size };

        // DIFAT: 109 entries live in the header, the rest in a chain of DIFAT
        // sectors whose last slot points at the next one.
        let mut fat_sectors: Vec<u32> = Vec::with_capacity(num_fat);
        for i in 0..109 {
            let v = u32le(bytes, 76 + i * 4);
            if v == FREE_SECT || v == END_OF_CHAIN {
                break;
            }
            fat_sectors.push(v);
        }
        let mut next = first_difat;
        let per = sec_size / 4 - 1;
        for _ in 0..num_difat.min(MAX_CHAIN) {
            if next == END_OF_CHAIN || next == FREE_SECT {
                break;
            }
            let sec = file.sector(next)?;
            for i in 0..per {
                let v = u32le(sec, i * 4);
                if v == FREE_SECT || v == END_OF_CHAIN {
                    continue;
                }
                fat_sectors.push(v);
            }
            next = u32le(sec, per * 4);
        }

        // The FAT itself: one u32 per sector of the file.
        let mut fat: Vec<u32> = Vec::with_capacity(fat_sectors.len() * sec_size / 4);
        for &s in &fat_sectors {
            let sec = file.sector(s)?;
            for i in 0..sec_size / 4 {
                fat.push(u32le(sec, i * 4));
            }
        }
        if fat.is_empty() {
            return Err("compound file has no FAT".into());
        }

        // Directory stream, then the root entry — the root's chain is the mini
        // stream that all short streams are carved out of.
        let dir_bytes = read_chain(&file, &fat, first_dir, usize::MAX)?;
        if dir_bytes.len() < DIR_ENTRY {
            return Err("compound file has no directory".into());
        }
        let root_start = u32le(&dir_bytes, 116);
        let root_size = u32le(&dir_bytes, 120) as usize;
        let mut root_clsid = [0u8; 16];
        root_clsid.copy_from_slice(&dir_bytes[80..96]);
        let mini_stream = if root_size > 0 {
            read_chain(&file, &fat, root_start, root_size)?
        } else {
            Vec::new()
        };

        // Mini FAT, chained through the normal FAT.
        let mini_fat_bytes = if num_mini_fat > 0 {
            read_chain(&file, &fat, first_mini_fat, usize::MAX)?
        } else {
            Vec::new()
        };
        let mini_fat: Vec<u32> = (0..mini_fat_bytes.len() / 4)
            .map(|i| u32le(&mini_fat_bytes, i * 4))
            .collect();

        // Walk the directory tree from the root, building `/`-joined paths.
        let count = dir_bytes.len() / DIR_ENTRY;
        let mut streams = BTreeMap::new();
        let mut visited = vec![false; count];
        // Child pointer of the root entry, at offset 76 of its 128-byte record.
        let root_child = u32le(&dir_bytes, 76);
        let mut stack: Vec<(u32, String)> = vec![(root_child, String::new())];
        while let Some((id, prefix)) = stack.pop() {
            if id == FREE_SECT || id as usize >= count || visited[id as usize] {
                continue;
            }
            visited[id as usize] = true;
            let e = &dir_bytes[id as usize * DIR_ENTRY..(id as usize + 1) * DIR_ENTRY];
            let name = entry_name(e);
            let kind = e[66];
            let left = u32le(e, 68);
            let right = u32le(e, 72);
            let child = u32le(e, 76);
            let start = u32le(e, 116);
            let size = u32le(e, 120) as usize;
            // Siblings share this entry's prefix; children nest under its name.
            stack.push((left, prefix.clone()));
            stack.push((right, prefix.clone()));
            let path =
                if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            match kind {
                1 => stack.push((child, path)), // storage
                2 => {
                    let data = if size < mini_cutoff {
                        read_mini(&mini_stream, &mini_fat, mini_size, start, size)
                    } else {
                        read_chain(&file, &fat, start, size).unwrap_or_default()
                    };
                    streams.insert(path, data);
                }
                _ => {}
            }
        }

        Ok(Cfb { streams, root_clsid })
    }

    /// Stream bytes by exact path (`Board6/Data`).
    pub fn stream(&self, path: &str) -> Option<&[u8]> {
        self.streams.get(path).map(Vec::as_slice)
    }

    /// Stream bytes by path, matched case-insensitively. Altium is consistent in
    /// practice, but the container spec is not, and a missed stream is silent.
    pub fn stream_ci(&self, path: &str) -> Option<&[u8]> {
        if let Some(d) = self.streams.get(path) {
            return Some(d);
        }
        self.streams
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(path))
            .map(|(_, v)| v.as_slice())
    }

    /// Every stream path, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.streams.keys().map(String::as_str)
    }

    /// Every stream, sorted by path.
    pub fn streams(&self) -> impl Iterator<Item = Stream> + '_ {
        self.streams
            .iter()
            .map(|(p, d)| Stream { path: p.clone(), data: d.clone() })
    }

    pub fn len(&self) -> usize {
        self.streams.len()
    }

    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }
}

/// Sector addressing over the raw file bytes.
struct Sectors<'a> {
    bytes: &'a [u8],
    sec_size: usize,
}

impl Sectors<'_> {
    fn sector(&self, n: u32) -> Result<&[u8], String> {
        let off = (n as usize + 1) * self.sec_size;
        self.bytes
            .get(off..off + self.sec_size)
            .ok_or_else(|| format!("sector {n} out of range"))
    }
}

/// Follow a FAT chain from `start`, concatenating sectors, truncated to `size`
/// bytes (`usize::MAX` = take the whole chain).
fn read_chain(file: &Sectors, fat: &[u32], start: u32, size: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut cur = start;
    let mut steps = 0;
    while cur != END_OF_CHAIN && cur != FREE_SECT {
        if steps > MAX_CHAIN {
            return Err("FAT chain too long (corrupt?)".into());
        }
        steps += 1;
        out.extend_from_slice(file.sector(cur)?);
        if size != usize::MAX && out.len() >= size {
            break;
        }
        cur = *fat.get(cur as usize).ok_or("FAT chain out of range")?;
    }
    if size != usize::MAX {
        out.truncate(size);
    }
    Ok(out)
}

/// Read a short stream out of the mini stream, following the mini FAT.
fn read_mini(
    mini_stream: &[u8],
    mini_fat: &[u32],
    mini_size: usize,
    start: u32,
    size: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    let mut cur = start;
    let mut steps = 0;
    while cur != END_OF_CHAIN && cur != FREE_SECT && out.len() < size {
        if steps > MAX_CHAIN {
            break;
        }
        steps += 1;
        let off = cur as usize * mini_size;
        let Some(chunk) = mini_stream.get(off..(off + mini_size).min(mini_stream.len())) else {
            break;
        };
        out.extend_from_slice(chunk);
        let Some(&next) = mini_fat.get(cur as usize) else {
            break;
        };
        cur = next;
    }
    out.truncate(size);
    out
}

/// Directory entry name: UTF-16LE in the first 64 bytes, length in bytes
/// (including the NUL) at offset 64.
fn entry_name(e: &[u8]) -> String {
    let len = u16le(e, 64) as usize;
    let len = len.min(64);
    let chars = len / 2;
    let units: Vec<u16> = (0..chars)
        .map(|i| u16le(e, i * 2))
        .take_while(|&c| c != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_cfb() {
        assert!(!is_cfb(b"hello"));
        assert!(Cfb::parse(b"hello").is_err());
    }

    #[test]
    fn reads_a_hand_built_v3_container() {
        let cfb = Cfb::parse(&test_support::tiny_cfb()).expect("parse");
        assert_eq!(cfb.stream("Storage/Data"), Some(&b"hello altium"[..]));
        assert_eq!(cfb.paths().collect::<Vec<_>>(), vec!["Storage/Data"]);
    }

    #[test]
    fn stream_lookup_is_case_insensitive_as_a_fallback() {
        let cfb = Cfb::parse(&test_support::tiny_cfb()).expect("parse");
        assert!(cfb.stream("storage/data").is_none());
        assert_eq!(cfb.stream_ci("storage/data"), Some(&b"hello altium"[..]));
    }
}
