//! Every segment, arc and text with its raw (class, subclass), in board
//! units, for matching against Allegro's own extract reports.
use eda_parse_orcad::allegro::{self, Data};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let db = allegro::open(std::path::Path::new(&path)).unwrap();
    let u = db.header.divisor as f64;
    let seg_kinds = [0x01u8, 0x15, 0x16, 0x17];
    for b in &db.stream.blocks {
        let (l, head) = match &b.data {
            Data::Graphic { layer, first_seg, .. } | Data::Shape { layer, first_seg, .. } | Data::Track { layer, first_seg, .. } | Data::Keepout { layer, first_seg } => (layer, *first_seg),
            Data::Text { layer, at, .. } => {
                println!("T {} {} {:.3} {:.3}", layer.class, layer.sub, at.0 as f64 / u, at.1 as f64 / u);
                continue;
            }
            _ => continue,
        };
        let mut k = head;
        let mut n = 0;
        while let Some(s) = db.stream.get(k) {
            if !seg_kinds.contains(&s.kind) || n > 100000 { break; }
            match &s.data {
                Data::Seg { a, b: e, .. } | Data::Arc { start: a, end: e, .. } => println!("L {} {} {:.3} {:.3} {:.3} {:.3}", l.class, l.sub, a.0 as f64 / u, a.1 as f64 / u, e.0 as f64 / u, e.1 as f64 / u),
                _ => {}
            }
            k = s.next;
            n += 1;
        }
    }
}
