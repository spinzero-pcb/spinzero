//! Integrity rules — the BOM must describe a board that can actually be built:
//! unique annotated designators, the fields every part needs, sane quantities.

use std::collections::BTreeMap;

use super::SECTION_INTEGRITY;
use crate::model::{systemic, BomItem, Ctx, Severity};
use crate::{re, Raw, Rule};

/// Same designator on more than one BOM line — broken annotation; the board cannot
/// be assembled from this BOM.
pub struct DuplicateRefdes;

impl Rule for DuplicateRefdes {
    fn id(&self) -> &'static str {
        "bom.duplicate_refdes"
    }
    fn section(&self) -> &'static str {
        SECTION_INTEGRITY
    }
    /// A designator collision is an error whether or not the part is built.
    fn scans_dnp(&self) -> bool {
        true
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for item in &ctx.items {
            for r in &item.refs {
                *index.entry(r.as_str()).or_insert(0) += 1;
            }
        }
        index
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(r, n)| {
                Raw::new(ctx.severity, format!("Duplicate reference designator: {r}"))
                    .detail(format!(
                        "Reference designator '{r}' appears on {n} BOM lines; designators must \
                         be unique."
                    ))
                    .fix("Re-annotate the schematic so each symbol has a unique reference designator.")
                    .evidence(format!("'{r}' on {n} BOM lines"))
                    .refdes([r.to_string()])
                    .key(r)
            })
            .collect()
    }
}

/// Refdes blank or containing '?' — the symbol was never annotated.
pub struct UnannotatedRefdes;

impl Rule for UnannotatedRefdes {
    fn id(&self) -> &'static str {
        "bom.unannotated_refdes"
    }
    fn section(&self) -> &'static str {
        SECTION_INTEGRITY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        ctx.items
            .iter()
            .filter(|item| {
                let r = item.reference.trim();
                r.is_empty() || r.contains('?') || item.refs.iter().any(|x| x.contains('?'))
            })
            .map(|item| {
                let shown = if item.reference.trim().is_empty() {
                    "(blank)"
                } else {
                    item.reference.trim()
                };
                Raw::new(ctx.severity, format!("Unannotated reference designator: {shown}"))
                    .detail(
                        "The symbol is not fully annotated (reference is blank or contains '?').",
                    )
                    .fix("Run schematic annotation so every symbol gets a concrete reference designator.")
                    .evidence(format!("Reference '{shown}', value '{}'", item.value()))
                    .item(item)
                    // A blank refdes anchors to nothing, so the value keeps two blank
                    // rows from folding into one comment.
                    .key(format!("{}|{}", item.reference, item.value()))
            })
            .collect()
    }
}

/// Non-standard refdes format — lower-case, word-length prefix, or a prefix outside
/// the house standard. Loose by default: KiCad itself allows `Module301`.
pub struct InvalidRefdesFormat;

impl Rule for InvalidRefdesFormat {
    fn id(&self) -> &'static str {
        "bom.invalid_refdes_format"
    }
    fn section(&self) -> &'static str {
        SECTION_INTEGRITY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        const DEFAULT_PATTERN: &str = r"^[A-Za-z][A-Za-z0-9_]*[0-9]$";
        let pattern = ctx
            .params
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_PATTERN)
            .to_string();
        // A profile's pattern is user data: an invalid one degrades to the built-in
        // rather than failing the whole check.
        let rx = match regex::Regex::new(&pattern) {
            Ok(rx) => rx,
            Err(_) => re!(r"^[A-Za-z][A-Za-z0-9_]*[0-9]$").clone(),
        };
        let allowed: Option<Vec<String>> = ctx
            .param_strings("allowed_prefixes")
            .map(|v| v.iter().map(|p| p.trim().to_uppercase()).collect());

        let mut out = Vec::new();
        for item in &ctx.items {
            for r in &item.refs {
                if r.contains('?') {
                    continue;
                }
                if !rx.is_match(r) {
                    out.push(
                        Raw::new(
                            ctx.severity,
                            format!("Reference designator not in expected format: {r}"),
                        )
                        .detail(format!(
                            "'{r}' does not match the expected pattern '{pattern}' (letter prefix \
                             followed by digits)."
                        ))
                        .fix("Use a standard prefix+number designator, e.g. R12, U3, C7.")
                        .evidence(format!("'{r}' vs pattern '{pattern}'"))
                        .refdes([r.clone()])
                        .key(r),
                    );
                    continue;
                }
                if let Some(allowed) = &allowed {
                    let prefix = re!(r"^([A-Za-z]+)")
                        .captures(r)
                        .map(|c| c[1].to_uppercase())
                        .unwrap_or_default();
                    if !allowed.contains(&prefix) {
                        out.push(
                            Raw::new(
                                ctx.severity,
                                format!("Unrecognized reference designator prefix: {r}"),
                            )
                            .detail(format!(
                                "Prefix '{prefix}' is not in the profile's allowed set ({}).",
                                allowed.join(", ")
                            ))
                            .fix("Use a recognized class prefix, or add it to allowed_prefixes.")
                            .refdes([r.clone()])
                            .key(format!("prefix:{r}")),
                        );
                    }
                }
            }
        }
        out
    }
}

/// Missing value, footprint, or another profile-configured required field.
pub struct MissingRequiredField;

impl Rule for MissingRequiredField {
    fn id(&self) -> &'static str {
        "bom.missing_required_field"
    }
    fn section(&self) -> &'static str {
        SECTION_INTEGRITY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let required = ctx.param_strings("required_fields").unwrap_or_default();
        // The bare board, test points and friends are not parts: they have no
        // manufacturer or MPN to populate, so required-field gaps are not defects.
        let items: Vec<&BomItem> = ctx
            .items
            .iter()
            .copied()
            .filter(|i| !i.non_orderable())
            .collect();
        if items.is_empty() || required.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut systemic_fields: Vec<&str> = Vec::new();
        // Gate per field: a schematic with no footprints assigned anywhere is one
        // process gap, not one finding per symbol.
        for f in &required {
            let missing_n = items.iter().filter(|i| !i.filled(f)).count();
            if systemic(missing_n, items.len(), ctx, 0.35, 8) {
                systemic_fields.push(f);
                out.push(
                    Raw::new(
                        Severity::Major,
                        format!(
                            "Required field '{f}' is not populated in this BOM ({missing_n} of {} \
                             parts lack it)",
                            items.len()
                        ),
                    )
                    .detail(format!(
                        "Most parts are missing '{f}' — the field is not being tracked, so parts \
                         are not fully specified."
                    ))
                    .fix(format!("Populate '{f}' for every part."))
                    .evidence(format!("{missing_n} of {} parts lack '{f}'", items.len()))
                    .key(format!("systemic:{f}")),
                );
            }
        }

        for item in items {
            let missing: Vec<&str> = required
                .iter()
                .map(|s| s.as_str())
                .filter(|f| !systemic_fields.contains(f) && !item.filled(f))
                .collect();
            if missing.is_empty() {
                continue;
            }
            out.push(
                Raw::new(
                    ctx.severity,
                    format!("Missing required field(s): {}", missing.join(", ")),
                )
                .detail(format!(
                    "Part {} is missing required BOM field(s): {}.",
                    item.label(),
                    missing.join(", ")
                ))
                .fix("Populate the missing field(s) so the part is fully specified.")
                .item(item)
                .key(missing.join(",")),
            );
        }
        out
    }
}

/// Blank, zero, negative, or non-numeric quantity on a populated line.
pub struct InvalidQuantity;

impl Rule for InvalidQuantity {
    fn id(&self) -> &'static str {
        "bom.invalid_quantity"
    }
    fn section(&self) -> &'static str {
        SECTION_INTEGRITY
    }

    fn check(&self, ctx: &Ctx) -> Vec<Raw> {
        let mut out = Vec::new();
        for item in &ctx.items {
            // A BOM with no quantity column at all is not "every line invalid" —
            // grouped quantity is derived from the designator count downstream.
            if item.dnp || !item.filled("quantity") {
                continue;
            }
            let raw = item.quantity.trim();
            let reason = if raw.is_empty() {
                "blank".to_string()
            } else {
                match item.quantity_int {
                    None => format!("non-numeric ('{raw}')"),
                    Some(q) if q <= 0 => q.to_string(),
                    Some(_) => continue,
                }
            };
            out.push(
                Raw::new(ctx.severity, format!("Invalid quantity: {reason}"))
                    .detail(format!(
                        "Populated line {} has an invalid build quantity ({reason}).",
                        item.label()
                    ))
                    .fix("Set a positive integer quantity, or mark the part DNP if unpopulated.")
                    .item(item)
                    .key("quantity"),
            );
        }
        out
    }
}
