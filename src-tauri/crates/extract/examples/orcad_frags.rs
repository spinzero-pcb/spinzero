//! Print the fragments of one design (development aid).
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut sink = |_: extract::pipeline::Msg| {};
    let l = extract::orcad::load(std::path::Path::new(&a[1]), &mut sink).unwrap();
    let mut u = extract::orcad::Unresolved::default();
    let sheets = extract::orcad::walk(&l.doc, &mut u);
    let pn = extract::orcad::netlist::power_names(&l.doc);
    let filt = a.get(2).cloned().unwrap_or_default();
    for s in &sheets {
        for f in extract::orcad::netlist::fragments(&l.doc, s, &pn, &mut u) {
            let line = format!("{} rank={} name={:?} keys={:?} terms={:?}", s.info.sheet_path, f.rank, f.name, f.keys, f.terminals.iter().map(|t| format!("{}.{}", t.designator, t.pin)).collect::<Vec<_>>());
            if filt.is_empty() || line.contains(&filt) { println!("{line}"); }
        }
    }
}
