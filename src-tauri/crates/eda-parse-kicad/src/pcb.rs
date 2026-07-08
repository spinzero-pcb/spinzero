//! Faithful in-memory model of a `.kicad_pcb` board file.
//!
//! Parses the parts the review bundle needs to draw a board: the layer stackup,
//! the net table, copper (tracks, vias, zone fills), pads, board- and
//! footprint-level graphics, footprint text, the 3D model references, and the
//! `Edge.Cuts` outline. Geometry is kept in board coordinates (millimetres,
//! Y-down) so every per-layer SVG the renderer emits shares one coordinate space
//! and the layers register when stacked. Unknown constructs are ignored.

use crate::schematic::{At, Pt};
use crate::sexpr::{self, Node, ParseError};

/// One entry of the board's `(layers …)` stackup.
#[derive(Debug, Clone, PartialEq)]
pub struct PcbLayer {
    pub ordinal: i64,
    pub name: String,
    /// Layer class token, e.g. `signal`, `power`, `user`.
    pub kind: String,
    /// Optional user-visible name (the 4th token), e.g. `(43 "User.3" user
    /// "Mechanical_Drawing")` — the display label the designer chose for a
    /// renamed/user layer. `None` when the layer keeps its canonical name.
    pub user_name: Option<String>,
}

/// A 3D model reference attached to a footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Model3D {
    pub path: String,
    pub offset: (f64, f64, f64),
    pub scale: (f64, f64, f64),
    pub rotate: (f64, f64, f64),
}

/// A primitive graphic shape, in whatever coordinate frame its owner uses
/// (board frame for `gr_*`, footprint-local frame for `fp_*`).
#[derive(Debug, Clone, PartialEq)]
pub enum PcbShape {
    Seg { a: Pt, b: Pt },
    Arc { start: Pt, mid: Pt, end: Pt },
    Rect { a: Pt, b: Pt, filled: bool },
    Circle { center: Pt, radius: f64, filled: bool },
    Poly { pts: Vec<Pt>, filled: bool },
}

/// A board- or footprint-level graphic on a single layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Graphic {
    pub layer: String,
    pub width: f64,
    pub shape: PcbShape,
    pub uuid: String,
}

/// Text justification (KiCad `(effects (justify …))`). Each axis is `-1`/`0`/`+1`
/// for left|top / center / right|bottom; the default (no `justify` token) is
/// centred on both axes, matching KiCad. `mirror` is parsed but left for the
/// renderer to apply (back layers are mirrored at the layer level).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextJustify {
    /// `-1` left, `0` centre, `+1` right.
    pub h: i8,
    /// `-1` top, `0` centre, `+1` bottom.
    pub v: i8,
    pub mirror: bool,
}

/// A text item (footprint reference/value/user text, or board text).
#[derive(Debug, Clone, PartialEq)]
pub struct PcbText {
    /// `reference` / `value` / `user` for footprint text; `gr_text` for board text.
    pub kind: String,
    pub text: String,
    pub at: At,
    pub layer: String,
    /// Glyph height (mm) — the first value of `(effects (font (size h w)))`.
    pub size: f64,
    /// Glyph width (mm) — the second `(size h w)` value. KiCad authors condensed
    /// (`w < h`) or expanded (`w > h`) text; equals `size` for a square glyph cell.
    pub width: f64,
    /// Stroke (pen) thickness in mm from `(font (thickness t))`, when specified. KiCad
    /// draws the stroke font at this pen width; the viewer uses it to match the weight.
    pub thickness: Option<f64>,
    /// `(effects (font … bold))` / `(bold yes)` — KiCad renders the stroke font
    /// heavier; the viewer matches it with a bold (faux-bold) Newstroke face.
    pub bold: bool,
    /// `(effects (font … italic))` / `(italic yes)` — KiCad shears the stroke
    /// glyphs by its 1/8 tilt; the viewer applies the same shear.
    pub italic: bool,
    /// `(layer "…" knockout)` — inverted silkscreen text: a filled background in the
    /// layer colour with the glyphs cut out to the board. The renderer fills the
    /// layer colour and knocks the glyphs through.
    pub knockout: bool,
    /// `(hide yes)` — KiCad does not plot this text; the renderer must skip it.
    pub hidden: bool,
    /// `(effects (justify …))` — anchor side the text grows from. Drives the SVG
    /// `text-anchor` / baseline and multi-line stacking direction.
    pub justify: TextJustify,
    /// Custom outline-font family from `(effects (font (face "Name")))` — e.g. Calibri.
    /// `None` ⇒ KiCad's built-in stroke font (Newstroke); the viewer only overrides the
    /// stroke font when a face is named, so the board shows the chosen typeface.
    pub font: Option<String>,
    pub uuid: String,
}

/// A copper track segment or arc.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub start: Pt,
    pub end: Pt,
    /// `Some(mid)` for an arc track; `None` for a straight segment.
    pub mid: Option<Pt>,
    pub width: f64,
    pub layer: String,
    /// Net code (KiCad ≤9, resolved against the board net table); `0` on KiCad 10,
    /// which dropped numeric codes and writes the name on the object directly.
    pub net: i64,
    /// Net name when the object carries it (KiCad 10); empty on KiCad ≤9, where the
    /// name comes from the board net table via `net`. See `read_net`.
    pub net_name: String,
    pub uuid: String,
}

/// A via.
#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    pub at: Pt,
    pub size: f64,
    pub drill: f64,
    /// Copper layers the via spans (`F.Cu`..`B.Cu` for a through via).
    pub layers: Vec<String>,
    /// KiCad `(remove_unused_layers yes)`: drop the annular ring on copper layers the via
    /// doesn't connect to. The renderer keeps the ring only on connected layers.
    pub remove_unused_layers: bool,
    /// Net code (KiCad ≤9); `0` on KiCad 10 (see `Track::net`).
    pub net: i64,
    /// Net name when carried on the object (KiCad 10); empty on KiCad ≤9 (see `read_net`).
    pub net_name: String,
    pub uuid: String,
}

/// One polygon of a `(zone)` on a single layer. A filled copper pour yields one
/// `Zone` per `(filled_polygon)` (`filled = true`); a keepout or not-yet-filled
/// zone yields one `Zone` from its `(polygon)` outline (`filled = false`).
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub net: i64,
    pub net_name: String,
    pub layer: String,
    pub pts: Vec<Pt>,
    /// True for a solid `filled_polygon`; false for an outline-only polygon
    /// (keepout / unfilled), which the renderer draws as a boundary, not copper.
    pub filled: bool,
    /// True when the zone carries a `(keepout …)` rule block.
    pub keepout: bool,
    pub uuid: String,
}

/// A footprint pad, in footprint-local coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub number: String,
    /// `smd` / `thru_hole` / `np_thru_hole` / `connect`.
    pub kind: String,
    /// `circle` / `rect` / `roundrect` / `oval` / `trapezoid` / `custom`.
    pub shape: String,
    /// Local position + orientation (absolute pad angle, includes the footprint's).
    pub at: At,
    pub size: (f64, f64),
    /// Drill diameter (0.0 for an SMD pad). For an oval/slot drill this is the first
    /// (`(drill oval W H)`) dimension W.
    pub drill: f64,
    /// Second drill dimension H for an oval/slot drill `(drill oval W H)`; 0.0 for a
    /// round drill. When set, the hole is a stadium of `drill`×`drill_h`.
    pub drill_h: f64,
    /// Corner-radius ratio for a `roundrect` pad (radius = ratio · min(w,h));
    /// 0.0 for other shapes.
    pub roundrect_rratio: f64,
    /// Per-pad solder-mask expansion in mm, when the pad overrides the board
    /// default (`(solder_mask_margin …)`); `None` to use the board default.
    pub mask_margin: Option<f64>,
    /// Layer globs the pad lives on, e.g. `*.Cu`, `F.Cu`, `*.Mask`.
    pub layers: Vec<String>,
    pub net: i64,
    pub net_name: String,
    pub uuid: String,
}

/// A placed footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    pub lib_id: String,
    pub reference: String,
    /// Board layer the footprint sits on (`F.Cu` / `B.Cu`).
    pub layer: String,
    pub at: At,
    pub uuid: String,
    /// `(attr … dnp)` — "do not populate"; the renderer marks it on the fab layer.
    pub dnp: bool,
    /// Graphic/courtyard extent transformed into board coordinates, if any.
    pub bbox: Option<(Pt, Pt)>,
    pub models: Vec<Model3D>,
    pub pads: Vec<Pad>,
    /// Footprint-local graphics (`fp_line`/`fp_rect`/`fp_circle`/`fp_arc`/`fp_poly`).
    pub graphics: Vec<Graphic>,
    /// Footprint-local text (reference / value / user).
    pub texts: Vec<PcbText>,
}

/// The parsed board.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcb {
    pub version: Option<i64>,
    pub layers: Vec<PcbLayer>,
    pub footprints: Vec<Footprint>,
    /// `Edge.Cuts` line segments (arcs/rects flattened to their endpoints).
    pub edges: Vec<(Pt, Pt)>,
    /// Net code -> net name, from the top-level `(net …)` table.
    pub nets: Vec<(i64, String)>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub zones: Vec<Zone>,
    /// Board-level graphics (`gr_*`), in board coordinates.
    pub graphics: Vec<Graphic>,
    /// Board-level text (`gr_text`), in board coordinates.
    pub texts: Vec<PcbText>,
    /// `(paper "A3")` — drawing-sheet size token, for the worksheet frame.
    pub paper: Option<String>,
    /// Explicit page dimensions `(w, h)` in mm for a `(paper "User" w h)` size.
    pub paper_dims: Option<(f64, f64)>,
    /// `(generator_version …)`, shown as "KiCad E.D.A. <v>" in the title block.
    pub generator_version: Option<String>,
    /// `(title_block …)` fields (each `None`/empty when absent).
    pub title: Option<String>,
    pub company: Option<String>,
    pub rev: Option<String>,
    pub date: Option<String>,
    /// Title-block comment lines (`(comment N "…")`), in order.
    pub comments: Vec<String>,
}

impl Pcb {
    /// Parse board source text.
    pub fn parse_str(src: &str) -> Result<Pcb, ParseError> {
        let root = sexpr::parse(src)?;
        // Reject a non-board root (a renamed `.kicad_sym` / netlist / `.kicad_wks`)
        // so a wrong file surfaces as an error instead of a silently blank board.
        if root.tag() != Some("kicad_pcb") {
            return Err(ParseError::WrongRoot {
                expected: "kicad_pcb",
                found: root.tag().map(str::to_string),
            });
        }
        Ok(Pcb::from_root(&root))
    }

    /// Board outline extent `(min, max)`, from the edge segments.
    pub fn outline_bbox(&self) -> Option<(Pt, Pt)> {
        let pts: Vec<Pt> = self.edges.iter().flat_map(|(a, b)| [*a, *b]).collect();
        bbox_of(&pts)
    }

    /// Net name for a net code (empty string for code 0 / unknown).
    pub fn net_name(&self, code: i64) -> &str {
        self.nets
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, n)| n.as_str())
            .unwrap_or("")
    }

    fn from_root(root: &Node) -> Pcb {
        let mut pcb = Pcb {
            version: root.field("version").and_then(|s| s.parse().ok()),
            layers: Vec::new(),
            footprints: Vec::new(),
            edges: Vec::new(),
            nets: Vec::new(),
            tracks: Vec::new(),
            vias: Vec::new(),
            zones: Vec::new(),
            graphics: Vec::new(),
            texts: Vec::new(),
            paper: root.field("paper").map(str::to_string),
            // `(paper "User" w h)` carries explicit dimensions after the token.
            paper_dims: root.child("paper").and_then(|p| p.pair_at(2)),
            generator_version: root.field("generator_version").map(str::to_string),
            title: root.child("title_block").and_then(|t| t.field("title")).map(str::to_string),
            company: root.child("title_block").and_then(|t| t.field("company")).map(str::to_string),
            rev: root.child("title_block").and_then(|t| t.field("rev")).map(str::to_string),
            date: root.child("title_block").and_then(|t| t.field("date")).map(str::to_string),
            comments: root.child("title_block").map(parse_comments).unwrap_or_default(),
        };

        if let Some(layers) = root.child("layers") {
            for l in layers.as_list().into_iter().flatten() {
                // `(0 "F.Cu" signal)` — ordinal, name, class.
                if let (Some(ordinal), Some(name)) =
                    (l.nth(0).and_then(Node::as_i64), l.nth(1).and_then(Node::as_str))
                {
                    pcb.layers.push(PcbLayer {
                        ordinal,
                        name: name.to_string(),
                        kind: l.nth(2).and_then(Node::as_str).unwrap_or("").to_string(),
                        user_name: l.nth(3).and_then(Node::as_str).map(String::from),
                    });
                }
            }
        }

        for n in root.as_list().into_iter().flatten() {
            match n.tag() {
                Some("net") => {
                    // `(net 1 "/ALLPST")`
                    if let Some(code) = n.nth(1).and_then(Node::as_i64) {
                        pcb.nets
                            .push((code, n.nth(2).and_then(Node::as_str).unwrap_or("").to_string()));
                    }
                }
                Some("footprint") => pcb.footprints.push(parse_footprint(n)),
                Some("segment") => {
                    if let Some(t) = parse_track(n, false) {
                        if t.layer == "Edge.Cuts" {
                            pcb.edges.push((t.start, t.end));
                        }
                        pcb.tracks.push(t);
                    }
                }
                Some("arc") => {
                    if let Some(t) = parse_track(n, true) {
                        // An `(arc)` mis-saved on Edge.Cuts must count toward the outline
                        // just as a `(segment)` does, else it is silently absent from the
                        // board extent.
                        if t.layer == "Edge.Cuts" {
                            if let Some(mid) = t.mid {
                                push_arc_edges(t.start, mid, t.end, &mut pcb.edges);
                            } else {
                                pcb.edges.push((t.start, t.end));
                            }
                        }
                        pcb.tracks.push(t);
                    }
                }
                Some("via") => {
                    if let Some(v) = parse_via(n) {
                        pcb.vias.push(v);
                    }
                }
                Some("zone") => parse_zone(n, &mut pcb.zones),
                Some("gr_line") | Some("gr_rect") | Some("gr_circle") | Some("gr_arc")
                | Some("gr_poly") | Some("gr_curve") => {
                    if let Some(g) = parse_graphic(n) {
                        if g.layer == "Edge.Cuts" {
                            push_edge_segments(&g, &mut pcb.edges);
                        }
                        pcb.graphics.push(g);
                    }
                }
                Some("gr_text") => {
                    if let Some(t) = parse_text(n, "gr_text", 1) {
                        pcb.texts.push(t);
                    }
                }
                Some("dimension") => parse_dimension(n, &mut pcb.graphics, &mut pcb.texts),
                _ => {}
            }
        }

        // Board-level text variables: KiCad 8+ mirrors the project's `text_variables`
        // into the .kicad_pcb as top-level `(property "KEY" "VALUE")` pairs, so the board
        // self-resolves when plotted. Expand `${KEY}` across all text + the title block —
        // e.g. a silkscreen `${PCB_PART_NUMBER}` reads "EX-0000035-00". (Footprint
        // `(property …)` are nested inside footprint nodes, so this only sees board ones.)
        let vars: Vec<(String, String)> = root
            .as_list()
            .into_iter()
            .flatten()
            .filter(|n| n.tag() == Some("property"))
            .filter_map(|n| {
                Some((
                    n.nth(1).and_then(Node::as_str)?.to_string(),
                    n.nth(2).and_then(Node::as_str)?.to_string(),
                ))
            })
            .collect();
        if !vars.is_empty() {
            for t in &mut pcb.texts {
                t.text = expand_vars(&t.text, &vars);
            }
            for fp in &mut pcb.footprints {
                for t in &mut fp.texts {
                    t.text = expand_vars(&t.text, &vars);
                }
            }
            for f in [&mut pcb.title, &mut pcb.company, &mut pcb.rev, &mut pcb.date] {
                if let Some(s) = f.as_mut() {
                    *s = expand_vars(s.as_str(), &vars);
                }
            }
            for cmt in &mut pcb.comments {
                *cmt = expand_vars(cmt.as_str(), &vars);
            }
        }

        pcb
    }
}

// ---------------------------------------------------------------- shared readers

fn layer_of(n: &Node) -> Option<&str> {
    n.child("layer")?.nth(1)?.as_str()
}

fn read_xy(n: &Node, tag: &str) -> Option<Pt> {
    let c = n.child(tag)?;
    Some(Pt { x: c.nth(1)?.as_f64()?, y: c.nth(2)?.as_f64()? })
}

fn read_at(n: &Node) -> At {
    match n.child("at") {
        Some(a) => At {
            x: a.nth(1).and_then(Node::as_f64).unwrap_or(0.0),
            y: a.nth(2).and_then(Node::as_f64).unwrap_or(0.0),
            angle: a.nth(3).and_then(Node::as_f64).unwrap_or(0.0),
        },
        None => At::default(),
    }
}

/// `(pts (xy a b) (xy c d) …)` -> point list.
fn read_pts(n: &Node) -> Vec<Pt> {
    let mut out = Vec::new();
    if let Some(pts) = n.child("pts") {
        for xy in pts.children("xy") {
            if let (Some(x), Some(y)) = (xy.nth(1).and_then(Node::as_f64), xy.nth(2).and_then(Node::as_f64)) {
                out.push(Pt { x, y });
            }
        }
    }
    out
}

fn xyz(n: &Node, tag: &str, default: f64) -> (f64, f64, f64) {
    match n.child(tag).and_then(|c| c.child("xyz")) {
        Some(v) => (
            v.nth(1).and_then(Node::as_f64).unwrap_or(default),
            v.nth(2).and_then(Node::as_f64).unwrap_or(default),
            v.nth(3).and_then(Node::as_f64).unwrap_or(default),
        ),
        None => (default, default, default),
    }
}

fn uuid_of(n: &Node) -> String {
    n.field("uuid").unwrap_or("").to_string()
}

/// Read an object's `(net …)` child as `(code, name)`, tolerating both KiCad net
/// encodings:
///   • ≤9 pad / net table: `(net <code> "<name>")` → `(code, name)`
///   • ≤9 track / via:      `(net <code>)`          → `(code, "")`
///   • 10 (every object):   `(net "<name>")`         → `(0, name)`
/// KiCad 10 dropped numeric net codes (and the board's top-level net table) and now
/// writes the net NAME directly on every pad/track/via/zone, so the name — not the
/// code — is what the renderer keys on. The s-expr parser preserves the quoted/bare
/// distinction, so a quoted first arg (`Node::Text`) is a KiCad-10 name and a bare
/// integer (`Node::Sym`) is a ≤9 code. Missing `(net …)` → `(0, "")`.
fn read_net(owner: &Node) -> (i64, String) {
    let Some(net) = owner.child("net") else { return (0, String::new()) };
    match net.nth(1) {
        // `(net "<name>")` — KiCad 10: name carried directly (quoted), no code.
        Some(Node::Text(name)) => (0, name.clone()),
        // `(net <code> ["<name>"])` — KiCad ≤9: numeric code, optional trailing name.
        Some(Node::Sym(code)) => (
            code.parse().unwrap_or(0),
            net.nth(2).and_then(Node::as_str).unwrap_or("").to_string(),
        ),
        _ => (0, String::new()),
    }
}

// ---------------------------------------------------------------- copper

fn parse_track(n: &Node, is_arc: bool) -> Option<Track> {
    let start = read_xy(n, "start")?;
    let end = read_xy(n, "end")?;
    let (net, net_name) = read_net(n);
    Some(Track {
        start,
        end,
        mid: if is_arc { read_xy(n, "mid") } else { None },
        width: n.field_f64("width").unwrap_or(0.15),
        layer: layer_of(n).unwrap_or("F.Cu").to_string(),
        net,
        net_name,
        uuid: uuid_of(n),
    })
}

fn parse_via(n: &Node) -> Option<Via> {
    let at = read_xy(n, "at")?;
    let layers = n
        .child("layers")
        .and_then(Node::as_list)
        .map(|l| l.iter().skip(1).filter_map(Node::as_str).map(String::from).collect())
        .unwrap_or_default();
    let (net, net_name) = read_net(n);
    // `(remove_unused_layers yes)` (bare token or explicit yes ⇒ true; explicit no ⇒ false).
    let remove_unused_layers = n
        .child("remove_unused_layers")
        .map(|c| c.nth(1).and_then(Node::as_str) != Some("no"))
        .unwrap_or(false);
    Some(Via {
        at,
        size: n.field_f64("size").unwrap_or(0.6),
        drill: n.field_f64("drill").unwrap_or(0.3),
        layers,
        remove_unused_layers,
        net,
        net_name,
        uuid: uuid_of(n),
    })
}

fn parse_zone(n: &Node, out: &mut Vec<Zone>) {
    // KiCad ≤9 zone: `(net <code>) (net_name "<name>")`; KiCad 10: `(net "<name>")`
    // with no separate `(net_name …)`. Prefer the explicit token, else the name the
    // `(net …)` field now carries.
    let (net, net_field_name) = read_net(n);
    let net_name = match n.field("net_name") {
        Some(s) => s.to_string(),
        None => net_field_name,
    };
    let keepout = n.child("keepout").is_some();
    let uuid = uuid_of(n);
    // One Zone per `(filled_polygon (layer L) (pts …))`. Older files keep a single
    // `(layer …)` with `(filled_polygon (pts …))` and no per-polygon layer.
    let fallback_layer = layer_of(n)
        .or_else(|| n.child("layers").and_then(|l| l.nth(1)).and_then(Node::as_str))
        .unwrap_or("F.Cu")
        .to_string();
    let mut emitted = false;
    for fp in n.children("filled_polygon") {
        let layer = fp
            .child("layer")
            .and_then(|l| l.nth(1))
            .and_then(Node::as_str)
            .unwrap_or(&fallback_layer)
            .to_string();
        let pts = read_pts(fp);
        if pts.len() >= 3 {
            out.push(Zone {
                net,
                net_name: net_name.clone(),
                layer,
                pts,
                filled: true,
                keepout,
                uuid: uuid.clone(),
            });
            emitted = true;
        }
    }
    // No solid fill (keepout, or a zone KiCad has not filled yet): keep the
    // user-drawn `(polygon)` outline so the region is still drawn, once per layer.
    if !emitted {
        let pts = n.child("polygon").map(read_pts).unwrap_or_default();
        if pts.len() >= 3 {
            let layers: Vec<String> = n
                .child("layers")
                .and_then(Node::as_list)
                .map(|l| l.iter().skip(1).filter_map(Node::as_str).map(String::from).collect())
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| vec![fallback_layer.clone()]);
            for layer in layers {
                out.push(Zone {
                    net,
                    net_name: net_name.clone(),
                    layer,
                    pts: pts.clone(),
                    filled: false,
                    keepout,
                    uuid: uuid.clone(),
                });
            }
        }
    }
}

// ---------------------------------------------------------------- graphics / text

fn parse_graphic(n: &Node) -> Option<Graphic> {
    let layer = layer_of(n)?.to_string();
    let width = n
        .child("stroke")
        .and_then(|s| s.field_f64("width"))
        .or_else(|| n.field_f64("width"))
        .unwrap_or(0.12);
    let filled = matches!(n.flag("fill"), Some(true))
        || n.child("fill").and_then(|f| f.nth(1)).and_then(Node::as_str) == Some("solid");
    let shape = match n.tag() {
        Some("gr_line") => PcbShape::Seg { a: read_xy(n, "start")?, b: read_xy(n, "end")? },
        Some("gr_rect") => PcbShape::Rect { a: read_xy(n, "start")?, b: read_xy(n, "end")?, filled },
        Some("gr_circle") => {
            let center = read_xy(n, "center")?;
            let edge = read_xy(n, "end")?;
            PcbShape::Circle { center, radius: (edge.x - center.x).hypot(edge.y - center.y), filled }
        }
        Some("gr_arc") => {
            PcbShape::Arc { start: read_xy(n, "start")?, mid: read_xy(n, "mid")?, end: read_xy(n, "end")? }
        }
        Some("gr_poly") | Some("gr_curve") => {
            let pts = read_pts(n);
            if pts.len() < 2 {
                return None;
            }
            PcbShape::Poly { pts, filled }
        }
        _ => return None,
    };
    Some(Graphic { layer, width, shape, uuid: uuid_of(n) })
}

/// Decompose a `(dimension …)` into renderable primitives: the witness/extension
/// lines, the dimension crossbar, the arrowheads, and the measurement label. KiCad
/// stores dimensions parametrically (`pts` + `height` + `orientation` + `style`) and
/// recomputes the geometry when it draws; we rebuild it the same way and push the line
/// segments as `Graphic`s + the label as a `PcbText`, so the ordinary layer renderer
/// draws them on the dimension's layer (e.g. a renamed "Mechanical_Drawing" user
/// layer). Aligned and orthogonal dimensions get full geometry; other types
/// (leader/center/radial) still contribute their text label.
fn parse_dimension(n: &Node, graphics: &mut Vec<Graphic>, texts: &mut Vec<PcbText>) {
    let layer = match layer_of(n) {
        Some(l) => l.to_string(),
        None => return,
    };
    let uuid = uuid_of(n);

    // Label first: the dimension's own `(gr_text "…")` — exact string, position and
    // size as KiCad laid it out. Emitted even for types whose geometry we don't model.
    if let Some(gt) = n.child("gr_text") {
        if let Some(t) = parse_text(gt, "gr_text", 1) {
            texts.push(t);
        }
    }

    let pts = read_pts(n);
    if pts.len() < 2 {
        return;
    }
    let (p0, p1) = (pts[0], pts[1]);
    let height = n.field_f64("height").unwrap_or(0.0);

    let style = n.child("style");
    let style_f = |tag: &str, dflt: f64| style.and_then(|s| s.field_f64(tag)).unwrap_or(dflt);
    let thickness = style_f("thickness", 0.15);
    let arrow_len = style_f("arrow_length", 1.27);
    let ext_offset = style_f("extension_offset", 0.5);
    let ext_overshoot = style_f("extension_height", 0.5);
    let arrow_outward = style
        .and_then(|s| s.field("arrow_direction"))
        != Some("inward");

    // Sign of the offset direction, but 0 for a zero-height (degenerate) dimension —
    // Rust's `f64::signum` returns +1.0 for +0.0, which would fabricate an extension
    // direction where the intent is "no offset".
    let hdir = if height == 0.0 { 0.0 } else { height.signum() };

    // Crossbar endpoints `cb0`/`cb1` and the unit "outward" extension direction
    // (feature point -> crossbar). Orthogonal projects the span onto one axis; aligned
    // runs parallel to p0->p1, offset perpendicular by `height`.
    let (cb0, cb1, ext_dir) = match n.field("type").unwrap_or("aligned") {
        "orthogonal" => {
            let horizontal = n.field("orientation").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0) == 0;
            if horizontal {
                let y = p0.y + height;
                (Pt { x: p0.x, y }, Pt { x: p1.x, y }, Pt { x: 0.0, y: hdir })
            } else {
                let x = p0.x + height;
                (Pt { x, y: p0.y }, Pt { x, y: p1.y }, Pt { x: hdir, y: 0.0 })
            }
        }
        "aligned" => {
            let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
            let len = dx.hypot(dy);
            if len < 1e-9 {
                return;
            }
            // Perpendicular unit vector (p0->p1 rotated +90°); the crossbar sits `height`
            // away along it.
            let perp = Pt { x: -dy / len, y: dx / len };
            let off = Pt { x: perp.x * height, y: perp.y * height };
            (
                Pt { x: p0.x + off.x, y: p0.y + off.y },
                Pt { x: p1.x + off.x, y: p1.y + off.y },
                Pt { x: perp.x * hdir, y: perp.y * hdir },
            )
        }
        // leader / center / radial: the label is already emitted; skip the geometry we
        // don't model rather than draw it wrong.
        _ => return,
    };

    let mut segs: Vec<(Pt, Pt)> = vec![(cb0, cb1)];
    // Extension (witness) lines: start just off each feature point, run past the crossbar.
    for (fp, cb) in [(p0, cb0), (p1, cb1)] {
        segs.push((
            Pt { x: fp.x + ext_dir.x * ext_offset, y: fp.y + ext_dir.y * ext_offset },
            Pt { x: cb.x + ext_dir.x * ext_overshoot, y: cb.y + ext_dir.y * ext_overshoot },
        ));
    }
    // Arrowheads: two wings per crossbar end, `arrow_len` long, fanned ±27.5° about the
    // crossbar axis. Outward arrows (KiCad default) put the tip at the crossbar end with
    // the wings opening toward the span — at cb0 they fan from +u (toward cb1), at cb1
    // from -u; `inward` swaps them.
    let (cbx, cby) = (cb1.x - cb0.x, cb1.y - cb0.y);
    let cblen = cbx.hypot(cby);
    if cblen > 1e-9 && arrow_len > 0.0 {
        let u = Pt { x: cbx / cblen, y: cby / cblen };
        let neg = Pt { x: -u.x, y: -u.y };
        let (b0, b1) = if arrow_outward { (u, neg) } else { (neg, u) };
        for (tip, base) in [(cb0, b0), (cb1, b1)] {
            for s in [1.0_f64, -1.0] {
                let a = 27.5_f64.to_radians() * s;
                let (ca, sa) = (a.cos(), a.sin());
                let dir = Pt { x: base.x * ca - base.y * sa, y: base.x * sa + base.y * ca };
                segs.push((tip, Pt { x: tip.x + dir.x * arrow_len, y: tip.y + dir.y * arrow_len }));
            }
        }
    }

    for (a, b) in segs {
        graphics.push(Graphic {
            layer: layer.clone(),
            width: thickness,
            shape: PcbShape::Seg { a, b },
            uuid: uuid.clone(),
        });
    }
}

/// Read a text node. `text_idx` is where the string payload sits: `1` for a
/// `(gr_text "str" …)`, `2` for a `(fp_text TYPE "str" …)` whose type token is
/// at index 1.
fn parse_text(n: &Node, kind: &str, text_idx: usize) -> Option<PcbText> {
    let text = n.nth(text_idx).and_then(Node::as_str)?.to_string();
    Some(PcbText {
        kind: kind.to_string(),
        text,
        at: read_at(n),
        layer: layer_of(n).unwrap_or("F.SilkS").to_string(),
        size: text_size(n),
        width: text_width(n),
        thickness: text_thickness(n),
        bold: font_bold(n),
        italic: font_italic(n),
        knockout: is_knockout(n),
        hidden: is_hidden(n),
        justify: read_justify(n),
        font: font_face(n),
        uuid: uuid_of(n),
    })
}

/// Whether a text node is bold. KiCad 7+ writes `(font … (bold yes))`; KiCad 5/6 a
/// bare `bold` token inside `(font …)`. Mirrors the schematic effects reader.
fn font_bold(n: &Node) -> bool {
    n.child("effects")
        .and_then(|e| e.child("font"))
        .is_some_and(|font| font.has_flag("bold"))
}

/// Custom outline-font family from `(effects (font (face "Name")))`, when the text
/// overrides KiCad's default stroke font. `None` (absent or empty) ⇒ the built-in
/// Newstroke stroke font, which the viewer renders glyph-for-glyph as before.
fn font_face(n: &Node) -> Option<String> {
    let face = n
        .child("effects")
        .and_then(|e| e.child("font"))
        .and_then(|f| f.field("face"))?
        .trim();
    (!face.is_empty()).then(|| face.to_string())
}

/// Whether a text node is italic — `(italic yes)` or a bare `italic` token, same
/// two encodings as bold.
fn font_italic(n: &Node) -> bool {
    n.child("effects")
        .and_then(|e| e.child("font"))
        .is_some_and(|font| font.has_flag("italic"))
}

/// Whether a text node is KiCad "knockout" (inverted) text — a bare `knockout` token
/// in its `(layer "…" knockout)` node.
fn is_knockout(n: &Node) -> bool {
    n.child("layer").is_some_and(|l| l.has_flag("knockout"))
}

/// Whether a text/property node is hidden: KiCad 7+ writes `(hide yes)`; older
/// files use a bare `hide` token. `has_flag` matches only a bare `Sym("hide")` for
/// the token form, so a `(property …)` whose quoted *value* is "hide" is not a hit.
fn is_hidden(n: &Node) -> bool {
    n.has_flag("hide")
}

/// Push an arc's outline contribution: the two chords `start→mid→end` plus any
/// axis-extreme (cardinal) points of the arc that fall strictly between its
/// endpoints. The chords alone capture the true bounding extreme only when it
/// happens to sit at `mid`, so a shallow asymmetric edge arc would let
/// `outline_bbox` clip the board and the renderer crop copper; the cardinal
/// extreme points close that gap. `Circle` is already special-cased by its caller.
fn push_arc_edges(start: Pt, mid: Pt, end: Pt, out: &mut Vec<(Pt, Pt)>) {
    out.push((start, mid));
    out.push((mid, end));
    for p in arc_extreme_points(start, mid, end) {
        // A degenerate segment contributes the point to the bbox without drawing.
        out.push((p, p));
    }
}

/// The cardinal (axis-extreme) points of the circle through `start`/`mid`/`end`
/// that actually lie on the arc swept `start→mid→end`. Empty when the three points
/// are collinear (no finite circle) — the chords then bound it exactly anyway.
fn arc_extreme_points(start: Pt, mid: Pt, end: Pt) -> Vec<Pt> {
    let (ax, ay, bx, by, cx, cy) = (start.x, start.y, mid.x, mid.y, end.x, end.y);
    // Circumcenter via the standard determinant form.
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-12 {
        return Vec::new(); // collinear / degenerate
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let center = Pt {
        x: (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d,
        y: (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d,
    };
    let r = (ax - center.x).hypot(ay - center.y);
    let ang = |p: Pt| (p.y - center.y).atan2(p.x - center.x);
    let (a_s, a_m, a_e) = (ang(start), ang(mid), ang(end));
    use std::f64::consts::{FRAC_PI_2, PI};
    [
        (Pt { x: center.x + r, y: center.y }, 0.0),
        (Pt { x: center.x, y: center.y + r }, FRAC_PI_2),
        (Pt { x: center.x - r, y: center.y }, PI),
        (Pt { x: center.x, y: center.y - r }, -FRAC_PI_2),
    ]
    .into_iter()
    .filter(|&(_, theta)| angle_on_arc(theta, a_s, a_m, a_e))
    .map(|(p, _)| p)
    .collect()
}

/// Whether angle `theta` lies on the arc running from `a_s` to `a_e` through `a_m`
/// (radians). The sweep direction is inferred from where `a_m` falls.
fn angle_on_arc(theta: f64, a_s: f64, a_m: f64, a_e: f64) -> bool {
    // CCW angular distance from x to y, in [0, 2π).
    let ccw = |x: f64, y: f64| (y - x).rem_euclid(std::f64::consts::TAU);
    if ccw(a_s, a_m) <= ccw(a_s, a_e) {
        // Arc sweeps CCW from start to end.
        ccw(a_s, theta) <= ccw(a_s, a_e)
    } else {
        // Arc sweeps CW: equivalently CCW from end to start.
        ccw(a_e, theta) <= ccw(a_e, a_s)
    }
}

/// Flatten an `Edge.Cuts` graphic into outline segments for the board bbox.
fn push_edge_segments(g: &Graphic, out: &mut Vec<(Pt, Pt)>) {
    match &g.shape {
        PcbShape::Seg { a, b } => out.push((*a, *b)),
        PcbShape::Arc { start, mid, end } => push_arc_edges(*start, *mid, *end, out),
        PcbShape::Rect { a, b, .. } => {
            let (c1, c2, c3, c4) =
                (*a, Pt { x: b.x, y: a.y }, *b, Pt { x: a.x, y: b.y });
            out.extend([(c1, c2), (c2, c3), (c3, c4), (c4, c1)]);
        }
        PcbShape::Circle { center, radius, .. } => {
            out.push((Pt { x: center.x - radius, y: center.y - radius }, Pt { x: center.x + radius, y: center.y + radius }));
        }
        PcbShape::Poly { pts, .. } => {
            for w in pts.windows(2) {
                out.push((w[0], w[1]));
            }
        }
    }
}

// ---------------------------------------------------------------- footprints

fn parse_footprint(n: &Node) -> Footprint {
    let lib_id = n.nth(1).and_then(Node::as_str).unwrap_or("").to_string();
    let at = read_at(n);
    let reference = n.property_value("Reference").unwrap_or("").to_string();
    let value = n.property_value("Value").unwrap_or("").to_string();
    // `(attr smd dnp …)` — the attr block is bare tokens after the placement type.
    let dnp = n
        .child("attr")
        .and_then(Node::as_list)
        .is_some_and(|l| l.iter().any(|t| t.as_str() == Some("dnp")));

    // Local graphic extent (courtyard, silk, lines), rotated into board space.
    let mut local = Vec::new();
    collect_points(n, &mut local);
    let bbox = bbox_of(&local).map(|(mn, mx)| {
        let corners = [(mn.x, mn.y), (mx.x, mn.y), (mx.x, mx.y), (mn.x, mx.y)];
        let placed: Vec<Pt> = corners.iter().map(|&(x, y)| place_fp(&at, x, y)).collect();
        bbox_of(&placed).unwrap()
    });

    let models = n
        .children("model")
        .map(|m| Model3D {
            path: m.nth(1).and_then(Node::as_str).unwrap_or("").to_string(),
            offset: xyz(m, "offset", 0.0),
            scale: xyz(m, "scale", 1.0),
            rotate: xyz(m, "rotate", 0.0),
        })
        .collect();

    let pads = n.children("pad").map(parse_pad).collect();

    let mut graphics = Vec::new();
    for c in n.as_list().into_iter().flatten() {
        match c.tag() {
            Some("fp_line") | Some("fp_rect") | Some("fp_circle") | Some("fp_arc")
            | Some("fp_poly") => {
                if let Some(g) = parse_graphic_fp(c) {
                    graphics.push(g);
                }
            }
            _ => {}
        }
    }

    // Reference / value text — KiCad stores them as `(property "Reference" …)` with
    // a position, and/or legacy `(fp_text reference …)`. Capture both shapes.
    let mut texts = Vec::new();
    for ft in n.children("fp_text") {
        let kind = ft.nth(1).and_then(Node::as_str).unwrap_or("user");
        if let Some(t) = parse_text(ft, kind, 2) {
            texts.push(t);
        }
    }
    if !reference.is_empty() && !texts.iter().any(|t| t.kind == "reference") {
        if let Some(p) = n.property_named("Reference") {
            texts.push(PcbText {
                kind: "reference".into(),
                text: reference.clone(),
                at: read_at(p),
                layer: layer_of(p).unwrap_or("F.SilkS").to_string(),
                size: text_size(p),
                width: text_width(p),
                thickness: text_thickness(p),
                bold: font_bold(p),
                italic: font_italic(p),
                knockout: is_knockout(p),
                hidden: is_hidden(p),
                justify: read_justify(p),
                font: font_face(p),
                uuid: uuid_of(p),
            });
        }
    }
    if !value.is_empty() && !texts.iter().any(|t| t.kind == "value") {
        if let Some(p) = n.property_named("Value") {
            texts.push(PcbText {
                kind: "value".into(),
                text: value.clone(),
                at: read_at(p),
                layer: layer_of(p).unwrap_or("F.Fab").to_string(),
                size: text_size(p),
                width: text_width(p),
                thickness: text_thickness(p),
                bold: font_bold(p),
                italic: font_italic(p),
                knockout: is_knockout(p),
                hidden: is_hidden(p),
                justify: read_justify(p),
                font: font_face(p),
                uuid: uuid_of(p),
            });
        }
    }

    // Expand KiCad's text placeholders so the fab/silk text reads as KiCad plots
    // it (e.g. an `${REFERENCE}` fab marker shows the actual designator).
    for t in &mut texts {
        if t.text.contains("${") {
            t.text = t.text.replace("${REFERENCE}", &reference).replace("${VALUE}", &value);
        }
    }

    Footprint {
        lib_id,
        reference,
        layer: layer_of(n).unwrap_or("F.Cu").to_string(),
        at,
        uuid: uuid_of(n),
        dnp,
        bbox,
        models,
        pads,
        graphics,
        texts,
    }
}

/// Font height (mm) of a text/property node's first `(effects (font (size h …)))`,
/// falling back to KiCad's 1 mm default.
fn text_size(n: &Node) -> f64 {
    n.child("effects")
        .and_then(|e| e.child("font"))
        .and_then(|f| f.child("size"))
        .and_then(|s| s.nth(1))
        .and_then(Node::as_f64)
        .unwrap_or(1.0)
}

/// Stroke thickness (mm) from `(effects (font (thickness t)))`, when the text
/// specifies one. `None` ⇒ KiCad's default pen (the renderer falls back to the font's
/// baked weight).
fn text_thickness(n: &Node) -> Option<f64> {
    n.child("effects")
        .and_then(|e| e.child("font"))
        .and_then(|f| f.field_f64("thickness"))
}

/// Font width (mm) — the second `(effects (font (size h w)))` value. Falls back to
/// the height (square glyph cell) when a width is absent, so callers can treat
/// `width == size` as "no horizontal scaling".
fn text_width(n: &Node) -> f64 {
    n.child("effects")
        .and_then(|e| e.child("font"))
        .and_then(|f| f.child("size"))
        .and_then(|s| s.nth(2))
        .and_then(Node::as_f64)
        .unwrap_or_else(|| text_size(n))
}

/// Title-block `(comment N "text")` lines, sorted by N (KiCad numbers them 1..9,
/// possibly sparse).
fn parse_comments(tb: &Node) -> Vec<String> {
    let mut cs: Vec<(i64, String)> = tb
        .children("comment")
        .filter_map(|c| Some((c.nth(1)?.as_i64()?, c.nth(2)?.as_str()?.to_string())))
        .collect();
    cs.sort_by_key(|(n, _)| *n);
    cs.into_iter().map(|(_, t)| t).collect()
}

/// Expand `${KEY}` occurrences using the board's text variables (the top-level
/// `(property "KEY" "VALUE")` pairs KiCad 8+ writes into the .kicad_pcb). One
/// non-recursive pass — enough for the flat keys KiCad stores; the cheap
/// `contains` guard skips the common no-placeholder text.
fn expand_vars(s: &str, vars: &[(String, String)]) -> String {
    if !s.contains("${") {
        return s.to_string();
    }
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

/// `(effects (justify left bottom mirror))` -> a `TextJustify`. Absent tokens keep
/// the centred default; unknown tokens are ignored.
fn read_justify(n: &Node) -> TextJustify {
    let mut j = TextJustify::default();
    let Some(node) = n.child("effects").and_then(|e| e.child("justify")) else {
        return j;
    };
    for c in node.as_list().into_iter().flatten() {
        if let Node::Sym(s) = c {
            match s.as_str() {
                "left" => j.h = -1,
                "right" => j.h = 1,
                "top" => j.v = -1,
                "bottom" => j.v = 1,
                "mirror" => j.mirror = true,
                _ => {}
            }
        }
    }
    j
}

fn parse_pad(n: &Node) -> Pad {
    let layers = n
        .child("layers")
        .and_then(Node::as_list)
        .map(|l| l.iter().skip(1).filter_map(Node::as_str).map(String::from).collect())
        .unwrap_or_default();
    let size = n.pair("size").unwrap_or((0.0, 0.0));
    let (net, net_name) = read_net(n);
    // Drill: `(drill D)` round, or `(drill oval W H)` slot. `drill` holds the round
    // diameter / oval width W; `drill_h` the oval's second dimension H (0.0 when round).
    // `(drill D (offset X Y))` keeps `drill = D` — the offset list carries no bare f64.
    let (drill, drill_h) = n
        .child("drill")
        .and_then(|d| d.as_list())
        .map(|l| {
            let nums: Vec<f64> = l.iter().filter_map(Node::as_f64).collect();
            let is_oval = l.iter().nth(1).and_then(Node::as_str) == Some("oval");
            let w = nums.first().copied().unwrap_or(0.0);
            let h = if is_oval { nums.get(1).copied().unwrap_or(0.0) } else { 0.0 };
            (w, h)
        })
        .unwrap_or((0.0, 0.0));
    Pad {
        number: n.nth(1).and_then(Node::as_str).unwrap_or("").to_string(),
        kind: n.nth(2).and_then(Node::as_str).unwrap_or("smd").to_string(),
        shape: n.nth(3).and_then(Node::as_str).unwrap_or("rect").to_string(),
        at: read_at(n),
        size,
        drill,
        drill_h,
        roundrect_rratio: n.field_f64("roundrect_rratio").unwrap_or(0.0),
        mask_margin: n.field_f64("solder_mask_margin"),
        layers,
        net,
        net_name,
        uuid: uuid_of(n),
    }
}

fn parse_graphic_fp(n: &Node) -> Option<Graphic> {
    let layer = layer_of(n).unwrap_or("F.SilkS").to_string();
    let width = n
        .child("stroke")
        .and_then(|s| s.field_f64("width"))
        .or_else(|| n.field_f64("width"))
        .unwrap_or(0.12);
    let filled = matches!(n.flag("fill"), Some(true))
        || n.child("fill").and_then(|f| f.nth(1)).and_then(Node::as_str) == Some("solid");
    let shape = match n.tag() {
        Some("fp_line") => PcbShape::Seg { a: read_xy(n, "start")?, b: read_xy(n, "end")? },
        Some("fp_rect") => PcbShape::Rect { a: read_xy(n, "start")?, b: read_xy(n, "end")?, filled },
        Some("fp_circle") => {
            let center = read_xy(n, "center")?;
            let edge = read_xy(n, "end")?;
            PcbShape::Circle { center, radius: (edge.x - center.x).hypot(edge.y - center.y), filled }
        }
        Some("fp_arc") => {
            PcbShape::Arc { start: read_xy(n, "start")?, mid: read_xy(n, "mid")?, end: read_xy(n, "end")? }
        }
        Some("fp_poly") => {
            let pts = read_pts(n);
            if pts.len() < 2 {
                return None;
            }
            PcbShape::Poly { pts, filled }
        }
        _ => return None,
    };
    Some(Graphic { layer, width, shape, uuid: uuid_of(n) })
}

/// Rotate a footprint-local point by the placement angle and translate to board
/// coordinates. KiCad board space is Y-down; a footprint angle θ maps to the SVG
/// `rotate(-θ)` the SVG output uses, i.e. x' = x·cosθ + y·sinθ, y' = -x·sinθ + y·cosθ.
fn place_fp(at: &At, lx: f64, ly: f64) -> Pt {
    let (s, co) = at.angle.to_radians().sin_cos();
    Pt { x: at.x + lx * co + ly * s, y: at.y - lx * s + ly * co }
}

/// Collect footprint-local graphic points (from `fp_line`/`fp_rect`/`fp_poly`/
/// `fp_circle` start/end/center/xy tags), skipping the `(at …)` placement and 3D
/// `(model …)` blocks which are not board geometry.
fn collect_points(n: &Node, out: &mut Vec<Pt>) {
    let Some(list) = n.as_list() else { return };
    match n.tag() {
        // Don't descend into placement or model transforms.
        Some("at") | Some("model") => return,
        Some("start") | Some("end") | Some("center") | Some("xy") | Some("mid") => {
            if let (Some(x), Some(y)) =
                (n.nth(1).and_then(Node::as_f64), n.nth(2).and_then(Node::as_f64))
            {
                out.push(Pt { x, y });
            }
        }
        _ => {}
    }
    for c in list {
        collect_points(c, out);
    }
}

fn bbox_of(points: &[Pt]) -> Option<(Pt, Pt)> {
    let first = points.first()?;
    let (mut min, mut max) = (*first, *first);
    for p in &points[1..] {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = r#"
    (kicad_pcb (version 20240101)
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
      (net 0 "")
      (net 1 "GND")
      (net 2 "/SIG")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 60 90)
        (property "Reference" "R1" (at 0 0 0) (layer "F.SilkS"))
        (fp_rect (start -1 -0.5) (end 1 0.5) (layer "F.CrtYd"))
        (fp_line (start -1 -0.5) (end 1 -0.5) (layer "F.SilkS") (stroke (width 0.12)))
        (pad "1" smd roundrect (at -0.8 0) (size 0.9 0.9) (layers "F.Cu") (net 1 "GND")
          (uuid "pad1"))
        (pad "2" smd roundrect (at 0.8 0) (size 0.9 0.9) (layers "F.Cu") (net 2 "/SIG")
          (uuid "pad2"))
        (model "${KICAD8_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603.step"
          (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0))))
      (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net 2)
        (uuid "t1"))
      (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 1) (uuid "v1"))
      (zone (net 1) (net_name "GND") (layer "F.Cu") (uuid "z1")
        (filled_polygon (layer "F.Cu")
          (pts (xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts"))
      (gr_line (start 100 0) (end 100 80) (layer "Edge.Cuts"))
      (gr_line (start 0 0) (end 0 80) (layer "Edge.Cuts"))
      (gr_line (start 0 80) (end 100 80) (layer "Edge.Cuts")))
    "#;

    #[test]
    fn parses_layers_footprints_edges() {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        assert_eq!(pcb.layers.len(), 3);
        assert_eq!(pcb.layers[0].name, "F.Cu");
        assert_eq!(pcb.footprints.len(), 1);
        let fp = &pcb.footprints[0];
        assert_eq!(fp.reference, "R1");
        assert_eq!(fp.layer, "F.Cu");
        assert_eq!(fp.at.angle, 90.0);
        assert!(fp.bbox.is_some());
        assert_eq!(fp.models.len(), 1);
        assert!(fp.models[0].path.ends_with("R_0603.step"));
        // Board outline spans the four edge lines.
        let (min, max) = pcb.outline_bbox().unwrap();
        assert_eq!((min.x, min.y), (0.0, 0.0));
        assert_eq!((max.x, max.y), (100.0, 80.0));
    }

    #[test]
    fn captures_optional_user_layer_name() {
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (43 "User.3" user "Mechanical_Drawing")
                  (44 "Edge.Cuts" user)))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        let user = pcb.layers.iter().find(|l| l.name == "User.3").unwrap();
        assert_eq!(user.user_name.as_deref(), Some("Mechanical_Drawing"));
        // A canonical-only layer carries no user name.
        assert_eq!(pcb.layers.iter().find(|l| l.name == "F.Cu").unwrap().user_name, None);
    }

    #[test]
    fn parses_copper_and_nets() {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        assert_eq!(pcb.net_name(1), "GND");
        assert_eq!(pcb.net_name(2), "/SIG");
        assert_eq!(pcb.tracks.len(), 1);
        assert_eq!(pcb.tracks[0].width, 0.25);
        assert_eq!(pcb.tracks[0].net, 2);
        assert_eq!(pcb.vias.len(), 1);
        assert_eq!(pcb.vias[0].layers, vec!["F.Cu", "B.Cu"]);
        assert_eq!(pcb.zones.len(), 1);
        assert_eq!(pcb.zones[0].net_name, "GND");
        assert_eq!(pcb.zones[0].pts.len(), 4);
    }

    #[test]
    fn parses_pads_and_fp_graphics() {
        let pcb = Pcb::parse_str(BOARD).unwrap();
        let fp = &pcb.footprints[0];
        assert_eq!(fp.pads.len(), 2);
        assert_eq!(fp.pads[0].number, "1");
        assert_eq!(fp.pads[0].net_name, "GND");
        assert_eq!(fp.pads[0].shape, "roundrect");
        assert!(fp.graphics.iter().any(|g| g.layer == "F.CrtYd"));
        assert!(fp.graphics.iter().any(|g| g.layer == "F.SilkS"));
        assert!(fp.texts.iter().any(|t| t.kind == "reference"));
    }

    // KiCad 10 dropped numeric net codes from the board file: there is no top-level
    // net table, and every pad/track/via/zone carries the net NAME directly as
    // `(net "<name>")`. The parser must read those names so the renderer can emit
    // `data-net` (net labels / highlight / selection all key on it).
    const BOARD_K10: &str = r#"
    (kicad_pcb (version 20260206) (generator_version "10.0")
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 50 60 0)
        (property "Reference" "R1" (at 0 0 0) (layer "F.SilkS"))
        (pad "1" smd roundrect (at -0.8 0) (size 0.9 0.9) (layers "F.Cu") (net "GND")
          (uuid "pad1"))
        (pad "2" smd roundrect (at 0.8 0) (size 0.9 0.9) (layers "F.Cu")
          (net "/SIG") (uuid "pad2")))
      (segment (start 0 0) (end 10 0) (width 0.25) (layer "F.Cu") (net "/SIG")
        (uuid "t1"))
      (via (at 5 5) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net "GND") (uuid "v1"))
      (zone (net "GND") (layer "F.Cu") (uuid "z1")
        (filled_polygon (layer "F.Cu")
          (pts (xy 0 0) (xy 20 0) (xy 20 20) (xy 0 20))))
      (gr_line (start 0 0) (end 100 0) (layer "Edge.Cuts")))
    "#;

    #[test]
    fn kicad10_net_names_carried_on_each_object() {
        let pcb = Pcb::parse_str(BOARD_K10).unwrap();
        // No numeric net table in KiCad 10 — names come straight off the objects.
        assert!(pcb.nets.is_empty());
        assert_eq!(pcb.tracks[0].net_name, "/SIG");
        assert_eq!(pcb.vias[0].net_name, "GND");
        assert_eq!(pcb.zones[0].net_name, "GND");
        assert_eq!(pcb.footprints[0].pads[0].net_name, "GND");
        assert_eq!(pcb.footprints[0].pads[1].net_name, "/SIG");
    }

    const BOARD2: &str = r#"
    (kicad_pcb (version 20241229)
      (layers (0 "F.Cu" signal) (31 "B.Cu" signal) (44 "Edge.Cuts" user))
      (net 0 "")
      (net 1 "GND")
      (footprint "R_0603" (layer "F.Cu") (uuid "fp1") (at 10 10 0)
        (attr smd dnp)
        (property "Reference" "R7" (at 0 -1 0) (layer "F.SilkS") (hide yes))
        (property "Value" "10k" (at 0 1 0) (layer "F.Fab"))
        (fp_text user "${REFERENCE}" (at 0 0 0) (layer "F.Fab"))
        (fp_rect (start -1 -0.5) (end 1 0.5) (layer "F.CrtYd"))
        (pad "1" smd roundrect (at -0.8 0) (size 0.9 1.0) (layers "F.Cu" "F.Mask")
          (roundrect_rratio 0.25) (solder_mask_margin 0.1) (net 1 "GND") (uuid "pad1")))
      (zone (net 0) (net_name "") (layer "B.Cu") (uuid "z2")
        (hatch edge 0.5)
        (keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed)
                 (copperpour not_allowed))
        (polygon (pts (xy 30 30) (xy 40 30) (xy 40 40) (xy 30 40)))))
    "#;

    #[test]
    fn parses_pad_radius_mask_margin_and_attrs() {
        let pcb = Pcb::parse_str(BOARD2).unwrap();
        let fp = &pcb.footprints[0];
        assert!(fp.dnp, "attr dnp should set the footprint DNP flag");
        let pad = &fp.pads[0];
        assert_eq!(pad.roundrect_rratio, 0.25);
        assert_eq!(pad.mask_margin, Some(0.1));
    }

    #[test]
    fn parses_oval_and_round_drill() {
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user))
          (net 0 "")
          (footprint "slot" (layer "F.Cu") (uuid "fp1") (at 10 10 90)
            (pad "" np_thru_hole oval (at 0 0 90) (size 32 17.5)
              (drill oval 32 17.5) (layers "*.Mask")))
          (footprint "round" (layer "F.Cu") (uuid "fp2") (at 20 20 0)
            (pad "1" thru_hole circle (at 0 0 0) (size 1.6 1.6)
              (drill 0.8) (layers "*.Cu"))))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        let oval = &pcb.footprints[0].pads[0];
        assert_eq!(oval.kind, "np_thru_hole");
        assert_eq!(oval.drill, 32.0, "oval drill width = first dim");
        assert_eq!(oval.drill_h, 17.5, "oval drill height = second dim");
        let round = &pcb.footprints[1].pads[0];
        assert_eq!(round.drill, 0.8);
        assert_eq!(round.drill_h, 0.0, "a round drill has no second dimension");
    }

    #[test]
    fn parses_via_remove_unused_layers() {
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (31 "B.Cu" signal))
          (net 0 "")
          (via (at 1 1) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (remove_unused_layers yes) (net 0))
          (via (at 2 2) (size 0.6) (drill 0.3) (layers "F.Cu" "B.Cu") (net 0)))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        assert!(pcb.vias[0].remove_unused_layers, "(remove_unused_layers yes) -> true");
        assert!(!pcb.vias[1].remove_unused_layers, "absent -> false");
    }

    #[test]
    fn marks_hidden_text_and_expands_placeholders() {
        let pcb = Pcb::parse_str(BOARD2).unwrap();
        let fp = &pcb.footprints[0];
        // The reference property is `(hide yes)` -> kept but flagged hidden.
        let r = fp.texts.iter().find(|t| t.kind == "reference").unwrap();
        assert!(r.hidden);
        // The visible value is not hidden.
        assert!(!fp.texts.iter().find(|t| t.kind == "value").unwrap().hidden);
        // `${REFERENCE}` expands to the actual designator.
        assert!(fp.texts.iter().any(|t| t.text == "R7" && t.layer == "F.Fab"));
        assert!(!fp.texts.iter().any(|t| t.text.contains("${")));
    }

    #[test]
    fn parses_bold_and_knockout_text() {
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (37 "F.SilkS" user))
          (net 0 "")
          (gr_text "INV" (at 1 1 0) (layer "F.SilkS" knockout)
            (effects (font (size 2 1.25) (thickness 0.275) (bold yes)) (justify bottom)))
          (gr_text "BARE" (at 3 3 0) (layer "F.SilkS")
            (effects (font (size 1 1) bold)))
          (gr_text "PLAIN" (at 2 2 0) (layer "F.SilkS")
            (effects (font (size 1 1)))))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        let inv = pcb.texts.iter().find(|t| t.text == "INV").unwrap();
        assert!(inv.bold, "(bold yes) should set bold");
        assert!(inv.knockout, "(layer .. knockout) should set knockout");
        // KiCad 5/6 bare `bold` token.
        assert!(pcb.texts.iter().find(|t| t.text == "BARE").unwrap().bold);
        let plain = pcb.texts.iter().find(|t| t.text == "PLAIN").unwrap();
        assert!(!plain.bold, "plain silk text is not bold");
        assert!(!plain.knockout, "plain silk text is not knockout");
        // Stroke thickness is captured when specified, absent otherwise (item 2 — the
        // viewer thickens the glyph strokes to the authored pen so silk weight matches).
        assert_eq!(inv.thickness, Some(0.275), "authored (thickness …) is captured");
        assert_eq!(plain.thickness, None, "no thickness ⇒ None (font default weight)");
    }

    #[test]
    fn captures_custom_outline_font_face() {
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (37 "F.SilkS" user))
          (net 0 "")
          (gr_text "CAL" (at 1 1 0) (layer "F.SilkS")
            (effects (font (face "Calibri") (size 1 1))))
          (gr_text "STROKE" (at 2 2 0) (layer "F.SilkS")
            (effects (font (size 1 1)))))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        // A named (face …) is carried so the viewer renders that real font…
        assert_eq!(
            pcb.texts.iter().find(|t| t.text == "CAL").unwrap().font.as_deref(),
            Some("Calibri"),
        );
        // …and text without one stays on the default KiCad stroke font.
        assert_eq!(pcb.texts.iter().find(|t| t.text == "STROKE").unwrap().font, None);
    }

    const BOARD_VARS: &str = r#"
    (kicad_pcb (version 20241229)
      (paper "A3")
      (title_block (title "${PCB_PART_NUMBER} board") (rev "${PCB_REVISION}"))
      (property "PCB_PART_NUMBER" "EX-0000035-00")
      (property "PCB_REVISION" "A")
      (layers (0 "F.Cu" signal) (37 "F.SilkS" user) (44 "Edge.Cuts" user))
      (net 0 "")
      (gr_text "${PCB_PART_NUMBER}/${PCB_REVISION}" (at 100 100 270) (layer "F.SilkS"))
      (footprint "logo" (layer "F.Cu") (uuid "fp1") (at 10 10 0)
        (property "Reference" "G1" (at 0 0 0) (layer "F.SilkS"))
        (fp_text user "${REFERENCE}-${PCB_PART_NUMBER}" (at 0 0 0) (layer "F.SilkS"))))
    "#;

    #[test]
    fn expands_board_text_variables() {
        let pcb = Pcb::parse_str(BOARD_VARS).unwrap();
        // Board gr_text resolves the project text variables.
        assert!(
            pcb.texts.iter().any(|t| t.text == "EX-0000035-00/A"),
            "gr_text must expand ${{PCB_PART_NUMBER}}/${{PCB_REVISION}}: {:?}",
            pcb.texts.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
        // Footprint text gets BOTH the footprint-scoped ${REFERENCE} and the board var.
        let fp = &pcb.footprints[0];
        assert!(fp.texts.iter().any(|t| t.text == "G1-EX-0000035-00"));
        // Title block fields expand too.
        assert_eq!(pcb.title.as_deref(), Some("EX-0000035-00 board"));
        assert_eq!(pcb.rev.as_deref(), Some("A"));
        // Nothing left unresolved.
        assert!(!pcb.texts.iter().any(|t| t.text.contains("${")));
        assert!(!fp.texts.iter().any(|t| t.text.contains("${")));
    }

    #[test]
    fn parses_keepout_zone_outline() {
        let pcb = Pcb::parse_str(BOARD2).unwrap();
        let z = pcb.zones.iter().find(|z| z.keepout).expect("keepout zone");
        assert!(!z.filled, "keepout has no copper fill");
        assert_eq!(z.layer, "B.Cu");
        assert_eq!(z.pts.len(), 4);
    }

    #[test]
    fn rejects_non_board_root() {
        // A renamed symbol library / netlist must error, not parse as an empty board.
        let sym = r#"(kicad_symbol_lib (version 20240101) (symbol "R"))"#;
        assert!(matches!(
            Pcb::parse_str(sym),
            Err(ParseError::WrongRoot { expected: "kicad_pcb", .. })
        ));
    }

    #[test]
    fn parses_user_paper_dims() {
        let src = r#"
        (kicad_pcb (version 20241229) (paper "User" 431.8 279.4)
          (layers (0 "F.Cu" signal)))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        assert_eq!(pcb.paper.as_deref(), Some("User"));
        assert_eq!(pcb.paper_dims, Some((431.8, 279.4)));
        // A standard token carries no explicit dims.
        let std = Pcb::parse_str(r#"(kicad_pcb (paper "A3") (layers (0 "F.Cu" signal)))"#).unwrap();
        assert_eq!(std.paper_dims, None);
    }

    #[test]
    fn edge_cuts_arc_extends_outline_bbox_to_its_axis_extremes() {
        // A large asymmetric Edge.Cuts arc on a circle of radius 10 about the origin,
        // sweeping CCW from -30° through 45° to 135°. Its true extremes are the axis
        // points at 0° (10,0) and 90° (0,10) — neither of which is an endpoint or the
        // `mid`, so flattening to chords alone would clip the board.
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user))
          (gr_arc (start 8.6603 -5.0) (mid 7.0711 7.0711) (end -7.0711 7.0711)
            (layer "Edge.Cuts")))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        let (min, max) = pcb.outline_bbox().expect("outline from the arc");
        assert!((max.x - 10.0).abs() < 1e-3, "captures the 0° extreme x=10, got {}", max.x);
        assert!((max.y - 10.0).abs() < 1e-3, "captures the 90° extreme y=10, got {}", max.y);
        assert!((min.y + 5.0).abs() < 1e-3, "start point sets min y=-5, got {}", min.y);
    }

    #[test]
    fn arc_track_on_edge_cuts_feeds_outline() {
        // An `(arc)` copper primitive mis-saved on Edge.Cuts still contributes to the
        // board extent (previously only `(segment)` did).
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (44 "Edge.Cuts" user))
          (arc (start 0 0) (mid 5 5) (end 10 0) (width 0.1) (layer "Edge.Cuts") (net 0)))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();
        assert!(pcb.outline_bbox().is_some(), "arc track on Edge.Cuts feeds the outline");
    }

    #[test]
    fn orthogonal_dimension_expands_to_lines_and_label() {
        // A real KiCad orthogonal dimension on a "Mechanical_Drawing" user layer.
        let src = r#"
        (kicad_pcb (version 20241229)
          (layers (0 "F.Cu" signal) (45 "User.4" user "Mechanical_Drawing"))
          (net 0 "")
          (dimension
            (type orthogonal)
            (layer "User.4")
            (uuid "dim1")
            (pts (xy 176.435 66.96) (xy 307.325 66.96))
            (height -27.14)
            (orientation 0)
            (style (thickness 0.1) (arrow_length 1.27) (arrow_direction outward)
              (extension_height 0.586) (extension_offset 0.5))
            (gr_text "130.8900 mm" (at 241.88 38.67 0) (layer "User.4") (uuid "dim1")
              (effects (font (size 1 1) (thickness 0.15))))))
        "#;
        let pcb = Pcb::parse_str(src).unwrap();

        // The measurement label is emitted as board text on the dimension's layer.
        let label = pcb.texts.iter().find(|t| t.text == "130.8900 mm").expect("dimension label");
        assert_eq!(label.layer, "User.4");

        // The parametric dimension expands to line segments on User.4: crossbar + two
        // extension lines + two arrowheads (2 wings each) = 7 segments, at the style width.
        let segs: Vec<_> = pcb.graphics.iter().filter(|g| g.layer == "User.4").collect();
        assert_eq!(segs.len(), 7, "crossbar + 2 extension + 4 arrow-wing segments");
        assert!(segs.iter().all(|g| (g.width - 0.1).abs() < 1e-9), "segments use the style thickness");

        // Crossbar sits `height` off the feature line (y = 66.96 - 27.14 = 39.82),
        // spanning the measured X range.
        let cross = segs
            .iter()
            .find_map(|g| match &g.shape {
                PcbShape::Seg { a, b } if (a.y - 39.82).abs() < 1e-3 && (b.y - 39.82).abs() < 1e-3 => Some((*a, *b)),
                _ => None,
            })
            .expect("horizontal crossbar at y=39.82");
        let (xmin, xmax) = (cross.0.x.min(cross.1.x), cross.0.x.max(cross.1.x));
        assert!((xmin - 176.435).abs() < 1e-3 && (xmax - 307.325).abs() < 1e-3, "crossbar spans the measured range");

        // An extension line starts just off a feature point (66.96 - 0.5 = 66.46) and
        // runs past the crossbar (39.82 - 0.586 = 39.234).
        assert!(
            segs.iter().any(|g| matches!(&g.shape,
                PcbShape::Seg { a, b }
                    if ((a.y - 66.46).abs() < 1e-3 && (b.y - 39.234).abs() < 1e-3)
                    || ((b.y - 66.46).abs() < 1e-3 && (a.y - 39.234).abs() < 1e-3))),
            "extension line from offset feature point past the crossbar"
        );
    }
}
