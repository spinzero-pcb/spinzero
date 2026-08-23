// findings.json — the one review contract, mirroring schemas/findings-1.0.json.
//
// Every review producer emits this document: the free deterministic BOM check
// (Rust `bom-rules`, confidence "Unvalidated") today, the paid detailed review
// later. The app therefore has ONE ingestion path — findings become review
// comments in bomcheck.rs, matched by `fingerprint` — and this file is what the
// BOM tab renders the run summary from.
//
// Keep in sync with schemas/findings-1.0.json and src-tauri/crates/bom-rules.

import type { Comment } from "./types";

export type FindingSeverity = "Critical" | "Major" | "Medium" | "Low" | "Question";
/** "Unvalidated" = a raw rule hit no validation pass has confirmed (the free tier). */
export type FindingConfidence = "High" | "Medium" | "Low" | "Unvalidated";

export interface FindingAnchor {
  /** "bom_row" anchors to designators; "bom" is a document-level finding. */
  type: "bom_row" | "bom";
  refdes?: string[];
  mpn?: string;
}

export interface Finding {
  /** Document-local id (B01…). NOT stable across runs — identity is `fingerprint`. */
  id: string;
  section: string;
  severity: FindingSeverity;
  confidence: FindingConfidence;
  rule_id: string | null;
  title: string;
  detail?: string;
  evidence?: string[];
  fix?: string;
  anchors: FindingAnchor[];
  /** blake3(rule_id | anchors | predicate) — the dedupe key across runs. */
  fingerprint: string;
}

export interface AuditEntry {
  item: string;
  result: "OK" | "GAP" | "TRUNCATED";
  note?: string;
  ref?: string;
}

export interface FindingsDoc {
  schema_version: string;
  engine_version: string;
  /** "bom-rules" (free, local) | "bom-detailed" (paid service). */
  pipeline: string;
  profile: string;
  generated_ts?: string;
  findings: Finding[];
  bom_audit: AuditEntry[];
  stats: { item_count: number; finding_count: number; duration_ms: number };
}

/** What `run_bom_check` returns: the document plus what ingestion did with it. */
export interface CheckOutcome {
  findings: FindingsDoc;
  session_id: string;
  filed: number;
  reopened: number;
  unchanged: number;
  auto_resolved: number;
  /** Well-filled BOM columns that mapped to no known field — a checker blind spot. */
  unmapped_columns: string[];
  comments: Comment[];
}

/** Where one logical field's data comes from. Mirrors `bom_rules::load::FieldMapping`. */
export interface FieldMapping {
  /** Logical field the rules read, e.g. "mpn", "lifecycle". */
  logical: string;
  /** Source column feeding it right now; "" = nothing feeds it. */
  column: string;
  /** What the alias table alone would have picked, so the dialog can offer "auto". */
  auto: string;
  /** `column` came from the approved mapping rather than the aliases. */
  overridden: boolean;
}

/** One real BOM column, with enough context to recognise it in a dropdown. */
export interface SourceColumn {
  name: string;
  /** 0..1 — share of rows carrying a value. */
  fill_rate: number;
  /** First non-empty cell, truncated backend-side. */
  sample: string;
}

/** What `get_bom_mapping` returns: the mapping to approve, and whether it ever was.
 *  Mirrors `bomcheck::MappingView` (which flattens `MappingPreview` into it). */
export interface MappingView {
  fields: FieldMapping[];
  columns: SourceColumn[];
  unmapped_columns: { column: string; fill_rate: number }[];
  row_count: number;
  /** False = the user has never been through the dialog, so a review should ask first. */
  approved: boolean;
}

/** End-application profiles, in the order the picker offers them. Mirrors
 *  `bom_rules::config::PROFILES`; the label is what the user sees. */
export const BOM_PROFILES = [
  { id: "default", label: "General" },
  { id: "industrial", label: "Industrial" },
  { id: "medical", label: "Medical" },
  { id: "automotive", label: "Automotive" },
] as const;

export type BomProfile = (typeof BOM_PROFILES)[number]["id"];

export function isBomProfile(v: unknown): v is BomProfile {
  return typeof v === "string" && BOM_PROFILES.some((p) => p.id === v);
}

export const SEVERITY_ORDER: FindingSeverity[] = [
  "Critical",
  "Major",
  "Medium",
  "Low",
  "Question",
];

/** Findings per severity, highest first — the summary strip's data. */
export function severityCounts(doc: FindingsDoc): { severity: FindingSeverity; n: number }[] {
  return SEVERITY_ORDER.map((severity) => ({
    severity,
    n: doc.findings.filter((f) => f.severity === severity).length,
  })).filter((s) => s.n > 0);
}
