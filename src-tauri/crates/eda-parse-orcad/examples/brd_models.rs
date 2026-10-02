//! Footprint definitions carrying 3D model fields (0x345 file, 0x346 placement).
use eda_parse_orcad::allegro::{self, blocks::FieldValue, Data};
fn main() {
    for p in std::env::args().skip(1) {
        let Ok(db) = allegro::open(std::path::Path::new(&p)) else { continue };
        let mut n = 0;
        let mut sample = Vec::new();
        for b in &db.stream.blocks {
            let Data::FootprintDef { name, fields, .. } = &b.data else { continue };
            let mut k = *fields;
            let mut seen = std::collections::HashSet::new();
            while let Some(f) = db.stream.get(k) {
                let Data::Field { code, value } = &f.data else { break };
                if !seen.insert(k) { break }
                if *code == 0x345 || *code == 0x346 {
                    n += 1;
                    if sample.len() < 4 { if let FieldValue::Text(t) = value { sample.push(format!("{}: {code:#x} {t}", db.s(*name))); } }
                }
                k = f.next;
            }
        }
        if n > 0 { println!("{n} {p}\n   {}", sample.join("\n   ")); }
    }
}
