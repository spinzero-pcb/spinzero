//! A hand-built compound file for the unit tests.

/// A minimal v3 (512-byte sector) compound file holding one storage with one
/// short stream: `Storage/Data` = "hello altium". Short means it lives in the
/// mini stream, so this exercises the mini-FAT path too.
pub fn tiny_cfb() -> Vec<u8> {
    const SEC: usize = 512;
    const END: u32 = 0xFFFF_FFFE;
    const FAT: u32 = 0xFFFF_FFFD;
    const FREE: u32 = 0xFFFF_FFFF;
    // header + FAT(0) + directory(1) + mini FAT(2) + mini stream(3)
    let mut f = vec![0u8; SEC * 5];

    let put32 = |f: &mut Vec<u8>, off: usize, v: u32| {
        f[off..off + 4].copy_from_slice(&v.to_le_bytes());
    };
    let put16 = |f: &mut Vec<u8>, off: usize, v: u16| {
        f[off..off + 2].copy_from_slice(&v.to_le_bytes());
    };

    f[..8].copy_from_slice(&super::MAGIC);
    put16(&mut f, 26, 0x003E); // minor version
    put16(&mut f, 26, 0x003E);
    put16(&mut f, 28, 0xFFFE); // byte order
    put16(&mut f, 30, 9); // sector shift -> 512
    put16(&mut f, 32, 6); // mini sector shift -> 64
    put32(&mut f, 44, 1); // FAT sectors
    put32(&mut f, 48, 1); // first directory sector
    put32(&mut f, 56, 4096); // mini stream cutoff
    put32(&mut f, 60, 2); // first mini FAT sector
    put32(&mut f, 64, 1); // mini FAT sectors
    put32(&mut f, 68, END); // first DIFAT sector
    put32(&mut f, 72, 0); // DIFAT sectors
    put32(&mut f, 76, 0); // DIFAT[0] -> FAT lives in sector 0
    for i in 1..109 {
        put32(&mut f, 76 + i * 4, FREE);
    }

    // FAT (sector 0 sits at file offset 512).
    let fat = SEC;
    for i in 0..SEC / 4 {
        put32(&mut f, fat + i * 4, FREE);
    }
    put32(&mut f, fat, FAT); // sector 0 is the FAT itself
    put32(&mut f, fat + 4, END); // 1: directory
    put32(&mut f, fat + 8, END); // 2: mini FAT
    put32(&mut f, fat + 12, END); // 3: mini stream

    // Mini FAT (sector 2): one used mini sector.
    let mfat = SEC * 3;
    for i in 0..SEC / 4 {
        put32(&mut f, mfat + i * 4, FREE);
    }
    put32(&mut f, mfat, END);

    // Mini stream (sector 3) holds the stream payload in mini sector 0.
    let mini = SEC * 4;
    f[mini..mini + 12].copy_from_slice(b"hello altium");

    // Directory (sector 1): root, storage, stream.
    let dir = SEC * 2;
    let entry = |f: &mut Vec<u8>,
                     idx: usize,
                     name: &str,
                     kind: u8,
                     left: u32,
                     right: u32,
                     child: u32,
                     start: u32,
                     size: u32| {
        let base = dir + idx * 128;
        let units: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        for (i, u) in units.iter().enumerate() {
            put16(f, base + i * 2, *u);
        }
        put16(f, base + 64, (units.len() * 2) as u16);
        f[base + 66] = kind;
        f[base + 67] = 1; // black
        put32(f, base + 68, left);
        put32(f, base + 72, right);
        put32(f, base + 76, child);
        put32(f, base + 116, start);
        put32(f, base + 120, size);
    };
    entry(&mut f, 0, "Root Entry", 5, FREE, FREE, 1, 3, 64);
    entry(&mut f, 1, "Storage", 1, FREE, FREE, 2, 0, 0);
    entry(&mut f, 2, "Data", 2, FREE, FREE, FREE, 0, 12);
    f
}
