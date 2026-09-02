// findings.json — the one review contract, mirroring schemas/findings-1.2.json.
//
// Every review producer emits this document: the free deterministic BOM check
// (Rust `bom-rules`, confidence "Unvalidated") today, the paid detailed review
// later. The app therefore has ONE ingestion path — findings become review
// comments in bomcheck.rs, matched by `fingerprint` — and this file is what the
// BOM tab renders the run summary from.
//
// Keep in sync with schemas/findings-1.2.json and src-tauri/crates/bom-rules.

import type { Comment } from "./types";

/** Two levels, deliberately. "Critical" = act before this ships; "Non-critical" =
 *  worth knowing, not a blocker. How SURE the reviewer is lives in `confidence`. */
export type FindingSeverity = "Critical" | "Non-critical";
/** What findings-1.0 and 1.1 called the same two levels. Documents in a user's project
 *  folder outlive the rename, so every reader normalises through `severityOf` rather
 *  than comparing `f.severity` directly. */
export type LegacyFindingSeverity = "Important" | "Observation";

/** One finding's severity in current vocabulary, whatever version wrote it. */
export function severityOf(f: { severity: string }): FindingSeverity {
  if (f.severity === "Important") return "Critical";
  if (f.severity === "Observation") return "Non-critical";
  return f.severity === "Critical" ? "Critical" : "Non-critical";
}
/** "High" = verified against a datasheet/distributor/KB record; "Low" = plausible but
 *  unverified, so the engineer must confirm it; "Unvalidated" = a raw rule hit no
 *  validation pass has looked at (the free tier). */
export type FindingConfidence = "High" | "Low" | "Unvalidated";

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

/** One stage of a review that did not fully run. Only non-clean stages appear, so a
 *  non-empty `run_health` means "this result is incomplete" — and says why. */
export interface RunHealthEntry {
  /** Producer stage id ("deterministic_rules", "judgment_pass"). */
  stage: string;
  /** "degraded" = it ran but covered less than it should; "failed" = it produced nothing. */
  status: "degraded" | "failed";
  detail?: string;
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
  /** Stages that degraded or failed. Absent on a clean run (and on the free tier,
   *  which is deterministic and has nothing to degrade). */
  run_health?: RunHealthEntry[];
  /** How this review was produced. Absent on the free tier. */
  execution?: Execution;
}

/** Which surface produced a review, and with which content.
 *
 *  Worth showing next to a clean result: a review reasoned by the user's own agent
 *  through the SpinZero MCP harness had our workflow, our evidence and our validation
 *  but somebody else's model doing the judging, and a reader is entitled to know that
 *  before they trust it. `prompt_pack` is the other half — two reviews of the same
 *  board that disagree are explained by a content version far more often than by a
 *  regression. Mirrors `$defs/execution` in `schemas/findings-1.2.json`. */
export interface Execution {
  surface: "local" | "mcp" | "hosted";
  /** What the client reported itself as. Never verified — read it as a claim. */
  model_reported?: string;
  /** "builtin/<hash>" or "pack/<version>". */
  prompt_pack?: string;
  rule_pack?: string;
  /** True when the datasheet coverage gate was deliberately overridden. */
  allow_low_coverage?: boolean;
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

/**
 * One findings document waiting in the project's review drop-box
 * (`<project>/reviews/inbox/`). Mirrors `bomcheck::InboxEntry`.
 *
 * The drop-box is how a review that ran outside the app gets in: the engine CLI on
 * this machine, or the user's own agent through the MCP server. It lands as review
 * comments through the same ingestion path a hosted review takes, so a finding both
 * tiers detect still refines one comment rather than filing two.
 */
export interface ReviewInboxEntry {
  /** Bare file name inside the inbox — what `importReviewInbox` is called with. */
  name: string;
  pipeline: string;
  engine_version: string;
  finding_count: number;
  /** Why this file cannot be imported, when it cannot. A junk file in the drop-box
   *  is shown rather than skipped: a review the user believes ran and cannot find is
   *  worse than an error message. */
  error: string | null;
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
 *  `bom_rules::config::PROFILES`; the label is what the user sees.
 *
 *  `default` is deliberately absent, and this is the visible half of a rule change:
 *  it is no longer a profile meaning "general", it is the profile meaning **nobody
 *  said**, and `config_for` gives it the strictest setting of every rule. The picker
 *  must therefore never offer it — offering it would present the strictest review as
 *  the neutral one. What used to be "General" is now `commercial`, with the same
 *  rules it always had. */
export const BOM_PROFILES = [
  { id: "commercial", label: "Commercial" },
  { id: "industrial", label: "Industrial" },
  { id: "medical", label: "Medical" },
  // Automotive is two answers, because AEC-Q200 is not one grade: a part can carry it
  // and still be excluded by its own manufacturer from braking and steering. Which of
  // those two facts is a Critical finding depends on which half of the car this board
  // is in, and one option could not ask. Mirrors `bom_rules::config::PROFILES`.
  { id: "automotive-comfort", label: "Automotive Infotainment, body and chassis" },
  { id: "automotive-safety", label: "Automotive Powertrain/Safety" },
] as const;

/** The single `automotive` id these two replaced. A project that stored it keeps
 *  resolving, to the STRICTER half: a board we know is automotive and do not know is
 *  comfort-only must not have its driving-function findings skipped. */
export const RETIRED_BOM_PROFILES: Record<string, BomProfile> = {
  automotive: "automotive-safety",
};

export type BomProfile = (typeof BOM_PROFILES)[number]["id"];

/** The unstated profile. Not selectable, not offered, and strictest — see
 *  `BOM_PROFILES`. Named rather than spelled "default" at each use so a search for
 *  it finds every place the concept appears. */
export const UNSTATED_BOM_PROFILE = "default";

/** Accepted, which is a wider set than offered: projects created before the rename
 *  have `"default"` persisted, and a stored value must keep resolving rather than
 *  failing validation and silently becoming something else. */
export function isBomProfile(v: unknown): v is BomProfile | typeof UNSTATED_BOM_PROFILE {
  return (
    typeof v === "string" &&
    (v === UNSTATED_BOM_PROFILE ||
      BOM_PROFILES.some((p) => p.id === v) ||
      v in RETIRED_BOM_PROFILES)
  );
}

/** A stored profile id in current vocabulary. Unstated stays unstated. */
export function resolveBomProfile(v: string): BomProfile | typeof UNSTATED_BOM_PROFILE {
  return RETIRED_BOM_PROFILES[v] ?? (isBomProfile(v) ? v : UNSTATED_BOM_PROFILE);
}

export const SEVERITY_ORDER: FindingSeverity[] = ["Critical", "Non-critical"];

/** Findings per severity, highest first — the summary strip's data. */
export function severityCounts(doc: FindingsDoc): { severity: FindingSeverity; n: number }[] {
  return SEVERITY_ORDER.map((severity) => ({
    severity,
    n: doc.findings.filter((f) => severityOf(f) === severity).length,
  })).filter((s) => s.n > 0);
}

/**
 * How this review was produced, in one chip and one tooltip — or null on the free
 * tier, which has no `execution` block because it has nothing to disclose.
 *
 * `Execution` was defined here and read by nothing, which meant the engine stamped
 * `prompt_pack` into every document and the engineer never saw it. That was half the
 * point of it existing: two reviews of the same board that disagree are explained by
 * a content version far more often than by a regression, and the version is no use in
 * a JSON file nobody opens. The other half is `surface` — a review reasoned by the
 * user's own agent had our workflow, our evidence and our validation but somebody
 * else's model doing the judging, and a reader is entitled to know that before they
 * trust a clean result.
 */
export function executionSummary(
  doc: FindingsDoc | null,
): { text: string; detail: string } | null {
  const e = doc?.execution;
  if (!e) return null;
  const SURFACE: Record<Execution["surface"], string> = {
    local: "Reviewed in SpinZero",
    mcp: "Reviewed by your assistant",
    hosted: "Reviewed on the hosted service",
  };
  const detail = [
    SURFACE[e.surface] ?? e.surface,
    // "Reported, never verified" is a real caveat and it is said, not implied.
    e.model_reported ? `Model: ${e.model_reported} (as reported by the client)` : "",
    e.prompt_pack ? `Prompts: ${e.prompt_pack}` : "",
    e.rule_pack ? `Rules: ${e.rule_pack}` : "",
    e.allow_low_coverage
      ? "Datasheet coverage gate was overridden for this run, so parts were judged without their datasheets."
      : "",
  ]
    .filter(Boolean)
    .join("\n");
  // The chip itself stays short: the content version is what a reader compares
  // between two runs, so it is the part that shows without hovering.
  return { text: e.prompt_pack ?? SURFACE[e.surface] ?? e.surface, detail };
}
