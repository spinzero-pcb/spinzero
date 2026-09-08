//! Altium layer identity: legacy id -> bundle `role`, side, and emit order.
//!
//! Two things have to be true at once (plan §6.1). Review logic keys on `role`
//! — `copper`, `silkscreen`, `mask`, `paste`, `edge`, `user` — so a rule never
//! needs to know which tool drew the board; the *name* the user sees stays
//! Altium's own (`L2_PWR`, `M15 (CMP_Courtyard_Top)`), which is what
//! `Board6` carries and [`eda_parse_altium::pcb`] resolves.
//!
//! A primitive names its layer by the **legacy** id, which is why every mapping
//! here is keyed by that and not by a name.

use eda_parse_altium::pcb::{PcbDoc, BOTTOM, KEEP_OUT, MULTI_LAYER, TOP};

/// The bundle role of a legacy layer id, given the name the design gives it.
///
/// The name only ever decides one thing: Altium designs conventionally draw the
/// board profile on a *mechanical* layer named for it, and the "board outline =
/// graphics on the edge layer" review move has to find that outline. The match
/// is deliberately narrow — a layer has to say it is the board shape — because
/// promoting the wrong mechanical layer to `edge` would make a dimension
/// drawing look like the board.
pub fn role(id: u8, name: &str) -> &'static str {
    match id {
        // Signal copper (1..=32) and the internal planes (39..=54) are both
        // copper as far as a review move is concerned.
        1..=32 | 39..=54 => "copper",
        33 | 34 => "silkscreen",
        35 | 36 => "paste",
        37 | 38 => "mask",
        // Altium's own board shape lives on the keep-out layer.
        KEEP_OUT => "edge",
        57..=72 if is_board_outline(name) => "edge",
        _ => "user",
    }
}

/// True for a mechanical layer whose name says it carries the board profile.
fn is_board_outline(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["board shape", "board outline", "board_outline", "boardoutline"]
        .iter()
        .any(|k| n.contains(k))
}

/// Board side of a legacy layer id, in the bundle's own vocabulary.
pub fn side(id: u8) -> Option<&'static str> {
    match id {
        TOP | 33 | 35 | 37 => Some("front"),
        BOTTOM | 34 | 36 | 38 => Some("back"),
        2..=31 | 39..=54 => Some("inner"),
        // Mechanical layers pair up by convention, not by rule, so they are
        // given no side rather than a guessed one.
        _ => None,
    }
}

/// Legacy ids of the copper layers a via or through-hole pad spans, in stack
/// order. `from`/`to` are the via's own start and end layers.
pub fn span(stack: &[u8], from: u8, to: u8) -> Vec<u8> {
    let pos = |id: u8| stack.iter().position(|&l| l == id);
    match (pos(from), pos(to)) {
        (Some(a), Some(b)) => stack[a.min(b)..=a.max(b)].to_vec(),
        // An unrecognised span is a through via, which is what Altium's own
        // default is; silently emitting nothing would hide the via entirely.
        _ => stack.to_vec(),
    }
}

/// The layers a primitive occupies. Everything but the multi-layer pseudo-layer
/// is on exactly the layer it names; a multi-layer pad or via is on every copper
/// layer of the stack.
pub fn occupies(stack: &[u8], id: u8) -> Vec<u8> {
    if id == MULTI_LAYER {
        stack.to_vec()
    } else {
        vec![id]
    }
}

/// The layers the geometry document carries, in emit order.
///
/// Order is the board's own: the copper stack top to bottom, then the fixed
/// fabrication layers, then whatever else the design actually drew on. A layer
/// with no primitives is left out unless it is part of the copper stack or the
/// edge layer, whose indices reviewers and the viewer expect to exist.
pub fn emit_order(pcb: &PcbDoc, used: &dyn Fn(u8) -> bool) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let push = |id: u8, out: &mut Vec<u8>| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    for &id in &pcb.stack {
        push(id, &mut out);
    }
    for id in [33u8, 34, 37, 38, 35, 36, KEEP_OUT] {
        push(id, &mut out);
    }
    let mut rest: Vec<u8> = pcb
        .layers
        .keys()
        .copied()
        .filter(|id| !out.contains(id) && used(*id))
        .collect();
    rest.sort_unstable();
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4.5 table, as a test. `role` is the review vocabulary, so a wrong
    /// answer here silently changes what every board rule sees.
    #[test]
    fn legacy_ids_carry_the_documented_roles() {
        assert_eq!(role(TOP, "L1_Top"), "copper");
        assert_eq!(role(16, ""), "copper", "a mid-layer");
        assert_eq!(role(BOTTOM, "L4_Bot"), "copper");
        assert_eq!(role(40, ""), "copper", "Internal Plane 2");
        assert_eq!(role(33, "Top Overlay"), "silkscreen");
        assert_eq!(role(37, "Top Solder"), "mask");
        assert_eq!(role(36, "Bottom Paste"), "paste");
        assert_eq!(role(KEEP_OUT, "Keep-Out Layer"), "edge");
        assert_eq!(role(71, "M15 (CMP_Courtyard_Top)"), "user", "Mechanical 15");
        assert_eq!(role(MULTI_LAYER, "Multi-Layer"), "user");
    }

    /// The corpus draws the board profile on a mechanical layer called
    /// "Board Shape". Leaving it a user layer hides the outline from the review
    /// move that looks for it.
    #[test]
    fn a_mechanical_layer_named_for_the_board_shape_is_the_edge() {
        assert_eq!(role(63, "Board Shape"), "edge");
        assert_eq!(role(63, "M7 (Board_Outline)"), "edge");
        assert_eq!(role(63, "M7 (Dimension_Top)"), "user");
        assert_eq!(role(63, "Shape"), "user", "the match has to say BOARD");
    }

    #[test]
    fn sides_follow_the_stack() {
        assert_eq!(side(TOP), Some("front"));
        assert_eq!(side(BOTTOM), Some("back"));
        assert_eq!(side(3), Some("inner"));
        assert_eq!(side(34), Some("back"));
        assert_eq!(side(71), None);
    }

    #[test]
    fn a_via_spans_the_stack_between_its_own_layers() {
        let stack = [1u8, 2, 3, 32];
        assert_eq!(span(&stack, 1, 32), vec![1, 2, 3, 32], "a through via");
        assert_eq!(span(&stack, 1, 2), vec![1, 2], "a blind via");
        assert_eq!(span(&stack, 3, 2), vec![2, 3], "start and end in either order");
        assert_eq!(span(&stack, 9, 9), vec![1, 2, 3, 32], "an unknown span stays visible");
    }

    #[test]
    fn a_multi_layer_pad_is_on_every_copper_layer() {
        let stack = [1u8, 2, 32];
        assert_eq!(occupies(&stack, MULTI_LAYER), vec![1, 2, 32]);
        assert_eq!(occupies(&stack, 33), vec![33]);
    }
}
