//! Where do wires and pins touch ports, off-page connectors and power symbols,
//! relative to the stored position and box? Development aid.

use std::collections::{BTreeMap, HashMap};
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
    let mut hist: BTreeMap<String, HashMap<String, usize>> = BTreeMap::new();
    let mut hits = 0; let mut misses = 0;
    for f in files {
        let Ok(d) = eda_parse_orcad::capture::open(&f) else { continue };
        for fo in &d.folders { for p in &fo.pages {
            // endpoints with degree
            let mut deg: HashMap<(i32,i32), usize> = HashMap::new();
            for w in &p.wires { *deg.entry(w.a).or_default() += 1; *deg.entry(w.b).or_default() += 1; }
            for pt in &p.parts { for pi in &pt.pins { *deg.entry(pi.pos).or_default() += 1; } }
            for (kind, list) in [("port", &p.ports), ("off", &p.offpages), ("pwr", &p.globals)] {
                for g in list.iter() {
                    let (x1, y1, x2, y2) = g.bbox;
                    // touching points: inside bbox expanded by 0, degree>=1
                    let mut found: Vec<(i32,i32)> = deg.keys().copied().filter(|q| q.0 >= x1.min(x2) && q.0 <= x1.max(x2) && q.1 >= y1.min(y2) && q.1 <= y1.max(y2)).collect();
                    found.sort();
                    let s = d.cache.symbol(&g.cache_name);
                    let sb = s.map(|s| s.bbox).unwrap_or((0,0,0,0));
                    let key = format!("{kind} t{} m{} sym{:?} w{}", g.orient.turns, g.orient.mirror as u8, (sb.2-sb.0, sb.3-sb.1), x2 - x1 - (sb.2 - sb.0) as i32 != 0);
                    let predicted: Vec<(i32,i32)> = s.map(|s| s.pins.iter().map(|sp| g.orient.place(sp.hot, g.origin(), (s.bbox.0 as i32, s.bbox.1 as i32, s.bbox.2 as i32, s.bbox.3 as i32))).collect()).unwrap_or_default();
                    if found.len() == 1 {
                        *hist.entry("~predicted".into()).or_default().entry(if predicted.contains(&found[0]) { "hit".into() } else { format!("miss {key}") }).or_default() += 1;
                    }
                    if found.len() == 1 {
                        hits += 1;
                        let q = found[0];
                        let rel = format!("pos{:+},{:+} box({:+}|{:+},{:+}|{:+})", q.0 - g.pos.0, q.1 - g.pos.1, q.0 - x1, q.0 - x2, q.1 - y1, q.1 - y2);
                        *hist.entry(key).or_default().entry(rel).or_default() += 1;
                    } else { misses += 1; }
                }
            }
        }}
    }
    println!("hits {hits} misses {misses}");
    for (k, v) in hist { let mut v: Vec<_> = v.into_iter().collect(); v.sort_by(|a,b| b.1.cmp(&a.1)); let tot: usize = v.iter().map(|x| x.1).sum(); if tot < 20 { continue; } println!("{k} [{tot}]: {:?}", &v[..v.len().min(4)]); }
}
