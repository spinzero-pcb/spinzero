//! Compiles schematic connectivity into nets.
//!
//! Approach: every electrical port (pin endpoint, wire endpoint, junction,
//! label anchor) becomes a node keyed by its exact nanometre coordinate. A wire
//! segment unions every port lying on it — which correctly joins T-junctions
//! (a wire ending mid-segment is a port on the other wire) while leaving plain
//! crossings unconnected (no shared port unless a junction sits there). Ports at
//! the same coordinate share a node, so abutting pins connect without a wire.
//! Each connected component with at least one pin is a net.

use std::collections::{BTreeMap, HashMap};

use eda_parse_kicad::schematic::{LabelKind, Schematic};
use serde::Serialize;

use crate::geom::{on_segment, place, to_nm, P};

/// A pin participating in a net.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Terminal {
    pub designator: String,
    pub pin: String,
    pub pin_name: String,
    pub pin_type: String,
}

/// SVG element uuids belonging to a net, bucketed by element kind. The kind keys
/// are the ones the viewer reads to classify a clicked element; `pins` is an
/// extra bucket used only to make pin glyphs net-addressable.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Graphical {
    pub wires: Vec<String>,
    pub junctions: Vec<String>,
    pub labels: Vec<String>,
    pub power_ports: Vec<String>,
    pub ports: Vec<String>,
    pub sheet_entries: Vec<String>,
    pub pins: Vec<String>,
}

impl Graphical {
    /// Every uuid in the net, regardless of kind — the source for `svg_to_net`.
    pub fn all_uuids(&self) -> impl Iterator<Item = &String> {
        self.wires
            .iter()
            .chain(&self.junctions)
            .chain(&self.labels)
            .chain(&self.power_ports)
            .chain(&self.ports)
            .chain(&self.sheet_entries)
            .chain(&self.pins)
    }

    fn sort(&mut self) {
        for v in [
            &mut self.wires,
            &mut self.junctions,
            &mut self.labels,
            &mut self.power_ports,
            &mut self.ports,
            &mut self.sheet_entries,
            &mut self.pins,
        ] {
            v.sort();
            v.dedup();
        }
    }

    /// Fold another bucket set into this one (used when merging same-named nets
    /// across sheets). The caller sorts/dedups afterwards.
    pub fn extend(&mut self, other: Graphical) {
        self.wires.extend(other.wires);
        self.junctions.extend(other.junctions);
        self.labels.extend(other.labels);
        self.power_ports.extend(other.power_ports);
        self.ports.extend(other.ports);
        self.sheet_entries.extend(other.sheet_entries);
        self.pins.extend(other.pins);
    }

    /// Public sort+dedup, for callers that build buckets incrementally.
    pub fn normalize(&mut self) {
        self.sort();
    }
}

/// A compiled net.
#[derive(Debug, Clone, PartialEq)]
pub struct Net {
    pub name: String,
    pub driver_kind: String,
    pub terminals: Vec<Terminal>,
    pub graphical: Graphical,
    /// The net's graphical elements bucketed by the sheet INSTANCE they were drawn
    /// on (`sheet_path_uuids`). A schematic file instantiated several times
    /// (gate_driver U/V/W, mosfet_temp_1..6) shares element uuids across instances,
    /// so the same uuid belongs to a *different* net per instance. This per-instance
    /// attribution is what `sheet_svg_to_nets` (and the viewer's per-sheet click
    /// resolution + cross-probe) needs — a flat uuid→sheet map collapses the
    /// repeated instances onto whichever was processed last.
    pub by_sheet: BTreeMap<String, Graphical>,
}

/// The kind of an addressable graphical element, used to route its uuid into the
/// matching `Graphical` bucket.
#[derive(Clone, Copy)]
enum GKind {
    Wire,
    Junction,
    Label,
    Port,
    PowerPort,
    Pin,
    SheetEntry,
}

/// An addressable element pinned to a connectivity node, resolved to a net later.
struct GElem {
    uuid: String,
    kind: GKind,
    node: usize,
}

/// Union-find over coordinates.
#[derive(Default)]
struct Conn {
    parent: Vec<usize>,
    ids: HashMap<P, usize>,
}

impl Conn {
    fn id(&mut self, c: P) -> usize {
        if let Some(&i) = self.ids.get(&c) {
            return i;
        }
        let i = self.parent.len();
        self.parent.push(i);
        self.ids.insert(c, i);
        i
    }

    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.parent[r] != r {
            r = self.parent[r];
        }
        let mut c = x;
        while self.parent[c] != r {
            let n = self.parent[c];
            self.parent[c] = r;
            c = n;
        }
        r
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

fn pin_type_token(etype: &str) -> String {
    etype.to_ascii_uppercase()
}

/// A label collected for naming a group.
struct Namer {
    node: usize,
    text: String,
    kind: LabelKind,
}

/// A connected fragment of one sheet instance, before cross-sheet merging.
/// `keys` are the merge identities it exposes (global/power/local labels and the
/// hierarchical sheet-pin ↔ label connections); fragments sharing any key form
/// one net across the whole hierarchy.
pub struct Frag {
    pub terminals: Vec<Terminal>,
    pub graphical: Graphical,
    pub keys: Vec<String>,
    pub name: String,
    pub driver_kind: String,
    /// The sheet instance (`sheet_path_uuids`) this fragment was computed on, kept
    /// through the merge so the resulting net knows which of its graphical elements
    /// live on which instance (see `Net::by_sheet`).
    pub sheet: String,
}

/// Driver-kind precedence for naming a merged net (strongest wins).
fn driver_rank(kind: &str) -> u8 {
    match kind {
        "global_power_pin" => 5,
        "global_label" => 4,
        "hier_label" => 3,
        "local_label" => 2,
        _ => 1,
    }
}

/// Compile the nets of a root schematic sheet (`/` sheet path).
pub fn compile(sch: &Schematic) -> Vec<Net> {
    compile_on_sheet(sch, "/", "/")
}

/// Compile the nets of one isolated sheet instance (single-sheet merge).
pub fn compile_on_sheet(sch: &Schematic, sheet_path: &str, sheet_path_uuids: &str) -> Vec<Net> {
    merge_frags(fragments(sch, sheet_path, sheet_path_uuids))
}

/// Compute the connected fragments of one sheet instance. `sheet_path` (human,
/// ends in `/`) prefixes local-label names; `sheet_path_uuids` (uuid chain, ends
/// in `/`) scopes hierarchical connections so a parent sheet pin links to the
/// matching child hierarchical label and nowhere else.
pub fn fragments(sch: &Schematic, sheet_path: &str, sheet_path_uuids: &str) -> Vec<Frag> {
    let mut conn = Conn::default();
    let mut pins: Vec<(usize, Terminal)> = Vec::new();
    let mut power: Vec<(usize, String)> = Vec::new();
    let mut namers: Vec<Namer> = Vec::new();
    // Addressable graphical elements, resolved to nets once connectivity settles.
    let mut elems: Vec<GElem> = Vec::new();

    // Pins (with placement transform applied).
    for sym in &sch.symbols {
        let Some(lib) = sch.lib_for(sym) else {
            continue;
        };
        let is_power = lib.power || sym.lib_id.starts_with("power:");
        // Per-instance reference (not the bare `Reference` property): a sheet
        // instantiated several times (gate_driver U/V/W) shares one drawing, so
        // each instance's pins must carry its own designator (U12/U13/U14) — else
        // the channels' nets are indistinguishable and collapse together.
        let designator = crate::design::reference_for(sym, sheet_path_uuids)
            .unwrap_or("")
            .to_string();
        for pin in &lib.pins {
            if pin.unit != 0 && pin.unit != sym.unit {
                continue;
            }
            let c = place(
                sym.at.x,
                sym.at.y,
                sym.at.angle,
                sym.mirror.as_deref(),
                pin.at.x,
                pin.at.y,
            );
            let node = conn.id(c);
            let pin_uuid = sym.pin_uuid(&pin.number).unwrap_or("");
            if !pin_uuid.is_empty() {
                elems.push(GElem {
                    uuid: pin_uuid.to_string(),
                    kind: if is_power { GKind::PowerPort } else { GKind::Pin },
                    node,
                });
            }
            // Power symbols are net-naming flags: their pin anchors and names the
            // net, but it is not a component terminal.
            if is_power {
                let val = sym.property("Value").unwrap_or("");
                // PWR_FLAG is a KiCad ERC-only marker: it sits on a net (to assert
                // it is driven) but never names it. Treating its "PWR_FLAG" value as
                // a net name would union every flagged rail (GND, +1.2V, +3.3V, …)
                // into one giant net via a shared `P:PWR_FLAG` merge key. Keep its
                // node/glyph for connectivity + addressability, just don't name it.
                if val != "PWR_FLAG" && sym.lib_id != "power:PWR_FLAG" {
                    power.push((node, val.to_string()));
                }
            } else {
                pins.push((
                    node,
                    Terminal {
                        designator: designator.clone(),
                        pin: pin.number.clone(),
                        pin_name: pin.name.clone(),
                        pin_type: pin_type_token(&pin.etype),
                    },
                ));
            }
        }
    }

    // Register the remaining port coordinates so wires can find them.
    let nm = |x: f64, y: f64| P { x: to_nm(x), y: to_nm(y) };
    for w in &sch.wires {
        let node = conn.id(nm(w.a.x, w.a.y));
        conn.id(nm(w.b.x, w.b.y));
        if !w.uuid.is_empty() {
            elems.push(GElem { uuid: w.uuid.clone(), kind: GKind::Wire, node });
        }
    }
    for j in &sch.junctions {
        let node = conn.id(nm(j.at.x, j.at.y));
        if !j.uuid.is_empty() {
            elems.push(GElem { uuid: j.uuid.clone(), kind: GKind::Junction, node });
        }
    }
    for l in &sch.labels {
        let node = conn.id(nm(l.at.x, l.at.y));
        namers.push(Namer {
            node,
            text: l.text.clone(),
            kind: l.kind,
        });
        if !l.uuid.is_empty() {
            let kind = match l.kind {
                LabelKind::Local => GKind::Label,
                LabelKind::Global | LabelKind::Hierarchical => GKind::Port,
            };
            elems.push(GElem { uuid: l.uuid.clone(), kind, node });
        }
    }
    // Sheet-symbol pins are the parent-side endpoints of hierarchical nets: each
    // is a port at its coordinate carrying the child sheet's path + pin name.
    let mut sheet_ports: Vec<(usize, String, String)> = Vec::new(); // node, child_path, name
    for sr in &sch.sheets {
        let child_path = format!("{sheet_path_uuids}{}/", sr.uuid);
        for sp in &sr.pins {
            let node = conn.id(nm(sp.at.x, sp.at.y));
            sheet_ports.push((node, child_path.clone(), sp.name.clone()));
            // Also record the pin's uuid against its net so the viewer can colour the
            // sheet-pin label by its net class (net-class driven, like a label).
            if !sp.uuid.is_empty() {
                elems.push(GElem { uuid: sp.uuid.clone(), kind: GKind::SheetEntry, node });
            }
        }
    }

    // Union every port lying on each wire segment.
    let coords: Vec<P> = conn.ids.keys().copied().collect();
    for w in &sch.wires {
        let a = nm(w.a.x, w.a.y);
        let b = nm(w.b.x, w.b.y);
        let on: Vec<usize> = coords
            .iter()
            .filter(|&&p| on_segment(a, b, p))
            .map(|&p| conn.id(p))
            .collect();
        for k in 1..on.len() {
            conn.union(on[0], on[k]);
        }
    }

    // Gather terminals, naming hints and hierarchical connections per component.
    #[derive(Default)]
    struct Group {
        terminals: Vec<Terminal>,
        power: Vec<String>,
        global: Vec<String>,
        hier: Vec<String>,
        local: Vec<String>,
        /// (child_path_uuids, pin_name) for sheet pins this group touches.
        sheet_pins: Vec<(String, String)>,
        graphical: Graphical,
    }
    let mut groups: HashMap<usize, Group> = HashMap::new();
    for (node, term) in pins {
        let r = conn.find(node);
        groups.entry(r).or_default().terminals.push(term);
    }
    // Power namers may sit on a connected component that has no component pins
    // (e.g. a lone GND flag on a wire stub), so create the group on demand.
    for (node, val) in power {
        let r = conn.find(node);
        groups.entry(r).or_default().power.push(val);
    }
    for nmr in namers {
        let r = conn.find(nmr.node);
        let g = groups.entry(r).or_default();
        match nmr.kind {
            LabelKind::Global => g.global.push(nmr.text),
            LabelKind::Hierarchical => g.hier.push(nmr.text),
            LabelKind::Local => g.local.push(nmr.text),
        }
    }
    for (node, child_path, name) in sheet_ports {
        let r = conn.find(node);
        groups.entry(r).or_default().sheet_pins.push((child_path, name));
    }
    for e in elems {
        let r = conn.find(e.node);
        let g = groups.entry(r).or_default();
        let bucket = match e.kind {
            GKind::Wire => &mut g.graphical.wires,
            GKind::Junction => &mut g.graphical.junctions,
            GKind::Label => &mut g.graphical.labels,
            GKind::Port => &mut g.graphical.ports,
            GKind::PowerPort => &mut g.graphical.power_ports,
            GKind::Pin => &mut g.graphical.pins,
            GKind::SheetEntry => &mut g.graphical.sheet_entries,
        };
        bucket.push(e.uuid);
    }

    // One fragment per connected component, with its merge keys.
    let mut frags = Vec::new();
    for (_root, mut g) in groups {
        let has_namer = !(g.power.is_empty()
            && g.global.is_empty()
            && g.hier.is_empty()
            && g.local.is_empty());
        // Keep anything that carries a terminal, a namer, or a hier connection
        // (the last lets pass-through nets bridge two child sheets).
        if !has_namer && g.terminals.is_empty() && g.sheet_pins.is_empty() {
            continue;
        }
        g.terminals
            .sort_by(|a, b| (&a.designator, &a.pin).cmp(&(&b.designator, &b.pin)));
        let (name, driver_kind) = if has_namer || !g.terminals.is_empty() {
            name_group(sheet_path, &g.power, &g.global, &g.hier, &g.local, &g.terminals)
        } else {
            (String::new(), "pin".to_string())
        };
        let mut keys = Vec::new();
        for x in &g.global {
            keys.push(format!("G:{x}"));
        }
        for x in &g.power {
            keys.push(format!("P:{x}"));
        }
        for x in &g.hier {
            keys.push(format!("H:{sheet_path_uuids}\u{0}{x}"));
        }
        for x in &g.local {
            keys.push(format!("L:{sheet_path_uuids}\u{0}{x}"));
        }
        for (child_path, pin_name) in &g.sheet_pins {
            keys.push(format!("H:{child_path}\u{0}{pin_name}"));
        }
        g.graphical.normalize();
        frags.push(Frag {
            terminals: g.terminals,
            graphical: g.graphical,
            keys,
            name,
            driver_kind,
            sheet: sheet_path_uuids.to_string(),
        });
    }
    frags
}

/// Merge fragments (from one or many sheets) into nets: fragments sharing any
/// key are one net. The merged name is taken from the strongest driver present.
pub fn merge_frags(frags: Vec<Frag>) -> Vec<Net> {
    let n = frags.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != r {
            let nx = parent[c];
            parent[c] = r;
            c = nx;
        }
        r
    }
    let mut key_first: HashMap<&str, usize> = HashMap::new();
    for (i, f) in frags.iter().enumerate() {
        for k in &f.keys {
            if let Some(&j) = key_first.get(k.as_str()) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                if a != b {
                    parent[a] = b;
                }
            } else {
                key_first.insert(k, i);
            }
        }
    }

    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        by_root.entry(r).or_default().push(i);
    }

    let mut slots: Vec<Option<Frag>> = frags.into_iter().map(Some).collect();
    let mut nets = Vec::new();
    for (_root, idxs) in by_root {
        let mut terminals = Vec::new();
        let mut graphical = Graphical::default();
        let mut by_sheet: BTreeMap<String, Graphical> = BTreeMap::new();
        let mut best: Option<(u8, String, String)> = None; // (rank, name, driver)
        for &i in &idxs {
            let f = slots[i].take().unwrap();
            terminals.extend(f.terminals);
            // Each fragment's graphical elements belong to its sheet instance; keep
            // that attribution (per instance) and accumulate the flat union too.
            by_sheet.entry(f.sheet).or_default().extend(f.graphical.clone());
            graphical.extend(f.graphical);
            if !f.name.is_empty() {
                let rank = driver_rank(&f.driver_kind);
                let better = match &best {
                    None => true,
                    Some((br, bn, _)) => rank > *br || (rank == *br && &f.name < bn),
                };
                if better {
                    best = Some((rank, f.name, f.driver_kind));
                }
            }
        }
        terminals.sort_by(|a, b| (&a.designator, &a.pin).cmp(&(&b.designator, &b.pin)));
        terminals.dedup();
        if terminals.is_empty() {
            continue;
        }
        graphical.normalize();
        for g in by_sheet.values_mut() {
            g.normalize();
        }
        let (name, driver_kind) = match best {
            Some((_, name, driver)) => (name, driver),
            None => name_group("/", &[], &[], &[], &[], &terminals),
        };
        nets.push(Net {
            name,
            driver_kind,
            terminals,
            graphical,
            by_sheet,
        });
    }
    // Total order → byte-deterministic design.json. `sort_by(name)` is *stable*, so
    // same-named nets (bus members split across hierarchical sheet pins) keep their
    // pre-sort order, which comes from HashMap iteration and shuffles every run.
    // Tie-break on the net's already-canonicalised content so distinct nets order
    // deterministically and truly-identical nets stay stable.
    nets.sort_by_cached_key(net_order_key);
    nets
}

/// Stable, total-order sort key for a compiled net (see `merge_frags`): name first,
/// then content (driver, terminal refs, graphical uuids) so the net array — and the
/// `uid`s the caller assigns by position — never depend on HashMap iteration order.
/// terminals + graphical buckets are already sorted/deduped above.
fn net_order_key(n: &Net) -> (String, String, String, String) {
    let terminals = n
        .terminals
        .iter()
        .map(|t| format!("{}.{}", t.designator, t.pin))
        .collect::<Vec<_>>()
        .join(",");
    let mut uuids: Vec<&str> = n.graphical.all_uuids().map(String::as_str).collect();
    uuids.sort_unstable();
    (n.name.clone(), n.driver_kind.clone(), terminals, uuids.join(","))
}

fn name_group(
    sheet_path: &str,
    power: &[String],
    global: &[String],
    hier: &[String],
    local: &[String],
    terminals: &[Terminal],
) -> (String, String) {
    if let Some(p) = power.iter().min() {
        return (p.clone(), "global_power_pin".into());
    }
    if let Some(g) = global.iter().min() {
        return (g.clone(), "global_label".into());
    }
    if let Some(h) = hier.iter().min() {
        return (h.clone(), "hier_label".into());
    }
    if let Some(l) = local.iter().min() {
        return (format!("{sheet_path}{l}"), "local_label".into());
    }
    // No namer: KiCad auto-names. A lone pin is "unconnected-(REF-PINNAME-PadN)"
    // (pin-name slashes escaped); a multi-pin unnamed net is "Net-(REF-PadN)".
    let t = &terminals[0];
    if terminals.len() == 1 {
        let name_part = if t.pin_name.is_empty() || t.pin_name == "~" {
            String::new()
        } else {
            format!("{}-", t.pin_name.replace('/', "{slash}"))
        };
        return (
            format!("unconnected-({}-{}Pad{})", t.designator, name_part, t.pin),
            "pin".into(),
        );
    }
    (format!("Net-({}-Pad{})", t.designator, t.pin), "pin".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two resistors sharing a wire on one pin, separate on the other.
    const SAMPLE: &str = r#"
    (kicad_sch
      (lib_symbols
        (symbol "Device:R"
          (symbol "R_1_1"
            (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
            (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (instances))
      (symbol (lib_id "Device:R") (at 100 110 0) (unit 1) (uuid "r2")
        (property "Reference" "R2") (instances))
      (wire (pts (xy 100 102.54) (xy 100 107.46)))
      (label "MID" (at 100 102.54 0)))
    "#;

    // Same topology as SAMPLE but with uuids on the wire, label and instance pins,
    // so the graphical attribution can be checked.
    const SAMPLE_UUIDS: &str = r#"
    (kicad_sch
      (lib_symbols
        (symbol "Device:R"
          (symbol "R_1_1"
            (pin passive line (at 0 2.54 270) (length 1.27) (name "~") (number "1"))
            (pin passive line (at 0 -2.54 90) (length 1.27) (name "~") (number "2")))))
      (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (uuid "r1")
        (property "Reference" "R1") (pin "1" (uuid "r1p1")) (pin "2" (uuid "r1p2")) (instances))
      (symbol (lib_id "Device:R") (at 100 110 0) (unit 1) (uuid "r2")
        (property "Reference" "R2") (pin "1" (uuid "r2p1")) (pin "2" (uuid "r2p2")) (instances))
      (wire (pts (xy 100 102.54) (xy 100 107.46)) (uuid "w1"))
      (label "MID" (at 100 102.54 0) (uuid "lbl1")))
    "#;

    #[test]
    fn attributes_graphical_elements_to_their_net() {
        let sch = Schematic::parse_str(SAMPLE_UUIDS).unwrap();
        let nets = compile(&sch);
        let mid = nets.iter().find(|n| n.name == "/MID").expect("named net /MID");
        // The wire, the label, and the two wired pins all belong to /MID.
        assert_eq!(mid.graphical.wires, vec!["w1".to_string()]);
        assert_eq!(mid.graphical.labels, vec!["lbl1".to_string()]);
        assert_eq!(mid.graphical.pins, vec!["r1p2".to_string(), "r2p1".to_string()]);
        // all_uuids covers every bucket.
        let all: Vec<&String> = mid.graphical.all_uuids().collect();
        assert!(all.contains(&&"w1".to_string()) && all.contains(&&"lbl1".to_string()));
        // The other two pins live on their own single-pin nets, each carrying its pin uuid.
        let lone: Vec<&String> = nets
            .iter()
            .filter(|n| n.terminals.len() == 1)
            .flat_map(|n| n.graphical.pins.iter())
            .collect();
        assert!(lone.contains(&&"r1p1".to_string()) && lone.contains(&&"r2p2".to_string()));
    }

    #[test]
    fn joins_two_pins_on_a_wire() {
        let sch = Schematic::parse_str(SAMPLE).unwrap();
        let nets = compile(&sch);
        // R1.pin2 (at 100,102.54) and R2.pin1 (at 100,107.46) are wired together.
        // Local labels take the root sheet-path prefix, so "MID" -> "/MID".
        let mid = nets.iter().find(|n| n.name == "/MID").expect("named net /MID");
        let mut who: Vec<_> = mid
            .terminals
            .iter()
            .map(|t| (t.designator.as_str(), t.pin.as_str()))
            .collect();
        who.sort();
        assert_eq!(who, vec![("R1", "2"), ("R2", "1")]);
        assert!(mid.terminals.iter().all(|t| t.pin_type == "PASSIVE"));
        // The other two pins are their own single-terminal nets.
        assert_eq!(nets.iter().filter(|n| n.terminals.len() == 1).count(), 2);
    }
}
