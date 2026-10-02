//! Components (0x06) with their instances (0x07) and the fields each carries.
use eda_parse_orcad::allegro::{self, blocks::FieldValue, Data};

fn fields(db: &allegro::Db, head: u32) -> Vec<String> {
    db.stream.chain(head).iter().filter_map(|b| match &b.data {
        Data::Field { code, value } => Some(format!("{code:#x}={}", match value {
            FieldValue::Text(t) => format!("{t:?}"),
            FieldValue::Int(i) => format!("i{i:#x}{}", db.strings.get(i).map(|s| format!("({s:?})")).unwrap_or_default()),
            v => format!("{v:?}"),
        })),
        _ => None,
    }).collect()
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let db = allegro::open(std::path::Path::new(&path)).unwrap();
    for b in &db.stream.blocks {
        let Data::Component { device_type, symbol, first_inst, fields: f, .. } = &b.data else { continue };
        println!("COMP dev {:?} sym {:?} fields {:?}", db.s(*device_type), db.s(*symbol), fields(&db, *f));
        for i in db.stream.chain(*first_inst) {
            if let Data::CompInst { refdes, fields: f, fp_inst, .. } = &i.data {
                println!("   {} fp_inst {:x} {:?}", db.s(*refdes), fp_inst, fields(&db, *f));
            }
        }
    }
    for b in &db.stream.blocks {
        if let Data::Net { name, fields: f, .. } = &b.data {
            let fs = fields(&db, *f);
            if !fs.is_empty() { println!("NET {} {:?}", db.s(*name), fs); }
        }
    }
}
