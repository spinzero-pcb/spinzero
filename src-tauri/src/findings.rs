//! The findings document, and the column mapping that goes with it.
//!
//! These are the PUBLISHED shapes — `schemas/findings-1.3.json` is the same document
//! written down — and they live here rather than inside the rule pack because the app
//! reads them from three places, only one of which is the rule pack:
//!
//!   * the free BOM check, which runs `bom-rules` as a subprocess;
//!   * the paid review, which arrives over the network;
//!   * the review drop-box, `<project>/reviews/inbox/`, which any MCP client can write.
//!
//! Two of those three exist whether or not a rule pack is installed on this machine,
//! so a type they all decode into cannot be owned by the rule pack. It used to be: the
//! crate was compiled into this app, and `bomcheck` imported `bom_rules::FindingsDoc`
//! for the network document as well. When the rules moved out, the types stayed.
//!
//! Unknown fields are ignored by serde on purpose. That is what lets the paid engine
//! add a field — `stats.tokens`, a new `run_health` status — and ship it to an app
//! built before it existed, which is the normal case, not the exception.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ the document

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
    /// `Unvalidated` from the free tier: a raw rule hit no validation pass has
    /// confirmed. The paid pipeline replaces it with High or Low.
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

/// One stage of a review that did not fully run (schema: `run_health`). The free tier
/// never emits these — it is deterministic and has nothing to degrade — but the paid
/// engine does, and the app shows them so an incomplete review cannot read as a clean
/// one.
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
    /// RFC3339, stamped by whoever ran the producer — the rule pack has no clock, so
    /// that its fixture output is byte-stable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generated_ts: String,
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub bom_audit: Vec<AuditEntry>,
    #[serde(default)]
    pub stats: Stats,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_health: Vec<RunHealthEntry>,
    /// How the review was produced, and which of the user's columns it read.
    ///
    /// Carried VERBATIM and never inspected here. Both blocks exist for the frontend,
    /// which decodes them against `schemas/findings-1.3.json`; giving Rust a second
    /// typed copy would be two definitions of one contract, and the day they drift is
    /// the day a field silently stops arriving. Untyped, they cannot drift — they can
    /// only be dropped, which is what this field exists to stop.
    ///
    /// Dropped is what they were: a document came in through the drop-box, was parsed
    /// into this struct, and went to the frontend without either one. So the
    /// provenance chip and the column-mapping panel had nothing to show, for every
    /// review that ever ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_mapping: Option<serde_json::Value>,
}

// ------------------------------------------------------------------- the mapping

/// What the rule pack actually read: which source column fed which logical field.
///
/// Reported rather than assumed. A finding that says "no manufacturer" is a different
/// claim depending on whether the BOM has no manufacturer column or has one nobody
/// mapped, and the second is our bug rather than the user's.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct MappingReport {
    /// logical field → the source column it was read from.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// supplier label → the source columns carrying its part numbers.
    #[serde(default)]
    pub supplier_columns: BTreeMap<String, Vec<String>>,
    /// Columns no logical field claimed, worst-offender (most filled) first.
    #[serde(default)]
    pub unmapped_columns: Vec<UnmappedColumn>,
    #[serde(default)]
    pub row_count: usize,
}

impl MappingReport {
    /// Unmapped columns filled on at least half the rows — the ones worth telling the
    /// user about (an unmapped column filled on 2% of rows is noise).
    pub fn notable_unmapped(&self) -> Vec<&UnmappedColumn> {
        self.unmapped_columns
            .iter()
            .filter(|u| u.fill_rate >= 0.5)
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnmappedColumn {
    pub column: String,
    /// 0..1 — how many rows carry a value in this column.
    pub fill_rate: f64,
}

/// The approval dialog's view: the alias guess, the approved mapping applied on top,
/// and the raw columns both were chosen from.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct MappingPreview {
    /// One entry per logical field the profile knows about, name-sorted. The dialog
    /// re-orders them for reading; this stays stable so two previews compare.
    #[serde(default)]
    pub fields: Vec<FieldMapping>,
    /// Every column the BOM actually has, so the dialog can offer them.
    #[serde(default)]
    pub columns: Vec<SourceColumn>,
    /// Columns no field claims — the ones whose data the rules cannot see at all.
    #[serde(default)]
    pub unmapped_columns: Vec<UnmappedColumn>,
    #[serde(default)]
    pub row_count: usize,
}

/// Where one logical field's data comes from, and whether that was our guess or the
/// user's decision.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FieldMapping {
    pub logical: String,
    /// Source column feeding this field right now; empty = nothing feeds it.
    pub column: String,
    /// What the aliases alone would have picked, so the dialog can offer "back to auto".
    pub auto: String,
    /// `column` differs from what the aliases alone would have picked — i.e. someone
    /// decided this, whether just now or in a mapping approved long ago.
    pub overridden: bool,
}

/// One real BOM column, with just enough context to recognise it in a dropdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceColumn {
    pub name: String,
    /// 0..1 — how many rows carry a value. A column filled on 3% of rows is rarely
    /// the one you meant to map.
    pub fill_rate: f64,
    /// First non-empty cell, truncated. "Value → 100nF" settles the question that the
    /// column name alone often does not.
    pub sample: String,
}
