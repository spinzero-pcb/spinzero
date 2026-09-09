//! Altium schematic connectivity, projected into the shared `netlist::Frag`.
//!
//! Shape: union-find over connection points, per sheet, then the shared
//! `netlist::merge_frags` across the hierarchy, then the Altium-specific naming
//! passes (case consolidation, power bridging) in `super::naming`.
//!
//! Connection points are compared **exactly**. The plan (§3.5) proposes the
//! sheet's `HotSpotGridSize` as a tolerance, but that is the snap radius Altium
//! uses while a wire is being drawn — 40 mil here, wide enough to join two
//! parallel wires one grid step apart. What the files mostly contain is on-grid
//! geometry: every one of the corpus's sheet entries lands exactly on a wire
//! endpoint. So exact endpoints plus point-on-segment containment (which is what
//! joins a T-junction) is the model.
//!
//! [`hot_spot_pass`] is the one exception, and it is a fallback rather than a
//! tolerance: a pin whose free end touches nothing at all joins the nearest wire
//! vertex inside the snap radius. It cannot change a connection the exact pass
//! already made. The corpus needs it — four of `TR1`'s pins on the 5BR design
//! end just short of the wire drawn to them — and every pin it joins is counted,
//! along with the pins that stay unconnected, so both show as numbers.

use std::collections::{BTreeMap, HashMap};

use eda_parse_altium::sch::{self, Pt, SchDoc};

use crate::netlist::{Frag, Graphical, Terminal};

use super::oid;

/// Supply names a hidden pin implicitly connects to (§4.4). Kept explicit
/// because it is load-bearing: miss it and every part with hidden power pins
/// reads as having unpowered supply pins.
const SUPPLY_NAMES: &[&str] = &[
    "GND", "VCC", "VDD", "VSS", "VEE", "AVDD", "AVSS", "DVDD", "DVSS", "AGND", "DGND", "VCCA",
    "VCCD", "VDDA", "VDDD",
];

/// Chassis names, all of which mean `GND`.
const CHASSIS_NAMES: &[&str] = &["CHASSI", "CHASSIS", "SHIELD", "EARTH", "GND_CHASSIS"];

/// The net a hidden pin implicitly joins, if any.
fn implicit_supply(pin: &sch::Pin) -> Option<String> {
    if !pin.hidden_net_name.is_empty() {
        return Some(pin.hidden_net_name.clone());
    }
    let up = pin.name.trim().to_ascii_uppercase();
    if CHASSIS_NAMES.contains(&up.as_str()) {
        return Some("GND".to_string());
    }
    SUPPLY_NAMES.contains(&up.as_str()).then_some(up)
}

/// What the netlist pass could not settle on one sheet.
#[derive(Debug, Default, Clone)]
pub struct SheetDiagnostics {
    /// Component pins with no wire, junction, label or port at their location.
    pub unconnected_pins: usize,
    /// Pins connected implicitly because they are hidden supply pins.
    pub hidden_supply_pins: usize,
    /// Bus polylines seen; buses are recorded, not expanded into member nets.
    pub buses: usize,
    /// Hidden pins naming no supply whose pad a visible pin already claims. They
    /// are a second connection point on that pad, so they get no terminal.
    pub hidden_pins_without_net: usize,
    /// Pins joined to a wire by the sheet's own snap radius rather than exactly.
    pub pins_joined_by_hot_spot: usize,
    /// Pins left unconnected because two wire vertices were equally near.
    pub hot_spot_ambiguous: usize,
    /// Net labels carrying no text. They name nothing and draw nothing.
    pub unnamed_labels: usize,
}

/// Union-find over exact connection points.
#[derive(Default)]
struct Conn {
    parent: Vec<usize>,
    ids: HashMap<Pt, usize>,
    /// How many elements registered each node. A node with one use is a point
    /// nothing else in the sheet touches, which is what the hot-spot pass looks
    /// for.
    uses: Vec<u32>,
}

impl Conn {
    fn id(&mut self, p: Pt) -> usize {
        if let Some(&i) = self.ids.get(&p) {
            self.uses[i] += 1;
            return i;
        }
        let i = self.parent.len();
        self.parent.push(i);
        self.uses.push(1);
        self.ids.insert(p, i);
        i
    }

    /// Look a point up without registering a use.
    fn at(&self, p: Pt) -> Option<usize> {
        self.ids.get(&p).copied()
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

/// Join a pin that reaches nothing to the nearest wire vertex inside the
/// sheet's own snap radius.
///
/// Exact matching stays the rule (see the module header), and this runs only
/// where exact matching found nothing at all. Altium snaps a pin's hot spot to a
/// nearby wire end while the designer draws, and it keeps the connection when
/// the two do not land on the same coordinate. `TR1` on the 5BR design is the
/// corpus case: pins 8, 10, 12 and 14 end 0.53 and 0.87 sheet units short of the
/// wire drawn to them, which is invisible on screen and cost four real
/// connections.
///
/// Three conditions bound it, so it can only add a connection the design draws:
///
/// 1. The pin's free end touches **nothing** — no wire, no label, no port, no
///    other pin. A pin already joined to anything is left alone.
/// 2. Only a wire VERTEX counts as a target, not a point part-way along a
///    segment. A pin that misses the middle of a wire is a broken design, not a
///    snap.
/// 3. One nearest vertex must win outright. Two at the same distance would be a
///    coin flip between two nets, so the pin stays unconnected and
///    `hot_spot_ambiguous` counts it.
fn hot_spot_pass(sch: &SchDoc, conn: &mut Conn, pin_ends: &[(usize, Pt)], diag: &mut SheetDiagnostics) {
    let tol = sch.sheet.hot_spot_grid;
    if tol <= 0 {
        return;
    }
    let limit = (tol as i128) * (tol as i128);
    for &(node, p) in pin_ends {
        if conn.uses[node] > 1 {
            continue;
        }
        if sch
            .wires
            .iter()
            .chain(&sch.buses)
            .any(|w| w.pts.windows(2).any(|s| on_segment(s[0], s[1], p)))
        {
            continue;
        }
        let mut near: Vec<(i128, usize)> = Vec::new();
        for v in sch.wires.iter().flat_map(|w| w.pts.iter()) {
            let d = dist2(*v, p);
            if d <= limit {
                if let Some(target) = conn.at(*v) {
                    near.push((d, target));
                }
            }
        }
        let Some(&(best, _)) = near.iter().min_by_key(|(d, _)| *d) else {
            continue;
        };
        // A tie only matters between two DIFFERENT nets. Two ends of one wire
        // equally near are one answer, not a choice.
        let mut roots: Vec<usize> = near
            .iter()
            .filter(|(d, _)| *d == best)
            .map(|&(_, t)| conn.find(t))
            .collect();
        roots.sort_unstable();
        roots.dedup();
        match roots.as_slice() {
            [target] => {
                conn.union(node, *target);
                diag.pins_joined_by_hot_spot += 1;
            }
            _ => diag.hot_spot_ambiguous += 1,
        }
    }
}

/// Squared distance between two points, in coordinate units.
fn dist2(a: Pt, b: Pt) -> i128 {
    let (dx, dy) = ((a.x - b.x) as i128, (a.y - b.y) as i128);
    dx * dx + dy * dy
}

/// True when `p` lies on the segment `a`-`b` (endpoints included). Integer maths
/// throughout, so there is no epsilon to tune.
fn on_segment(a: Pt, b: Pt, p: Pt) -> bool {
    let cross = (b.x - a.x) as i128 * (p.y - a.y) as i128 - (b.y - a.y) as i128 * (p.x - a.x) as i128;
    if cross != 0 {
        return false;
    }
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// The kind of an addressable element, routing its uuid into a `Graphical` bucket.
///
/// Ids come through [`super::oid`], the same handle the renderer stamps as
/// `data-uuid`. Altium names no junction and drops the id on a few percent of
/// pins and graphics; skipping those left the SVG drawing a group the
/// cross-probe indexes could not resolve.
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

struct GElem {
    uuid: String,
    kind: GKind,
    node: usize,
}

/// A namer attached to a connectivity node.
struct Namer {
    node: usize,
    text: String,
    kind: NameKind,
}

#[derive(Clone, Copy, PartialEq)]
enum NameKind {
    NetLabel,
    PowerPort,
    SheetEntry,
    Port,
}

/// Compute the connected fragments of one sheet instance.
///
/// `sheet_path_uuids` (the chain of sheet-symbol `UniqueID`s, ends in `/`)
/// scopes local labels and hierarchical connections to this instance, so a
/// parent's sheet entry links to the matching child port and nowhere else.
pub fn fragments(
    sch: &SchDoc,
    sheet_path_uuids: &str,
    opts: &eda_parse_altium::CompileOptions,
    channel: Option<&super::design::Channel>,
    diag: &mut SheetDiagnostics,
) -> Vec<Frag> {
    let mut conn = Conn::default();
    let mut pins: Vec<(usize, Terminal)> = Vec::new();
    // Each visible pin's free end, for the hot-spot pass below.
    let mut pin_ends: Vec<(usize, Pt)> = Vec::new();
    let mut namers: Vec<Namer> = Vec::new();
    let mut elems: Vec<GElem> = Vec::new();
    // Implicit connections for hidden supply pins, keyed by net name.
    let mut implicit: Vec<(usize, String)> = Vec::new();
    // Parent-side hierarchy endpoints: (node, child sheet path, entry name).
    let mut entry_links: Vec<(usize, String, String)> = Vec::new();
    // Child-side hierarchy endpoints: (node, this sheet path, port name).
    let mut port_links: Vec<(usize, String)> = Vec::new();

    // Every wire and bus vertex is a node, so segments can find what sits on them.
    for w in sch.wires.iter().chain(&sch.buses) {
        for p in &w.pts {
            conn.id(*p);
        }
    }
    diag.buses += sch.buses.len();

    for c in &sch.components {
        let base = c.designator.trim();
        if base.is_empty() {
            continue;
        }
        // Terminals must carry the SAME designator the component list does, or
        // a channel's nets name parts that are not in the design.
        let designator = match channel {
            Some(ch) => ch.designator(base),
            None => base.to_string(),
        };
        // Pads the symbol draws a VISIBLE pin for. A hidden pin sharing one of
        // these is a second connection point on the same pad, and the visible
        // pin is the one the sheet can reach.
        let drawn: std::collections::HashSet<&str> = super::design::placed_pins(c)
            .filter(|p| !p.hidden())
            .map(|p| p.number.as_str())
            .collect();
        for pin in super::design::placed_pins(c) {
            // A hidden pin is not drawn, so nothing on the sheet can reach it.
            // It connects only through the supply its name or its
            // `hidden_net_name` states. One that names no supply and shares its
            // pad with a visible pin adds nothing but a second row: `TR1` on the
            // 5BR design has five of those, and each one became its own
            // `unconnected-(TR1-PadN)` net beside the connected pad.
            //
            // A hidden pin with a pad of its OWN keeps its terminal. That pad
            // really is unconnected, and dropping it would hide an unconnected
            // pad from the review, which is the opposite of the point.
            let supply = pin.hidden().then(|| implicit_supply(pin)).flatten();
            if pin.hidden() && supply.is_none() && drawn.contains(pin.number.as_str()) {
                diag.hidden_pins_without_net += 1;
                continue;
            }
            let node = conn.id(pin.connection());
            elems.push(GElem { uuid: oid(&pin.uuid, "pin", pin.at), kind: GKind::Pin, node });
            pins.push((
                node,
                Terminal {
                    designator: designator.clone(),
                    pin: pin.number.clone(),
                    pin_name: pin.name.clone(),
                    pin_type: sch::pin_type(pin.electrical).to_string(),
                },
            ));
            if let Some(net) = supply {
                implicit.push((node, net));
            }
            if !pin.hidden() {
                pin_ends.push((node, pin.connection()));
            }
        }
    }

    for j in &sch.junctions {
        let node = conn.id(j.at);
        elems.push(GElem { uuid: oid(&j.uuid, "j", j.at), kind: GKind::Junction, node });
    }
    // Net labels match on strict fractional coordinates — the tolerance is
    // asymmetric, and a loose match attaches a label to the wrong net.
    //
    // A label with NO TEXT names nothing and draws nothing, so it is not a
    // namer. Altium keeps a great many of them: 31 on one MCU144E1 sheet and 27
    // on another. Each one carried the same empty merge key, and 112 fragments
    // chained through it into a single 67-terminal net where the reference has
    // 37 separate ones. An empty name is not a name.
    for l in &sch.net_labels {
        if l.text.trim().is_empty() {
            diag.unnamed_labels += 1;
            continue;
        }
        let node = conn.id(l.at);
        namers.push(Namer { node, text: l.text.clone(), kind: NameKind::NetLabel });
        elems.push(GElem { uuid: oid(&l.uuid, "nl", l.at), kind: GKind::Label, node });
    }
    // A power port still DRAWS its rail symbol with no text, so it stays an
    // addressable element. It just does not name the net.
    for p in &sch.power_ports {
        let node = conn.id(p.at);
        if !p.text.trim().is_empty() {
            namers.push(Namer { node, text: p.text.clone(), kind: NameKind::PowerPort });
        }
        elems.push(GElem { uuid: oid(&p.uuid, "pp", p.at), kind: GKind::PowerPort, node });
    }
    // A port connects at BOTH edges; taking only its origin misses half of a
    // design's port connections.
    for p in &sch.ports {
        let ts = p.terminals();
        let node = conn.id(ts[0]);
        let far = conn.id(ts[1]);
        conn.union(node, far);
        // A nameless port draws its outline but links to nothing: a hierarchy
        // link keyed on an empty name would join every nameless port in the
        // design.
        if !p.name.trim().is_empty() {
            namers.push(Namer { node, text: p.name.clone(), kind: NameKind::Port });
            port_links.push((node, p.name.clone()));
        }
        elems.push(GElem { uuid: oid(&p.uuid, "p", p.at), kind: GKind::Port, node });
    }
    // Sheet entries store no coordinate; theirs is computed from the parent
    // symbol's rectangle plus `Side` and `DistanceFromTop`.
    for s in &sch.sheet_symbols {
        let child = format!("{sheet_path_uuids}{}/", s.uuid);
        for e in &s.entries {
            let node = conn.id(s.entry_point(e));
            if !e.name.trim().is_empty() {
                entry_links.push((node, child.clone(), e.name.clone()));
                namers.push(Namer { node, text: e.name.clone(), kind: NameKind::SheetEntry });
            }
            elems.push(GElem {
                uuid: oid(&e.uuid, "se", s.entry_point(e)),
                kind: GKind::SheetEntry,
                node,
            });
        }
    }
    for w in &sch.wires {
        if let Some(first) = w.pts.first() {
            let node = conn.id(*first);
            elems.push(GElem { uuid: oid(&w.uuid, "wire", *first), kind: GKind::Wire, node });
        }
    }

    // Union every registered point lying on each WIRE segment.
    let coords: Vec<Pt> = conn.ids.keys().copied().collect();
    for w in &sch.wires {
        for seg in w.pts.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let on: Vec<usize> = coords
                .iter()
                .filter(|&&p| on_segment(a, b, p))
                .filter_map(|&p| conn.at(p))
                .collect();
            for k in 1..on.len() {
                conn.union(on[0], on[k]);
            }
        }
    }
    // A BUS is not a net. It joins only its own vertices, so a multi-segment bus
    // stays one fragment and nothing else is pulled onto it. Altium connects a
    // wire to a bus through a bus entry and the result is a bus MEMBER, which is
    // a name this extraction does not expand (`buses_not_expanded`).
    //
    // Treating a bus like a wire made it swallow everything drawn across it: on
    // the 10KW gate-driver board one `GND_X_[1...12]` bus became a single
    // 96-terminal net where the reference has twelve, one per member.
    for b in &sch.buses {
        let on: Vec<usize> = b.pts.iter().filter_map(|&p| conn.at(p)).collect();
        for k in 1..on.len() {
            conn.union(on[0], on[k]);
        }
    }

    hot_spot_pass(sch, &mut conn, &pin_ends, diag);

    // Gather each connected group.
    #[derive(Default)]
    struct Group {
        terminals: Vec<Terminal>,
        labels: Vec<String>,
        power: Vec<String>,
        entries: Vec<String>,
        ports: Vec<String>,
        implicit: Vec<String>,
        entry_links: Vec<(String, String)>,
        port_links: Vec<String>,
        graphical: Graphical,
    }
    let mut groups: HashMap<usize, Group> = HashMap::new();
    for (node, t) in pins {
        let r = conn.find(node);
        groups.entry(r).or_default().terminals.push(t);
    }
    for n in namers {
        let r = conn.find(n.node);
        let g = groups.entry(r).or_default();
        match n.kind {
            NameKind::NetLabel => g.labels.push(n.text),
            NameKind::PowerPort => g.power.push(n.text),
            NameKind::SheetEntry => g.entries.push(n.text),
            NameKind::Port => g.ports.push(n.text),
        }
    }
    for (node, net) in implicit {
        let r = conn.find(node);
        groups.entry(r).or_default().implicit.push(net);
    }
    for (node, child, name) in entry_links {
        let r = conn.find(node);
        groups.entry(r).or_default().entry_links.push((child, name));
    }
    for (node, name) in port_links {
        let r = conn.find(node);
        groups.entry(r).or_default().port_links.push(name);
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

    let mut frags = Vec::new();
    for (_root, mut g) in groups {
        let named = !(g.labels.is_empty()
            && g.power.is_empty()
            && g.implicit.is_empty()
            && (g.entries.is_empty() || !opts.allow_sheet_entry_net_names)
            && (g.ports.is_empty() || !opts.allow_port_net_names));
        if g.terminals.is_empty() && g.entry_links.is_empty() && g.port_links.is_empty() && !named {
            continue;
        }
        if g.terminals.len() == 1 && !named && g.entry_links.is_empty() && g.port_links.is_empty() {
            diag.unconnected_pins += 1;
        }
        diag.hidden_supply_pins += g.implicit.len();
        g.terminals
            .sort_by(|a, b| (&a.designator, &a.pin).cmp(&(&b.designator, &b.pin)));
        let (name, driver_kind) = name_group(
            &g.labels, &g.power, &g.entries, &g.ports, &g.implicit, &g.terminals, opts, channel,
        );

        // Merge keys. Power and implicit-supply names are global; net labels are
        // sheet-local unless the design compiles with global scope; hierarchy
        // links are scoped to the sheet instance they cross.
        let mut keys = Vec::new();
        for x in g.power.iter().chain(&g.implicit) {
            keys.push(format!("P:{}", x.to_ascii_uppercase()));
        }
        let label_global = opts.hierarchy_mode == eda_parse_altium::HierarchyMode::Global;
        for x in &g.labels {
            if label_global {
                keys.push(format!("G:{}", x.to_ascii_uppercase()));
            } else {
                keys.push(format!("L:{sheet_path_uuids}\u{0}{}", x.to_ascii_uppercase()));
            }
        }
        // A port on this sheet joins the sheet entry of this sheet's placement
        // in its parent; both sides key on (this sheet instance, name).
        for name in &g.port_links {
            keys.push(format!("H:{sheet_path_uuids}\u{0}{}", name.to_ascii_uppercase()));
        }
        for (child, name) in &g.entry_links {
            keys.push(format!("H:{child}\u{0}{}", name.to_ascii_uppercase()));
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

/// Name a group, in Altium's precedence.
///
/// Net labels come FIRST by default and power ports second —
/// `PowerPortNamesTakePriority` swaps them. Sheet entries and ports may name a
/// net only when the project's compile options allow it, which is on for entries
/// and off for ports by default.
#[allow(clippy::too_many_arguments)]
fn name_group(
    labels: &[String],
    power: &[String],
    entries: &[String],
    ports: &[String],
    implicit: &[String],
    terminals: &[Terminal],
    opts: &eda_parse_altium::CompileOptions,
    channel: Option<&super::design::Channel>,
) -> (String, String) {
    // Altium names a net after the label itself — the sheet path is NOT part of
    // the name, which is what the board's own net table shows. Scope is a merge
    // question, not a naming one. A repeated sheet is the exception: its local
    // names carry the channel index, so `DESAT` on two placements is `DESAT_1`
    // and `DESAT_2` on the board.
    let suffix = channel.map(|c| c.net_suffix()).unwrap_or_default();
    let label = labels
        .iter()
        .min()
        .map(|l| (format!("{l}{suffix}"), "local_label".to_string()));
    let pwr = power
        .iter()
        .min()
        .map(|p| (p.clone(), "global_power_pin".to_string()));
    let (first, second) = if opts.power_port_names_take_priority {
        (pwr.clone(), label.clone())
    } else {
        (label, pwr)
    };
    if let Some(x) = first.or(second) {
        return x;
    }
    if let Some(x) = implicit.iter().min() {
        return (x.clone(), "global_power_pin".into());
    }
    if opts.allow_sheet_entry_net_names {
        if let Some(x) = entries.iter().min() {
            return (x.clone(), "hier_label".into());
        }
    }
    if opts.allow_port_net_names {
        if let Some(x) = ports.iter().min() {
            return (x.clone(), "hier_label".into());
        }
    }
    // Nets bridged only through the hierarchy still need a name; fall back to
    // the entry/port name before auto-naming from a pin.
    if let Some(x) = entries.iter().chain(ports).min() {
        return (x.clone(), "hier_label".into());
    }
    let Some(t) = terminals.first() else {
        return (String::new(), "pin".into());
    };
    if terminals.len() == 1 {
        let part = if t.pin_name.is_empty() || t.pin_name == "~" {
            String::new()
        } else {
            format!("{}-", t.pin_name.replace('/', "{slash}"))
        };
        return (format!("unconnected-({}-{}Pad{})", t.designator, part, t.pin), "pin".into());
    }
    // Altium auto-names an unnamed net `Net<designator>_<pin>` after its first
    // terminal, and that is the name the board carries — `NetC19_2`, not
    // `NETC19_2`, which is what the §8.3 differential and the board's own
    // `Nets6` table both say. Using KiCad's `Net-(REF-PadN)` form here would
    // disagree with every board net.
    (format!("Net{}_{}", t.designator, t.pin), "pin".into())
}

/// Consolidate net names that differ only in case.
///
/// Altium net names are case-insensitive: `VCC` and `Vcc` are one net. The merge
/// keys are already upper-cased so the fragments join; this picks the display
/// spelling — the most-used one, ties broken alphabetically so the output stays
/// byte-deterministic.
pub fn consolidate_case(nets: &mut [crate::netlist::Net]) {
    let mut votes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for n in nets.iter() {
        *votes
            .entry(n.name.to_ascii_uppercase())
            .or_default()
            .entry(n.name.clone())
            .or_default() += 1;
    }
    let display: BTreeMap<String, String> = votes
        .into_iter()
        .filter_map(|(up, spellings)| {
            spellings
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|(name, _)| (up, name))
        })
        .collect();
    for n in nets.iter_mut() {
        if let Some(d) = display.get(&n.name.to_ascii_uppercase()) {
            n.name = d.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_parse_altium::sch::{Component, ComponentKind, NetLabel, Pin, Port, PowerPort, SheetEntry, SheetSymbol, Wire, UNIT};
    use eda_parse_altium::CompileOptions;

    fn p(x: i64, y: i64) -> Pt {
        Pt { x: x * UNIT, y: y * UNIT }
    }

    /// `at` is a pin's BODY end; zero length puts the connection point there
    /// too, so these fixtures read as coordinates on the wire.
    fn pin(number: &str, name: &str, at: Pt, conglomerate: i64) -> Pin {
        Pin {
            number: number.into(),
            name: name.into(),
            electrical: 4,
            conglomerate,
            length: 0,
            at,
            part_id: 1,
            uuid: format!("pin-{number}-{}-{}", at.x, at.y),
            ..Default::default()
        }
    }

    fn comp(designator: &str, pins: Vec<Pin>) -> Component {
        Component {
            library_ref: "R".into(),
            designator: designator.into(),
            part_count: 1,
            current_part_id: 1,
            kind: ComponentKind::Standard,
            uuid: format!("c-{designator}"),
            pins,
            ..Default::default()
        }
    }

    fn nets(sch: &SchDoc, opts: &CompileOptions) -> Vec<crate::netlist::Net> {
        let mut d = SheetDiagnostics::default();
        let mut n = crate::netlist::merge_frags(fragments(sch, "/", opts, None, &mut d));
        consolidate_case(&mut n);
        n
    }

    /// A wire joins two pins, and a net label on that wire names the result.
    #[test]
    fn a_wire_joins_pins_and_a_label_names_them() {
        let sch = SchDoc {
            components: vec![
                comp("R1", vec![pin("2", "~", p(100, 100), 32)]),
                comp("R2", vec![pin("1", "~", p(100, 140), 32)]),
            ],
            wires: vec![Wire { pts: vec![p(100, 100), p(100, 140)], uuid: "w1".into(), ..Default::default() }],
            net_labels: vec![NetLabel { at: p(100, 100), text: "MID".into(), uuid: "l1".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let n = nets(&sch, &CompileOptions::board_project());
        let mid = n.iter().find(|n| n.name == "MID").expect("named MID");
        let who: Vec<_> = mid.terminals.iter().map(|t| t.designator.as_str()).collect();
        assert_eq!(who, vec!["R1", "R2"]);
        assert_eq!(mid.graphical.wires, vec!["w1".to_string()]);
    }

    /// An unnamed net takes Altium's own auto-name, spelled the way the board's
    /// `Nets6` table spells it: `NetC19_2`, not `NETC19_2`. Ours disagreed with
    /// every auto-named board net in case until the plan §8.3 differential said
    /// so.
    #[test]
    fn an_unnamed_net_is_named_the_way_the_board_names_it() {
        let sch = SchDoc {
            components: vec![
                comp("C19", vec![pin("2", "~", p(100, 100), 32)]),
                comp("R4", vec![pin("1", "~", p(100, 140), 32)]),
            ],
            wires: vec![Wire { pts: vec![p(100, 100), p(100, 140)], uuid: "w1".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let n = nets(&sch, &CompileOptions::board_project());
        let names: Vec<&str> = n.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["NetC19_2"]);
    }

    /// Corner case 16: labels outrank power ports by default, and
    /// `PowerPortNamesTakePriority` is what swaps them.
    #[test]
    fn net_labels_outrank_power_ports_by_default() {
        let sch = SchDoc {
            components: vec![
                comp("R1", vec![pin("1", "~", p(100, 100), 32)]),
                comp("R2", vec![pin("1", "~", p(100, 140), 32)]),
            ],
            wires: vec![Wire { pts: vec![p(100, 100), p(100, 140)], uuid: "w1".into(), ..Default::default() }],
            net_labels: vec![NetLabel { at: p(100, 100), text: "RAIL".into(), uuid: "l1".into(), ..Default::default() }],
            power_ports: vec![PowerPort { at: p(100, 140), text: "P5V".into(), style: 2, uuid: "pp1".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let opts = CompileOptions::board_project();
        assert_eq!(nets(&sch, &opts)[0].name, "RAIL");
        let mut swapped = opts.clone();
        swapped.power_port_names_take_priority = true;
        assert_eq!(nets(&sch, &swapped)[0].name, "P5V");
    }

    /// Corner case 17: a port's far edge is a connection point too.
    #[test]
    fn a_port_connects_at_both_of_its_edges() {
        let sch = SchDoc {
            components: vec![
                comp("R1", vec![pin("1", "~", p(60, 620), 32)]),
                comp("R2", vec![pin("1", "~", p(130, 620), 32)]),
            ],
            ports: vec![Port { at: p(60, 620), width: 70, name: "IN_P".into(), io_type: 2, uuid: "prt".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let n = nets(&sch, &CompileOptions::board_project());
        assert_eq!(n.len(), 1, "both pins share the port's net");
        let who: Vec<_> = n[0].terminals.iter().map(|t| t.designator.as_str()).collect();
        assert_eq!(who, vec!["R1", "R2"]);
    }

    /// Corner case 18: a sheet entry's hotspot bridges parent and child.
    #[test]
    fn a_sheet_entry_bridges_to_the_childs_port() {
        let parent = SchDoc {
            components: vec![comp("R1", vec![pin("1", "~", p(340, 660), 32)])],
            sheet_symbols: vec![SheetSymbol {
                at: p(190, 710),
                xsize: 150,
                ysize: 110,
                uuid: "SYM".into(),
                name: "Sub".into(),
                filename: "sub.SchDoc".into(),
                entries: vec![SheetEntry {
                    name: "SIG".into(),
                    side: 1,
                    distance_from_top: 5,
                    io_type: 2,
                    uuid: "e1".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..SchDoc::default()
        };
        let child = SchDoc {
            components: vec![comp("C1", vec![pin("1", "~", p(60, 620), 32)])],
            ports: vec![Port { at: p(60, 620), width: 70, name: "SIG".into(), io_type: 2, uuid: "prt".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let opts = CompileOptions::board_project();
        let mut d = SheetDiagnostics::default();
        let mut frags = fragments(&parent, "/", &opts, None, &mut d);
        frags.extend(fragments(&child, "/SYM/", &opts, None, &mut d));
        let n = crate::netlist::merge_frags(frags);
        let sig = n.iter().find(|n| n.name == "SIG").expect("hierarchical net SIG");
        let who: Vec<_> = sig.terminals.iter().map(|t| t.designator.as_str()).collect();
        assert_eq!(who, vec!["C1", "R1"]);
    }

    /// Corner case 15: a hidden supply pin is connected even with no wire.
    #[test]
    fn hidden_supply_pins_join_their_rail() {
        let hidden = pin("8", "VDD", p(500, 500), 32 | super::sch::PIN_HIDDEN_BIT);
        let sch = SchDoc {
            components: vec![
                comp("U1", vec![hidden]),
                comp("C1", vec![pin("1", "~", p(200, 200), 32)]),
            ],
            power_ports: vec![PowerPort { at: p(200, 200), text: "VDD".into(), style: 2, uuid: "pp".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let mut d = SheetDiagnostics::default();
        let mut n = crate::netlist::merge_frags(fragments(&sch, "/", &CompileOptions::board_project(), None, &mut d));
        consolidate_case(&mut n);
        let vdd = n.iter().find(|n| n.name == "VDD").expect("VDD net");
        let who: Vec<_> = vdd.terminals.iter().map(|t| t.designator.as_str()).collect();
        assert_eq!(who, vec!["C1", "U1"], "the hidden pin joins the rail");
        assert_eq!(d.hidden_supply_pins, 1);
    }

    /// A wire drawn a fraction short of a pin still connects it. Four of `TR1`'s
    /// pins on the 5BR design end 0.53 and 0.87 sheet units inside the wire, a
    /// gap of 5 to 9 mil that is invisible on screen and cost four real nets.
    #[test]
    fn a_pin_a_fraction_short_of_its_wire_still_joins_it() {
        let short = Pt { x: 100 * UNIT + UNIT / 2, y: 100 * UNIT };
        let sch = SchDoc {
            sheet: sch::SheetProps { hot_spot_grid: 4 * UNIT, ..Default::default() },
            components: vec![
                comp("R1", vec![pin("1", "~", short, 32)]),
                comp("R2", vec![pin("1", "~", p(100, 140), 32)]),
            ],
            wires: vec![Wire { pts: vec![p(100, 100), p(100, 140)], uuid: "w".into(), ..Default::default() }],
            ..SchDoc::default()
        };
        let mut d = SheetDiagnostics::default();
        let n = crate::netlist::merge_frags(fragments(&sch, "/", &CompileOptions::board_project(), None, &mut d));
        assert_eq!(n.len(), 1, "one net, not a wire net plus an orphan pin");
        assert_eq!(n[0].terminals.len(), 2);
        assert_eq!(d.pins_joined_by_hot_spot, 1);
    }

    /// The snap radius never overrides an exact match, and never guesses. A pin
    /// already on a wire is left alone, and two wire ends equally near leave the
    /// pin unconnected rather than picking one of two nets.
    #[test]
    fn the_snap_radius_neither_overrides_nor_guesses() {
        let sch = SchDoc {
            sheet: sch::SheetProps { hot_spot_grid: 4 * UNIT, ..Default::default() },
            components: vec![
                comp("R1", vec![pin("1", "~", p(100, 100), 32)]),
                comp("R2", vec![pin("1", "~", p(200, 100), 32)]),
            ],
            // R1 sits on its own wire. R2 sits exactly between two wire ends.
            wires: vec![
                Wire { pts: vec![p(100, 100), p(100, 140)], uuid: "w1".into(), ..Default::default() },
                Wire { pts: vec![p(199, 100), p(199, 140)], uuid: "w2".into(), ..Default::default() },
                Wire { pts: vec![p(201, 100), p(201, 140)], uuid: "w3".into(), ..Default::default() },
            ],
            ..SchDoc::default()
        };
        let mut d = SheetDiagnostics::default();
        let n = crate::netlist::merge_frags(fragments(&sch, "/", &CompileOptions::board_project(), None, &mut d));
        assert_eq!(d.pins_joined_by_hot_spot, 0, "R1 was already on its wire");
        assert_eq!(d.hot_spot_ambiguous, 1, "R2 had two nets equally near");
        let r2 = n.iter().find(|n| n.terminals.iter().any(|t| t.designator == "R2")).unwrap();
        assert_eq!(r2.terminals.len(), 1, "R2 stays on a net of its own");
    }

    /// A hidden pin sharing a pad with a visible one is a second connection
    /// point on that pad, not a second pad. `TR1` on the 5BR design has five,
    /// and each one became its own `unconnected-(TR1-PadN)` net beside the
    /// connected pad. A hidden pin with a pad of its OWN keeps its terminal: the
    /// pad is unconnected, and the review has to be able to see that.
    #[test]
    fn a_hidden_pin_loses_its_terminal_only_to_a_visible_one() {
        let hid = |n: &str| pin(n, "SENSE", p(500, 500), 32 | super::sch::PIN_HIDDEN_BIT);
        let sch = SchDoc {
            components: vec![comp("U1", vec![hid("1"), hid("9"), pin("1", "~", p(100, 100), 32)])],
            ..SchDoc::default()
        };
        let mut d = SheetDiagnostics::default();
        let n = crate::netlist::merge_frags(fragments(&sch, "/", &CompileOptions::board_project(), None, &mut d));
        assert_eq!(d.hidden_pins_without_net, 1, "only the one sharing pad 1");
        let mut pins: Vec<&str> = n.iter().flat_map(|n| n.terminals.iter()).map(|t| t.pin.as_str()).collect();
        pins.sort_unstable();
        assert_eq!(pins, vec!["1", "9"], "pad 9 stays visible as unconnected");
    }

    /// A bus is not a net. A wire drawn across one, and a pin on that wire, must
    /// not join the bus: on the 10KW gate-driver board one `GND_X_[1...12]` bus
    /// crossed the sheet and swallowed 96 terminals where the reference has
    /// twelve nets, one per bus member.
    #[test]
    fn a_bus_does_not_swallow_what_is_drawn_across_it() {
        let sch = SchDoc {
            components: vec![
                comp("R1", vec![pin("1", "~", p(100, 100), 32)]),
                comp("R2", vec![pin("1", "~", p(300, 100), 32)]),
            ],
            // One horizontal bus, and two separate vertical wires crossing it.
            buses: vec![Wire { pts: vec![p(0, 100), p(400, 100)], uuid: "b".into(), ..Default::default() }],
            wires: vec![
                Wire { pts: vec![p(100, 100), p(100, 200)], uuid: "w1".into(), ..Default::default() },
                Wire { pts: vec![p(300, 100), p(300, 200)], uuid: "w2".into(), ..Default::default() },
            ],
            net_labels: vec![
                NetLabel { at: p(100, 200), text: "A".into(), uuid: "l1".into(), ..Default::default() },
                NetLabel { at: p(300, 200), text: "B".into(), uuid: "l2".into(), ..Default::default() },
            ],
            ..SchDoc::default()
        };
        let n = nets(&sch, &CompileOptions::board_project());
        let named: Vec<&str> = n.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(named, vec!["A", "B"], "two nets, not one");
    }

    /// Corner case 19: `VCC` and `Vcc` are one net, reported under one spelling.
    #[test]
    fn net_names_consolidate_case_insensitively() {
        let sch = SchDoc {
            components: vec![
                comp("R1", vec![pin("1", "~", p(100, 100), 32)]),
                comp("R2", vec![pin("1", "~", p(300, 300), 32)]),
                comp("R3", vec![pin("1", "~", p(500, 500), 32)]),
            ],
            power_ports: vec![
                PowerPort { at: p(100, 100), text: "VCC".into(), style: 2, uuid: "a".into(), ..Default::default() },
                PowerPort { at: p(300, 300), text: "Vcc".into(), style: 2, uuid: "b".into(), ..Default::default() },
                PowerPort { at: p(500, 500), text: "VCC".into(), style: 2, uuid: "c".into(), ..Default::default() },
            ],
            ..SchDoc::default()
        };
        let n = nets(&sch, &CompileOptions::board_project());
        assert_eq!(n.len(), 1, "one net, not two");
        assert_eq!(n[0].name, "VCC", "the most-used spelling wins");
        assert_eq!(n[0].terminals.len(), 3);
    }
}
