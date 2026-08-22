//! BOM check (free tier): run the deterministic `bom-rules` crate over the crunched
//! BOM, then ingest its `findings.json` as review comments.
//!
//! Ingestion is the part that matters long-term: the paid detailed review (plan §10)
//! emits the *same* findings document and lands through this same path — see
//! `ingest_findings` in lib.rs, which feeds it the service's document. Reconciliation
//! is per-producer (`source_for`), and the identity key is the finding
//! `fingerprint`:
//!
//! - fingerprint already on a comment → leave the thread alone (only the severity is
//!   refreshed), so replies/assignments survive a re-run;
//! - fingerprint gone → auto-resolve with `AUTO_RESOLVED_REASON`;
//! - previously auto-resolved fingerprint returns → re-open it. A comment a *human*
//!   resolved or dismissed stays that way — re-running a check must never overrule a
//!   person's judgment.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use bom_rules::{config, load, FindingsDoc};

use crate::design::BomLine;
use crate::reviews;

/// Reason stamped on comments the checker closed itself — and the marker that lets a
/// later run tell its own auto-resolve apart from a human "resolved".
pub const AUTO_RESOLVED_REASON: &str = "no longer detected by the BOM check";

/// Which producer filed a comment. Reconciliation is scoped to ONE producer: a
/// re-run of the free check never auto-resolves a paid finding, and vice versa,
/// while a finding both tiers detect shares a fingerprint and so shares a comment —
/// which is how the paid review *refines* the free result instead of duplicating it
/// (plan §10.4).
///
/// The review comment's `source` follows the pipeline: deterministic rules file as
/// "rule", the LLM pipeline as "agent". The rail already renders those two
/// differently, so a reader can tell a machine-checked claim from a judged one.
pub fn source_for(pipeline: &str) -> &'static str {
    if pipeline == "bom-rules" {
        "rule"
    } else {
        "agent"
    }
}

#[derive(Serialize)]
pub struct CheckOutcome {
    /// The full findings document (schema `findings-1.0.json`) — the BOM tab renders
    /// from this directly, without waiting for a comment reload.
    pub findings: FindingsDoc,
    /// Review session the comments were filed into.
    pub session_id: String,
    pub filed: usize,
    pub reopened: usize,
    pub unchanged: usize,
    pub auto_resolved: usize,
    /// Well-filled BOM columns that mapped to no known field — a checker blind spot
    /// the user should see, not a design defect.
    pub unmapped_columns: Vec<String>,
    /// Comments after ingestion, so the frontend refreshes in one round-trip.
    pub comments: Vec<reviews::Comment>,
}

/// Findings-schema severity → the review comment vocabulary (four levels).
fn comment_severity(severity: &str) -> &'static str {
    match severity {
        "Critical" => "critical",
        "Major" => "major",
        "Medium" => "minor",
        _ => "info",
    }
}

/// The BOM rows as the rules want them: raw (header, cell) pairs. The crunched BOM
/// carries every symbol field verbatim in `fields`, which is exactly the shape a real
/// BOM CSV has — so the same alias mapping works on both.
///
/// Designators, quantity and DNP come from the extractor's own columns rather than
/// the field map (KiCad's virtual fields are authoritative there), so any raw field
/// that would map to the same logical field is dropped first to avoid a double map.
fn rows_from_bom_lines(lines: &[BomLine], profile: &str) -> Vec<load::Row> {
    let cfg = config::config_for(profile);
    let shadowed: BTreeSet<String> = ["reference", "quantity", "dnp"]
        .iter()
        .flat_map(|logical| load::alias_canon_set(&cfg, logical))
        .collect();

    // One header set for every row: rules read columns, and a column that exists on
    // only some rows would make fill rates and "is this column present" meaningless.
    let mut headers: BTreeSet<String> = BTreeSet::new();
    for line in lines {
        for key in line.fields.keys() {
            if !shadowed.contains(&load::canon_header(key)) {
                headers.insert(key.clone());
            }
        }
    }

    lines
        .iter()
        .map(|line| {
            let mut row: load::Row = vec![
                ("Reference".to_string(), line.designators.join(", ")),
                ("Quantity".to_string(), line.qty.to_string()),
                ("DNP".to_string(), if line.dnp { "DNP".into() } else { String::new() }),
            ];
            for h in &headers {
                row.push((h.clone(), line.fields.get(h).cloned().unwrap_or_default()));
            }
            row
        })
        .collect()
}

/// Run the deterministic checks over the crunched BOM. Pure: no project writes.
pub fn run_rules(lines: &[BomLine], profile: &str) -> (FindingsDoc, load::MappingReport) {
    let started = std::time::Instant::now();
    let rows = rows_from_bom_lines(lines, profile);
    let (items, mapping) = load::items_from_rows(&rows, &config::config_for(profile));
    let mut doc = bom_rules::run(&items, profile, &mapping);
    doc.generated_ts = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    doc.stats.duration_ms = started.elapsed().as_millis() as u64;
    (doc, mapping)
}

/// Title for the auto-created session. Date-scoped: a run re-run the same day lands
/// in the same session, a run next week starts a fresh one. The tier is part of the
/// title because the two producers reconcile separately — mixing them in one session
/// would make "what did the detailed review add?" unanswerable in the rail.
fn session_title(now: OffsetDateTime, pipeline: &str) -> String {
    let label = if pipeline == "bom-rules" { "BOM check" } else { "Detailed BOM review" };
    format!(
        "{label} {:04}-{:02}-{:02}",
        now.year(),
        now.month() as u8,
        now.day()
    )
}

/// Find (or create) the session findings are filed into.
fn ensure_session(pcbreview: &Path, user: &str, title: &str) -> Result<String, String> {
    if let Some(existing) = reviews::list_sessions(pcbreview)
        .into_iter()
        .find(|s| s.title == title)
    {
        return Ok(existing.id);
    }
    let sessions = reviews::apply_session_action(
        pcbreview,
        user,
        reviews::SessionActionInput {
            action: "create".into(),
            session_id: None,
            title: Some(title.to_string()),
            status: None,
        },
    )?;
    sessions
        .iter()
        .find(|s| s.title == title)
        .map(|s| s.id.clone())
        .ok_or_else(|| "could not create the BOM check session".to_string())
}

/// The comment body a finding becomes: the description on its own. The rule title
/// and the suggested fix are deliberately left out — the detail already says what is
/// wrong, and a reviewer reads a comment, not a rule report.
fn body_of(finding: &bom_rules::Finding) -> String {
    if finding.detail.is_empty() {
        finding.title.clone()
    } else {
        finding.detail.clone()
    }
}

fn blank_action(action: &str) -> reviews::ActionInput {
    reviews::ActionInput {
        action: action.to_string(),
        comment_id: None,
        comment_ids: None,
        anchor: None,
        view: None,
        session_id: None,
        base_revision: None,
        object_hash: None,
        object_meta: None,
        source: None,
        predicate: None,
        evidence: None,
        fingerprint: None,
        body: None,
        severity: None,
        status: None,
        reason: None,
        assignee: None,
        author_name: None,
    }
}

/// File `doc`'s findings as review comments, reconciling against what a previous run
/// left behind. Returns the outcome the BOM tab renders.
pub fn ingest(
    pcbreview: &Path,
    user: &str,
    base_revision: Option<String>,
    doc: FindingsDoc,
    mapping: &load::MappingReport,
) -> Result<CheckOutcome, String> {
    let now = OffsetDateTime::now_utc();
    let session_id = ensure_session(pcbreview, user, &session_title(now, &doc.pipeline))?;

    let source = source_for(&doc.pipeline);
    let existing = reviews::list_comments(pcbreview);

    // Two different scopes, for two different questions.
    //
    // `filed_by_a_checker` — every machine-filed comment, whichever tier filed it.
    // A finding whose fingerprint is already here is the SAME defect, so it updates
    // that comment instead of filing a second one: this is what makes the paid review
    // visibly *refine* the free result rather than double it (plan §10.4).
    let filed_by_a_checker: BTreeMap<String, &reviews::Comment> = existing
        .iter()
        .filter(|c| c.source == "rule" || c.source == "agent")
        .filter_map(|c| c.fingerprint.clone().map(|f| (f, c)))
        .collect();

    // `mine` — comments THIS producer filed. Only these may be auto-resolved: a tier
    // must never close a comment it could not have produced (the free rules cannot
    // re-derive a judgment finding, so a free re-run must not "no longer detect" it).
    let mine: BTreeMap<String, &reviews::Comment> = existing
        .iter()
        .filter(|c| {
            c.predicate
                .as_ref()
                .and_then(|p| p.get("pipeline"))
                .and_then(|v| v.as_str())
                == Some(doc.pipeline.as_str())
        })
        .filter_map(|c| c.fingerprint.clone().map(|f| (f, c)))
        .collect();

    // Every action this run takes, applied as one log write and one fold at the end.
    // Filing them one at a time rewrote the whole event log and re-folded every log per
    // finding — 22 findings were 22 rewrites and 22 folds.
    let mut actions: Vec<reviews::ActionInput> = Vec::new();
    let mut filed = 0;
    let mut reopened = 0;
    let mut unchanged = 0;
    let mut auto_resolved = 0;
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for finding in &doc.findings {
        seen.insert(finding.fingerprint.clone());
        let severity = comment_severity(&finding.severity).to_string();
        let predicate = serde_json::json!({
            "pipeline": doc.pipeline,
            "profile": doc.profile,
            "rule_id": finding.rule_id,
            "engine_version": doc.engine_version,
            "confidence": finding.confidence,
        });

        match filed_by_a_checker.get(&finding.fingerprint) {
            Some(comment) => {
                let id = comment.id.clone();
                // A person's resolve/dismiss stands; only our own auto-resolve reopens.
                let ours = comment.reason.as_deref() == Some(AUTO_RESOLVED_REASON);
                if ours && (comment.status == "resolved" || comment.status == "dismissed") {
                    let mut action = blank_action("status");
                    action.comment_id = Some(id.clone());
                    action.status = Some("open".into());
                    action.reason = Some("detected again by the BOM check".into());
                    actions.push(action);
                    reopened += 1;
                } else {
                    unchanged += 1;
                }
                // A profile change can re-rank the same defect; keep the comment,
                // refresh the severity.
                if comment.severity.as_deref() != Some(severity.as_str()) {
                    let mut action = blank_action("severity");
                    action.comment_id = Some(id);
                    action.severity = Some(severity);
                    actions.push(action);
                }
            }
            None => {
                let mut action = blank_action("create");
                action.anchor = Some(anchor_for(finding));
                action.view = Some("bom".into());
                action.session_id = Some(session_id.clone());
                action.base_revision = base_revision.clone();
                action.source = Some(source.into());
                action.severity = Some(severity);
                action.body = Some(body_of(finding));
                action.predicate = Some(predicate);
                action.evidence = Some(serde_json::json!({
                    "section": finding.section,
                    "evidence": finding.evidence,
                    "anchors": finding.anchors,
                }));
                action.fingerprint = Some(finding.fingerprint.clone());
                actions.push(action);
                filed += 1;
            }
        }
    }

    // Anything this producer filed before and no longer detects is closed with a
    // reason, so the rail shows *why* it went away instead of silently dropping it.
    for (fingerprint, comment) in &mine {
        if seen.contains(fingerprint) || comment.status != "open" {
            continue;
        }
        let mut action = blank_action("status");
        action.comment_id = Some(comment.id.clone());
        action.status = Some("resolved".into());
        action.reason = Some(AUTO_RESOLVED_REASON.into());
        actions.push(action);
        auto_resolved += 1;
    }

    let comments = reviews::apply_actions(pcbreview, user, actions)?;

    log::info!(
        "bom check ({}, profile {}): {} findings — {} new, {} unchanged, {} reopened, {} auto-resolved",
        doc.pipeline,
        doc.profile,
        doc.findings.len(),
        filed,
        unchanged,
        reopened,
        auto_resolved
    );

    Ok(CheckOutcome {
        session_id,
        filed,
        reopened,
        unchanged,
        auto_resolved,
        unmapped_columns: mapping
            .notable_unmapped()
            .iter()
            .map(|u| u.column.clone())
            .collect(),
        comments,
        findings: doc,
    })
}

/// Where a finding hangs in the review UI: on its first BOM row, or on the BOM as a
/// whole for a document-level finding ("this BOM has no lifecycle column").
fn anchor_for(finding: &bom_rules::Finding) -> reviews::Anchor {
    let refdes = finding
        .anchors
        .iter()
        .find(|a| a.kind == "bom_row")
        .and_then(|a| a.refdes.first().cloned());
    match refdes {
        Some(r) => reviews::Anchor {
            kind: "component".into(),
            r#ref: r,
            sheet: None,
            rect: None,
            at: None,
        },
        // A "bom" anchor points at no object, so it never participates in the ⟳
        // drift loop — same contract as a region anchor.
        None => reviews::Anchor {
            kind: "bom".into(),
            r#ref: "BOM".into(),
            sheet: None,
            rect: None,
            at: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("spinzero_bomchk_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn line(item: i64, designators: &[&str], value: &str, mpn: &str) -> BomLine {
        let mut fields = HashMap::new();
        fields.insert("Value".to_string(), value.to_string());
        fields.insert("Footprint".to_string(), "R_0402_1005Metric".to_string());
        fields.insert("Manufacturer".to_string(), "Yageo".to_string());
        fields.insert("Manufacturer Part Number".to_string(), mpn.to_string());
        BomLine {
            item,
            qty: designators.len() as i64,
            designators: designators.iter().map(|s| s.to_string()).collect(),
            value: value.into(),
            footprint: "R_0402_1005Metric".into(),
            mpn: mpn.into(),
            dnp: false,
            fields,
        }
    }

    #[test]
    fn extractor_columns_win_over_symbol_fields() {
        // A symbol field literally named "Reference" must not shadow the extractor's
        // designator list, or a grouped line would check only its first part.
        let mut l = line(1, &["R1", "R2"], "10k", "RC0402FR-0710KL");
        l.fields.insert("Reference".into(), "WRONG".into());
        let rows = rows_from_bom_lines(&[l], "default");
        let reference: Vec<&String> = rows[0]
            .iter()
            .filter(|(h, _)| h == "Reference")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(reference, vec![&"R1, R2".to_string()]);
    }

    #[test]
    fn findings_become_comments_then_auto_resolve_when_fixed() {
        let root = temp_root("ingest");
        let pcb = root.join(".pcbreview");

        // A duplicate designator across two lines: one Critical finding.
        let broken = vec![
            line(1, &["R1"], "10k", "RC0402FR-0710KL"),
            line(2, &["R1"], "4k7", "RC0402FR-074K7L"),
        ];
        let (doc, mapping) = run_rules(&broken, "default");
        assert!(doc
            .findings
            .iter()
            .any(|f| f.rule_id.as_deref() == Some("bom.duplicate_refdes")));
        let out = ingest(&pcb, "alice", Some("r1".into()), doc, &mapping).expect("ingest");
        assert!(out.filed > 0);
        let filed_first = out.filed;
        let dup = out
            .comments
            .iter()
            .find(|c| c.severity.as_deref() == Some("critical"))
            .expect("critical comment filed");
        assert_eq!(dup.source, "rule");
        assert_eq!(dup.view, "bom");
        assert_eq!(dup.anchor.r#ref, "R1");
        assert!(dup.fingerprint.is_some());

        // Re-running an unchanged BOM must not duplicate anything.
        let (doc, mapping) = run_rules(&broken, "default");
        let again = ingest(&pcb, "alice", Some("r1".into()), doc, &mapping).expect("re-ingest");
        assert_eq!(again.filed, 0, "a re-run must not re-file findings");
        assert_eq!(again.unchanged, filed_first);

        // Fix the BOM: the comment closes itself, with a reason.
        let fixed = vec![
            line(1, &["R1"], "10k", "RC0402FR-0710KL"),
            line(2, &["R2"], "4k7", "RC0402FR-074K7L"),
        ];
        let (doc, mapping) = run_rules(&fixed, "default");
        let out = ingest(&pcb, "alice", Some("r2".into()), doc, &mapping).expect("ingest fixed");
        assert!(out.auto_resolved > 0);
        let dup = out
            .comments
            .iter()
            .find(|c| c.id == dup.id)
            .expect("comment still exists");
        assert_eq!(dup.status, "resolved");
        assert_eq!(dup.reason.as_deref(), Some(AUTO_RESOLVED_REASON));

        // Break it again: our own auto-resolve reopens.
        let (doc, mapping) = run_rules(&broken, "default");
        let out = ingest(&pcb, "alice", Some("r3".into()), doc, &mapping).expect("ingest broken");
        assert!(out.reopened > 0);
        assert_eq!(out.filed, 0, "reopen, never re-file");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A findings document as the paid service would send it: same fingerprint as a
    /// free-tier finding, higher confidence, plus a judgment finding of its own.
    fn paid_doc(free: &FindingsDoc) -> FindingsDoc {
        let mut doc = free.clone();
        doc.pipeline = "bom-detailed".into();
        doc.engine_version = "engine-0.1.0".into();
        for f in &mut doc.findings {
            f.confidence = "High".into();
        }
        doc.findings.push(bom_rules::Finding {
            id: "B99".into(),
            section: "BOM · Judgment".into(),
            severity: "Major".into(),
            confidence: "Medium".into(),
            rule_id: None,
            title: "R1 MPN decodes to 4.7k but the value says 10k".into(),
            detail: String::new(),
            evidence: vec![],
            fix: String::new(),
            anchors: vec![bom_rules::Anchor { kind: "bom_row".into(), refdes: vec!["R1".into()] }],
            fingerprint: "judgment0000fingerprint"[..16].to_string(),
        });
        doc
    }

    #[test]
    fn the_paid_review_refines_the_free_comment_instead_of_duplicating_it() {
        let root = temp_root("tiers");
        let pcb = root.join(".pcbreview");
        let broken = vec![
            line(1, &["R1"], "10k", "RC0402FR-0710KL"),
            line(2, &["R1"], "4k7", "RC0402FR-074K7L"),
        ];

        // Free tier files its findings.
        let (free, mapping) = run_rules(&broken, "default");
        let free_count = free.findings.len();
        let out = ingest(&pcb, "alice", None, free.clone(), &mapping).expect("free ingest");
        assert_eq!(out.filed, free_count);

        // The paid review reports the same defects (same fingerprints) plus one of
        // its own: only the new one is filed, the rest update in place.
        let paid = paid_doc(&free);
        let out = ingest(&pcb, "alice", None, paid, &load::MappingReport::default())
            .expect("paid ingest");
        assert_eq!(out.filed, 1, "only the judgment finding is new");
        assert_eq!(out.unchanged, free_count, "the rule findings refine the existing comments");
        assert_eq!(
            out.comments.iter().filter(|c| c.fingerprint.is_some()).count(),
            free_count + 1,
            "no duplicate comment for a defect both tiers found"
        );
        let judgment = out
            .comments
            .iter()
            .find(|c| c.source == "agent")
            .expect("the judgment finding filed as an agent comment");
        assert_eq!(judgment.view, "bom");

        // A later FREE run must not close the judgment finding: the rules cannot
        // produce it, so "no longer detected" would be a lie.
        let (free_again, mapping) = run_rules(&broken, "default");
        let out = ingest(&pcb, "alice", None, free_again, &mapping).expect("free re-ingest");
        assert_eq!(out.auto_resolved, 0);
        let judgment = out
            .comments
            .iter()
            .find(|c| c.source == "agent")
            .expect("still there");
        assert_eq!(judgment.status, "open");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_human_resolve_survives_a_re_run() {
        let root = temp_root("human");
        let pcb = root.join(".pcbreview");
        let broken = vec![
            line(1, &["R1"], "10k", "RC0402FR-0710KL"),
            line(2, &["R1"], "4k7", "RC0402FR-074K7L"),
        ];
        let (doc, mapping) = run_rules(&broken, "default");
        let out = ingest(&pcb, "alice", None, doc, &mapping).expect("ingest");
        let id = out.comments[0].id.clone();

        let mut action = blank_action("status");
        action.comment_id = Some(id.clone());
        action.status = Some("dismissed".into());
        action.reason = Some("intentional, variant build".into());
        reviews::apply_action(&pcb, "alice", action).expect("dismiss");

        let (doc, mapping) = run_rules(&broken, "default");
        let out = ingest(&pcb, "alice", None, doc, &mapping).expect("re-ingest");
        let c = out.comments.iter().find(|c| c.id == id).expect("still there");
        assert_eq!(c.status, "dismissed", "a person's judgment is not overruled");
        let _ = std::fs::remove_dir_all(&root);
    }
}
