//! Learn the placement transform per stored orientation from parts, whose
//! pins are stored both in symbol space and at absolute page positions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); } else if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("dsn")).unwrap_or(false) { out.push(p); }
    }
}

fn main() {
    let mut files = Vec::new();
    walk(Path::new(&std::env::args().nth(1).unwrap()), &mut files);
    // 8 candidate linear maps (a,b,c,d): x' = a x + b y, y' = c x + d y
    let cands: Vec<(i32, i32, i32, i32)> = vec![(1,0,0,1),(0,-1,1,0),(-1,0,0,-1),(0,1,-1,0),(-1,0,0,1),(0,1,1,0),(1,0,0,-1),(0,-1,-1,0)];
    let mut tally: BTreeMap<(u8, bool), BTreeMap<usize, usize>> = BTreeMap::new();
    let mut offs: BTreeMap<(u8, bool), BTreeMap<String, usize>> = BTreeMap::new();
    for f in files {
        let Ok(d) = eda_parse_orcad::capture::open(&f) else { continue };
        for fo in &d.folders { for p in &fo.pages { for part in &p.parts {
            let Some(s) = d.symbol_for(part) else { continue };
            if part.pins.len() < 2 { continue; }
            let key = (part.orient.turns, part.orient.mirror);
            let b = (s.bbox.0 as i32, s.bbox.1 as i32, s.bbox.2 as i32, s.bbox.3 as i32);
            let fits = part.pins.iter().all(|pi| s.pins.iter().find(|x| x.slot == pi.slot()).map(|sp| part.orient.place(sp.hot, part.pos, b) == pi.pos).unwrap_or(false));
            *tally.entry(key).or_default().entry(if fits { 100 } else { 101 }).or_default() += 1;
            for (ci, &(a,b,c,dd)) in cands.iter().enumerate() {
                // offset must be constant
                let mut off = None; let mut ok = true;
                for pi in &part.pins {
                    let Some(sp) = s.pins.iter().find(|x| x.slot == pi.slot()) else { ok = false; break };
                    let (x, y) = sp.hot;
                    let t = (a*x + b*y, c*x + dd*y);
                    let o = (pi.pos.0 - t.0, pi.pos.1 - t.1);
                    match off { None => off = Some(o), Some(q) if q == o => {}, _ => { ok = false; break } }
                }
                if ok {
                    *tally.entry(key).or_default().entry(ci).or_default() += 1;
                    let o = off.unwrap();
                    let (bx1, by1, bx2, by2) = s.bbox;
                    let rel = format!("off-pos=({},{}) symbbox=({},{},{},{})", o.0 - part.pos.0, o.1 - part.pos.1, bx1, by1, bx2, by2);
                    *offs.entry(key).or_default().entry(if o == part.pos { "pos".into() } else { rel }).or_default() += 1;
                }
            }
        }}}
    }
    for (k, v) in &tally { println!("{k:?}: place() fits {} misses {}", v.get(&100).unwrap_or(&0), v.get(&101).unwrap_or(&0)); }
    for (k, v) in &offs { let mut v: Vec<_> = v.iter().collect(); v.sort_by(|a, b| b.1.cmp(a.1)); println!("{k:?}: {:?}", &v[..v.len().min(4)]); }
}
