//! Cross-field contradictions — two cells in the same row asserting different things.
//! Every one of these was found on a real reference project (2026-07 sweep); they are
//! the highest-signal deterministic checks because a contradiction cannot be a taste
//! difference: one of the two cells is simply stale.

use std::collections::{BTreeMap, BTreeSet};

use super::SECTION_CONTRADICTIONS;
use crate::model::{refs_of, BomItem, Ctx};
use crate::value::extract_aux_specs;
use crate::{re, Raw, Rule};

/// DNP part carries MPN or distributor PN — confirm it is a variant placeholder.
/// Off by default: sourcing data on a DNP line is the normal way to carry a build
/// option. (Unlike the Python original this rule opts into DNP rows — without that
/// it could never fire at all.)
pub struct DnpButHasSourcing;

impl Rule for DnpButHasSourcing {
    fn id(&self) -> &'static str {
        "bom.dnp_but_has_sourcing"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }
    fn scans_dnp(&self) -> bool {
        true
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let hits: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| i.dnp && (i.filled("mpn") || !i.supplier_pns.is_empty()))
            .collect();
        if hits.is_empty() {
            return Vec::new();
        }
        let agg_min = ctx.param_usize("systemic_min", 5);
        if hits.len() >= agg_min {
            return vec![Raw::new(
                ctx.severity,
                format!("{} DNP parts carry sourcing information", hits.len()),
            )
            .detail(format!(
                "{} parts are marked DNP but keep sourcing data — consistent with variant \
                 placeholders; confirm the intent once.",
                hits.len()
            ))
            .fix("If truly DNP, clear the sourcing fields; if these are variant options, this is informational only.")
            .evidence(format!("Affected: {}", refs_of(&hits, 12).join(", ")))
            .key("systemic")];
        }
        hits.iter()
            .map(|item| {
                Raw::new(ctx.severity, "DNP part carries sourcing information")
                    .detail(format!(
                        "Part {} is marked DNP but has sourcing data; confirm this is a variant \
                         placeholder.",
                        item.label()
                    ))
                    .fix("If truly DNP, clear the sourcing fields; if a variant option, this is informational only.")
                    .item(item)
                    .key("dnp_sourced")
            })
            .collect()
    }
}

/// Excluded-from-BOM part (fiducial, mount hole) still carries sourcing data — the
/// two assertions contradict each other.
pub struct ExcludedButSourced;

impl Rule for ExcludedButSourced {
    fn id(&self) -> &'static str {
        "bom.excluded_but_sourced"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        ctx.items
            .iter()
            .filter(|i| i.exclude_from_bom && (i.filled("mpn") || !i.supplier_pns.is_empty()))
            .map(|item| {
                Raw::new(ctx.severity, "Excluded-from-BOM part carries sourcing information")
                    .detail(format!(
                        "Part {} is marked EXCLUDE_FROM_BOM but carries sourcing data; these two \
                         assertions contradict each other.",
                        item.label()
                    ))
                    .fix(
                        "If it is genuinely not a BOM part, clear the sourcing fields; otherwise \
                         remove the exclude flag.",
                    )
                    .item(item)
                    .key("excluded_sourced")
            })
            .collect()
    }
}

/// Voltage/tolerance embedded in the Value string contradicts the dedicated column.
/// Real case (StickHub): Value '0.1uF 100V' with a Voltage column reading '16V'.
pub struct ValueAuxContradiction;

impl Rule for ValueAuxContradiction {
    fn id(&self) -> &'static str {
        "bom.value_aux_contradiction"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut out = Vec::new();
        for item in &ctx.items {
            if !item.filled("value") {
                continue;
            }
            let aux = extract_aux_specs(item.value());
            if aux.is_empty() {
                continue;
            }
            let checks: [(Option<f64>, &str, &str); 2] = [
                (aux.voltage, "voltage", item.voltage()),
                (aux.tolerance_pct, "tolerance", item.tolerance()),
            ];
            for (embedded, column, raw) in checks {
                let (Some(embedded), false) = (embedded, raw.trim().is_empty()) else {
                    continue;
                };
                let Some(m) = re!(r"(\d+(?:\.\d+)?)").captures(raw) else {
                    continue;
                };
                let Ok(col_val) = m[1].parse::<f64>() else {
                    continue;
                };
                if (embedded - col_val).abs() <= 1e-9 * col_val.abs().max(1.0) {
                    continue;
                }
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!(
                            "Value string and {column} column disagree on {}",
                            item.label()
                        ),
                    )
                    .detail(format!(
                        "Value '{}' embeds {column} {embedded} but the {column} column says \
                         '{raw}' — one of the two is stale.",
                        item.value()
                    ))
                    .fix(format!(
                        "Reconcile the {column} between the value string and the column."
                    ))
                    .item(item)
                    .key(format!("aux:{column}")),
                );
            }
        }
        out
    }
}

/// Imperial ↔ metric chip-size equivalences (EIA codes).
const IMPERIAL_TO_METRIC: &[(&str, &str)] = &[
    ("01005", "0402"), ("0201", "0603"), ("0402", "1005"), ("0603", "1608"),
    ("0805", "2012"), ("1206", "3216"), ("1210", "3225"), ("1806", "4516"),
    ("1812", "4532"), ("2010", "5025"), ("2220", "5750"), ("2512", "6332"),
];

/// All chip-size codes in `text`, expanded with their imperial/metric twins — so
/// 0603 (imperial) and 1608 (metric) compare equal, because they are the same part.
fn size_closure(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for m in re!(r"(?:^|[^0-9])(01005|[0-9]{4})(?:[^0-9]|$)").captures_iter(text) {
        let code = m[1].to_string();
        let known = IMPERIAL_TO_METRIC
            .iter()
            .any(|(i, m)| *i == code || *m == code);
        if !known {
            continue;
        }
        out.insert(code.clone());
        for (imperial, metric) in IMPERIAL_TO_METRIC {
            if *imperial == code {
                out.insert((*metric).to_string());
            }
            if *metric == code {
                out.insert((*imperial).to_string());
            }
        }
    }
    out
}

/// Package/size column contradicts the chip size encoded in the footprint name.
/// Real case (StickHub): Package '1608' on footprint '2012_C'.
pub struct PackageFootprintMismatch;

impl Rule for PackageFootprintMismatch {
    fn id(&self) -> &'static str {
        "bom.package_footprint_mismatch"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut combos: BTreeMap<(String, String), Vec<&BomItem>> = BTreeMap::new();
        for item in &ctx.items {
            if !(item.filled("package") && item.filled("footprint")) {
                continue;
            }
            let pkg_codes = size_closure(item.package());
            let fp_codes = size_closure(item.footprint());
            if pkg_codes.is_empty()
                || fp_codes.is_empty()
                || pkg_codes.intersection(&fp_codes).next().is_some()
            {
                continue;
            }
            combos
                .entry((
                    item.package().trim().to_string(),
                    item.footprint().trim().to_string(),
                ))
                .or_default()
                .push(item);
        }

        combos
            .into_iter()
            .map(|((pkg, fp), items)| {
                let suffix = if items.len() > 1 {
                    format!(" on {} parts", items.len())
                } else {
                    format!(" on {}", items[0].label())
                };
                Raw::new(
                    ctx.severity,
                    format!("Package '{pkg}' does not match footprint size{suffix}"),
                )
                .detail(format!(
                    "The package column says '{pkg}' but the footprint '{fp}' encodes a different \
                     chip size (imperial/metric equivalence already accounted for)."
                ))
                .fix("Reconcile the package column with the assigned footprint.")
                .evidence(format!("Affected: {}", refs_of(&items, 12).join(", ")))
                .refdes(items.iter().filter(|i| !i.reference.is_empty()).map(|i| i.reference.clone()))
                .key(format!("{pkg}|{fp}"))
            })
            .collect()
    }
}

/// Distributor order code (an LCSC C-code) sitting in a value/description/MPN field.
/// Real cases: Description = 'C5138758' (cm5), Value = 'SMBJ24A_C908801' (openair).
pub struct MisplacedDistributorCode;

impl Rule for MisplacedDistributorCode {
    fn id(&self) -> &'static str {
        "bom.misplaced_distributor_code"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let fields = ctx.param_strings("fields").unwrap_or_else(|| {
            vec!["value".to_string(), "description".to_string(), "mpn".to_string()]
        });
        let mut out = Vec::new();
        for item in &ctx.items {
            for field in &fields {
                if !item.filled(field) {
                    continue;
                }
                let raw = item.field(field).trim();
                let Some(m) = re!(r"(?:^|[^A-Za-z0-9])(C\d{5,8})(?:[^0-9]|$)").captures(raw) else {
                    continue;
                };
                let code = m[1].to_string();
                let shown: String = raw.chars().take(60).collect();
                out.push(
                    Raw::new(
                        ctx.severity,
                        format!("Distributor order code in {field} field: '{code}'"),
                    )
                    .detail(format!(
                        "Part {} carries what looks like an LCSC order code ('{code}') inside its \
                         {field} field ('{shown}'); order codes belong in a distributor column.",
                        item.label()
                    ))
                    .fix(format!(
                        "Move the order code to an LCSC column and put the real {field} here."
                    ))
                    .item(item)
                    .key(format!("{field}:{code}")),
                );
            }
        }
        out
    }
}

/// DNP intent written into value/description text while the DNP flag is unset.
/// Assembly tooling reads the flag, not the prose — so this part WILL be placed.
pub struct DnpMarkerInText;

impl Rule for DnpMarkerInText {
    fn id(&self) -> &'static str {
        "bom.dnp_marker_in_text"
    }
    fn section(&self) -> &'static str {
        SECTION_CONTRADICTIONS
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut out = Vec::new();
        for item in &ctx.items {
            if item.dnp || item.non_orderable() {
                continue;
            }
            let mut marker: Option<(&str, String, String)> = None;
            for field in ["value", "description"] {
                let raw = item.field(field).trim();
                if raw.is_empty() {
                    continue;
                }
                let hit = re!(
                    r"(?i)\b(dnp|dni|not\s+populated|unpopulated|do\s+not\s+(?:populate|place|fit|install)|not\s+fitted|no\s+stuff|no\s+mount)\b"
                )
                .find(raw)
                .map(|m| m.as_str().to_string())
                .or_else(|| {
                    (field == "value")
                        .then(|| re!(r"(?i)[_\-/]NM$").find(raw).map(|m| m.as_str().to_string()))
                        .flatten()
                });
                if let Some(token) = hit {
                    marker = Some((field, raw.to_string(), token));
                    break;
                }
            }
            let Some((field, raw, token)) = marker else {
                continue;
            };
            let shown: String = raw.chars().take(60).collect();
            out.push(
                Raw::new(
                    ctx.severity,
                    format!(
                        "DNP marker in {field} text but DNP flag not set: {}",
                        item.label()
                    ),
                )
                .detail(format!(
                    "Part {} says '{token}' in its {field} ('{shown}') but is not marked DNP — \
                     assembly tooling reads the flag, not the text, so this part WILL be placed.",
                    item.label()
                ))
                .fix("Set the DNP attribute on the symbol (or remove the stale text marker).")
                .item(item)
                .key(format!("dnp_text:{field}")),
            );
        }
        out
    }
}
