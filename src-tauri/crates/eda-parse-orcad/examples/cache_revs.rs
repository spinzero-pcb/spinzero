//! Every stored revision of one cache name: box and pins (slot, name, hot).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = eda_parse_orcad::capture::open(std::path::Path::new(&a[1])).unwrap();
    for (k, v) in &d.cache.symbols {
        if k != &a[2] { continue }
        for (i, s) in v.iter().enumerate() {
            println!("rev {i} bbox {:?} pins {:?}", s.bbox, s.pins.iter().map(|p| (p.slot, p.name.clone(), p.hot)).collect::<Vec<_>>());
        }
    }
    for f in &d.folders { for p in &f.pages { for part in &p.parts {
        if part.cache_name == a[2] {
            let s = d.symbol_for(part);
            println!("{} pos {:?} {:?} pins {:?} -> fits {:?}", part.reference, part.pos, part.orient, part.pins.iter().map(|q| (q.index, q.pos)).collect::<Vec<_>>(), s.map(|s| eda_parse_orcad::capture::doc::fits(part, s)));
        }
    }}}
}
