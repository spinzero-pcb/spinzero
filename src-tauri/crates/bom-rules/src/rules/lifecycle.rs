//! Lifecycle, handling and compliance rules.
//!
//! Each distinguishes two different claims that share a rule: "this part is EOL"
//! (blocks the build, uses `severity`) and "this BOM has no lifecycle column"
//! (unfinished homework a distributor lookup closes, uses `missing_severity`).

use std::collections::BTreeMap;

use super::SECTION_LIFECYCLE;
use crate::model::{refs_of, BomItem, Ctx, Severity};
use crate::{re, Raw, Rule};

const LC_DEFAULT_FLAGGED: &[&str] = &[
    "obsolete", "nrnd", "not recommended", "not recommended for new designs", "eol",
    "end of life", "end-of-life", "last time buy", "last-time-buy", "ltb", "discontinued",
];

/// Part is NRND / EOL / obsolete / discontinued — or the BOM can't say either way.
pub struct LifecycleStatus;

impl Rule for LifecycleStatus {
    fn id(&self) -> &'static str {
        "bom.lifecycle_status"
    }
    fn section(&self) -> &'static str {
        SECTION_LIFECYCLE
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let flagged: Vec<String> = ctx
            .param_strings("flag_statuses")
            .unwrap_or_else(|| LC_DEFAULT_FLAGGED.iter().map(|s| s.to_string()).collect())
            .iter()
            .map(|s| s.trim().to_lowercase())
            .collect();

        if !ctx.all_items.iter().any(|i| i.filled("lifecycle")) {
            return vec![Raw::new(
                ctx.missing_sev(Severity::Low),
                "Lifecycle status not verifiable from BOM",
            )
            .detail(
                "The BOM has no lifecycle column; it cannot confirm that all parts are active \
                 (not obsolete / NRND / EOL).",
            )
            .fix(
                "Add a lifecycle status column (from distributor/manufacturer data) and confirm \
                 no part is obsolete, NRND, or end-of-life before release.",
            )
            .key("no_lifecycle_column")];
        }

        let mut by_status: BTreeMap<String, Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            if !item.filled("lifecycle") {
                continue;
            }
            let status = item.lifecycle().trim().to_string();
            let lower = status.to_lowercase();
            if flagged.iter().any(|t| lower.contains(t.as_str())) {
                by_status.entry(status).or_default().push(item);
            }
        }

        let agg_min = ctx.param_usize("systemic_min", 4);
        let mut out = Vec::new();
        for (status, items) in by_status {
            if items.len() >= agg_min {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "{} parts have lifecycle '{status}' (not active)",
                            items.len()
                        ),
                    )
                    .detail(format!(
                        "{} parts report lifecycle '{status}', indicating they are not \
                         recommended for new designs.",
                        items.len()
                    ))
                    .fix(
                        "Replace with active equivalents, or qualify alternates and confirm \
                         stock/last-time-buy quantities.",
                    )
                    .evidence(format!("Affected: {}", refs_of(&items, 12).join(", ")))
                    .key(format!("systemic:{status}")),
                );
                continue;
            }
            for item in items {
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("Part lifecycle is '{status}' (not active)"),
                    )
                    .detail(format!(
                        "Part {} reports lifecycle '{status}', indicating it is not recommended \
                         for new designs.",
                        item.label()
                    ))
                    .fix(
                        "Replace with an active equivalent, or qualify an alternate and confirm \
                         stock/last-time-buy quantities.",
                    )
                    .item(item)
                    .key(format!("lifecycle:{status}")),
                );
            }
        }
        out
    }
}

/// MSL (J-STD-020) absent on moisture-sensitive parts — the assembly house needs it
/// for storage and bake-before-reflow.
pub struct MissingMsl;

impl Rule for MissingMsl {
    fn id(&self) -> &'static str {
        "bom.missing_msl"
    }
    fn section(&self) -> &'static str {
        SECTION_LIFECYCLE
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let applies_to_all = ctx.param_bool("applies_to_all", false);
        let prefixes: Vec<String> = ctx
            .param_strings("applies_to_prefixes")
            .unwrap_or_default()
            .iter()
            .map(|p| p.trim().to_uppercase())
            .collect();
        let in_scope = |item: &BomItem| -> bool {
            if item.dnp || item.non_orderable() {
                return false;
            }
            if applies_to_all {
                return true;
            }
            let prefix = re!(r"^[A-Za-z]+")
                .find(item.reference.trim())
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            prefixes.contains(&prefix)
        };

        let scoped: Vec<&BomItem> = ctx.items.iter().copied().filter(|i| in_scope(i)).collect();
        if scoped.is_empty() {
            return Vec::new();
        }
        if !ctx.all_items.iter().any(|i| i.filled("msl")) {
            return vec![Raw::new(
                ctx.severity,
                "No MSL (moisture sensitivity level) data in BOM",
            )
            .detail(format!(
                "No MSL column; {} moisture-sensitive part(s) need an MSL rating for correct \
                 assembly-house storage and bake-before-reflow handling.",
                scoped.len()
            ))
            .fix("Add an MSL column (J-STD-020 level 1–6) for all moisture-sensitive parts.")
            .key("no_msl_column")];
        }

        let missing: Vec<&BomItem> = scoped
            .iter()
            .copied()
            .filter(|i| !i.filled("msl"))
            .collect();
        if !missing.is_empty() && crate::model::systemic(missing.len(), scoped.len(), ctx, 0.35, 8) {
            return vec![Raw::new(
                ctx.severity,
                format!(
                    "MSL not populated for most moisture-sensitive parts ({} of {})",
                    missing.len(),
                    scoped.len()
                ),
            )
            .detail("An MSL column exists but is largely unpopulated for moisture-sensitive parts.")
            .fix("Populate the MSL level (J-STD-020 level 1–6) for all in-scope parts.")
            .evidence(format!("Affected: {}", refs_of(&missing, 12).join(", ")))
            .key("systemic")];
        }
        missing
            .iter()
            .map(|item| {
                Raw::new(ctx.severity, "Missing MSL rating")
                    .detail(format!(
                        "Part {} has no MSL rating; the assembly house needs it for storage and \
                         bake-before-reflow.",
                        item.label()
                    ))
                    .fix("Populate the MSL level (J-STD-020 level 1–6) for this part.")
                    .item(item)
                    .key("missing_msl")
            })
            .collect()
    }
}

const AECQ_DEFAULT_NEGATIVE: &[&str] = &[
    "no", "n", "none", "n/a", "na", "not qualified", "not aec-q", "false", "0",
];

/// AEC-Q qualification absent or negative — enabled only in the automotive profile.
pub struct MissingAecq;

impl Rule for MissingAecq {
    fn id(&self) -> &'static str {
        "bom.missing_aecq"
    }
    fn section(&self) -> &'static str {
        SECTION_LIFECYCLE
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let negatives: Vec<String> = ctx
            .param_strings("negative_values")
            .unwrap_or_else(|| AECQ_DEFAULT_NEGATIVE.iter().map(|s| s.to_string()).collect())
            .iter()
            .map(|v| v.trim().to_lowercase())
            .collect();
        let populated: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.dnp && !i.non_orderable())
            .collect();
        if populated.is_empty() {
            return Vec::new();
        }
        if !ctx.all_items.iter().any(|i| i.filled("aecq")) {
            return vec![Raw::new(ctx.severity, "No AEC-Q qualification data in BOM")
                .detail(
                    "Automotive design; the BOM has no AEC-Q column — part qualification \
                     (Q100/Q101/Q200) cannot be confirmed.",
                )
                .fix(
                    "Add an AEC-Q column and confirm every part is automotive-qualified, or \
                     document an approved exception.",
                )
                .key("no_aecq_column")];
        }

        let hits: Vec<(&BomItem, String)> = populated
            .iter()
            .filter_map(|item| {
                let value = if item.filled("aecq") {
                    item.aecq().trim().to_string()
                } else {
                    String::new()
                };
                if !value.is_empty() && !negatives.contains(&value.to_lowercase()) {
                    return None;
                }
                Some((
                    *item,
                    if value.is_empty() { "(blank)".to_string() } else { value },
                ))
            })
            .collect();

        if hits.is_empty() {
            return Vec::new();
        }
        // One review point for the whole gap, listing every affected part: the
        // reviewer's decision ("qualify these parts, or record an exception") is the
        // same for all of them, so N identical comments would only be N times the noise.
        // The BOM marks every covered row, so the reviewer still sees each one in place.
        let refs: Vec<String> = hits
            .iter()
            .map(|(item, _)| item.reference.clone())
            .filter(|r| !r.is_empty())
            .collect();
        // Listed by part, not by designator: what gets qualified (or excepted) is an MPN,
        // and one MPN usually spans many rows. Designators ride along for the lookup.
        let mut by_part: Vec<(String, String, Vec<String>)> = Vec::new();
        for (item, value) in &hits {
            let part = if item.mpn().trim().is_empty() {
                item.label().to_string()
            } else {
                item.mpn().trim().to_string()
            };
            let refdes = item.label().to_string();
            match by_part.iter_mut().find(|(p, v, _)| p == &part && v == value) {
                Some((_, _, refs)) => refs.push(refdes),
                None => by_part.push((part, value.clone(), vec![refdes])),
            }
        }
        let listed = by_part
            .iter()
            .map(|(part, value, refs)| {
                // One part can span dozens of rows; the designators are a lookup aid, not
                // the point, so the line stays readable.
                let shown = refs.len().min(8);
                let mut list = refs[..shown].join(", ");
                if refs.len() > shown {
                    list.push_str(&format!(", +{} more", refs.len() - shown));
                }
                format!("  • {part} — AEC-Q: {value} ({list})")
            })
            .collect::<Vec<_>>()
            .join("\n");
        vec![Raw::new(
            ctx.severity,
            if hits.len() == 1 {
                "Part not AEC-Q qualified".to_string()
            } else {
                format!("{} parts not AEC-Q qualified", hits.len())
            },
        )
        .detail(format!(
            "{} of {} populated parts have no positive AEC-Q status in an automotive design.\n\nNot qualified:\n{listed}",
            hits.len(),
            populated.len()
        ))
        .fix(
            "Use AEC-Q-qualified equivalents, or record an approved exception with the qualification grade.",
        )
        .refdes(refs)
        .key("aecq")]
    }
}

/// RoHS / REACH status absent or explicitly non-compliant.
pub struct MissingCompliance;

impl Rule for MissingCompliance {
    fn id(&self) -> &'static str {
        "bom.missing_compliance"
    }
    fn section(&self) -> &'static str {
        SECTION_LIFECYCLE
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        const DEFAULT_NONCOMPLIANT: &[&str] = &[
            "no", "n", "non-compliant", "noncompliant", "not compliant", "fail", "false",
        ];
        let required: Vec<String> = ctx
            .param_strings("required")
            .unwrap_or_else(|| vec!["rohs".to_string()])
            .iter()
            .map(|f| f.trim().to_lowercase())
            .collect();
        let noncompliant: Vec<String> = ctx
            .param_strings("noncompliant_values")
            .unwrap_or_else(|| DEFAULT_NONCOMPLIANT.iter().map(|s| s.to_string()).collect())
            .iter()
            .map(|v| v.trim().to_lowercase())
            .collect();
        let populated: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.dnp && !i.non_orderable())
            .collect();
        if populated.is_empty() {
            return Vec::new();
        }
        let miss_sev = ctx.missing_sev(Severity::Medium);

        let mut out = Vec::new();
        for field in &required {
            let label = match field.as_str() {
                "rohs" => "RoHS".to_string(),
                "reach" => "REACH/SVHC".to_string(),
                other => other.to_uppercase(),
            };
            if !ctx.all_items.iter().any(|i| i.filled(field)) {
                out.push(
                    Raw::new(miss_sev, format!("No {label} compliance data in BOM"))
                        .detail(format!(
                            "The BOM has no {label} column; {label} compliance cannot be confirmed."
                        ))
                        .fix(format!(
                            "Add a {label} status column and populate it for every part."
                        ))
                        .key(format!("no_column:{field}")),
                );
                continue;
            }
            // A blank cell means "not looked up yet"; an explicit "No" means the part
            // fails. Same rule, two severities.
            let hits: Vec<(&BomItem, String, Severity)> = populated
                .iter()
                .filter_map(|item| {
                    let value = if item.filled(field) {
                        item.field(field).trim().to_string()
                    } else {
                        String::new()
                    };
                    if !value.is_empty() && !noncompliant.contains(&value.to_lowercase()) {
                        return None;
                    }
                    let blank = value.is_empty();
                    Some((
                        *item,
                        if blank { "(blank)".to_string() } else { value },
                        if blank { miss_sev } else { ctx.severity },
                    ))
                })
                .collect();

            if !hits.is_empty() && crate::model::systemic(hits.len(), populated.len(), ctx, 0.35, 8)
            {
                let agg_sev = if hits.iter().any(|(_, v, _)| v != "(blank)") {
                    ctx.severity
                } else {
                    miss_sev
                };
                let items: Vec<&BomItem> = hits.iter().map(|(i, _, _)| *i).collect();
                out.push(
                    Raw::new(
                        agg_sev,
                        format!(
                            "{label} status missing or negative on {} of {} parts",
                            hits.len(),
                            populated.len()
                        ),
                    )
                    .detail(format!(
                        "A {label} column exists but most parts have no positive {label} status."
                    ))
                    .fix(format!("Populate {label} status for every part."))
                    .evidence(format!("Affected: {}", refs_of(&items, 12).join(", ")))
                    .key(format!("systemic:{field}")),
                );
                continue;
            }
            for (item, shown, item_sev) in hits {
                out.push(
                    Raw::new(item_sev, format!("{label} status missing or non-compliant"))
                        .detail(format!(
                            "Part {} has {label} status '{shown}'.",
                            item.label()
                        ))
                        .fix(format!(
                            "Confirm {label} status; use a compliant alternate if the part fails."
                        ))
                        .item(item)
                        .key(format!("compliance:{field}")),
                );
            }
        }
        out
    }
}
