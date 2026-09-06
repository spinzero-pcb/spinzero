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

        // Same distinction the compliance rule draws: a missing column and an empty
        // one are different jobs for the engineer. See `Ctx::has_column`.
        if ctx.column_wholly_blank("lifecycle") {
            return vec![Raw::new(
                ctx.missing_sev(Severity::NonCritical),
                "Lifecycle column is empty on every line",
            )
            .detail(format!(
                "The BOM has a lifecycle column, but not one of the {} rows carries a value in \
                 it, so the BOM cannot confirm that all parts are active (not obsolete / NRND / \
                 EOL).",
                ctx.all_items.len()
            ))
            .fix(
                "Populate the lifecycle column that is already there, from distributor or \
                 manufacturer data, and confirm no part is obsolete, NRND, or end-of-life \
                 before release.",
            )
            .key("empty_lifecycle_column")];
        }
        if !ctx.has_column("lifecycle") {
            return vec![Raw::new(
                ctx.missing_sev(Severity::NonCritical),
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

/// What to call one BOM row in a finding title: its MPN where it has one, otherwise
/// its designators. A per-row finding names the part it is about and nothing else.
fn part_label(item: &BomItem) -> String {
    if item.mpn().trim().is_empty() {
        item.label().to_string()
    } else {
        item.mpn().trim().to_string()
    }
}

/// Every designator on one row, so the finding anchors to all the placements it
/// covers and not only to the first of them.
fn row_refs(item: &BomItem) -> Vec<String> {
    let refs: Vec<String> = item
        .refs
        .iter()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect();
    if refs.is_empty() && !item.reference.trim().is_empty() {
        return vec![item.reference.trim().to_string()];
    }
    refs
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
        if ctx.column_wholly_blank("msl") {
            return vec![Raw::new(ctx.severity, "MSL column is empty on every line")
                .detail(format!(
                    "The BOM has an MSL column but no row carries a value in it; {} \
                     moisture-sensitive part(s) need an MSL rating for correct assembly-house \
                     storage and bake-before-reflow handling.",
                    scoped.len()
                ))
                .fix(
                    "Populate the MSL column that is already there (J-STD-020 level 1-6) for \
                     all moisture-sensitive parts.",
                )
                .key("empty_msl_column")];
        }
        if !ctx.has_column("msl") {
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
        if missing.is_empty() {
            return Vec::new();
        }
        // ONE ROW, ONE FINDING. This used to be a single candidate naming every
        // affected row. That lands on the first of them and reads there as a claim
        // about parts that are not in it, and the engineer working row by row cannot
        // close it for one part without closing it for all of them. The gap is per
        // row, so the finding is too.
        missing
            .iter()
            .map(|item| {
                Raw::new(
                    ctx.severity,
                    format!("{} has no MSL rating", part_label(item)),
                )
                .detail(
                    "This moisture-sensitive part leaves the MSL column blank; the assembly \
                     house needs the level for storage and bake-before-reflow handling."
                        .to_string(),
                )
                .fix("Record this part's MSL level (J-STD-020 level 1 to 6) from its datasheet.")
                .refdes(row_refs(item))
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
        if ctx.column_wholly_blank("aecq") {
            return vec![Raw::new(ctx.severity, "AEC-Q column is empty on every line")
                .detail(
                    "Automotive design; the BOM has an AEC-Q column but no row carries a value \
                     in it, so part qualification (Q100/Q101/Q200) cannot be confirmed from \
                     this BOM.",
                )
                .fix(
                    "Populate the AEC-Q column that is already there, confirming every part is \
                     automotive-qualified or documenting an approved exception.",
                )
                .key("empty_aecq_column")];
        }
        if !ctx.has_column("aecq") {
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

        // A declared "NO" and an empty cell are DIFFERENT CLAIMS and must not share a
        // finding. "NO" is the designer stating the part is not qualified — actionable
        // at the profile's severity, no further evidence needed. Blank is *unknown*:
        // the part may well be qualified and simply undocumented, which is the common
        // case (a Sumida CDRH127L125NP-221MC reads "Qualified to AEC-Q200." on page 1
        // of its datasheet while carrying no AEC-Q column and no Digi-Key parameter).
        //
        // Merging them, as this rule used to, reports a qualification failure against a
        // correctly-chosen part. That is the most expensive false positive this pack
        // can emit: the designer re-sources a part that was already right. So blanks
        // go out under `missing_severity` as a data gap, with wording that tells the
        // validation stage exactly what to confirm against the datasheet. Both knobs
        // are configured to Major (see `config.rs`): one severity category, two
        // findings — the split is in the claim, not in the ranking.
        let mut declared: Vec<(&BomItem, String)> = Vec::new();
        let mut blank: Vec<&BomItem> = Vec::new();
        for item in &populated {
            let value = if item.filled("aecq") {
                item.aecq().trim().to_string()
            } else {
                String::new()
            };
            if value.is_empty() {
                blank.push(*item);
            } else if negatives.contains(&value.to_lowercase()) {
                declared.push((*item, value));
            }
        }

        let mut out = Vec::new();

        // ONE ROW, ONE FINDING, for the same reason the MSL rule files per row: a
        // candidate naming four parts lands on the first of them, carries three other
        // MPNs into a comment box beside a row they have nothing to do with, and
        // cannot be closed for one part at a time.
        for (item, value) in &declared {
            out.push(
                Raw::new(
                    ctx.severity,
                    format!("{} is declared not AEC-Q qualified", part_label(item)),
                )
                .detail(format!(
                    "The BOM's own AEC-Q column reads '{value}' for this row on an automotive \
                     design, so the part is stated to be unqualified rather than undocumented."
                ))
                .fix(
                    "Move to an AEC-Q qualified equivalent, or record an approved exception with \
                     the qualification grade.",
                )
                .refdes(row_refs(item))
                .key("aecq_declared_negative"),
            );
        }

        for item in &blank {
            out.push(
                Raw::new(
                    ctx.missing_sev(Severity::NonCritical),
                    format!("{} has no AEC-Q status recorded", part_label(item)),
                )
                .detail(
                    "This row leaves the AEC-Q column empty on an automotive design. Empty \
                     means UNKNOWN, not unqualified: the part may well be qualified with the \
                     field simply undocumented, and its datasheet settles it either way."
                        .to_string(),
                )
                .fix(
                    "Confirm this part's AEC-Q grade against its datasheet and record it; \
                     escalate only if the datasheet shows it is not qualified.",
                )
                .refdes(row_refs(item))
                .key("aecq_not_recorded"),
            );
        }

        out
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
        let miss_sev = ctx.missing_sev(Severity::NonCritical);

        let mut out = Vec::new();
        for field in &required {
            let label = match field.as_str() {
                "rohs" => "RoHS".to_string(),
                "reach" => "REACH/SVHC".to_string(),
                other => other.to_uppercase(),
            };
            // Two different defects that used to be reported as one. A BOM with no
            // REACH column needs a column added; a BOM whose REACH column is blank on
            // every line needs it populated, and telling that engineer to "add a REACH
            // column" sends them looking for something already in their header.
            if ctx.column_wholly_blank(field) {
                out.push(
                    Raw::new(
                        miss_sev,
                        format!("{label} column is empty on every line"),
                    )
                    .detail(format!(
                        "The BOM has a {label} column, but not one of the {} rows carries a \
                         value in it, so {label} compliance cannot be confirmed from this BOM.",
                        ctx.all_items.len()
                    ))
                    .fix(format!(
                        "Populate the {label} column that is already there, from manufacturer \
                         or distributor declarations."
                    ))
                    .key(format!("empty_column:{field}")),
                );
                continue;
            }
            if !ctx.has_column(field) {
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
