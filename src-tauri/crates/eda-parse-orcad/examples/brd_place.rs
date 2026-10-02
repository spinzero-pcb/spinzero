//! Print every placed footprint as refdes, x, y (mm), rotation, side, symbol,
//! in the shape of Allegro's own `place_txt.txt`, for checking against it.
use eda_parse_orcad::allegro::{self, Data};

fn main() {
    let path = std::env::args().nth(1).expect("board");
    let db = allegro::open(std::path::Path::new(&path)).unwrap();
    let sc = db.header.scale();
    eprintln!("{} {:?} divisor {} scale {sc}", db.header.ver.label(), db.header.units, db.header.divisor);
    for b in &db.stream.blocks {
        let Data::FootprintDef { name, first_inst, .. } = &b.data else { continue };
        for i in db.stream.chain(*first_inst) {
            let Data::FootprintInst { bottom, rotation, at, inst_ref, .. } = &i.data else { continue };
            let refdes = match db.stream.get(*inst_ref).map(|x| &x.data) {
                Some(Data::CompInst { refdes, .. }) => db.s(*refdes).to_string(),
                _ => "?".into(),
            };
            println!(
                "{refdes:<12} {:>10.4} {:>10.4} {:>8} {} {}",
                at.0 as f64 * sc,
                at.1 as f64 * sc,
                *rotation as f64 / 1000.0,
                if *bottom { "m" } else { " " },
                db.s(*name)
            );
        }
    }
}
