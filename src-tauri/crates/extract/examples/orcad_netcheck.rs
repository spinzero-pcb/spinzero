//! Compare our OrCAD netlist with Capture's own (`pstxnet.dat`) for every
//! design under a directory that ships one. Development aid.
//! `cargo run -p extract --example orcad_netcheck -- <dir> [-v]`

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().map(|n| n != ".git").unwrap_or(true) {
                walk(&p, out);
            }
        } else if p.file_name().and_then(|n| n.to_str()).map(|n| n.eq_ignore_ascii_case("pstxnet.dat")).unwrap_or(false) {
            out.push(p);
        }
    }
}

/// One node of Capture's expanded netlist.
struct Node {
    r: String,
    pin: String,
    /// The pin's name as Capture's netlist spells it.
    name: String,
}

/// Capture's expanded netlist: net name -> nodes.
fn read_pstxnet(p: &Path) -> BTreeMap<String, Vec<Node>> {
    let t = String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_string();
    let mut out: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    let mut cur = String::new();
    let mut lines = t.lines().peekable();
    while let Some(l) = lines.next() {
        let l = l.trim();
        if l == "NET_NAME" {
            if let Some(n) = lines.next() {
                cur = n.trim().trim_matches('\'').to_string();
                out.entry(cur.clone()).or_default();
            }
        } else if let Some(rest) = l.strip_prefix("NODE_NAME") {
            let mut it = rest.split_whitespace();
            if let (Some(r), Some(pin)) = (it.next(), it.next()) {
                let _path = lines.next();
                let name = lines.next().unwrap_or("").trim().trim_end_matches(":;").trim_matches('\'').to_string();
                out.entry(cur.clone()).or_default().push(Node { r: r.to_string(), pin: pin.to_string(), name });
            }
        }
    }
    out
}

/// Hidden power pins: Capture's Allegro netlist leaves them off the nets and
/// states them on the part (`POWER_PINS='(VCC:14)'` in pstchip.dat, the part
/// named per designator in pstxprt.dat). Returns net -> nodes to add.
fn power_pins(dir: &Path) -> BTreeMap<String, Vec<Node>> {
    let read = |n: &str| std::fs::read(dir.join(n)).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let chip = read("pstchip.dat");
    let prt = read("pstxprt.dat");
    let mut prim_pins: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    let mut cur = String::new();
    for l in chip.lines() {
        let l = l.trim();
        if let Some(r) = l.strip_prefix("primitive ") {
            cur = r.trim_end_matches(';').trim_matches('\'').to_string();
        } else if let Some(r) = l.strip_prefix("POWER_PINS='(") {
            let inner = r.trim_end_matches(";").trim_end_matches('\'').trim_end_matches(')');
            for grp in inner.split(");(") {
                if let Some((net, pins)) = grp.split_once(':') {
                    prim_pins.entry(cur.clone()).or_default().push((net.to_string(), pins.split(',').map(|x| x.trim().to_string()).collect()));
                }
            }
        }
    }
    let mut out: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    let mut lines = prt.lines();
    while let Some(l) = lines.next() {
        if l.trim() == "PART_NAME" {
            if let Some(n) = lines.next() {
                let mut it = n.trim().splitn(2, ' ');
                let (r, prim) = (it.next().unwrap_or(""), it.next().unwrap_or("").trim_end_matches(":;").trim_matches('\''));
                for (net, pins) in prim_pins.get(prim).cloned().unwrap_or_default() {
                    for pin in pins {
                        out.entry(net.clone()).or_default().push(Node { r: r.to_string(), pin, name: String::new() });
                    }
                }
            }
        }
    }
    out
}

/// The design a netlist belongs to: a .DSN in the netlist folder's parent
/// (or grandparent) whose netlist header names it.
fn design_for(net: &Path) -> Option<PathBuf> {
    let mut d = net.parent()?.parent()?.to_path_buf();
    for _ in 0..2 {
        let mut dsns: Vec<PathBuf> = std::fs::read_dir(&d).ok()?.flatten().map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("dsn")).unwrap_or(false))
            .collect();
        dsns.sort();
        if let Some(x) = dsns.into_iter().find(|p| extract::orcad::is_orcad_project(p)) {
            return Some(x);
        }
        d = d.parent()?.to_path_buf();
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "-v");
    let mut nets = Vec::new();
    walk(Path::new(&args[1]), &mut nets);
    nets.sort();
    let (mut tot_nets, mut tot_part, mut tot_name, mut tot_pins, mut tot_pins_ok) = (0, 0, 0, 0, 0);
    let (mut tot_parts, mut tot_parts_ok) = (0, 0);
    for netf in nets {
        let Some(dsn) = design_for(&netf) else { println!("no design for {}", netf.display()); continue };
        let mut sink = |_: extract::pipeline::Msg| {};
        let l = match extract::orcad::load(&dsn, &mut sink) { Ok(l) => l, Err(e) => { println!("FAIL {}: {e}", dsn.display()); continue } };
        let (model, src, _) = extract::orcad::build_design(&l, "", "", true);
        let mut theirs = read_pstxnet(&netf);
        for (net, nodes) in power_pins(netf.parent().unwrap()) {
            let e = theirs.entry(net).or_default();
            for n in nodes {
                if !e.iter().any(|x| x.r == n.r && x.pin == n.pin) {
                    e.push(n);
                }
            }
        }
        // ours: (ref,pin) -> net name
        let mut pin_net: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut pin_name: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut ours: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        for n in &model.nets {
            for t in &n.terminals {
                pin_net.insert((t.designator.clone(), t.pin.clone()), n.name.clone());
                pin_name.insert((t.designator.clone(), t.pin.clone()), t.pin_name.clone());
                ours.entry(n.name.clone()).or_default().insert((t.designator.clone(), t.pin.clone()));
            }
        }
        let (mut n_nets, mut part_ok, mut name_ok, mut pins, mut pins_ok) = (0, 0, 0, 0, 0);
        let mut bad = Vec::new();
        // A node naming an instance we place under another designator (or not
        // at all) says the netlist predates the design: such nets are not
        // comparable.
        let mut stale = 0;
        for (name, nodes) in &theirs {
            if nodes.is_empty() || name == "NC" { continue; }
            let consistent = nodes.iter().all(|n| {
                n.name.is_empty() && pin_name.contains_key(&(n.r.clone(), n.pin.clone()))
                    || pin_name.get(&(n.r.clone(), n.pin.clone())).map(|x| x.eq_ignore_ascii_case(&n.name)).unwrap_or(false)
            });
            if !consistent {
                stale += 1;
                if verbose {
                    for n in nodes.iter().filter(|n| !pin_name.get(&(n.r.clone(), n.pin.clone())).map(|x| x.eq_ignore_ascii_case(&n.name)).unwrap_or(false)).take(2) {
                        bad.push(format!("  stale node {} {} '{}' ours {:?}", n.r, n.pin, n.name, pin_name.get(&(n.r.clone(), n.pin.clone()))));
                    }
                }
                continue;
            }
            let set: BTreeSet<(String, String)> = nodes.iter().map(|n| (n.r.clone(), n.pin.clone())).collect();
            let set = &set;
            n_nets += 1;
            pins += set.len();
            let first = set.iter().next().unwrap();
            let Some(on) = pin_net.get(first) else { bad.push(format!("  missing pin {first:?} of {name}")); continue };
            let os = &ours[on];
            if os == set { part_ok += 1; pins_ok += set.len(); } else {
                pins_ok += set.intersection(os).count();
                let missing: Vec<_> = set.difference(os).take(4).collect();
                let extra: Vec<_> = os.difference(set).take(4).collect();
                bad.push(format!("  {name} vs ours {on}: missing {missing:?} extra {extra:?}"));
            }
            if on.eq_ignore_ascii_case(name) { name_ok += 1; } else if verbose { bad.push(format!("  name {name} ours {on}")); }
        }
        // Parts: Capture's part list (pstxprt.dat) against our components.
        let prt = std::fs::read(netf.parent().unwrap().join("pstxprt.dat")).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
        let mut their_parts: BTreeSet<String> = BTreeSet::new();
        let mut it = prt.lines();
        while let Some(l) = it.next() {
            if l.trim() == "PART_NAME" {
                if let Some(n) = it.next() { if let Some(r) = n.split_whitespace().next() { their_parts.insert(r.to_string()); } }
            }
        }
        let our_parts: BTreeSet<String> = model.components.iter().map(|c| c.designator.clone()).collect();
        let parts_missing: Vec<_> = their_parts.difference(&our_parts).take(6).cloned().collect();
        let parts_extra = our_parts.difference(&their_parts).count();
        tot_parts += their_parts.len(); tot_parts_ok += their_parts.intersection(&our_parts).count();
        if verbose && !parts_missing.is_empty() { bad.push(format!("  parts missing {parts_missing:?}")); }
        println!("parts {}/{} (+{} ours) | ", their_parts.intersection(&our_parts).count(), their_parts.len(), parts_extra);
        println!("{:5}/{:5} nets same pins, {:5} same name, pins {}/{}, stale {} | {} (unconnected {})",
            part_ok, n_nets, name_ok, pins_ok, pins, stale, dsn.display(), src.unresolved.unconnected_pins);
        if verbose { for b in bad.iter().take(400) { println!("{b}"); } }
        tot_nets += n_nets; tot_part += part_ok; tot_name += name_ok; tot_pins += pins; tot_pins_ok += pins_ok;
    }
    println!("PARTS {tot_parts_ok}/{tot_parts}");
    println!("TOTAL {tot_part}/{tot_nets} nets same pins ({:.1}%), {tot_name} same name ({:.1}%), pins {tot_pins_ok}/{tot_pins}",
        100.0 * tot_part as f64 / tot_nets.max(1) as f64, 100.0 * tot_name as f64 / tot_nets.max(1) as f64);
}
