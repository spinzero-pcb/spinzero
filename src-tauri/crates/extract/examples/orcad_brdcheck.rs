//! For every Allegro board under the given roots: build the geometry (timing
//! it), and where an Allegro netlist (`pstxnet.dat`) sits beside the board,
//! compare each pad's net with the netlist's.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use extract::orcad::pcb;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("brd")).unwrap_or(false) {
            out.push(p);
        }
    }
}

fn read_pstxnet(p: &Path) -> BTreeMap<(String, String), String> {
    let t = String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_string();
    let mut out = BTreeMap::new();
    let mut cur = String::new();
    let mut lines = t.lines();
    while let Some(l) = lines.next() {
        let l = l.trim();
        if l == "NET_NAME" {
            if let Some(n) = lines.next() {
                cur = n.trim().trim_matches('\'').to_string();
            }
        } else if let Some(rest) = l.strip_prefix("NODE_NAME") {
            let mut it = rest.split_whitespace();
            if let (Some(r), Some(pin)) = (it.next(), it.next()) {
                out.insert((r.to_uppercase(), pin.to_uppercase()), cur.clone());
            }
        }
    }
    out
}

fn main() {
    let mut boards = Vec::new();
    for a in std::env::args().skip(1) {
        walk(Path::new(&a), &mut boards);
    }
    boards.sort();
    let (mut tot_pins, mut tot_same, mut tot_name) = (0, 0, 0);
    for p in &boards {
        let t = Instant::now();
        let b = match pcb::load_board(p) {
            Ok(b) => b,
            Err(e) => {
                println!("ERR  {}  {e}", p.display());
                continue;
            }
        };
        let built = pcb::build(&b, "");
        let g = &built.geometry;
        let ms = t.elapsed().as_millis();
        let mut line = format!(
            "{:>5}ms {:<5} L{:>3} C{:>5} N{:>5} P{:>6} V{:>6} T{:>7} Z{:>5} tmpl{:>6} approx{:>4}{}",
            ms,
            built.stats.format.trim_start_matches("Allegro "),
            g.layers.len(),
            g.components.len(),
            g.nets.len() - 1,
            g.pads.len(),
            g.vias.len(),
            g.tracks.seg.w.len() + g.tracks.arc.w.len(),
            g.zones.len(),
            built.stats.template_objects,
            built.stats.pads_approximated,
            if built.stats.stopped.is_some() { " STOPPED" } else { "" },
        );
        let net = p.parent().unwrap().join("pstxnet.dat");
        if net.exists() {
            let theirs = read_pstxnet(&net);
            let ours: BTreeMap<(String, String), String> = pcb::board_netlist(&b)
                .into_iter()
                .flat_map(|(n, ts)| ts.into_iter().map(move |(r, p)| ((r.to_uppercase(), p.to_uppercase()), n.clone())))
                .collect();
            // Grouping agreement: two pins share a net in ours iff they do in theirs.
            let common: Vec<_> = theirs.keys().filter(|k| ours.contains_key(*k)).collect();
            let mut same = 0;
            let mut name = 0;
            let mut ours_to_theirs: BTreeMap<&String, BTreeMap<&String, usize>> = BTreeMap::new();
            for k in &common {
                *ours_to_theirs.entry(&ours[*k]).or_default().entry(&theirs[*k]).or_default() += 1;
            }
            for k in &common {
                let o = &ours[*k];
                let best = ours_to_theirs[o].iter().max_by_key(|(_, n)| **n).map(|(t, _)| *t).unwrap();
                if best == &theirs[*k] {
                    same += 1;
                }
                if o.eq_ignore_ascii_case(&theirs[*k]) {
                    name += 1;
                }
            }
            tot_pins += theirs.len();
            tot_same += same;
            tot_name += name;
            line += &format!("  net: {}/{} pins on board, {} grouped alike, {} same name", common.len(), theirs.len(), same, name);
        }
        println!("{line}  {}", p.display());
    }
    println!("TOTAL netlist pins {tot_pins}, grouped alike {tot_same}, same name {tot_name}");
}
