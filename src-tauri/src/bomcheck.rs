//! BOM check (free tier): run the deterministic `bom-rules` crate over the crunched
//! BOM, then ingest its `findings.json` as review comments.
//!
//! Ingestion is the part that matters long-term: the paid detailed review (plan §10)
//! emits the *same* findings document, so it lands through this same path with a
//! higher confidence and updates the free-tier comments in place. The identity key is
//! the finding `fingerprint`:
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

/// Marks which producer filed a comment, so a re-run only ever auto-resolves its own
/// findings and never touches the paid review's (or a human's).
const PIPELINE: &str = "bom-rules";

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

/// Title for the auto-created session. Date-scoped: a check re-run the same day lands
/// in the same session, a check next week starts a fresh one.
fn session_title(now: OffsetDateTime) -> String {
    format!(
        "BOM check {:04}-{:02}-{:02}",
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

/// The comment body a finding becomes: the claim, why it matters, then the fix.
fn body_of(finding: &bom_rules::Finding) -> String {
    let mut body = finding.title.clone();
    if !finding.detail.is_empty() {
        body.push_str("\n\n");
        body.push_str(&finding.detail);
    }
    if !finding.fix.is_empty() {
        body.push_str("\n\nFix: ");
        body.push_str(&finding.fix);
    }
    body
}

fn blank_action(action: &str) -> reviews::ActionInput {
    reviews::ActionInput {
        action: action.to_string(),
        comment_id: None,
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
    let session_id = ensure_session(pcbreview, user, &session_title(now))?;

    // Every comment this producer has ever filed in this project, by fingerprint.
    let existing = reviews::list_comments(pcbreview);
    let mine: BTreeMap<String, &reviews::Comment> = existing
        .iter()
        .filter(|c| {
            c.source == "rule"
                && c.predicate
                    .as_ref()
                    .and_then(|p| p.get("pipeline"))
                    .and_then(|v| v.as_str())
                    == Some(PIPELINE)
        })
        .filter_map(|c| c.fingerprint.clone().map(|f| (f, c)))
        .collect();

    let mut comments = existing.clone();
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

        match mine.get(&finding.fingerprint) {
            Some(comment) => {
                let id = comment.id.clone();
                // A person's resolve/dismiss stands; only our own auto-resolve reopens.
                let ours = comment.reason.as_deref() == Some(AUTO_RESOLVED_REASON);
                if ours && (comment.status == "resolved" || comment.status == "dismissed") {
                    let mut action = blank_action("status");
                    action.comment_id = Some(id.clone());
                    action.status = Some("open".into());
                    action.reason = Some("detected again by the BOM check".into());
                    comments = reviews::apply_action(pcbreview, user, action)?;
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
                    comments = reviews::apply_action(pcbreview, user, action)?;
                }
            }
            None => {
                let mut action = blank_action("create");
                action.anchor = Some(anchor_for(finding));
                action.view = Some("bom".into());
                action.session_id = Some(session_id.clone());
                action.base_revision = base_revision.clone();
                action.source = Some("rule".into());
                action.severity = Some(severity);
                action.body = Some(body_of(finding));
                action.predicate = Some(predicate);
                action.evidence = Some(serde_json::json!({
                    "section": finding.section,
                    "evidence": finding.evidence,
                    "anchors": finding.anchors,
                }));
                action.fingerprint = Some(finding.fingerprint.clone());
                comments = reviews::apply_action(pcbreview, user, action)?;
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
        comments = reviews::apply_action(pcbreview, user, action)?;
        auto_resolved += 1;
    }

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
