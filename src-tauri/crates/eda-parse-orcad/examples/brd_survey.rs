//! Walk every .brd under a directory and report how far the object stream reads.
use std::path::{Path, PathBuf};
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() { let p = e.path(); if p.is_dir() { if p.file_name().map(|n| n != ".git").unwrap_or(true) { walk(&p, out); } } else if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("brd")).unwrap_or(false) { out.push(p); } }
}
fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) { walk(Path::new(&a), &mut files); }
    files.sort();
    for f in files {
        let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
        match eda_parse_orcad::allegro::open(&f) {
            Ok(db) => {
                let last = db.stream.blocks.last().map(|b| b.offset).unwrap_or(0);
                println!("{:5} {:>9} blocks {:7}/{:7} strings {:6} end {:>9}/{:>9} mis {} stop {:?} | {} [{}]",
                    db.header.ver.label(), size, db.stream.blocks.len(), db.header.object_count, db.strings.len(), last, size, db.stream.misaligned,
                    db.stream.stopped.as_deref().map(|s| &s[..s.len().min(70)]), f.file_name().unwrap().to_string_lossy(), db.header.program.trim());
            }
            Err(e) => println!("ERR {} : {e}", f.display()),
        }
    }
}
