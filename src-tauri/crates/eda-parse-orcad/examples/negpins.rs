//! What does a negative placed-pin index correlate with? Development aid.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() { let p = e.path(); if p.is_dir() { walk(&p, out); } else if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("dsn")).unwrap_or(false) { out.push(p); } }
}
fn main() {
    let mut files = Vec::new();
    walk(Path::new(&std::env::args().nth(1).unwrap()), &mut files);
    let mut t: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        let Ok(d) = eda_parse_orcad::capture::open(&f) else { continue };
        for fo in &d.folders { for p in &fo.pages {
            let wires: std::collections::HashMap<u32, &eda_parse_orcad::capture::page::Wire> = p.wires.iter().map(|w| (w.db_id, w)).collect();
            let mut ends: std::collections::HashMap<(i32,i32), usize> = Default::default();
            for w in &p.wires { *ends.entry(w.a).or_default() += 1; *ends.entry(w.b).or_default() += 1; }
            for part in &p.parts {
                let any_neg = part.pins.iter().any(|x| x.index < 0);
                let all_neg = part.pins.iter().all(|x| x.index < 0);
                for pi in &part.pins {
                    let wired = ends.contains_key(&pi.pos);
                    let _ = (all_neg, any_neg);
                    let wa = if pi.word_a == 0 { "a=0".to_string() } else { match wires.get(&pi.word_a) { Some(w) => format!("a=wire touching={}", w.a == pi.pos || w.b == pi.pos), None => "a=other".into() } };
                    let k = format!("neg={} wired={} {wa} b={}", pi.index < 0, wired, pi.word_b != 0);
                    *t.entry(k).or_default() += 1;
                }
            }
        }}
    }
    for (k, v) in t { println!("{v:7} {k}"); }
}
