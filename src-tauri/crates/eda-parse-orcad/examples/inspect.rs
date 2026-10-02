//! Print one Capture design's pages in a readable form (development aid).
//! `cargo run -p eda-parse-orcad --example inspect -- <file.DSN> [page-filter]`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let d = eda_parse_orcad::capture::open(std::path::Path::new(&args[1])).unwrap();
    let filt = args.get(2).cloned().unwrap_or_default();
    for (i, f) in d.lib.fonts.iter().enumerate() {
        println!("font {} h={} w={} w8={} esc={} it={} {:?}", i + 1, f.height, f.width, f.weight, f.escapement, f.italic, f.face);
    }
    println!("version {:?} root={:?} folders={:?}", d.lib.version, d.lib.root_name,
        d.folders.iter().map(|f| (f.name.clone(), f.pages.len(), f.visible)).collect::<Vec<_>>());
    for f in &d.folders {
        for p in &f.pages {
            if !filt.is_empty() && !p.name.contains(&filt) { continue; }
            println!("== {}/{} size={} w={} h={} metric={}", f.name, p.name, p.size_name, p.settings.width, p.settings.height, p.settings.metric);
            println!(" nets: {:?}", p.nets.iter().map(|n| (n.id, n.name.clone())).collect::<Vec<_>>());
            println!(" net_names: {:?}", p.net_names.iter().map(|n| (n.id, n.name.clone())).collect::<Vec<_>>());
            println!(" groups: {:?}", p.groups);
            for w in &p.wires {
                println!(" wire db={} net={} {:?}-{:?} bus={} aliases={:?}", w.db_id, w.net_id, w.a, w.b, w.bus, w.aliases.iter().map(|a| (&a.name, a.pos)).collect::<Vec<_>>());
            }
            for pt in &p.parts {
                println!(" part db={} ref={:?} val={:?} cache={:?} pkg={:?} unit={} pos={:?} or={:?} props={:?}", pt.db_id, pt.reference, pt.value, pt.cache_name, pt.package, pt.unit_index, pt.pos, pt.orient, pt.props);
                for dp in &pt.display {
                    println!("    disp {:?} at ({}, {}) font {} rot {} mode {}", dp.name, dp.x, dp.y, dp.font, dp.rotation, dp.mode);
                }
                for pi in &pt.pins {
                    println!("    pin idx={} pos={:?} a={:#x} b={:#x} rest={} props={:?}", pi.index, pi.pos, pi.word_a, pi.word_b, pi.display.len(), pi.props);
                }
            }
            for b in &p.blocks {
                println!(" block db={} name={:?} ref={:?} impl={:?} rect={:?} pins={:?}", b.db_id, b.name, b.reference, b.implementation, b.rect, b.pins.iter().map(|x| (&x.name, x.pos)).collect::<Vec<_>>());
            }
            for (k, l) in [("port", &p.ports), ("global", &p.globals), ("offpage", &p.offpages)] {
                for g in l.iter() {
                    println!(" {k} db={} name={:?} cache={:?} pos={:?} or={:?} bbox={:?}", g.db_id, g.name, g.cache_name, g.pos, g.orient, g.bbox);
                    for dp in &g.display {
                        println!("    disp {:?} at ({}, {}) font {} rot {} mode {:#x}", dp.name, dp.x, dp.y, dp.font, dp.rotation, dp.mode);
                    }
                }
            }
            println!(" erc={} busentries={} graphics={} titleblocks={}", p.erc.len(), p.bus_entries.len(), p.graphics.len(), p.title_blocks.len());
            for g in p.graphics.iter().chain(&p.title_blocks) {
                println!(" graphic kind={} db={} name={:?} cache={:?} pos={:?} or={:?} bbox={:?} body={:?}", g.kind, g.db_id, g.name, g.cache_name, g.pos, g.orient, g.bbox,
                    g.body.as_ref().map(|b| (b.bbox, b.prims.len(), b.prims.first().cloned())));
                for dp in &g.display {
                    println!("    disp {:?} at ({}, {}) font {} rot {} mode {:#x}", dp.name, dp.x, dp.y, dp.font, dp.rotation, dp.mode);
                }
            }
        }
    }
    if let Some(t) = &d.tree {
        println!("== tree view={} power={:?}", t.view, t.power);
        fn walk(s: &eda_parse_orcad::capture::hierarchy::Scope, ind: usize) {
            println!("{:ind$}nets {:?}", "", s.nets.iter().map(|n| (n.db_id, n.name.clone())).collect::<Vec<_>>(), ind = ind);
            for o in &s.occurrences {
                println!("{:ind$}occ own={} target={} folder={:?} ref={:?} unit={:?} props={:?} pins={}", "", o.own_db_id, o.target_db_id, o.child_folder, o.reference, o.unit, o.props, o.pins.len(), ind = ind);
                walk(&o.nested, ind + 2);
            }
        }
        walk(&t.root, 1);
    }
    for (k, v) in d.cache.symbols.iter().filter(|(k, _)| std::env::var("SYM").map(|f| k.contains(&f)).unwrap_or(true)).take(40) {
        let s = &v[0];
        println!("sym {k} kind={} bbox={:?} pins={:?} general={:?}", s.kind, s.bbox, s.pins.iter().map(|p| (p.slot, p.name.clone(), p.hot, p.etype, p.shape)).collect::<Vec<_>>(), s.general);
    }
    for (k, p) in d.cache.packages.iter().take(40) {
        println!("pkg {k} prefix={:?} fp={:?} devs={:?} props={:?}", p.ref_prefix, p.footprint, p.devices.iter().map(|d| (d.unit.clone(), d.pins.iter().map(|x| x.as_ref().map(|y| y.number.clone())).collect::<Vec<_>>())).collect::<Vec<_>>(), p.props);
    }
    for (k, p) in &d.cache.packages { for dv in &p.devices { let ig: Vec<_> = dv.pins.iter().flatten().filter(|x| x.ignored).map(|x| x.number.clone()).collect(); if !ig.is_empty() { println!("ignored {k} {}: {:?}", dv.unit, ig); } } }
    for n in &d.notes { println!("note {n}"); }
}
