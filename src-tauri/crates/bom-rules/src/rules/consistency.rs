//! Consistency rules — the BOM must not contradict itself: one part number is one
//! part, one value is spelled one way, a quantity matches its designators.

use std::collections::{BTreeMap, BTreeSet};

use super::sourcing::{ident_label, pn_identifier};
use super::SECTION_CONSISTENCY;
use crate::model::{refs_of, BomItem, Ctx};
use crate::value::{mag_key, normalize_eng_value};
use crate::{re, Raw, Rule};

/// The component class a refdes prefix implies.
fn prefix_class(ctx: &Ctx, prefix: &str) -> Option<String> {
    const DEFAULTS: &[(&str, &str)] = &[
        ("R", "resistor"), ("RN", "resistor"), ("RV", "resistor"),
        ("C", "capacitor"),
        ("L", "inductor"), ("FB", "inductor"),
        ("D", "diode"), ("LED", "diode"), ("CR", "diode"),
        ("Q", "transistor"),
        ("U", "ic"), ("IC", "ic"),
        ("Y", "crystal"), ("X", "crystal"), ("XTAL", "crystal"),
        ("J", "connector"), ("P", "connector"), ("CN", "connector"), ("CON", "connector"),
        ("SW", "switch"), ("F", "fuse"), ("T", "transformer"),
        ("TP", "testpoint"), ("H", "mounting"), ("MH", "mounting"), ("MK", "mounting"),
        ("BT", "battery"), ("BAT", "battery"),
    ];
    let prefix = prefix.to_uppercase();
    if let Some(map) = ctx.param_map("prefix_classes") {
        return map
            .into_iter()
            .find(|(k, _)| k.to_uppercase() == prefix)
            .map(|(_, v)| v.to_lowercase());
    }
    DEFAULTS
        .iter()
        .find(|(k, _)| *k == prefix)
        .map(|(_, v)| (*v).to_string())
}

/// Same value written in different notation ("470nF" vs "0.47uF") or case/spacing —
/// the lines don't group, so procurement buys the same part twice.
pub struct ValueFormatConsistency;

impl ValueFormatConsistency {
    fn class_of(item: &BomItem) -> String {
        re!(r"^([A-Za-z]+)")
            .captures(item.reference.trim())
            .map(|c| c[1].to_uppercase())
            .unwrap_or_default()
    }

    fn flag<'a>(
        groups: impl Iterator<Item = Vec<(&'a BomItem, String)>>,
        ctx: &Ctx,
        kind: &str,
    ) -> Vec<Raw> {
        let mut out = Vec::new();
        for members in groups {
            let spellings: BTreeSet<&str> = members.iter().map(|(_, r)| r.as_str()).collect();
            if spellings.len() <= 1 {
                continue;
            }
            // The most common spelling is the one everything else should become.
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for (_, raw) in &members {
                *counts.entry(raw.as_str()).or_insert(0) += 1;
            }
            let canonical = counts
                .iter()
                .max_by_key(|(_, n)| **n)
                .map(|(s, _)| (*s).to_string())
                .unwrap_or_default();
            for (item, raw) in &members {
                if *raw == canonical {
                    continue;
                }
                let reason = if kind == "notation" {
                    format!("resolves to the same magnitude as '{canonical}' but uses different notation")
                } else {
                    format!("differs only in case/spacing from '{canonical}'")
                };
                out.push(
                    Raw::new(ctx.severity, format!("Inconsistent value spelling: '{raw}'"))
                        .detail(format!("Value '{raw}' on {} {reason}.", item.label()))
                        .fix(format!("Normalize to '{canonical}' for consistent grouping."))
                        .evidence(format!(
                            "Spellings in this group: {}",
                            spellings.iter().copied().collect::<Vec<_>>().join(", ")
                        ))
                        .item(item)
                        .key(format!("{kind}:{raw}")),
                );
            }
        }
        out
    }
}

impl Rule for ValueFormatConsistency {
    fn id(&self) -> &'static str {
        "bom.value_format_consistency"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        // Values that parse as magnitudes group by class+magnitude; everything else
        // groups by squashed text (so only case/spacing differences are flagged).
        let mut eng: BTreeMap<(String, String), Vec<(&BomItem, String)>> = BTreeMap::new();
        let mut text: BTreeMap<String, Vec<(&BomItem, String)>> = BTreeMap::new();
        for item in &ctx.items {
            if !item.filled("value") {
                continue;
            }
            let raw = item.value().trim().to_string();
            match normalize_eng_value(&raw) {
                Some(mag) => eng
                    .entry((Self::class_of(item), mag_key(mag, 9)))
                    .or_default()
                    .push((item, raw)),
                None => text
                    .entry(raw.split_whitespace().collect::<String>().to_lowercase())
                    .or_default()
                    .push((item, raw)),
            }
        }
        let mut out = Self::flag(eng.into_values(), ctx, "notation");
        out.extend(Self::flag(text.into_values(), ctx, "case"));
        out
    }
}

/// Parts sharing a sourcing ID disagree on value or footprint — the same part number
/// cannot be several different parts.
pub struct InconsistentFieldsSameMpn;

impl Rule for InconsistentFieldsSameMpn {
    fn id(&self) -> &'static str {
        "bom.inconsistent_fields_same_mpn"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }
    /// One MPN with two footprints is a contradiction in the data itself — it stays
    /// wrong if a variant ever populates the DNP line, so DNP lines count here.
    fn scans_dnp(&self) -> bool {
        true
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let compare: Vec<String> = ctx
            .param_strings("compare_fields")
            .unwrap_or_else(|| vec!["value".into(), "footprint".into()]);
        let mut groups: BTreeMap<(String, String), Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            if let Some(ident) = pn_identifier(item) {
                groups.entry(ident).or_default().push(item);
            }
        }

        let mut out = Vec::new();
        for (ident, members) in groups {
            if members.len() < 2 {
                continue;
            }
            let label = ident_label(&ident);
            for field in &compare {
                let mut spellings: BTreeMap<String, Vec<&BomItem>> = BTreeMap::new();
                for m in &members {
                    if m.filled(field) {
                        spellings
                            .entry(m.field(field).trim().to_string())
                            .or_default()
                            .push(m);
                    }
                }
                if spellings.len() <= 1 {
                    continue;
                }
                if field == "value" {
                    let mags: Vec<Option<f64>> =
                        spellings.keys().map(|v| normalize_eng_value(v)).collect();
                    let parsed: BTreeSet<String> = mags
                        .iter()
                        .filter_map(|m| m.map(|v| mag_key(v, 9)))
                        .collect();
                    // Differently spelled but electrically identical → not a defect.
                    if !mags.iter().any(|m| m.is_none()) && parsed.len() == 1 {
                        continue;
                    }
                    // Label-like values ("USB1".."USB7" on identical connectors): Value
                    // is routinely a functional label, not a spec. Only flag those when
                    // the footprints disagree too.
                    if mags.iter().any(|m| m.is_none()) {
                        let fps: BTreeSet<String> = members
                            .iter()
                            .filter(|m| m.filled("footprint"))
                            .map(|m| m.footprint().trim().to_lowercase())
                            .collect();
                        if fps.len() <= 1 {
                            continue;
                        }
                    }
                }
                let detail: Vec<String> = spellings
                    .iter()
                    .map(|(v, its)| {
                        let refs: Vec<&str> = its
                            .iter()
                            .take(6)
                            .map(|i| i.reference.as_str())
                            .collect();
                        format!("'{v}' on {}", refs.join(", "))
                    })
                    .collect();
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("Parts sharing {label} disagree on {field}"),
                    )
                    .detail(format!(
                        "{} parts share {label} but carry {} different {field}s: {}. The same \
                         part number cannot be several different parts — some of these lines \
                         have a wrong value or a wrong PN.",
                        members.len(),
                        spellings.len(),
                        detail.join("; ")
                    ))
                    .fix(format!(
                        "Parts with the same part number must have identical {field}; reconcile \
                         the mismatch."
                    ))
                    .refdes(members.iter().filter(|m| !m.reference.is_empty()).map(|m| m.reference.clone()))
                    .key(format!("{label}:{field}")),
                );
            }
        }
        out
    }
}

/// Refdes prefix, footprint, and value unit signal different component classes —
/// a 10uF value on a resistor footprint, C20 on an inductor.
pub struct ComponentClassConsistency;

impl ComponentClassConsistency {
    fn value_class(value: &str) -> Option<&'static str> {
        let s = value.trim().to_lowercase().replace(['µ', 'μ'], "u");
        if s.is_empty() {
            return None;
        }
        if re!(r"(?i)^\d*\.?\d+\s*[pnumkmgµμ]?\s*hz$").is_match(&s) {
            return Some("crystal");
        }
        if re!(r"(?i)^\d*\.?\d+\s*[pnumkmgµμ]?\s*f$").is_match(&s) {
            return Some("capacitor");
        }
        if re!(r"(?i)^\d*\.?\d+\s*[pnumkmgµμ]?\s*h$").is_match(&s) {
            return Some("inductor");
        }
        if s.replace('ω', "ohm").contains("ohm") {
            return Some("resistor");
        }
        None
    }

    /// Footprint NAME patterns, library-agnostic: KiCad `C_0402_1005Metric`, easyeda
    /// `C0402`, house styles `1005_C` / `R_1005_C`.
    fn footprint_class(footprint: &str, classes: &[(String, String)]) -> Option<String> {
        let fp = footprint.trim().to_lowercase();
        let (lib, name) = match fp.rsplit_once(':') {
            Some((l, n)) => (l.to_string(), n.to_string()),
            None => (String::new(), fp.clone()),
        };
        for (kw, class) in classes {
            if lib.starts_with(kw.as_str()) {
                return Some(class.clone());
            }
        }
        const NAME_PATTERNS: &[(&str, &str)] = &[
            (r"^cp?_?\d", "capacitor"),
            (r"^r_?\d", "resistor"),
            (r"^l_?\d", "inductor"),
            (r"^(?:d|led)_?\d|^led[_-]", "diode"),
            (r"^sw[_-]", "switch"),
            (r"_c$", "capacitor"),
            (r"_r$", "resistor"),
            (r"_l$", "inductor"),
        ];
        for (pattern, class) in NAME_PATTERNS {
            if regex::Regex::new(pattern)
                .map(|rx| rx.is_match(&name))
                .unwrap_or(false)
            {
                return Some((*class).to_string());
            }
        }
        None
    }
}

impl Rule for ComponentClassConsistency {
    fn id(&self) -> &'static str {
        "bom.component_class_consistency"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        const DEFAULT_FP_CLASSES: &[(&str, &str)] = &[
            ("resistor", "resistor"), ("capacitor", "capacitor"), ("inductor", "inductor"),
            ("ferrite", "inductor"), ("choke", "inductor"), ("crystal", "crystal"),
            ("oscillator", "crystal"), ("resonator", "crystal"), ("led", "diode"),
            ("diode", "diode"), ("connector", "connector"), ("mountinghole", "mounting"),
            ("testpoint", "testpoint"), ("fuse", "fuse"), ("transformer", "transformer"),
            ("button", "switch"), ("switch", "switch"),
        ];
        let fp_classes: Vec<(String, String)> = ctx
            .param_map("footprint_classes")
            .map(|m| {
                m.into_iter()
                    .map(|(k, v)| (k.to_lowercase(), v.to_lowercase()))
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_FP_CLASSES
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect()
            });

        // Signature = the exact set of contradicting signals, so a design that
        // systematically puts capacitors on Resistor_SMD footprints is ONE library
        // convention to confirm, not N mistakes.
        let mut groups: BTreeMap<Vec<(String, String)>, Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            let mut signals: Vec<(String, String)> = Vec::new();
            if let Some(m) = re!(r"^([A-Za-z]+)").captures(item.reference.trim()) {
                if let Some(c) = prefix_class(ctx, &m[1]) {
                    signals.push(("reference".into(), c));
                }
            }
            if item.filled("footprint") {
                if let Some(c) = Self::footprint_class(item.footprint(), &fp_classes) {
                    signals.push(("footprint".into(), c));
                }
            }
            if item.filled("value") {
                if let Some(c) = Self::value_class(item.value()) {
                    signals.push(("value".into(), c.to_string()));
                }
            }
            let distinct: BTreeSet<&str> = signals.iter().map(|(_, c)| c.as_str()).collect();
            if distinct.len() >= 2 {
                signals.sort();
                groups.entry(signals).or_default().push(item);
            }
        }

        let agg_min = ctx.param_usize("systemic_min", 4);
        let mut out = Vec::new();
        for (signature, members) in groups {
            let detail: Vec<String> = signature
                .iter()
                .map(|(src, class)| format!("{src} says {class}"))
                .collect();
            let detail = detail.join(", ");
            let signature_key: Vec<String> = signature
                .iter()
                .map(|(s, c)| format!("{s}={c}"))
                .collect();
            let signature_key = signature_key.join(",");
            if members.len() >= agg_min {
                let sample = members[0];
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "Component class mismatch on {} parts ({detail})",
                            members.len()
                        ),
                    )
                    .detail(format!(
                        "{} parts share the same class contradiction ({detail}) — e.g. {}: value \
                         '{}' on footprint '{}'. Likely a deliberate library convention; verify once.",
                        members.len(),
                        sample.label(),
                        sample.value(),
                        sample.footprint()
                    ))
                    .fix(
                        "Confirm the footprint choice is intentional for these parts, or reassign \
                         the correct footprint class.",
                    )
                    .evidence(format!("Affected: {}", refs_of(&members, 12).join(", ")))
                    .key(format!("systemic:{signature_key}")),
                );
                continue;
            }
            for item in members {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("Component class mismatch on {}", item.label()),
                    )
                    .detail(format!(
                        "Signals on {} disagree: {detail}. Value '{}' on footprint '{}'.",
                        item.label(),
                        item.value(),
                        item.footprint()
                    ))
                    .fix(
                        "Reconcile refdes, footprint, and value so they all describe the same \
                         component class.",
                    )
                    .item(item)
                    .key(signature_key.clone()),
                );
            }
        }
        out
    }
}

/// Declared quantity doesn't match the number of reference designators on the line.
pub struct GroupedQtyVsCount;

impl Rule for GroupedQtyVsCount {
    fn id(&self) -> &'static str {
        "bom.grouped_qty_vs_count"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        ctx.items
            .iter()
            .filter_map(|item| {
                let qty = item.quantity_int?;
                let n = item.refs.len() as i64;
                if n == 0 || qty == n {
                    return None;
                }
                Some(
                    Raw::new(
                        ctx.severity,
                        format!("Quantity ({qty}) does not match designator count ({n})"),
                    )
                    .detail(format!(
                        "BOM line {} declares quantity {qty} but lists {n} reference designator(s).",
                        item.label()
                    ))
                    .fix("Reconcile the quantity with the number of reference designators.")
                    .evidence(format!("Designators: {}", item.refs.join(", ")))
                    .item(item)
                    .key("qty_vs_refdes"),
                )
            })
            .collect()
    }
}

/// Same PN split across lines because the value was spelled differently.
pub struct DuplicateLineItemsSamePn;

impl Rule for DuplicateLineItemsSamePn {
    fn id(&self) -> &'static str {
        "bom.duplicate_line_items_same_pn"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut groups: BTreeMap<(String, String), Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            if let Some(ident) = pn_identifier(item) {
                groups.entry(ident).or_default().push(item);
            }
        }
        let mut out = Vec::new();
        for (ident, members) in groups {
            let spellings: BTreeSet<String> = members
                .iter()
                .filter(|m| m.filled("value"))
                .map(|m| m.value().trim().to_string())
                .collect();
            if spellings.len() < 2 {
                continue;
            }
            let mags: Vec<Option<f64>> = spellings.iter().map(|v| normalize_eng_value(v)).collect();
            // Only equivalent-but-differently-spelled values are a split line; genuinely
            // different values are a different rule's problem.
            if mags.iter().any(|m| m.is_none()) {
                continue;
            }
            let distinct: BTreeSet<String> =
                mags.iter().filter_map(|m| m.map(|v| mag_key(v, 9))).collect();
            if distinct.len() != 1 {
                continue;
            }
            let label = ident_label(&ident);
            let spelled: Vec<String> = spellings.iter().cloned().collect();
            out.push(
                Raw::new(
                    ctx.severity,
                    format!("Same part ({label}) split into separate lines by value spelling"),
                )
                .detail(format!(
                    "Part {label} appears on multiple lines with equivalent but \
                     differently-spelled values ({}). It should be a single line item.",
                    spelled.join(", ")
                ))
                .fix("Use one consistent value spelling so the lines merge into a single grouped item.")
                .refdes(members.iter().filter(|m| !m.reference.is_empty()).map(|m| m.reference.clone()))
                .key(label),
            );
        }
        out
    }
}

/// Multiple part numbers for functionally equivalent parts (same value + footprint).
pub struct RedundantMpnSameValue;

impl Rule for RedundantMpnSameValue {
    fn id(&self) -> &'static str {
        "bom.redundant_mpn_same_value"
    }
    fn section(&self) -> &'static str {
        SECTION_CONSISTENCY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let compare: Vec<String> = ctx
            .param_strings("compare_fields")
            .unwrap_or_else(|| vec!["value".into(), "footprint".into()]);
        let key_part = |field: &str, raw: &str| -> String {
            if field == "value" {
                if let Some(mag) = normalize_eng_value(raw) {
                    return format!("#{}", mag_key(mag, 6));
                }
            }
            raw.trim().to_lowercase()
        };

        let mut groups: BTreeMap<Vec<String>, Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            if item.dnp || pn_identifier(item).is_none() {
                continue;
            }
            if compare.iter().any(|f| !item.filled(f)) {
                continue;
            }
            let key: Vec<String> = compare
                .iter()
                .map(|f| key_part(f, item.field(f)))
                .collect();
            groups.entry(key).or_default().push(item);
        }

        let mut out = Vec::new();
        for members in groups.into_values() {
            let idents: BTreeSet<String> = members
                .iter()
                .filter_map(|m| pn_identifier(m).map(|i| ident_label(&i)))
                .collect();
            if idents.len() < 2 {
                continue;
            }
            let sample = members[0];
            let descriptor: Vec<String> = compare
                .iter()
                .map(|f| format!("{f}={}", sample.field(f).trim()))
                .collect();
            let idents: Vec<String> = idents.into_iter().collect();
            out.push(
                Raw::new(ctx.severity, "Multiple part numbers for functionally equivalent parts")
                    .detail(format!(
                        "Parts that look equivalent ({}) use {} different part numbers: {}.",
                        descriptor.join(", "),
                        idents.len(),
                        idents.join(", ")
                    ))
                    .fix(
                        "If they are truly interchangeable, standardize on one PN. If they differ \
                         by tolerance/rating/grade, capture that in the BOM.",
                    )
                    .refdes(members.iter().filter(|m| !m.reference.is_empty()).map(|m| m.reference.clone()))
                    .key(idents.join("|")),
            );
        }
        out
    }
}
