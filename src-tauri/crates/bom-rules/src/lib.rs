//! Deterministic BOM checks — the free tier of the SpinZero review.
//!
//! One input (BOM rows as they exist in the source), one output (`findings.json`
//! v1.1, `schemas/findings-1.1.json`). The paid review engine emits the *same*
//! document with validated confidences, so the app ingests both through a single
//! path and a re-run updates comments by `fingerprint` instead of re-filing them.
//!
//! The shared fixtures in `schemas/rule-fixtures/` are what pins the rule set.
//!
//! ```no_run
//! use bom_rules::{config, load, run};
//! let rows = load::parse_csv("Reference,Value\nR1,10k\n");
//! let (items, mapping) = load::items_from_rows(&rows, &config::config_for("default"));
//! let doc = run(&items, "default", &mapping);
//! assert_eq!(doc.schema_version, "1.1");
//! ```

pub mod config;
pub mod load;
pub mod model;
pub mod value;

mod rules;

use serde::{Deserialize, Serialize};

pub use model::{BomItem, Severity};

/// Compile a literal regex once per call site. Only ever used with literals known at
/// compile time — patterns that come from config go through `regex::Regex::new(..).ok()`
/// so a hand-edited profile can never panic a review.
#[macro_export]
macro_rules! re {
    ($pattern:expr) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            #[allow(clippy::expect_used)]
            regex::Regex::new($pattern).expect("literal regex in bom-rules is valid")
        })
    }};
}

/// A rule's raw output, before it becomes a schema finding.
#[derive(Clone, Debug)]
pub struct Raw {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub fix: String,
    pub evidence: Vec<String>,
    /// Designators this finding anchors to. Empty = a document-level finding about
    /// the BOM as a whole ("this BOM has no lifecycle column").
    pub refdes: Vec<String>,
    /// Stable discriminator for the fingerprint: what *specifically* this finding is
    /// about, independent of counts and wording. Two runs of the same rule over the
    /// same defect must produce the same key, or the comment gets re-filed.
    pub key: String,
}

impl Raw {
    pub fn new(severity: Severity, title: impl Into<String>) -> Raw {
        Raw {
            severity,
            title: title.into(),
            detail: String::new(),
            fix: String::new(),
            evidence: Vec::new(),
            refdes: Vec::new(),
            key: String::new(),
        }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Raw {
        self.detail = d.into();
        self
    }
    pub fn fix(mut self, f: impl Into<String>) -> Raw {
        self.fix = f.into();
        self
    }
    pub fn evidence(mut self, e: impl Into<String>) -> Raw {
        self.evidence.push(e.into());
        self
    }
    pub fn refdes(mut self, refs: impl IntoIterator<Item = String>) -> Raw {
        self.refdes.extend(refs);
        self
    }
    pub fn item(mut self, item: &BomItem) -> Raw {
        if !item.reference.is_empty() {
            self.refdes.push(item.reference.clone());
        }
        self
    }
    pub fn key(mut self, k: impl Into<String>) -> Raw {
        self.key = k.into();
        self
    }
}

/// One deterministic check.
pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    /// Findings-schema section, e.g. `BOM · Sourcing`.
    fn section(&self) -> &'static str;
    /// DNP lines are not built, so their metadata gaps are not defects — the runner
    /// hands each rule the populated lines only. A rule that genuinely reasons about
    /// DNP (a duplicate refdes is an error whoever owns it) opts back in here.
    fn scans_dnp(&self) -> bool {
        false
    }
    fn check(&self, ctx: &model::Ctx) -> Vec<Raw>;
}

// ---------------------------------------------------------------- output document
//
// These types are `Deserialize` as well as `Serialize` because findings.json travels
// BOTH ways: this crate writes it for the free tier, and the app reads the paid
// engine's document back through the same types so that one ingestion path
// (`bomcheck::ingest`) serves both tiers. Unknown fields are ignored by serde, which
// is what lets the engine add `stats.tokens` without breaking the app.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Anchor {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refdes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub section: String,
    pub severity: String,
    /// Always `Unvalidated` here: these are raw rule hits that no validation pass has
    /// confirmed. The paid pipeline replaces this with High or Low.
    pub confidence: String,
    #[serde(default)]
    pub rule_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fix: String,
    pub anchors: Vec<Anchor>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub item: String,
    /// `OK` | `GAP` | `TRUNCATED`
    pub result: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Stats {
    #[serde(default)]
    pub item_count: usize,
    #[serde(default)]
    pub finding_count: usize,
    #[serde(default)]
    pub duration_ms: u64,
}

/// One stage of a review that did not fully run (schema: `run_health`). The free
/// tier never emits these — it is deterministic and has nothing to degrade — but the
/// paid engine does, and the app shows them so an incomplete review cannot read as
/// a clean one.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunHealthEntry {
    /// Producer stage id ("fp_validation", "judgment_pass").
    pub stage: String,
    /// `degraded` (ran, covered less than it should) | `failed` (produced nothing).
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindingsDoc {
    pub schema_version: String,
    #[serde(default)]
    pub engine_version: String,
    pub pipeline: String,
    pub profile: String,
    /// RFC3339; the caller stamps it (this crate has no clock by design — it must be
    /// deterministic for the fixture tests).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generated_ts: String,
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub bom_audit: Vec<AuditEntry>,
    #[serde(default)]
    pub stats: Stats,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_health: Vec<RunHealthEntry>,
}

/// blake3(rule_id | anchor set | normalized predicate), truncated to 16 hex chars —
/// the dedupe key from the findings schema. Anchors are sorted so a rule that
/// enumerates parts in a different order on the next run still matches.
pub fn fingerprint(rule_id: &str, refdes: &[String], key: &str) -> String {
    let mut refs: Vec<String> = refdes.iter().map(|r| r.trim().to_uppercase()).collect();
    refs.sort();
    refs.dedup();
    let payload = format!(
        "{}|{}|{}",
        rule_id,
        refs.join(","),
        key.trim().to_lowercase()
    );
    blake3::hash(payload.as_bytes()).to_hex()[..16].to_string()
}

/// Run every enabled rule for `profile` over `items` and assemble `findings.json`.
///
/// Never fails: a rule that panics on pathological data would take the app down, so
/// each rule runs inside `catch_unwind` and a failure becomes a Low finding naming
/// the rule instead of a crash.
pub fn run(items: &[BomItem], profile: &str, mapping: &load::MappingReport) -> FindingsDoc {
    let config = config::config_for(profile);
    let rules_cfg = config.get("rules").cloned().unwrap_or(serde_json::Value::Null);
    // No rows at all is a *loading* problem (unmapped columns, an empty BOM export),
    // not a review result. Column-absence rules would otherwise report a BOM that
    // tracks nothing, which reads as a design defect and isn't one.
    if items.is_empty() {
        return FindingsDoc {
            schema_version: "1.1".into(),
            engine_version: format!("bom-rules/{}", env!("CARGO_PKG_VERSION")),
            pipeline: "bom-rules".into(),
            profile: profile.to_string(),
            generated_ts: String::new(),
            findings: Vec::new(),
            bom_audit: Vec::new(),
            stats: Stats::default(),
            run_health: Vec::new(),
        };
    }
    let populated: Vec<&BomItem> = items.iter().filter(|i| !i.dnp).collect();
    let all: Vec<&BomItem> = items.iter().collect();
    // Which logical fields the header actually carries. Rules ask this instead of
    // asking the rows, so "no REACH column" and "the REACH column is empty" stop
    // being the same finding — see `Ctx::has_column`.
    let mapped_fields: std::collections::BTreeSet<String> = mapping.fields.keys().cloned().collect();

    let mut raws: Vec<(&'static str, &'static str, Raw)> = Vec::new();
    for rule in rules::all_rules() {
        let cfg = rules_cfg.get(rule.id());
        let enabled = cfg
            .and_then(|c| c.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !enabled {
            continue;
        }
        let severity = cfg
            .and_then(|c| c.get("severity"))
            .and_then(|v| v.as_str())
            .map(Severity::parse)
            .unwrap_or(Severity::Observation);
        let params = cfg
            .and_then(|c| c.get("params"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let ctx = model::Ctx {
            items: if rule.scans_dnp() { all.clone() } else { populated.clone() },
            all_items: items,
            mapped_fields: &mapped_fields,
            severity,
            params,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rule.check(&ctx)));
        match result {
            Ok(found) => {
                for raw in found {
                    raws.push((rule.id(), rule.section(), raw));
                }
            }
            Err(_) => raws.push((
                rule.id(),
                rule.section(),
                Raw::new(Severity::Observation, format!("Rule execution error: {}", rule.id()))
                    .detail(
                        "This check could not run on this BOM; the rest of the review is \
                         unaffected. Please report the BOM shape that triggered it."
                            .to_string(),
                    )
                    .key("panic"),
            )),
        }
    }

    let mut raws = fold_dnp_only(raws, items);

    // Severity-sorted, then stable by rule and anchor so ids and ordering don't
    // shuffle between runs on an unchanged BOM.
    raws.sort_by(|a, b| {
        a.2.severity
            .rank()
            .cmp(&b.2.severity.rank())
            .then_with(|| a.0.cmp(b.0))
            .then_with(|| a.2.refdes.cmp(&b.2.refdes))
            .then_with(|| a.2.title.cmp(&b.2.title))
    });

    let findings: Vec<Finding> = raws
        .into_iter()
        .enumerate()
        .map(|(i, (rule_id, section, raw))| Finding {
            id: format!("B{:02}", i + 1),
            section: section.to_string(),
            severity: raw.severity.as_str().to_string(),
            confidence: "Unvalidated".into(),
            rule_id: Some(rule_id.to_string()),
            fingerprint: fingerprint(rule_id, &raw.refdes, &raw.key),
            anchors: vec![if raw.refdes.is_empty() {
                Anchor { kind: "bom".into(), refdes: Vec::new() }
            } else {
                Anchor { kind: "bom_row".into(), refdes: raw.refdes.clone() }
            }],
            title: raw.title,
            detail: raw.detail,
            evidence: raw.evidence,
            fix: raw.fix,
        })
        .collect();

    FindingsDoc {
        schema_version: "1.1".into(),
        engine_version: format!("bom-rules/{}", env!("CARGO_PKG_VERSION")),
        pipeline: "bom-rules".into(),
        profile: profile.to_string(),
        generated_ts: String::new(),
        bom_audit: audit(items, &findings, mapping),
        run_health: Vec::new(),
        stats: Stats {
            item_count: items.len(),
            finding_count: findings.len(),
            duration_ms: 0,
        },
        findings,
    }
}

/// The rule id the folded do-not-populate note is filed under. Not a rule — no
/// `Rule` impl produces it — but findings carry a `rule_id` and the app groups by
/// it, so the fold needs one of its own rather than borrowing a real rule's.
pub const DNP_FOLD_RULE_ID: &str = "bom.dnp_lines";

/// Collapse every finding that lands ONLY on do-not-populate lines into one
/// informational note.
///
/// A DNP line is not built. Its metadata gaps, its odd values and its leftover
/// sourcing data are therefore not defects — but they are not nothing either: a
/// line marked DNP that still carries a part number is usually a build variant, and
/// an engineer wants to confirm that intent *once*, not read six separate findings
/// about it. Six rows of noise is how a reader learns to skim a report, and the
/// things worth reading are in the same list.
///
/// The test is deliberately strict: EVERY designator the finding names must be a
/// DNP line. A contradiction that spans a populated part and a DNP one — the same
/// MPN on two footprints, one of each — is a real defect about the populated part
/// and survives untouched.
fn fold_dnp_only(
    raws: Vec<(&'static str, &'static str, Raw)>,
    items: &[BomItem],
) -> Vec<(&'static str, &'static str, Raw)> {
    let mut dnp: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in items {
        for r in &item.refs {
            let key = r.trim().to_ascii_uppercase();
            if key.is_empty() {
                continue;
            }
            if item.dnp {
                dnp.insert(key.clone());
            }
            known.insert(key);
        }
    }
    if dnp.is_empty() {
        return raws;
    }

    let anchored_only_on_dnp = |raw: &Raw| -> bool {
        // A document-level finding ("this BOM has no RoHS column") anchors to nothing
        // and is about the whole BOM, DNP lines included. It is not a DNP note.
        if raw.refdes.is_empty() {
            return false;
        }
        raw.refdes.iter().all(|r| {
            let key = r.trim().to_ascii_uppercase();
            // An unknown designator is not evidence of DNP; treat it as populated so a
            // stale reference can never demote a real finding to an informational one.
            known.contains(&key) && dnp.contains(&key)
        })
    };

    let (folded, kept): (Vec<_>, Vec<_>) =
        raws.into_iter().partition(|(_, _, raw)| anchored_only_on_dnp(raw));
    if folded.is_empty() {
        return kept;
    }

    let mut refdes: Vec<String> = folded.iter().flat_map(|(_, _, r)| r.refdes.clone()).collect();
    refdes.sort();
    refdes.dedup();

    // Each folded finding keeps its own sentence: the point is to spend ONE entry in
    // the report on DNP lines, not to throw away what the rules noticed.
    let mut lines: Vec<String> = folded
        .iter()
        .map(|(rule_id, _, raw)| {
            let refs = if raw.refdes.is_empty() {
                String::new()
            } else {
                format!(" ({})", raw.refdes.join(", "))
            };
            format!("  \u{2022} {}{} [{}]", raw.title, refs, rule_id)
        })
        .collect();
    lines.sort();
    lines.dedup();

    let mut note = Raw::new(
        Severity::Observation,
        format!(
            "{} note{} on do-not-populate lines",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        ),
    )
    .detail(format!(
        "These lines are marked DNP, so nothing here is built and none of it is a defect. \
         Collected into one note because a DNP line carrying a part number is normally a \
         build variant, and that intent is worth confirming once rather than row by row.\n\n{}",
        lines.join("\n")
    ))
    .fix(
        "Confirm these are intentional build variants. If a line is truly unused, clear its \
         sourcing fields; if it is a variant, nothing needs to change."
            .to_string(),
    )
    .key("dnp_lines".to_string());
    note.refdes = refdes;

    let mut out = kept;
    out.push((DNP_FOLD_RULE_ID, rules::SECTION_CONTRADICTIONS, note));
    out
}

/// The high-level data-quality summary that heads a review: four questions a
/// procurement engineer asks of any BOM, answered OK or GAP.
fn audit(items: &[BomItem], findings: &[Finding], mapping: &load::MappingReport) -> Vec<AuditEntry> {
    let fired = |rule: &str| findings.iter().any(|f| f.rule_id.as_deref() == Some(rule));
    let orderable: Vec<&BomItem> = items.iter().filter(|i| !i.dnp && !i.non_orderable()).collect();
    let missing_src = orderable
        .iter()
        .filter(|i| !(i.filled("mpn") || i.filled("mpn_alt") || !i.supplier_pns.is_empty()))
        .count();
    let refdes_bad = fired("bom.duplicate_refdes") || fired("bom.unannotated_refdes");
    let lifecycle_tracked = items.iter().any(|i| i.filled("lifecycle"));
    let rohs_tracked = items.iter().any(|i| i.filled("rohs"));
    // A column in the header that nobody filled is a different audit note from no
    // column at all — the same distinction the rules draw. See `Ctx::has_column`.
    let has_col = |field: &str| mapping.fields.contains_key(field);
    let lifecycle_col = lifecycle_tracked || has_col("lifecycle");
    let rohs_col = rohs_tracked || has_col("rohs");

    let mut out = vec![
        AuditEntry {
            item: "Reference designators — unique, annotated".into(),
            result: if refdes_bad { "GAP".into() } else { "OK".into() },
            note: if refdes_bad {
                "Duplicate or unannotated designators found — see the findings below.".into()
            } else {
                format!("{} lines, all designators unique and annotated.", items.len())
            },
        },
        AuditEntry {
            item: "Sourcing IDs — MPN or distributor PN on every populated part".into(),
            result: if missing_src == 0 { "OK".into() } else { "GAP".into() },
            note: format!(
                "{} of {} orderable parts carry no sourcing identifier.",
                missing_src,
                orderable.len()
            ),
        },
        AuditEntry {
            item: "Lifecycle status verifiable".into(),
            result: if lifecycle_tracked { "OK".into() } else { "GAP".into() },
            note: if lifecycle_tracked {
                "Lifecycle column present.".into()
            } else if lifecycle_col {
                "Lifecycle column present but empty on every line; status could not be \
                 confirmed for any part."
                    .into()
            } else {
                "No lifecycle column; status could not be confirmed for any part.".into()
            },
        },
        AuditEntry {
            item: "RoHS compliance".into(),
            result: if rohs_tracked { "OK".into() } else { "GAP".into() },
            note: if rohs_tracked {
                "RoHS column present.".into()
            } else if rohs_col {
                "RoHS column present but empty on every line.".into()
            } else {
                "No RoHS column in the BOM.".into()
            },
        },
    ];

    // A well-filled column nobody understood is the most common cause of a false
    // "this data is missing" — say so explicitly rather than letting rules imply it.
    let unmapped = mapping.notable_unmapped();
    if !unmapped.is_empty() {
        let cols: Vec<String> = unmapped.iter().take(6).map(|u| u.column.clone()).collect();
        out.push(AuditEntry {
            item: "Column mapping".into(),
            result: "GAP".into(),
            note: format!(
                "{} well-filled column(s) did not map to a known BOM field: {}. If one carries \
                 MPN/manufacturer/lifecycle data, the checks below could not see it.",
                unmapped.len(),
                cols.join(", ")
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_for(csv: &str, profile: &str) -> FindingsDoc {
        let rows = load::parse_csv(csv);
        let (items, mapping) = load::items_from_rows(&rows, &config::config_for(profile));
        run(&items, profile, &mapping)
    }

    #[test]
    fn clean_bom_produces_no_findings_and_a_clean_audit() {
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN,Lifecycle,RoHS\n\
                   R1,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,Active,Yes\n\
                   R2,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,Active,Yes\n\
                   C1,100nF,C_0402_1005Metric,1,Murata,GRM155R61A104KA01D,Active,Yes\n";
        let doc = doc_for(csv, "default");
        assert!(doc.findings.is_empty(), "unexpected: {:?}", doc.findings);
        assert!(doc.bom_audit.iter().all(|a| a.result == "OK"));
        assert_eq!(doc.stats.item_count, 3);
    }

    #[test]
    fn a_column_that_exists_but_is_empty_is_not_reported_as_a_missing_column() {
        // The real MC-02-CONTROL shape: the header carries RoHS, REACH and Lifecycle
        // and not one row fills them. Telling this engineer to "add a REACH column"
        // sends them looking for something already in their own header, and the fix
        // they actually need ("populate the one you have") never gets said.
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN,RoHS,REACH,Lifecycle\n\
                   R1,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,,,\n\
                   R2,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,,,\n\
                   C1,100nF,C_0402_1005Metric,1,Murata,GRM155R61A104KA01D,,,\n";
        let doc = doc_for(csv, "default");

        for f in &doc.findings {
            let text = format!("{} {}", f.title, f.detail);
            assert!(
                !text.contains("has no RoHS column")
                    && !text.contains("has no REACH")
                    && !text.contains("has no lifecycle column")
                    && !text.contains("No lifecycle column"),
                "a present-but-empty column was reported as missing: {} / {}",
                f.title,
                text
            );
        }
        for entry in &doc.bom_audit {
            assert!(
                !entry.note.starts_with("No RoHS column"),
                "audit still claims the column is absent: {entry:?}"
            );
        }

        // And the gap is still reported, just as the right gap.
        assert!(
            doc.findings.iter().any(|f| f.title.contains("empty on every line")),
            "the empty columns produced no finding at all: {:?}",
            doc.findings.iter().map(|f| &f.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_column_the_bom_genuinely_lacks_is_still_reported_as_missing() {
        // The other half: no compliance columns in the header at all. This wording is
        // correct here and must survive the fix above.
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN\n\
                   R1,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL\n\
                   C1,100nF,C_0402_1005Metric,1,Murata,GRM155R61A104KA01D\n";
        let doc = doc_for(csv, "default");
        assert!(
            doc.findings
                .iter()
                .any(|f| f.title.contains("No RoHS") || f.title.contains("Lifecycle status not verifiable")),
            "a BOM with no compliance columns reported no missing-column finding: {:?}",
            doc.findings.iter().map(|f| &f.title).collect::<Vec<_>>()
        );
        assert!(
            !doc.findings.iter().any(|f| f.title.contains("empty on every line")),
            "a column that does not exist was reported as an empty column"
        );
    }

    #[test]
    fn findings_that_land_only_on_dnp_lines_collapse_into_one_info_note() {
        // Two DNP lines, each carrying sourcing data and a bad value. Before the fold
        // that was four separate findings about parts nobody is building — enough
        // noise that a reader learns to skim, and the real defects are in the same
        // list. R1 is populated and broken in the same way, and must survive on its
        // own at its own severity.
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN,DNP\n\
                   R1,10k,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,\n\
                   R90,,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,DNP\n\
                   R91,,R_0402_1005Metric,1,Yageo,RC0402FR-0710KL,DNP\n";
        let doc = doc_for(csv, "default");

        let folded: Vec<&Finding> = doc
            .findings
            .iter()
            .filter(|f| f.rule_id.as_deref() == Some(DNP_FOLD_RULE_ID))
            .collect();
        assert!(folded.len() <= 1, "the fold must produce at most one note: {folded:?}");

        // Nothing else may anchor exclusively to a DNP line.
        for f in &doc.findings {
            if f.rule_id.as_deref() == Some(DNP_FOLD_RULE_ID) {
                continue;
            }
            let refs: Vec<&str> = f
                .anchors
                .iter()
                .flat_map(|a| a.refdes.iter().map(|r| r.as_str()))
                .collect();
            assert!(
                refs.is_empty() || !refs.iter().all(|r| *r == "R90" || *r == "R91"),
                "{} still reports a DNP-only finding: {:?}",
                f.rule_id.as_deref().unwrap_or("?"),
                refs
            );
        }

        // If the fold fired, it is informational and names the lines it covers.
        if let Some(note) = folded.first() {
            assert_eq!(note.severity, "Low", "the DNP note must be informational");
            let refs: Vec<&str> = note
                .anchors
                .iter()
                .flat_map(|a| a.refdes.iter().map(|r| r.as_str()))
                .collect();
            assert!(refs.contains(&"R90") && refs.contains(&"R91"), "refs: {refs:?}");
        }
    }

    #[test]
    fn a_defect_spanning_a_populated_and_a_dnp_line_is_not_folded_away() {
        // The exception that makes the fold safe: one MPN on two footprints, one of
        // them populated. That is a real defect about the populated part, and
        // demoting it to an informational DNP note would hide it.
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN,DNP\n\
                   C31,22uF,C_1210_3225Metric_5mil,1,Samsung,CL32Y226KAVVPJE,\n\
                   C32,22uF,C_1210_3225Metric_6mil,1,Samsung,CL32Y226KAVVPJE,DNP\n";
        let doc = doc_for(csv, "default");

        let spanning: Vec<&Finding> = doc
            .findings
            .iter()
            .filter(|f| {
                f.anchors
                    .iter()
                    .any(|a| a.refdes.iter().any(|r| r == "C31"))
            })
            .collect();
        assert!(
            spanning.iter().all(|f| f.rule_id.as_deref() != Some(DNP_FOLD_RULE_ID)),
            "a finding touching the populated C31 was folded into the DNP note"
        );
    }

    #[test]
    fn findings_are_severity_sorted_and_numbered() {
        let csv = "Reference,Value,Footprint,MPN\n\
                   R1,10k,R_0402,C1\n\
                   R1,10k,R_0402,C1\n";
        let doc = doc_for(csv, "default");
        assert!(!doc.findings.is_empty());
        assert_eq!(doc.findings[0].id, "B01");
        let ranks: Vec<u8> = doc
            .findings
            .iter()
            .map(|f| Severity::parse(&f.severity).rank())
            .collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "not severity-sorted");
        // Duplicate designator is the most severe thing in this BOM.
        assert_eq!(doc.findings[0].rule_id.as_deref(), Some("bom.duplicate_refdes"));
    }

    #[test]
    fn declared_non_aecq_and_unrecorded_aecq_are_separate_findings() {
        // The split exists so L1 — whose datasheet says "Qualified to AEC-Q200." but
        // whose BOM cell is empty — is not reported as "declared not qualified"
        // alongside D1's explicit NO. Both ride at Important: same severity category,
        // different claims, different wording, different fix.
        let csv = "Reference,Value,Footprint,Quantity,Manufacturer,MPN,AEC-Q,MSL,RoHS,REACH,Lifecycle\n\
                   D1,SS2150,D_SMA,1,MCC,SS2150-LTP,NO,1,Yes,Yes,Active\n\
                   L1,220uH,L_1210,1,Sumida,CDRH127L125NP-221MC,,1,Yes,Yes,Active\n\
                   R1,10k,R_0402,1,Yageo,RC0402FR-0710KL,YES,1,Yes,Yes,Active\n";
        let doc = doc_for(csv, "automotive");
        let aecq: Vec<&Finding> = doc
            .findings
            .iter()
            .filter(|f| f.rule_id.as_deref() == Some("bom.missing_aecq"))
            .collect();
        assert_eq!(aecq.len(), 2, "expected a declared-negative and a not-recorded finding: {aecq:?}");

        let declared = aecq
            .iter()
            .find(|f| f.title.contains("declared"))
            .expect("a declared-not-qualified finding");
        assert_eq!(declared.severity, "Important");
        assert!(declared.anchors.iter().any(|a| a.refdes.iter().any(|r| r == "D1")));
        assert!(
            !declared.anchors.iter().any(|a| a.refdes.iter().any(|r| r == "L1")),
            "the blank-column part must not be listed as declared-unqualified"
        );

        let unrecorded = aecq
            .iter()
            .find(|f| f.title.contains("no AEC-Q status recorded"))
            .expect("an unrecorded-status finding");
        assert_eq!(unrecorded.severity, "Important", "a blank cell is a data gap, not a failed part");
        assert_eq!(
            declared.severity, unrecorded.severity,
            "both AEC-Q findings share one severity category"
        );
        assert!(unrecorded.anchors.iter().any(|a| a.refdes.iter().any(|r| r == "L1")));
        // The qualified part appears in neither.
        for f in &aecq {
            assert!(!f.anchors.iter().any(|a| a.refdes.iter().any(|r| r == "R1")));
        }
    }

    #[test]
    fn fingerprint_matches_the_engine_port() {
        // Pinned cross-runtime vector. The paid engine (spinzero-private
        // `engine/src/fingerprint.ts`) re-implements this hash so a validated finding
        // UPDATES the free tier's comment instead of filing a second one beside it;
        // if this constant ever has to change, both sides move together or the app
        // silently starts duplicating every finding.
        assert_eq!(
            fingerprint("bom.duplicate_refdes", &["R2".into(), "r1".into()], "R1 Key"),
            "d74f820695871dbd"
        );
    }

    #[test]
    fn fingerprints_are_stable_across_runs_and_unique_per_defect() {
        let csv = "Reference,Value,Footprint,MPN\nR1,10k,R_0402,TBD\nR2,10k,R_0402,TBD\n";
        let a = doc_for(csv, "default");
        let b = doc_for(csv, "default");
        let fa: Vec<&str> = a.findings.iter().map(|f| f.fingerprint.as_str()).collect();
        let fb: Vec<&str> = b.findings.iter().map(|f| f.fingerprint.as_str()).collect();
        assert_eq!(fa, fb, "fingerprints must be deterministic");
        let unique: std::collections::BTreeSet<&str> = fa.iter().copied().collect();
        assert_eq!(unique.len(), fa.len(), "fingerprints must not collide");
    }

    #[test]
    fn empty_bom_is_handled() {
        let doc = doc_for("Reference,Value\n", "default");
        assert_eq!(doc.stats.item_count, 0);
        assert!(doc.findings.is_empty());
    }

    #[test]
    fn document_matches_the_published_schema_shape() {
        let csv = "Reference,Value,Footprint,MPN\nR1,10k,R_0402,TBD\n";
        let doc = doc_for(csv, "default");
        let v = serde_json::to_value(&doc).expect("serializes");
        assert_eq!(v["schema_version"], "1.1");
        assert_eq!(v["pipeline"], "bom-rules");
        let f = &v["findings"][0];
        for key in ["id", "section", "severity", "confidence", "title", "fingerprint"] {
            assert!(f.get(key).is_some(), "finding missing required key {key}");
        }
        assert_eq!(f["confidence"], "Unvalidated");
        assert!(f["anchors"][0]["type"].is_string());
    }
}
