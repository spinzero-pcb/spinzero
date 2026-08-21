//! Engineering-value normalizer — the Rust twin of `bom_model.py`'s
//! `normalize_eng_value` / `extract_aux_specs`. Both runtimes must agree on these,
//! because several rules compare values only after normalizing them ("470nF" and
//! "0.47uF" are the same part; "USB1" and "USB2" are not values at all).

use crate::re;

/// Multiplier for an engineering prefix. Case matters only for m (milli) vs M (mega);
/// R/r is the resistor decimal-point convention (0R, 470R, 4R7) with multiplier 1.
fn prefix_mult(p: &str) -> Option<f64> {
    Some(match p {
        "p" | "P" => 1e-12,
        "n" | "N" => 1e-9,
        "u" | "U" | "µ" | "μ" => 1e-6,
        "m" => 1e-3,
        "k" | "K" => 1e3,
        "M" => 1e6,
        "G" | "g" => 1e9,
        "R" | "r" => 1.0,
        "" => 1.0,
        _ => return None,
    })
}

/// Parse an engineering-notation string → magnitude, or `None` when it is not a
/// value at all (a label like "USB1", a part name, free text).
///
/// Handles 470nF, 0.47uF, 4u7, 2K2, 0R, 470R, 4R7, 22p, 100P, 8MHz, 6.8 uH …
pub fn normalize_eng_value(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    let stripped = re!(r"(?i)(Hz|F|H|Ω|ohms?|W|V|A)$").replace(s.trim(), "");
    let s = stripped.trim();
    if let Some(c) = re!(r"^([0-9]*\.?[0-9]+)\s*([pnumkMGRPNU µμgr]?)\s*$").captures(s) {
        let num: f64 = c.get(1)?.as_str().parse().ok()?;
        return prefix_mult(c.get(2).map_or("", |m| m.as_str()).trim()).map(|m| num * m);
    }
    // Embedded-prefix form: 4u7, 2K2, 4R7. (`K` is deliberately included here; the
    // Python original omits it from this pattern, so "2K2" fails to parse there — a
    // latent bug worth fixing on that side too, tracked in schemas/rule-fixtures.)
    if let Some(c) = re!(r"^([0-9]+)([pnumkKMGRPNUµμgr])([0-9]*)$").captures(s) {
        let (left, prefix, right) = (
            c.get(1)?.as_str(),
            c.get(2)?.as_str(),
            c.get(3).map_or("", |m| m.as_str()),
        );
        let joined = if right.is_empty() {
            left.to_string()
        } else {
            format!("{left}.{right}")
        };
        let num: f64 = joined.parse().ok()?;
        return prefix_mult(prefix).map(|m| num * m);
    }
    None
}

/// Auxiliary spec tokens embedded in a Value string ("0.1uF 100V 10%", "1uF/50V").
/// Returned so a rule can cross-check them against the dedicated columns.
#[derive(Default, Debug, Clone, Copy)]
pub struct AuxSpecs {
    pub voltage: Option<f64>,
    pub tolerance_pct: Option<f64>,
}

impl AuxSpecs {
    pub fn is_empty(&self) -> bool {
        self.voltage.is_none() && self.tolerance_pct.is_none()
    }
}

pub fn extract_aux_specs(s: &str) -> AuxSpecs {
    let mut out = AuxSpecs::default();
    if let Some(c) = re!(r"(?i)(?:^|[\s/,])(\d+(?:\.\d+)?)\s*(k?)V(?:$|[\s/,])").captures(s) {
        if let Ok(n) = c[1].parse::<f64>() {
            out.voltage = Some(n * if c[2].eq_ignore_ascii_case("k") { 1e3 } else { 1.0 });
        }
    }
    if let Some(c) = re!(r"(?:^|[\s/,±(])(\d+(?:\.\d+)?)\s*%").captures(s) {
        if let Ok(n) = c[1].parse::<f64>() {
            out.tolerance_pct = Some(n);
        }
    }
    out
}

/// Format a magnitude as the grouping key both runtimes use (`%.9g` in Python).
pub fn mag_key(v: f64, sig: usize) -> String {
    let s = format!("{:.*e}", sig.saturating_sub(1), v);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eng_values_normalize() {
        let cases: &[(&str, f64)] = &[
            ("470nF", 470e-9),
            ("0.47uF", 470e-9),
            ("4u7", 4.7e-6),
            ("2K2", 2200.0),
            ("0R", 0.0),
            ("470R", 470.0),
            ("4R7", 4.7),
            ("22p", 22e-12),
            ("8MHz", 8e6),
            ("6.8 uH", 6.8e-6),
            ("10k", 10000.0),
            ("1m", 1e-3),
            ("1M", 1e6),
        ];
        for (raw, want) in cases {
            let got = normalize_eng_value(raw).unwrap_or_else(|| panic!("{raw} did not parse"));
            assert!((got - want).abs() <= want.abs() * 1e-9 + 1e-15, "{raw}: {got} != {want}");
        }
        // Labels and part names are not values.
        for raw in ["USB1", "", "LDO_3V3", "TBD"] {
            assert!(normalize_eng_value(raw).is_none(), "{raw} should not parse");
        }
    }

    #[test]
    fn equivalent_spellings_share_a_key() {
        let a = normalize_eng_value("470nF").unwrap();
        let b = normalize_eng_value("0.47uF").unwrap();
        assert_eq!(mag_key(a, 9), mag_key(b, 9));
    }

    #[test]
    fn aux_specs_extract() {
        let a = extract_aux_specs("0.1uF 100V 10%");
        assert_eq!(a.voltage, Some(100.0));
        assert_eq!(a.tolerance_pct, Some(10.0));
        assert_eq!(extract_aux_specs("1uF/50V").voltage, Some(50.0));
        assert!(extract_aux_specs("10k").is_empty());
    }
}
