//! Re-encoding for the images Altium embeds in a document.
//!
//! Altium's `Storage` stream keeps whatever the designer dropped on the sheet.
//! A PNG or a JPEG is already compressed and passes straight through, but the
//! corpus is full of **uncompressed 24-bit BMPs** — a title-block logo runs to
//! 1.3 MB, and base64 in an SVG makes that 1.8 MB. One corpus project's eight
//! sheets carried 19 MB of image against 700 KB of actual drawing.
//!
//! So a BMP is re-encoded as a PNG here. Nothing else about the picture
//! changes: same pixels, same dimensions, just deflated with the zlib the crate
//! already links for the `Storage` stream itself.

use std::io::Write as _;

/// The PNG file signature.
const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// True for a payload already in a format a browser draws.
pub fn is_web_format(b: &[u8]) -> bool {
    b.starts_with(&SIG) || b.starts_with(&[0xFF, 0xD8, 0xFF]) || b.starts_with(b"GIF8")
}

/// Re-encode an uncompressed BMP as a PNG, or `None` when the bytes are not a
/// BMP this understands.
///
/// Deliberately narrow: `BI_RGB` at 8, 24 or 32 bits per pixel, which is every
/// BMP in the corpus — a logo saved from a paint program is usually palettised,
/// not truecolour. A run-length-encoded or sub-byte BMP returns `None` and the
/// caller keeps the original, which still draws, just larger.
pub fn bmp_to_png(b: &[u8]) -> Option<Vec<u8>> {
    if !b.starts_with(b"BM") || b.len() < 54 {
        return None;
    }
    let u32_at = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    };
    let i32_at = |o: usize| -> Option<i32> {
        Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    };
    let u16_at = |o: usize| -> Option<u16> {
        Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
    };

    let data_at = u32_at(10)? as usize;
    let width = i32_at(18)?;
    let raw_height = i32_at(22)?;
    let bpp = u16_at(28)?;
    let compression = u32_at(30)?;
    if compression != 0 || !(bpp == 8 || bpp == 24 || bpp == 32) || width <= 0 || raw_height == 0 {
        return None;
    }
    // A palettised BMP stores an index per pixel; the table sits between the
    // info header and the pixels, four bytes an entry, blue first like
    // everything else in the format.
    let palette: Vec<[u8; 3]> = if bpp == 8 {
        let start = 14 + u32_at(14)? as usize;
        let table = b.get(start..data_at)?;
        table.chunks_exact(4).map(|e| [e[2], e[1], e[0]]).collect()
    } else {
        Vec::new()
    };
    if bpp == 8 && palette.is_empty() {
        return None;
    }
    // A negative height means the rows are stored top-down; the usual BMP is
    // bottom-up, which is why a naive read shows the picture upside down.
    let bottom_up = raw_height > 0;
    let (w, h) = (width as usize, raw_height.unsigned_abs() as usize);
    // Guard against a header that claims more pixels than the file can hold.
    let bytes_per_px = bpp as usize / 8;
    let stride = (w * bytes_per_px).div_ceil(4) * 4;
    if data_at.saturating_add(stride.checked_mul(h)?) > b.len() {
        return None;
    }

    // PNG scanlines: one filter byte (0 = none) then RGB(A), which is BGR(A)
    // the other way round.
    let out_px = if bpp == 32 { 4 } else { 3 };
    let mut raw = Vec::with_capacity(h * (1 + w * out_px));
    for row in 0..h {
        let src_row = if bottom_up { h - 1 - row } else { row };
        let base = data_at + src_row * stride;
        raw.push(0);
        for x in 0..w {
            let p = base + x * bytes_per_px;
            if bpp == 8 {
                let rgb = palette.get(b[p] as usize).copied().unwrap_or([0, 0, 0]);
                raw.extend_from_slice(&rgb);
                continue;
            }
            raw.push(b[p + 2]);
            raw.push(b[p + 1]);
            raw.push(b[p]);
            if out_px == 4 {
                raw.push(b[p + 3]);
            }
        }
    }

    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&raw).ok()?;
    let idat = z.finish().ok()?;

    let mut png = Vec::with_capacity(idat.len() + 64);
    png.extend_from_slice(&SIG);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if out_px == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &idat);
    chunk(&mut png, b"IEND", &[]);
    Some(png)
}

/// Append one PNG chunk: length, type, payload, CRC over type + payload.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// PNG's CRC-32 (the standard reflected polynomial), computed a byte at a time.
struct Crc(u32);

impl Crc {
    fn new() -> Crc {
        Crc(0xFFFF_FFFF)
    }
    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u32;
            for _ in 0..8 {
                self.0 = if self.0 & 1 != 0 { (self.0 >> 1) ^ 0xEDB8_8320 } else { self.0 >> 1 };
            }
        }
    }
    fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 bottom-up 24-bit BMP: red, green on the bottom row; blue, white on
    /// the top. Each row is 6 bytes padded to 8.
    fn bmp_2x2() -> Vec<u8> {
        let mut b = vec![0u8; 54];
        b[0..2].copy_from_slice(b"BM");
        b[10..14].copy_from_slice(&54u32.to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&2i32.to_le_bytes());
        b[22..26].copy_from_slice(&2i32.to_le_bytes());
        b[26..28].copy_from_slice(&1u16.to_le_bytes());
        b[28..30].copy_from_slice(&24u16.to_le_bytes());
        // Bottom row first, BGR.
        b.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // red, green
        b.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // blue, white
        b
    }

    #[test]
    fn a_bmp_becomes_a_png_and_shrinks() {
        let bmp = bmp_2x2();
        let png = bmp_to_png(&bmp).expect("a 24-bit BI_RGB bmp converts");
        assert!(png.starts_with(&SIG));
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 2, "width");
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 2, "height");
        assert_eq!(png[24], 8, "8 bits per channel");
        assert_eq!(png[25], 2, "truecolour, no alpha");
        assert!(png.ends_with(b"IEND\xAE\x42\x60\x82"), "the IEND chunk and its known CRC");
    }

    /// The rows come out the right way up: a bottom-up BMP's last row is the
    /// PNG's first, and reading it straight through would print the logo
    /// upside down with no error anywhere.
    #[test]
    fn a_bottom_up_bmp_is_not_flipped() {
        use std::io::Read as _;
        let png = bmp_to_png(&bmp_2x2()).unwrap();
        // IHDR is 25 bytes of chunk; the IDAT length/type follow it.
        let idat_at = 8 + 25;
        let len = u32::from_be_bytes(png[idat_at..idat_at + 4].try_into().unwrap()) as usize;
        let z = &png[idat_at + 8..idat_at + 8 + len];
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(z).read_to_end(&mut raw).unwrap();
        // filter, blue, white | filter, red, green
        assert_eq!(raw, vec![0, 0, 0, 255, 255, 255, 255, 0, 255, 0, 0, 0, 255, 0]);
    }

    /// A logo saved from a paint program is usually palettised, and reading it
    /// as truecolour would print the palette indices as pixels.
    #[test]
    fn a_palettised_bmp_resolves_through_its_table() {
        use std::io::Read as _;
        let mut b = vec![0u8; 54];
        b[0..2].copy_from_slice(b"BM");
        b[10..14].copy_from_slice(&(54u32 + 8).to_le_bytes()); // 2 palette entries
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&2i32.to_le_bytes());
        b[22..26].copy_from_slice(&1i32.to_le_bytes());
        b[26..28].copy_from_slice(&1u16.to_le_bytes());
        b[28..30].copy_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&[0, 0, 255, 0]); // index 0 -> red (BGRA)
        b.extend_from_slice(&[0, 255, 0, 0]); // index 1 -> green
        b.extend_from_slice(&[0, 1, 0, 0]); // one row: index 0, index 1, padded
        let png = bmp_to_png(&b).expect("an 8-bit bmp converts");
        let idat_at = 8 + 25;
        let len = u32::from_be_bytes(png[idat_at..idat_at + 4].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&png[idat_at + 8..idat_at + 8 + len])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, vec![0, 255, 0, 0, 0, 255, 0], "filter, red, green");
    }

    #[test]
    fn a_format_this_does_not_understand_is_left_alone() {
        assert!(bmp_to_png(b"not a bitmap at all, really").is_none());
        let mut rle = bmp_2x2();
        rle[30..34].copy_from_slice(&1u32.to_le_bytes()); // BI_RLE8
        assert!(bmp_to_png(&rle).is_none(), "a compressed bmp is not decoded");
        let mut mono = bmp_2x2();
        mono[28..30].copy_from_slice(&1u16.to_le_bytes());
        assert!(bmp_to_png(&mono).is_none(), "a sub-byte depth is not decoded");
    }

    #[test]
    fn already_compressed_formats_are_recognised() {
        assert!(is_web_format(&SIG));
        assert!(is_web_format(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!is_web_format(b"BM"));
    }
}
