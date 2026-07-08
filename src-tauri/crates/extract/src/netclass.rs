//! Net-class colour resolution from a `.kicad_pro` project file.
//!
//! KiCad stores schematic net colours indirectly: each net belongs to a net
//! class, and a class can carry a `schematic_color`. Membership comes from three
//! places (highest priority first): a direct per-net colour override
//! (`net_colors`), an explicit per-net class assignment (`netclass_assignments`),
//! and pattern rules (`netclass_patterns`).
//!
//! Matching our net names to KiCad's canonical names is imperfect — KiCad
//! prefixes the sheet path and disambiguates duplicates with a `_N` suffix
//! (`/CAN_RX_2`) that our netlister does not reproduce. So we match tolerantly:
//! exact name, a leading-`/` form, and finally a normalised comparison of the
//! last path segment with any trailing `_<digits>` stripped. This recovers the
//! common cases (the CAN bus turning purple) without claiming full parity.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use regex::Regex;
use serde_json::Value;

/// Resolved net-class colour data for a project.
#[derive(Debug, Default)]
pub struct NetClassColors {
    /// Class name → `#RRGGBB`, only for classes that set a schematic colour.
    class_color: BTreeMap<String, String>,
    /// Ordered pattern rules: `(pattern, class)`.
    patterns: Vec<(String, String)>,
    /// Explicit per-net class assignment: net name → class names.
    assignments: BTreeMap<String, Vec<String>>,
    /// Direct per-net colour override: net name → `#RRGGBB`.
    net_colors: BTreeMap<String, String>,
}

/// Parse `rgb(r,g,b)` / `rgba(r,g,b,a)` into `#RRGGBB`, dropping fully
/// transparent colours (KiCad's "unset" sentinel) and white-on-nothing.
fn parse_rgb(s: &str) -> Option<String> {
    let inner = s.trim().strip_prefix("rgb")?;
    let inner = inner.trim_start_matches('a');
    let inner = inner.trim().strip_prefix('(')?.strip_suffix(')')?;
    let parts: Vec<f64> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() < 3 {
        return None;
    }
    let a = parts.get(3).copied().unwrap_or(1.0);
    if a <= 0.0 {
        return None;
    }
    Some(format!("#{:02X}{:02X}{:02X}", parts[0] as u8, parts[1] as u8, parts[2] as u8))
}

impl NetClassColors {
    /// Read net-class colour data from a project file's JSON. Returns an empty
    /// (no-op) instance on any parse problem, so colouring simply doesn't apply.
    pub fn from_pro(path: &std::path::Path) -> NetClassColors {
        let Ok(text) = std::fs::read_to_string(path) else {
            return NetClassColors::default();
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return NetClassColors::default();
        };
        let ns = &json["net_settings"];
        let mut out = NetClassColors::default();

        if let Some(classes) = ns["classes"].as_array() {
            for cl in classes {
                if let (Some(name), Some(color)) = (cl["name"].as_str(), cl["schematic_color"].as_str()) {
                    if let Some(hex) = parse_rgb(color) {
                        out.class_color.insert(name.to_string(), hex);
                    }
                }
            }
        }
        if let Some(pats) = ns["netclass_patterns"].as_array() {
            for p in pats {
                if let (Some(pat), Some(cls)) = (p["pattern"].as_str(), p["netclass"].as_str()) {
                    out.patterns.push((pat.to_string(), cls.to_string()));
                }
            }
        }
        if let Some(asg) = ns["netclass_assignments"].as_object() {
            for (net, classes) in asg {
                let names: Vec<String> = classes
                    .as_array()
                    .map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if !names.is_empty() {
                    out.assignments.insert(net.clone(), names);
                }
            }
        }
        if let Some(nc) = ns["net_colors"].as_object() {
            for (net, color) in nc {
                if let Some(hex) = color.as_str().and_then(parse_rgb) {
                    out.net_colors.insert(net.clone(), hex);
                }
            }
        }
        out
    }

    /// True when no colours are defined (skip the whole pass).
    pub fn is_empty(&self) -> bool {
        self.net_colors.is_empty()
            && self.patterns.is_empty()
            && self.assignments.is_empty()
    }

    /// First class (in priority order) whose colour applies to `net`.
    fn class_color_for(&self, net: &str) -> Option<&String> {
        // Try the assignment table under a few name forms.
        for key in name_forms(net) {
            if let Some(classes) = self.assignments.get(&key) {
                for cls in classes {
                    if let Some(hex) = self.class_color.get(cls) {
                        return Some(hex);
                    }
                }
            }
        }
        // Then pattern rules, first match wins (KiCad evaluates in order).
        for (pat, cls) in &self.patterns {
            if pattern_matches(pat, net) {
                if let Some(hex) = self.class_color.get(cls) {
                    return Some(hex);
                }
            }
        }
        None
    }

    /// Direct colour of a named net class (for directive-flag glyphs, which carry
    /// their class name rather than a net name).
    pub fn class_hex(&self, class: &str) -> Option<String> {
        self.class_color.get(class).cloned()
    }

    /// All net-class names that apply to `net`, highest-priority first (explicit
    /// per-net assignment wins, then the first matching pattern rule — KiCad
    /// evaluates patterns in order). Empty when no rule matches: the caller treats
    /// that as the implicit "Default" class. Unlike `color_for`, this returns the
    /// class even when it carries no schematic colour, so the viewer's net card
    /// shows the real class (e.g. CAN_RX → 50-ohm) instead of always "Default".
    pub fn classes_for(&self, net: &str) -> Vec<String> {
        for key in name_forms(net) {
            if let Some(classes) = self.assignments.get(&key) {
                if !classes.is_empty() {
                    return classes.clone();
                }
            }
        }
        for (pat, cls) in &self.patterns {
            if pattern_matches(pat, net) {
                return vec![cls.clone()];
            }
        }
        Vec::new()
    }

    /// True when the project defines any net-class membership rules (patterns or
    /// per-net assignments) — i.e. `classes_for` can ever return a non-default
    /// class. Independent of whether those classes carry colours.
    pub fn has_classes(&self) -> bool {
        !self.patterns.is_empty() || !self.assignments.is_empty()
    }

    /// Resolve the schematic colour for a net name, or `None` for the default.
    pub fn color_for(&self, net: &str) -> Option<String> {
        for key in name_forms(net) {
            if let Some(hex) = self.net_colors.get(&key) {
                return Some(hex.clone());
            }
        }
        self.class_color_for(net).cloned()
    }
}

/// Candidate KiCad-name forms for one of our net names: as-is and a leading-`/`
/// variant (root nets are written `/NAME` in the project file).
fn name_forms(net: &str) -> Vec<String> {
    if net.starts_with('/') {
        vec![net.to_string()]
    } else {
        vec![net.to_string(), format!("/{net}")]
    }
}

/// Last path segment with a trailing `_<digits>` disambiguation suffix removed.
fn normalize(name: &str) -> &str {
    let seg = name.rsplit('/').next().unwrap_or(name);
    match seg.rsplit_once('_') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => seg,
    }
}

/// Whether a KiCad net-class pattern matches one of our net names. KiCad's
/// membership matcher is a *combined* one — it accepts a pattern as either a
/// `*`/`?` wildcard or a regular expression and takes a match from either. We do
/// the same: try the wildcard first, then the pattern as an anchored regex (this
/// is what catches `in\d+`, `uio\d+`, `.*out\d+` and friends, which the wildcard
/// reads literally), then the tolerant normalised comparison.
fn pattern_matches(pattern: &str, net: &str) -> bool {
    for form in name_forms(net) {
        if glob_match(pattern, &form) {
            return true;
        }
    }
    // KiCad also evaluates the pattern as a regex (anchored, full-match). An invalid
    // regex — e.g. the wildcard-only `+?V?` — simply doesn't compile and is skipped,
    // leaving the wildcard result above to stand.
    if let Some(re) = pattern_regex(pattern) {
        if name_forms(net).iter().any(|form| re.is_match(form)) {
            return true;
        }
    }
    // Tolerant fallback: compare last segments with `_N` disambiguation stripped.
    let np = normalize(pattern);
    let nn = normalize(net);
    !np.is_empty() && (np == nn || glob_match(np, nn))
}

/// Compile a KiCad net-class pattern as an anchored (full-match) regex, caching by
/// pattern text so a project's handful of patterns compile once rather than per
/// (pattern, net) pair. Returns `None` for patterns that aren't valid regexes
/// (e.g. wildcard-only `+?V?`), which the wildcard matcher handles instead.
fn pattern_regex(pattern: &str) -> Option<Regex> {
    thread_local! {
        static CACHE: RefCell<HashMap<String, Option<Regex>>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(pattern.to_string())
            .or_insert_with(|| Regex::new(&format!("^(?:{pattern})$")).ok())
            .clone()
    })
}

/// Minimal KiCad-style glob match (`*` any run, `?` one char), full-string.
/// Iterative two-pointer matcher — O(n·m) worst case and O(1) stack — so a `*`-heavy
/// pattern or a very long net name (both come from syncable, untrusted files) can't
/// backtrack exponentially or overflow the stack the way a recursive matcher would.
fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    // Backtrack point: the last `*` in the pattern and the text position to retry from.
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(sp) = star {
            // Mismatch: let the last `*` swallow one more text byte and retry after it.
            pi = sp + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    // Any pattern tail must be all `*` to match the now-empty remainder.
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rgb_forms() {
        assert_eq!(parse_rgb("rgb(106, 94, 255)").as_deref(), Some("#6A5EFF"));
        assert_eq!(parse_rgb("rgba(0, 0, 0, 0.000)"), None); // transparent = unset
        assert_eq!(parse_rgb("rgb(255, 8, 14)").as_deref(), Some("#FF080E"));
    }

    #[test]
    fn glob_match_no_backtracking_blowup_or_overflow() {
        // A `*`-heavy non-match must return promptly (no exponential backtracking).
        assert!(!glob_match("*a*a*a*a*a*a*a*a*a*a*a*a*", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
        // A very long input against `*` must not overflow the stack.
        let long = "x".repeat(200_000);
        assert!(glob_match("*", &long));
        assert!(glob_match("x*x", &long));
        // Semantics preserved (the cases the recursive version handled).
        assert!(glob_match("CAN_*", "CAN_H"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("?", ""));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn normalize_strips_disambiguation() {
        assert_eq!(normalize("/CAN_RX_2"), "CAN_RX");
        assert_eq!(normalize("CAN_RX"), "CAN_RX");
        assert_eq!(normalize("/F280039/CAN_RXD"), "CAN_RXD");
        assert_eq!(normalize("/A/B_12"), "B");
    }

    #[test]
    fn matches_can_pattern_via_normalization() {
        // Our net "CAN_RX" should match KiCad's stale-disambiguated "/CAN_RX_2".
        assert!(pattern_matches("/CAN_RX_2", "CAN_RX"));
        assert!(pattern_matches("/CAN_TX_2", "CAN_TX"));
        // But not an unrelated CAN net.
        assert!(!pattern_matches("/F280039/CAN_RXD", "CAN_RX"));
    }

    #[test]
    fn matches_regex_patterns_like_kicad() {
        // KiCad netclass patterns are commonly regexes (the tiny_tapeout demo uses
        // `in\d+`, `uio\d+`, `.*out\d+`). The wildcard matcher reads `\d+` literally,
        // so these need the regex path.
        assert!(pattern_matches(r"in\d+", "in6"));
        assert!(pattern_matches(r"in\d+", "in19"));
        assert!(pattern_matches(r"uio\d+", "uio0"));
        assert!(pattern_matches(r".*out\d+", "uo_out3"));
        // Anchored full-match: a leading prefix must be covered by the pattern.
        assert!(!pattern_matches(r"in\d+", "main6"));
        assert!(!pattern_matches(r"in\d+", "in"));
        // A wildcard-only pattern that is not a valid regex still works via globbing.
        assert!(pattern_matches("+?V?", "+5V0"));
        assert!(pattern_matches("usb_d*", "usb_dp"));
    }

    #[test]
    fn resolves_color_priority() {
        let mut c = NetClassColors::default();
        c.class_color.insert("50-ohm".into(), "#6A5EFF".into());
        c.patterns.push(("/CAN_RX_2".into(), "50-ohm".into()));
        c.assignments.insert("/CANH".into(), vec!["Isolated".into()]);
        c.class_color.insert("Isolated".into(), "#484848".into());
        c.net_colors.insert("+5V_ISO".into(), "#19E0FF".into());

        assert_eq!(c.color_for("CAN_RX").as_deref(), Some("#6A5EFF")); // via pattern
        assert_eq!(c.color_for("CANH").as_deref(), Some("#484848")); // via assignment (/CANH)
        assert_eq!(c.color_for("+5V_ISO").as_deref(), Some("#19E0FF")); // direct net colour
        assert_eq!(c.color_for("SOME_OTHER_NET"), None);
    }
}
