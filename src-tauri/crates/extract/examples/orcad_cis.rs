//! Print a design's CIS variants and groups resolved to designators.
fn main() {
    let p = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let l = extract::orcad::load(&p, &mut |_| {}).unwrap();
    let (_, _, sheets) = extract::orcad::build_design(&l, "", "", false);
    let ids = extract::orcad::design::occurrence_designators(&l.doc, &sheets);
    let Some(cis) = &l.doc.cis else { println!("no CIS"); return };
    for v in &cis.variants {
        println!("variant {} groups {:?} bom_parts {:?}", v.name, v.groups, v.bom_parts.as_ref().map(|b| b.len()));
    }
    for g in cis.groups.values() {
        let on: Vec<String> = g.members.iter().filter(|m| m.0).map(|m| ids.get(&m.1).cloned().unwrap_or(format!("?{}", m.1))).collect();
        let off: Vec<String> = g.members.iter().filter(|m| !m.0).map(|m| ids.get(&m.1).cloned().unwrap_or(format!("?{}", m.1))).collect();
        println!("group {} on {:?}\n      off {:?} updates {}", g.name, on, off, g.updates.len());
    }
}
