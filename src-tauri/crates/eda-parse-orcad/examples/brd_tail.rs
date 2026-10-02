//! Print the last blocks read and header facts (development aid).
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let db = eda_parse_orcad::allegro::open(std::path::Path::new(&p)).unwrap();
    println!("ver {} end27 {:#x} objects {} stop {:?}", db.header.ver.label(), db.header.end_0x27, db.header.object_count, db.stream.stopped);
    let n = db.stream.blocks.len();
    for b in &db.stream.blocks[n.saturating_sub(8)..] {
        println!("  {:#x} kind {:#04x} key {:#x}", b.offset, b.kind, b.key);
    }
    println!("counts {:?}", db.stream.counts);
}
