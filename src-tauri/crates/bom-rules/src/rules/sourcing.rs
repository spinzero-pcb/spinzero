//! Sourcing rules — can each part actually be bought, and is its identity stable?
//!
//! These are the coverage-gated ones: a BOM that simply doesn't track MPNs is ONE
//! process gap, not one finding per row. Per-item findings appear only when most
//! parts carry the data and a few slipped through — those are the real mistakes.

use std::collections::{BTreeMap, BTreeSet};

use super::SECTION_SOURCING;
use crate::model::{refs_of, systemic, BomItem, Ctx, Severity};
use crate::{re, Raw, Rule};

/// Vocabulary that means "nothing is here yet" wherever it appears in an id field.
pub(crate) const PH_TOKENS: &[&str] = &[
    "tbd", "tba", "tbc", "generic", "na", "none", "null", "nil", "xxx", "placeholder",
    "todo", "fixme", "unknown", "dnp", "example", "sample",
];

/// Strip punctuation/space and lower-case, so "N/A", "n.a." and "NA" are one token.
pub(crate) fn canon_placeholder(v: &str) -> String {
    re!(r"[\s\-_/.\\]+").replace_all(v.trim(), "").to_lowercase()
}

/// The sourcing identity a part is grouped by: its MPN, else its first distributor
/// PN. Placeholder ids (N/A, TBD) group unrelated parts, so they are never identity.
pub(crate) fn pn_identifier(item: &BomItem) -> Option<(String, String)> {
    if item.filled("mpn") {
        let pn = item.mpn().trim();
        if PH_TOKENS.contains(&canon_placeholder(pn).as_str()) {
            return None;
        }
        return Some(("MPN".to_string(), pn.to_lowercase()));
    }
    let (supplier, pn) = item.supplier_pns.first()?;
    if PH_TOKENS.contains(&canon_placeholder(pn).as_str()) {
        return None;
    }
    Some((supplier.clone(), pn.trim().to_lowercase()))
}

pub(crate) fn ident_label(ident: &(String, String)) -> String {
    format!("{} {}", ident.0, ident.1)
}

/// No MPN and no distributor PN on a populated part — it cannot be ordered or traced.
pub struct MissingSourcingId;

impl Rule for MissingSourcingId {
    fn id(&self) -> &'static str {
        "bom.missing_sourcing_id"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let eligible: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.dnp && !i.non_orderable())
            .collect();
        let missing: Vec<&BomItem> = eligible
            .iter()
            .copied()
            .filter(|i| !(i.filled("mpn") || i.filled("mpn_alt") || !i.supplier_pns.is_empty()))
            .collect();
        if missing.is_empty() {
            return Vec::new();
        }
        if systemic(missing.len(), eligible.len(), ctx, 0.35, 8) {
            return vec![Raw::new(
                Severity::Major,
                format!(
                    "BOM does not track sourcing identifiers ({} of {} parts have no MPN or \
                     distributor part number)",
                    missing.len(),
                    eligible.len()
                ),
            )
            .detail(
                "Most populated parts carry no manufacturer or distributor part number — sourcing \
                 is not tracked in this BOM, so parts cannot be ordered or traced from it.",
            )
            .fix(
                "Add manufacturer + MPN (or distributor part number) columns and populate them \
                 before releasing the BOM for procurement.",
            )
            .evidence(format!("Affected: {}", refs_of(&missing, 12).join(", ")))
            .key("systemic")];
        }
        missing
            .iter()
            .map(|item| {
                Raw::new(
                    ctx.severity,
                    "No sourcing identifier (no MPN, no distributor part number)",
                )
                .detail(format!(
                    "Part {} has neither a manufacturer part number nor any distributor part \
                     number, while most other parts in this BOM do — likely an oversight.",
                    item.label()
                ))
                .fix("Add a manufacturer part number (with manufacturer) or a distributor part number.")
                .item(item)
                .key("no_sourcing_id")
            })
            .collect()
    }
}

/// Only a distributor PN present, no manufacturer PN — sourcing breaks if the
/// distributor delists or renumbers.
pub struct DistributorPnOnlyNoMpn;

impl Rule for DistributorPnOnlyNoMpn {
    fn id(&self) -> &'static str {
        "bom.distributor_pn_only_no_mpn"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let eligible: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.supplier_pns.is_empty() || i.filled("mpn") || i.filled("mpn_alt"))
            .collect();
        let hits: Vec<&BomItem> = eligible
            .iter()
            .copied()
            .filter(|i| !i.supplier_pns.is_empty() && !(i.filled("mpn") || i.filled("mpn_alt")))
            .collect();
        if hits.is_empty() {
            return Vec::new();
        }
        if systemic(hits.len(), eligible.len(), ctx, 0.35, 8) {
            let suppliers: BTreeSet<&str> = hits
                .iter()
                .flat_map(|i| i.supplier_pns.iter().map(|(s, _)| s.as_str()))
                .collect();
            let suppliers: Vec<&str> = suppliers.into_iter().collect();
            return vec![Raw::new(
                ctx.severity,
                format!(
                    "BOM sources parts by distributor part number only ({} of {} sourced parts \
                     have no MPN)",
                    hits.len(),
                    eligible.len()
                ),
            )
            .detail(format!(
                "Sourcing in this BOM relies on distributor part numbers ({}) without \
                 manufacturer part numbers; sourcing breaks if the distributor delists or \
                 renumbers.",
                suppliers.join(", ")
            ))
            .fix("Add manufacturer and manufacturer part number columns for stable sourcing identity.")
            .evidence(format!("Affected: {}", refs_of(&hits, 12).join(", ")))
            .key("systemic")];
        }
        hits.iter()
            .map(|item| {
                let listed: Vec<String> = item
                    .supplier_pns
                    .iter()
                    .map(|(s, pn)| format!("{s}:{pn}"))
                    .collect();
                Raw::new(
                    ctx.severity,
                    "Distributor part number only; no manufacturer part number",
                )
                .detail(format!(
                    "Part {} is sourced only by distributor PN ({}); sourcing breaks if the \
                     distributor delists or renumbers.",
                    item.label(),
                    listed.join(", ")
                ))
                .fix("Add manufacturer and manufacturer part number for stable sourcing identity.")
                .item(item)
                .key("distributor_only")
            })
            .collect()
    }
}

/// Manufacturer present without MPN, or MPN present without manufacturer.
pub struct ManufacturerMpnUnpaired;

impl Rule for ManufacturerMpnUnpaired {
    fn id(&self) -> &'static str {
        "bom.manufacturer_mpn_unpaired"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let eligible: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.non_orderable())
            .collect();
        let mut missing_mpn: Vec<&BomItem> = Vec::new();
        let mut missing_mfr: Vec<&BomItem> = Vec::new();
        for item in &eligible {
            match (item.filled("manufacturer"), item.filled("mpn")) {
                (true, false) => missing_mpn.push(item),
                (false, true) => missing_mfr.push(item),
                _ => {}
            }
        }

        let mut out = Vec::new();
        for (missing_side, present_side, items) in [
            ("MPN", "manufacturer", &missing_mpn),
            ("manufacturer", "MPN", &missing_mfr),
        ] {
            if items.is_empty() {
                continue;
            }
            if systemic(items.len(), eligible.len(), ctx, 0.35, 8) {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "BOM tracks {present_side} but not {missing_side} ({} of {} parts \
                             affected)",
                            items.len(),
                            eligible.len()
                        ),
                    )
                    .detail(format!(
                        "Parts across this BOM carry a {present_side} without a {missing_side}; \
                         the two should travel together for unambiguous sourcing."
                    ))
                    .fix(format!(
                        "Add a {missing_side} column and populate it for all sourced parts."
                    ))
                    .evidence(format!("Affected: {}", refs_of(items, 12).join(", ")))
                    .key(format!("systemic:{missing_side}")),
                );
                continue;
            }
            for item in items.iter() {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "Manufacturer/MPN unpaired: has {present_side}, missing {missing_side}"
                        ),
                    )
                    .detail(format!(
                        "Part {} has {present_side} but no {missing_side}; the two should travel \
                         together.",
                        item.label()
                    ))
                    .fix(format!(
                        "Add the {missing_side} so manufacturer and part number form a complete pair."
                    ))
                    .item(item)
                    .key(format!("unpaired:{missing_side}")),
                );
            }
        }
        out
    }
}

/// Distributor PN doesn't match its expected pattern (an LCSC code is `C` + digits).
pub struct SupplierPnFormat;

impl Rule for SupplierPnFormat {
    fn id(&self) -> &'static str {
        "bom.supplier_pn_format"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let Some(patterns) = ctx.param_map("patterns") else {
            return Vec::new();
        };
        // A profile's pattern is user data — an unparseable one is skipped, never fatal.
        let compiled: BTreeMap<String, (regex::Regex, String)> = patterns
            .into_iter()
            .filter_map(|(s, p)| regex::Regex::new(&p).ok().map(|rx| (s, (rx, p))))
            .collect();

        let mut out = Vec::new();
        for item in &ctx.items {
            for (supplier, cell) in &item.supplier_pns {
                let Some((rx, pattern)) = compiled.get(supplier) else {
                    continue;
                };
                // A cell may list alternates ("C106200,C1525"); check each token.
                let bad: Vec<&str> = re!(r"[,;]\s*")
                    .split(cell.trim())
                    .filter(|pn| !pn.is_empty() && !rx.is_match(pn))
                    .collect();
                if bad.is_empty() {
                    continue;
                }
                let pn = bad.join(", ");
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("{supplier} part number format looks off: {pn}"),
                    )
                    .detail(format!(
                        "{supplier} PN '{pn}' on {} does not match the expected pattern \
                         '{pattern}'.",
                        item.label()
                    ))
                    .fix(format!(
                        "Verify the {supplier} part number; it may be mistyped or in the wrong column."
                    ))
                    .item(item)
                    .key(format!("{supplier}:{pn}")),
                );
            }
        }
        out
    }
}

/// No datasheet reference. Off by default — the paid review resolves datasheets
/// itself; on in the medical profile, where the dossier must cite one per part.
pub struct MissingDatasheet;

impl Rule for MissingDatasheet {
    fn id(&self) -> &'static str {
        "bom.missing_datasheet"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let eligible: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.dnp && !i.non_orderable())
            .collect();
        let missing: Vec<&BomItem> = eligible
            .iter()
            .copied()
            .filter(|i| !i.filled("datasheet"))
            .collect();
        if missing.is_empty() {
            return Vec::new();
        }
        // Datasheets are rarely tracked exhaustively; itemize only when the BOM
        // clearly tries to track them and a handful of parts slipped through.
        if systemic(missing.len(), eligible.len(), ctx, 0.15, 5) {
            return vec![Raw::new(
                ctx.severity,
                format!(
                    "Datasheet coverage is low ({} of {} parts have one)",
                    eligible.len() - missing.len(),
                    eligible.len()
                ),
            )
            .detail("Most parts in this BOM carry no datasheet reference.")
            .fix(
                "Populate datasheet URLs (at least for actives and critical passives) to aid \
                 review and verification.",
            )
            .evidence(format!("Affected: {}", refs_of(&missing, 12).join(", ")))
            .key("systemic")];
        }
        missing
            .iter()
            .map(|item| {
                Raw::new(ctx.severity, "Missing datasheet")
                    .detail(format!("Part {} has no datasheet reference.", item.label()))
                    .fix("Add a datasheet URL or document reference to aid review and verification.")
                    .item(item)
                    .key("missing_datasheet")
            })
            .collect()
    }
}

const DS_PLACEHOLDER: &[&str] = &[
    "tbd", "tba", "na", "none", "null", "todo", "fixme", "placeholder", "unknown", "xxx",
];

/// Datasheet filled but not usable: a local path, a bare scheme, or placeholder text.
/// Distinct from a *missing* datasheet — this one looks populated.
pub struct InvalidDatasheet;

impl InvalidDatasheet {
    fn diagnose(raw: &str, allowed: &[String], allow_local: bool) -> Option<String> {
        if DS_PLACEHOLDER.contains(&canon_placeholder(raw).as_str()) {
            return Some("is placeholder text, not a document reference".into());
        }
        if raw.starts_with("\\\\") || re!(r"^[A-Za-z]:[\\/]").is_match(raw) {
            return (!allow_local)
                .then(|| "is a local filesystem path, not a shareable reference".to_string());
        }
        if let Some(m) = re!(r"^([A-Za-z][A-Za-z0-9+.\-]*):").captures(raw) {
            let scheme = m[1].to_lowercase();
            if scheme == "file" {
                return (!allow_local)
                    .then(|| "is a local file: URL, not a shareable reference".to_string());
            }
            if !allowed.contains(&scheme) {
                return Some(format!("uses unsupported URL scheme '{scheme}'"));
            }
            let rest = &raw[m[0].len()..];
            if rest.trim_start_matches('/').is_empty() {
                return Some("is a bare URL scheme with no host or path".into());
            }
            return None;
        }
        if !allow_local && (raw.contains('\\') || raw.starts_with('/')) {
            return Some("looks like a local file path rather than a URL".into());
        }
        if !raw.contains('.') && !raw.contains('/') {
            return Some("is not a URL or document reference".into());
        }
        None
    }
}

impl Rule for InvalidDatasheet {
    fn id(&self) -> &'static str {
        "bom.invalid_datasheet"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let allowed: Vec<String> = ctx
            .param_strings("allowed_schemes")
            .unwrap_or_else(|| vec!["http".into(), "https".into()])
            .iter()
            .map(|s| s.trim().to_lowercase())
            .collect();
        let allow_local = ctx.param_bool("allow_local_paths", false);

        // Group by the *problem*: 40 parts sharing one broken library convention is
        // one finding, not 40.
        let mut by_problem: BTreeMap<String, Vec<(&BomItem, String)>> = BTreeMap::new();
        for item in &ctx.items {
            if !item.filled("datasheet") {
                continue;
            }
            let raw = item.datasheet().trim().to_string();
            if let Some(problem) = Self::diagnose(&raw, &allowed, allow_local) {
                by_problem.entry(problem).or_default().push((item, raw));
            }
        }

        let agg_min = ctx.param_usize("systemic_min", 4);
        let mut out = Vec::new();
        for (problem, hits) in by_problem {
            if hits.len() >= agg_min {
                let samples: Vec<String> = {
                    let uniq: BTreeSet<&str> = hits.iter().map(|(_, r)| r.as_str()).collect();
                    uniq.into_iter().take(3).map(String::from).collect()
                };
                let items: Vec<&BomItem> = hits.iter().map(|(i, _)| *i).collect();
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "{} datasheet references are not usable (each {problem})",
                            hits.len()
                        ),
                    )
                    .detail(format!(
                        "{} parts share the same datasheet problem — likely a library-wide \
                         convention.\n\nExamples:\n{}",
                        hits.len(),
                        samples.iter().map(|s| format!("  • {s}")).collect::<Vec<_>>().join("\n")
                    ))
                    .fix("Provide complete, resolvable https:// URLs to the manufacturers' documents.")
                    .evidence(format!("Affected: {}", refs_of(&items, 12).join(", ")))
                    .key(format!("systemic:{problem}")),
                );
                continue;
            }
            for (item, raw) in hits {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("Datasheet reference not usable: '{raw}'"),
                    )
                    .detail(format!("Part {} datasheet '{raw}' {problem}.", item.label()))
                    .fix("Provide a complete, resolvable https:// URL to the manufacturer's document.")
                    .item(item)
                    .key(format!("datasheet:{problem}")),
                );
            }
        }
        out
    }
}

/// TBD / N/A / ??? in an id field — looks populated but cannot be ordered.
pub struct PlaceholderPartNumber;

impl Rule for PlaceholderPartNumber {
    fn id(&self) -> &'static str {
        "bom.placeholder_part_number"
    }
    fn section(&self) -> &'static str {
        SECTION_SOURCING
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let fields = ctx
            .param_strings("fields")
            .unwrap_or_else(|| vec!["mpn".to_string()]);
        let tokens: Vec<String> = ctx
            .param_strings("tokens")
            .unwrap_or_else(|| PH_TOKENS.iter().map(|s| s.to_string()).collect())
            .iter()
            .map(|t| canon_placeholder(t))
            .filter(|t| !t.is_empty())
            .collect();

        let mut out = Vec::new();
        for item in &ctx.items {
            for field in &fields {
                if !item.filled(field) {
                    continue;
                }
                let raw = item.field(field).trim();
                let is_token = tokens.contains(&canon_placeholder(raw));
                let is_structural = re!(r"(?i)^[?\-_*x×#~.]+$").is_match(raw);
                if !(is_token || is_structural) {
                    continue;
                }
                out.push(
                    Raw::new(ctx.severity, format!("Placeholder {field}: '{raw}'"))
                        .detail(format!(
                            "Part {} has '{raw}' in {field}; it looks populated but cannot be \
                             purchased or verified.",
                            item.label()
                        ))
                        .fix(format!(
                            "Replace the placeholder with the actual {field}, or clear it so it \
                             reports as genuinely missing."
                        ))
                        .item(item)
                        .key(format!("{field}:{}", raw.to_lowercase())),
                );
            }
        }
        out
    }
}
