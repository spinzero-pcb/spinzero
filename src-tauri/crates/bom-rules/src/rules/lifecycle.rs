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

/// The part list a grouped finding carries in its description: one line per
/// (part, note), designators riding along for the lookup.
///
/// Listed by part, not by designator: what gets qualified or populated is an MPN, and
/// one MPN usually spans many rows. `note` is the per-part remark ("AEC-Q: (blank)");
/// pass an empty string when the gap itself is the whole story.
fn listed_by_part(hits: &[(&BomItem, String)]) -> String {
    let mut by_part: Vec<(String, String, Vec<String>)> = Vec::new();
    for (item, note) in hits {
        let part = if item.mpn().trim().is_empty() {
            item.label().to_string()
        } else {
            item.mpn().trim().to_string()
        };
        let refdes = item.label().to_string();
        match by_part.iter_mut().find(|(p, n, _)| p == &part && n == note) {
            Some((_, _, refs)) => refs.push(refdes),
            None => by_part.push((part, note.clone(), vec![refdes])),
        }
    }
    by_part
        .iter()
        .map(|(part, note, refs)| {
            // One part can span dozens of rows; the designators are a lookup aid, not
            // the point, so the line stays readable.
            let shown = refs.len().min(8);
            let mut list = refs[..shown].join(", ");
            if refs.len() > shown {
                list.push_str(&format!(", +{} more", refs.len() - shown));
            }
            if note.is_empty() {
                format!("  • {part} ({list})")
            } else {
                format!("  • {part} — {note} ({list})")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        if missing.is_empty() {
            return Vec::new();
        }
        // One review point for the whole gap, same shape as AEC-Q: the decision
        // ("populate the MSL level for these parts") is identical for every row, so N
        // comments would be N times the noise. The BOM marks every covered row.
        let refs: Vec<String> = missing
            .iter()
            .map(|item| item.reference.clone())
            .filter(|r| !r.is_empty())
            .collect();
        let hits: Vec<(&BomItem, String)> =
            missing.iter().map(|item| (*item, String::new())).collect();
        let listed = listed_by_part(&hits);
        vec![Raw::new(
            ctx.severity,
            if missing.len() == 1 {
                "Missing MSL rating".to_string()
            } else {
                format!("{} parts have no MSL rating", missing.len())
            },
        )
        .detail(format!(
            "{} of {} moisture-sensitive part(s) have no MSL rating; the assembly house needs \
             it for storage and bake-before-reflow handling.\n\nNo MSL rating:\n{listed}",
            missing.len(),
            scoped.len()
        ))
        .fix("Populate the MSL level (J-STD-020 level 1–6) for these parts.")
        .refdes(refs)
        .key("missing_msl")]
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

        // One review point per class, listing every affected part: the reviewer's
        // decision is the same for all of them, so N identical comments would only be
        // N times the noise. The BOM marks every covered row, so the reviewer still
        // sees each one in place.
        if !declared.is_empty() {
            let refs: Vec<String> = declared
                .iter()
                .map(|(item, _)| item.reference.clone())
                .filter(|r| !r.is_empty())
                .collect();
            let labelled: Vec<(&BomItem, String)> = declared
                .iter()
                .map(|(item, value)| (*item, format!("AEC-Q: {value}")))
                .collect();
            let listed = listed_by_part(&labelled);
            out.push(
                Raw::new(
                    ctx.severity,
                    if declared.len() == 1 {
                        "Part declared not AEC-Q qualified".to_string()
                    } else {
                        format!("{} parts declared not AEC-Q qualified", declared.len())
                    },
                )
                .detail(format!(
                    "{} of {} populated parts carry an explicit negative AEC-Q status in an \
                     automotive design.\n\nDeclared not qualified:\n{listed}",
                    declared.len(),
                    populated.len()
                ))
                .fix(
                    "Use AEC-Q-qualified equivalents, or record an approved exception with the \
                     qualification grade.",
                )
                .refdes(refs)
                .key("aecq_declared_negative"),
            );
        }

        if !blank.is_empty() {
            let refs: Vec<String> = blank
                .iter()
                .map(|item| item.reference.clone())
                .filter(|r| !r.is_empty())
                .collect();
            let labelled: Vec<(&BomItem, String)> = blank
                .iter()
                .map(|item| (*item, "AEC-Q: (blank)".to_string()))
                .collect();
            let listed = listed_by_part(&labelled);
            out.push(
                Raw::new(
                    ctx.missing_sev(Severity::Low),
                    format!(
                        "{} part{} have no AEC-Q status recorded",
                        blank.len(),
                        if blank.len() == 1 { "" } else { "s" }
                    ),
                )
                .detail(format!(
                    "{} of {} populated parts leave the AEC-Q column empty in an automotive \
                     design. Empty means UNKNOWN, not unqualified — several of these are \
                     usually qualified parts with an undocumented field, and the datasheet \
                     settles it either way.\n\nNo AEC-Q status recorded:\n{listed}",
                    blank.len(),
                    populated.len()
                ))
                .fix(
                    "Confirm each part's AEC-Q grade against its datasheet and record it; escalate \
                     only the parts the datasheet shows are not qualified.",
                )
                .refdes(refs)
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
