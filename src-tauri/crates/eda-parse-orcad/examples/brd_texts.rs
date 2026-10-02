//! Every text block: whether a placed footprint's text chain reaches it, its
//! layer, position, rotation, font and string.
use std::collections::HashSet;
use eda_parse_orcad::allegro::{self, Data};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let db = allegro::open(std::path::Path::new(&path)).unwrap();
    let sc = db.header.scale();
    let mut owned = HashSet::new();
    for b in &db.stream.blocks {
        if let Data::FootprintInst { text, .. } = &b.data {
            for t in db.stream.chain(*text) { owned.insert(t.key); }
        }
    }
    for b in &db.stream.blocks {
        if let Data::Fonts(f) = &b.data { println!("fonts {:?}", f.iter().map(|f| (f.height as f64 * sc, f.width as f64 * sc, f.stroke as f64 * sc)).collect::<Vec<_>>()); }
    }
    for b in &db.stream.blocks {
        let Data::Text { layer, sgraphic, at, rotation, font, align, reversal, group } = &b.data else { continue };
        let s = match db.stream.get(*sgraphic).map(|x| &x.data) { Some(Data::StrGraphic { text, at: a2, .. }) => format!("{text:?} sg@({:.3},{:.3})", a2.0 as f64 * sc, a2.1 as f64 * sc), _ => "?".into() };
        println!("{} {:02x}/{:02x} ({:.3},{:.3}) rot {} font {font} align {align} rev {reversal} grp {group:x} next {:x} {s}",
            if owned.contains(&b.key) { "FP" } else { "BD" }, layer.class, layer.sub, at.0 as f64 * sc, at.1 as f64 * sc, rotation, b.next);
    }
}
