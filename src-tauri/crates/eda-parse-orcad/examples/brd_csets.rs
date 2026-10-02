//! Physical constraint sets: name and each per-layer record, in mils.
use eda_parse_orcad::allegro::{self, Data};
fn main() {
    let db = allegro::open(std::path::Path::new(&std::env::args().nth(1).unwrap())).unwrap();
    let mil = db.header.scale() / 0.0254;
    println!("{}", db.header.ver.label());
    for b in &db.stream.blocks {
        if let Data::ConstraintSet { name, records, field } = &b.data {
            println!("{:?} name id {name:#x} field {field:#x} -> {:?}", db.s(*name), db.stream.get(*field).map(|x| &x.data));
            for r in records.iter().take(2) {
                println!("   {:?}", r.iter().map(|v| (*v as f64 * mil * 100.0).round() / 100.0).collect::<Vec<_>>());
            }
        }
    }
}
