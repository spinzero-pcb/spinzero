//! Capture connectivity compiled into fragments for the shared merge.
//!
//! Capture stores its own answer for most of this: every wire names a net id,
//! and the page's net table names the id. What it does not store is which pin
//! sits on which wire, so a page is compiled from both:
//!
//! - every wire end, pin connection point, power-symbol and connector hot point
//!   is a node at its exact coordinate (database units, no tolerance);
//! - a wire joins its two ends and every node lying on it — a wire ended on
//!   another wire's middle is a T, and Capture draws the dot for it;
//! - wires sharing a stored net id are one net, which is how Capture joins the
//!   pieces of one aliased net on a page;
//! - a power symbol joins every node at its hot point to the global net it
//!   names; an off-page connector does the same within its folder instance;
//!   a port meets the block pin of the same name on the parent page;
//! - a hidden power pin has no node: it joins the global net its name names.
//!
//! Names follow Capture's own: the page net table's name for a stored net, the
//! occurrence tree's generated `N<id>` names for unnamed ones.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use eda_parse_orcad::capture::page::{Graphic, Page};
use eda_parse_orcad::capture::symbol::PinType;
use eda_parse_orcad::capture::CaptureDoc;

use super::design::{part_id, resolve};
use super::{SheetInstance, Unresolved};
use crate::netlist::{Frag, Graphical, Terminal};

/// Naming precedence, as Capture's netlister resolves it. The level comes
/// first: a net crossing the hierarchy is named from the level above (a port's
/// net takes the parent's name, even a generated one). Within one level: a
/// power symbol, then an off-page connector, then a user net name, then a
/// port, then the name Capture generated. Power names are global and rank as
/// the top level wherever they are drawn.
mod rank {
    pub const POWER: u8 = 6;
    pub const OFFPAGE: u8 = 5;
    pub const USER: u8 = 4;
    pub const PORT: u8 = 3;
    pub const GENERATED: u8 = 2;
}

/// `kind` ranked at hierarchy `depth`; the low bit is left free for a
/// preferred name within the same kind.
fn rank_at(kind: u8, depth: usize) -> u8 {
    (15 - depth.min(15) as u8) * 16 + kind * 2
}

/// Whether a user net name (an alias, or a named stored net) joins the nets of
/// that name on the other pages of the same folder instance. Capture's
/// netlister does; the corpus netlists agree (see the implementation notes).
const NAMES_SPAN_FOLDER: bool = true;

type P = (i32, i32);

#[derive(Default)]
struct Uf {
    parent: Vec<usize>,
}

impl Uf {
    fn add(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
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

#[derive(Default)]
struct Graph {
    uf: Uf,
    at: HashMap<P, usize>,
    netid: HashMap<u32, usize>,
}

impl Graph {
    fn node(&mut self, p: P) -> usize {
        if let Some(&i) = self.at.get(&p) {
            return i;
        }
        let i = self.uf.add();
        self.at.insert(p, i);
        i
    }
    fn net(&mut self, id: u32) -> usize {
        if let Some(&i) = self.netid.get(&id) {
            return i;
        }
        let i = self.uf.add();
        self.netid.insert(id, i);
        i
    }
}

/// True when `p` lies on the closed segment a–b.
fn on_segment(p: P, a: P, b: P) -> bool {
    let (px, py) = (p.0 as i64, p.1 as i64);
    let (ax, ay, bx, by) = (a.0 as i64, a.1 as i64, b.0 as i64, b.1 as i64);
    if px < ax.min(bx) || px > ax.max(bx) || py < ay.min(by) || py > ay.max(by) {
        return false;
    }
    (bx - ax) * (py - ay) - (by - ay) * (px - ax) == 0
}

/// Connection points of a placed power symbol, off-page connector or port:
/// its cached symbol's pin hot points taken through the placement, or the
/// placement point when the symbol draws no pin.
pub fn hot_points(doc: &CaptureDoc, g: &Graphic) -> Vec<P> {
    let pins = g
        .body
        .as_ref()
        .filter(|b| !b.pins.is_empty())
        .or_else(|| doc.cache.symbol(&g.cache_name))
        .map(|s| {
            let b = (s.bbox.0 as i32, s.bbox.1 as i32, s.bbox.2 as i32, s.bbox.3 as i32);
            s.pins.iter().map(|p| g.orient.place(p.hot, g.origin(), b)).collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if pins.is_empty() {
        vec![g.pos]
    } else {
        pins
    }
}

/// What a node-set carries before it becomes a fragment.
#[derive(Default)]
struct Bucket {
    terminals: Vec<Terminal>,
    graphical: Graphical,
    keys: BTreeSet<String>,
    /// (rank, name, driver kind)
    names: Vec<(u8, String, String)>,
    has_wire: bool,
    no_connect_only: bool,
    /// Names of visible power-type pins in the bucket.
    power_pins: Vec<String>,
    /// (part id, zero-based slot) of every pin, for Capture's name for a net
    /// no wire carries.
    pin_ids: Vec<(u32, usize)>,
}

/// Key a name for merging. Capture's netlist is case-insensitive.
fn up(s: &str) -> String {
    s.trim().to_uppercase()
}

/// `N12345` → 12345: a name Capture generated from an object id.
fn generated_id(name: &str) -> Option<u32> {
    let d = name.strip_prefix('N')?;
    let digits: String = d.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() != d.len() {
        return None;
    }
    digits.parse().ok()
}

/// Separates a local name from its folder-instance suffix until the merge is
/// done (a unit separator cannot occur in a Capture name).
const SUFFIX_MARK: char = '\u{1F}';

/// Resolve local names after the merge, the way Capture's netlister does: a
/// net named inside a child folder instance keeps its bare name unless another
/// net in the design has the same bare name, in which case it takes `_` and
/// the block name (`N00439_MV1`, `LED_BLUE1_LED MODULE`). Nets of the root
/// folder never take a suffix.
pub fn finalize_names(nets: &mut [crate::netlist::Net]) {
    let base = |n: &str| n.split(SUFFIX_MARK).next().unwrap_or(n).to_string();
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in nets.iter() {
        *count.entry(base(&n.name)).or_default() += 1;
    }
    for n in nets.iter_mut() {
        if let Some((b, sfx)) = n.name.split_once(SUFFIX_MARK) {
            n.name = if count.get(b).copied().unwrap_or(0) > 1 { format!("{b}{sfx}") } else { b.to_string() };
        }
    }
    nets.sort_by_cached_key(crate::netlist::net_order_key);
}

/// Every name the design uses as a global supply: power symbols and power
/// pins. A stored net carrying one of these names joins that global net.
pub fn power_names(doc: &CaptureDoc) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for f in &doc.folders {
        for p in &f.pages {
            for g in &p.globals {
                if !g.name.trim().is_empty() {
                    out.insert(up(&g.name));
                }
            }
        }
    }
    for revs in doc.cache.symbols.values() {
        for s in revs {
            for p in &s.pins {
                if p.etype == PinType::Power && !p.name.trim().is_empty() && s.kind == 24 {
                    out.insert(up(&p.name));
                }
            }
        }
    }
    out
}

pub fn is_generated(name: &str) -> bool {
    generated_id(name).is_some()
}

/// Compile one sheet instance into fragments.
pub fn fragments(
    doc: &CaptureDoc,
    inst: &SheetInstance,
    power_names: &BTreeSet<String>,
    diag: &mut Unresolved,
) -> Vec<Frag> {
    let page: &Page = &doc.folders[inst.folder].pages[inst.page];
    let mut g = Graph::default();
    let scope_key = &inst.folder_key;

    // Node attachments, resolved to buckets after the union.
    enum Att {
        Terminal(Terminal, String, bool),
        PinId(u32, usize),
        PowerPinName(String),
        Wire(String),
        Alias(String, String),
        Power(String, String),
        Offpage(String, String),
        Port(String, String),
        BlockPin(String, String, String),
        NetName(u32),
    }
    let mut atts: Vec<(usize, Att)> = Vec::new();

    // Wires. Buses are recorded, not expanded.
    let mut segs: Vec<(P, P, usize)> = Vec::new();
    for w in &page.wires {
        if w.bus {
            diag.buses_not_expanded += 1;
            continue;
        }
        let a = g.node(w.a);
        let b = g.node(w.b);
        g.uf.union(a, b);
        let n = g.net(w.net_id);
        g.uf.union(a, n);
        segs.push((w.a, w.b, a));
        atts.push((a, Att::Wire(part_id(w.db_id))));
        atts.push((n, Att::NetName(w.net_id)));
        for (i, al) in w.aliases.iter().enumerate() {
            atts.push((a, Att::Alias(al.name.clone(), format!("{}:a{i}", part_id(w.db_id)))));
        }
    }

    let page_net_ids: BTreeSet<u32> = page.nets.iter().map(|n| n.id).chain(page.net_names.iter().map(|n| n.id)).collect();

    // Parts.
    for part in &page.parts {
        let r = resolve(doc, inst, part);
        for pin in &part.pins {
            let slot = pin.slot();
            let sp = r.symbol.and_then(|s| s.pins.iter().find(|p| p.slot == slot));
            let number = r.pin_number(slot).unwrap_or_else(|| (slot + 1).to_string());
            let t = Terminal {
                designator: r.designator.clone(),
                pin: number,
                pin_name: sp.map(|p| p.name.clone()).unwrap_or_default(),
                pin_type: sp.map(|p| p.etype.token()).unwrap_or("PASSIVE").to_string(),
            };
            // A hidden pin is connected but not drawn, so it has no handle on
            // the sheet for the net to point at.
            let uuid = if sp.map(|p| p.hidden()).unwrap_or(false) { String::new() } else { format!("{}:{slot}", part_id(part.db_id)) };
            let hidden_power = sp.map(|p| p.hidden() && p.etype == PinType::Power).unwrap_or(false);
            if hidden_power {
                // Joins the global net its name names, wherever it is drawn —
                // and the page net Capture stored for it, which is how a
                // hidden VCC pin lands on a net Capture also calls +5V.
                let n = if pin.word_b != 0 && page_net_ids.contains(&pin.word_b) {
                    g.net(pin.word_b)
                } else {
                    g.uf.add()
                };
                diag.hidden_supply_pins += 1;
                atts.push((n, Att::Terminal(t, uuid, false)));
                let name = sp.map(|p| p.name.clone()).unwrap_or_default();
                atts.push((n, Att::Power(name, String::new())));
                continue;
            }
            if pin.no_connect() {
                diag.pins_marked_no_connect += 1;
            }
            let n = g.node(pin.pos);
            atts.push((n, Att::Terminal(t, uuid, false)));
            atts.push((n, Att::PinId(part.db_id, slot)));
            if let Some(p) = sp.filter(|p| p.etype == PinType::Power && !p.name.trim().is_empty()) {
                atts.push((n, Att::PowerPinName(p.name.clone())));
            }
        }
    }

    // Power symbols, off-page connectors and ports.
    for gl in &page.globals {
        for p in hot_points(doc, gl) {
            let n = g.node(p);
            atts.push((n, Att::Power(gl.name.clone(), part_id(gl.db_id))));
        }
    }
    for op in &page.offpages {
        for p in hot_points(doc, op) {
            let n = g.node(p);
            atts.push((n, Att::Offpage(op.name.clone(), part_id(op.db_id))));
        }
    }
    for port in &page.ports {
        for p in hot_points(doc, port) {
            let n = g.node(p);
            atts.push((n, Att::Port(port.name.clone(), part_id(port.db_id))));
        }
    }
    for b in &page.blocks {
        let child = format!("hier:{}/{}", inst.folder_key, b.db_id);
        for bp in &b.pins {
            if bp.bus || bp.no_connect {
                continue;
            }
            let n = g.node(bp.pos);
            atts.push((n, Att::BlockPin(bp.name.clone(), format!("{}:{}", part_id(b.db_id), bp.name), child.clone())));
        }
    }

    // T-joins: a node lying on a wire belongs to it. Axis-aligned wires are
    // bucketed by their fixed coordinate; the rare diagonal ones are scanned.
    let mut horiz: HashMap<i32, Vec<(i32, i32, usize)>> = HashMap::new();
    let mut vert: HashMap<i32, Vec<(i32, i32, usize)>> = HashMap::new();
    let mut diag_segs: Vec<(P, P, usize)> = Vec::new();
    for &(a, b, n) in &segs {
        if a.1 == b.1 {
            horiz.entry(a.1).or_default().push((a.0.min(b.0), a.0.max(b.0), n));
        } else if a.0 == b.0 {
            vert.entry(a.0).or_default().push((a.1.min(b.1), a.1.max(b.1), n));
        } else {
            diag_segs.push((a, b, n));
        }
    }
    let points: Vec<(P, usize)> = g.at.iter().map(|(&p, &i)| (p, i)).collect();
    for (p, i) in points {
        if let Some(v) = horiz.get(&p.1) {
            for &(x0, x1, n) in v {
                if p.0 >= x0 && p.0 <= x1 {
                    g.uf.union(i, n);
                }
            }
        }
        if let Some(v) = vert.get(&p.0) {
            for &(y0, y1, n) in v {
                if p.1 >= y0 && p.1 <= y1 {
                    g.uf.union(i, n);
                }
            }
        }
        for &(a, b, n) in &diag_segs {
            if on_segment(p, a, b) {
                g.uf.union(i, n);
            }
        }
    }

    // Stored net ids that the geometry joined: Capture would have given them
    // one id. Counted, geometry wins.
    {
        let mut roots: HashMap<usize, usize> = HashMap::new();
        let ids: Vec<usize> = g.netid.values().copied().collect();
        for n in ids {
            *roots.entry(g.uf.find(n)).or_default() += 1;
        }
        diag.stored_nets_joined_by_geometry += roots.values().filter(|&&c| c > 1).map(|c| c - 1).sum::<usize>();
    }

    // Occurrence-scoped generated names (N<wire id>) by wire id.
    let mut generated: HashMap<u32, String> = HashMap::new();
    if let Some(s) = &inst.scope {
        for n in &s.nets {
            if let Some(id) = generated_id(&n.name) {
                generated.insert(id, n.name.clone());
            }
        }
    }
    let wire_net: HashMap<u32, u32> = page.wires.iter().map(|w| (w.db_id, w.net_id)).collect();
    let mut net_generated: HashMap<u32, String> = HashMap::new();
    for (wid, name) in &generated {
        if let Some(&nid) = wire_net.get(wid) {
            net_generated.entry(nid).or_insert_with(|| name.clone());
        }
    }
    // The page's own names for each stored net, in table order. Capture lists
    // an id once per name it carries; the last is the net's name and the
    // others name the same net (a VCC hidden pin and a +5V symbol shorted).
    let mut net_names: HashMap<u32, Vec<String>> = HashMap::new();
    for n in &page.net_names {
        net_names.entry(n.id).or_default().push(n.name.clone());
    }

    // A name local to a child folder instance is tagged with that instance's
    // suffix; [`finalize_names`] keeps the suffix only where the bare name
    // would collide.
    let local = |n: &str| {
        if inst.suffix.is_empty() {
            up(n)
        } else {
            format!("{}{SUFFIX_MARK}{}", up(n), up(&inst.suffix))
        }
    };
    let page_power: BTreeSet<String> = page.globals.iter().map(|g| up(&g.name)).collect();
    let page_offpage: BTreeSet<String> = page.offpages.iter().map(|g| up(&g.name)).collect();
    let page_ports: BTreeSet<String> = page.ports.iter().map(|g| up(&g.name)).collect();
    let mut buckets: BTreeMap<usize, Bucket> = BTreeMap::new();
    let folder_scope = if NAMES_SPAN_FOLDER { scope_key.clone() } else { inst.info.sheet_path_uuids.clone() };
    for (n, att) in atts {
        let root = g.uf.find(n);
        let b = buckets.entry(root).or_default();
        match att {
            Att::Terminal(t, uuid, nc) => {
                b.terminals.push(t);
                if !uuid.is_empty() {
                    b.graphical.pins.push(uuid);
                }
                if nc {
                    b.no_connect_only = true;
                }
            }
            Att::PowerPinName(n) => b.power_pins.push(n),
            Att::PinId(id, slot) => b.pin_ids.push((id, slot)),
            Att::Wire(u) => {
                b.has_wire = true;
                b.graphical.wires.push(u);
            }
            Att::Alias(name, u) => {
                b.graphical.labels.push(u);
                if !name.trim().is_empty() {
                    b.keys.insert(format!("name:{folder_scope}:{}", up(&name)));
                }
            }
            Att::Power(name, u) => {
                if !u.is_empty() {
                    b.graphical.power_ports.push(u);
                }
                if !name.trim().is_empty() {
                    b.keys.insert(format!("pwr:{}", up(&name)));
                    b.names.push((rank_at(rank::POWER, 0), up(&name), "global_power_pin".into()));
                }
            }
            Att::Offpage(name, u) => {
                b.graphical.labels.push(u);
                if !name.trim().is_empty() {
                    b.keys.insert(format!("off:{scope_key}:{}", up(&name)));
                    // An off-page connector also meets a net alias of its name
                    // in the same folder (measured: CutiePi's LOL / LOR, one net
                    // on the board built from Capture's netlist).
                    b.keys.insert(format!("name:{folder_scope}:{}", up(&name)));
                    b.names.push((rank_at(rank::OFFPAGE, inst.depth), local(&name), "global_label".into()));
                }
            }
            Att::Port(name, u) => {
                b.graphical.ports.push(u);
                if name.trim().is_empty() {
                    continue;
                }
                match &inst.hier_key {
                    Some(k) => {
                        b.keys.insert(format!("{k}:{}", up(&name)));
                    }
                    None => {
                        b.keys.insert(format!("off:{scope_key}:{}", up(&name)));
                    }
                }
                b.names.push((rank_at(rank::PORT, inst.depth), local(&name), "hier_label".into()));
            }
            Att::BlockPin(name, u, child) => {
                b.graphical.sheet_entries.push(u);
                if !name.trim().is_empty() {
                    b.keys.insert(format!("{child}:{}", up(&name)));
                }
            }
            Att::NetName(id) => {
                if let Some(names) = net_names.get(&id) {
                    for (i, name) in names.iter().enumerate() {
                        // The first listed name is the net's own; the others
                        // name the same net (measured: LSB0 over LSB1..LSB3).
                        let pref = u8::from(i == 0);
                        if is_generated(name) {
                            b.names.push((rank_at(rank::GENERATED, inst.depth) + pref, local(name), "pin".into()));
                            continue;
                        }
                        // The table also carries the names of the ports,
                        // connectors and power symbols on the net; such a name
                        // ranks as what it names, so a port's name does not
                        // outrank the level above it.
                        let u = up(name);
                        let (kind, driver) = if page_power.contains(&u) {
                            (rank::POWER, "global_power_pin")
                        } else if page_offpage.contains(&u) {
                            (rank::OFFPAGE, "global_label")
                        } else if page_ports.contains(&u) {
                            (rank::PORT, "hier_label")
                        } else {
                            (rank::USER, "local_label")
                        };
                        if power_names.contains(&u) {
                            b.keys.insert(format!("pwr:{u}"));
                        } else if kind != rank::PORT {
                            b.keys.insert(format!("name:{folder_scope}:{u}"));
                        }
                        let depth = if kind == rank::POWER { 0 } else { inst.depth };
                        let shown = if kind == rank::POWER { u } else { local(name) };
                        b.names.push((rank_at(kind, depth) + pref, shown, driver.into()));
                    }
                } else if let Some(name) = net_generated.get(&id) {
                    b.names.push((rank_at(rank::GENERATED, inst.depth), local(name), "pin".into()));
                }
            }
        }
    }

    let mut out = Vec::new();
    for (_, mut b) in buckets {
        // A visible power pin left otherwise unconnected joins the net its
        // name names, as a hidden one always does (measured against Capture's
        // netlist: unwired NC1/GND power pins land on the named net, wired
        // ones keep the net they are wired to).
        if b.terminals.len() == 1 && !b.has_wire && b.keys.is_empty() {
            if let Some(name) = b.power_pins.first().cloned() {
                diag.hidden_supply_pins += 1;
                b.keys.insert(format!("pwr:{}", up(&name)));
                b.names.push((rank_at(rank::POWER, 0), up(&name), "global_power_pin".into()));
            }
        }
        if b.terminals.is_empty() && b.keys.is_empty() {
            continue;
        }
        if b.terminals.len() == 1 && !b.has_wire && b.keys.is_empty() && !b.no_connect_only {
            diag.unconnected_pins += 1;
        }
        // A net no wire carries has no stored name; Capture names it after its
        // lowest-numbered part: `N`, the part id padded to five digits, and the
        // pin's zero-based slot.
        if !b.has_wire {
            if let Some(&(id, slot)) = b.pin_ids.iter().min() {
                let name = format!("N{id:05}{slot}");
                b.names.push((rank_at(rank::GENERATED, inst.depth), local(&name), "pin".into()));
            }
        }
        // The strongest name; ties broken by the name itself for determinism.
        b.names.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
        let (rank, name, driver) = b.names.into_iter().next().unwrap_or((0, String::new(), "pin".into()));
        let name = if name.is_empty() { fallback_name(&b.terminals) } else { name };
        b.graphical.normalize();
        out.push(Frag {
            terminals: b.terminals,
            graphical: b.graphical,
            keys: b.keys.into_iter().collect(),
            name,
            driver_kind: driver,
            rank,
            classes: Vec::new(),
            sheet: inst.info.sheet_path_uuids.clone(),
        });
    }
    out
}

/// A name for a net Capture's file does not name: Capture's own convention for
/// a wireless net, `N` plus the part id padded to five digits plus the pin.
fn fallback_name(terminals: &[Terminal]) -> String {
    let mut t: Vec<&Terminal> = terminals.iter().collect();
    t.sort_by(|a, b| (&a.designator, &a.pin).cmp(&(&b.designator, &b.pin)));
    match t.first() {
        Some(t) => format!("Net-({}-Pad{})", t.designator, t.pin),
        None => String::new(),
    }
}
