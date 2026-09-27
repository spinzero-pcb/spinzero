// findings.json — the one review contract, mirroring schemas/findings-1.3.json.
//
// Every review producer emits this document: the free deterministic BOM check
// (Rust `bom-rules`, confidence "Unvalidated") today, the paid detailed review
// later. The app therefore has ONE ingestion path — findings become review
// comments in bomcheck.rs, matched by `fingerprint` — and this file is what the
// BOM tab renders the run summary from.
//
// Keep in sync with schemas/findings-1.3.json and src-tauri/src/findings.rs.

import type { Comment } from "./types";

/** Three levels. "Critical" = act before this ships; "Non-critical" = worth knowing,
 *  not a blocker; "Not verified" = the review could not check this, so it makes no
 *  claim either way. How SURE the reviewer is about a claim it DID make lives in
 *  `confidence`.
 *
 *  "Not verified" is the absence of a finding, not a quiet one. Anything that ranks,
 *  colours or counts severities has to keep it apart from the two that say something
 *  about the board — a coverage gap listed among defects reads as a defect. */
export type FindingSeverity = "Critical" | "Non-critical" | "Not verified";

/** Whether a finding says something about the BOARD, as against saying the review
 *  could not look. The predicate every count, colour and sort should ask, so a new
 *  severity of either kind lands correctly without hunting for comparisons. */
export function isClaim(f: { severity: string }): boolean {
  return severityOf(f) !== "Not verified";
}
/** What findings-1.0 and 1.1 called the same two levels. Documents in a user's project
 *  folder outlive the rename, so every reader normalises through `severityOf` rather
 *  than comparing `f.severity` directly. */
export type LegacyFindingSeverity = "Important" | "Observation";

/** One finding's severity in current vocabulary, whatever version wrote it. */
export function severityOf(f: { severity: string }): FindingSeverity {
  if (f.severity === "Important") return "Critical";
  if (f.severity === "Observation") return "Non-critical";
  if (f.severity === "Critical") return "Critical";
  // Named rather than left to the fallback: a coverage gap that fell through to
  // "Non-critical" would be counted and coloured as something we found wrong with the
  // board, which is the opposite of what it says.
  if (f.severity === "Not verified") return "Not verified";
  return "Non-critical";
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
  /** Which BOM column the review actually read each field from. Absent on the free
   *  tier. NOT the same thing as `BomMappingDialog`'s mapping: that one is what the
   *  app's own rules will read next time, this one is what this review did read. */
  column_mapping?: ColumnMapping;
}

/** Mirrors `$defs/column_mapping` in `schemas/findings-1.3.json`. */
export interface ColumnMapping {
  fields: {
    field: string;
    column?: string | null;
    /** Which tier supplied it: `confirmed` is the user's own answer to the review's
     *  preflight, the rest are our resolver's. */
    via?: "confirmed" | "canonical" | "alternate" | "borrowed" | null;
    /** Share of rows with something in that column, counted over the BOM the review
     *  read. Printed rather than recounted, so the app and the page agree. */
    fill_rate?: number;
  }[];
  unmapped_columns?: { column: string; fill_rate?: number }[];
  /** False when nobody said what the board is for, so every rule ran at its
   *  strictest. A reader has to know before deciding whether a finding applies. */
  profile_stated?: boolean;
}

/** Which surface produced a review, and with which content.
 *
 *  Worth showing next to a clean result: a review reasoned by the user's own agent
 *  through the SpinZero MCP harness had our workflow, our evidence and our validation
 *  but somebody else's model doing the judging, and a reader is entitled to know that
 *  before they trust it. `prompt_pack` is the other half — two reviews of the same
 *  board that disagree are explained by a content version far more often than by a
 *  regression. Mirrors `$defs/execution` in `schemas/findings-1.3.json`. */
export interface Execution {
  surface: "local" | "mcp" | "hosted";
  /** What the client reported itself as. Never verified — read it as a claim. */
  model_reported?: string;
  /** "builtin/<hash>" or "pack/<version>". */
  prompt_pack?: string;
  rule_pack?: string;
  /** Which server build ran: "dev/<commit>" from source, "release/<version>+<commit>"
   *  from the installer. */
  build?: string;
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

/** Where one logical field's data comes from. Mirrors `findings::FieldMapping` in Rust. */
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
 *  the rule pack's `config::PROFILES`; the label is what the user sees.
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
  // is in, and one option could not ask. Mirrors the rule pack's `config::PROFILES`.
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

export const SEVERITY_ORDER: FindingSeverity[] = ["Critical", "Non-critical", "Not verified"];

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
    e.build ? `Build: ${e.build}` : "",
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

// ---- what the review could NOT do -----------------------------------------
//
// Everything below is a port of the same derivation in the review server's
// `report.ts`, deliberately rather than a second opinion. Two derivations of one
// coverage number disagree eventually, and then the page and the app contradict each
// other about a review the customer has already read. Change one, change both.

/** Audit items that are NOT coverage, by name.
 *
 *  There is no structural way to tell "the run could not do this" from "the BOM does
 *  not say this" — both arrive as a GAP — so the difference is stated. RoHS and
 *  lifecycle are facts about the BOM and are already filed as findings; rule
 *  candidates and the judgment pass describe our own pipeline, which is telemetry;
 *  the column mapping report gets a table of its own. */
const NOT_COVERAGE =
  /^(rohs compliance|lifecycle status verifiable|rule candidates|judgment pass|column mapping report)$/i;

/** Audit lines about a rule the judgment pass threw out. Interesting to us, noise to
 *  the engineer: a rule that did not fire is not a finding. */
const DISMISSED_RULE = /^Rule /;

/** One item the review could not verify: a short line, and the whole note behind it. */
export interface CoverageGap {
  item: string;
  /** The first sentence, and never more than a line of it. */
  note: string;
  /** The whole note, for the tooltip. */
  full: string;
  /** The designators on the BOM row this miss happened on, as the reader wrote them.
   *  Empty for a miss about the BOM as a whole, and for every pre-1.3 document, where
   *  the gaps were prose and had no row. */
  refdes?: string;
  /** The part number the miss is about. Empty on the same two cases as `refdes`. */
  mpn?: string;
}

/**
 * What the review could not check.
 *
 * TWO sources, and the order matters. From findings-1.3 a coverage gap is a finding
 * at severity `Not verified`, anchored to the BOM row it happened on — so it can be
 * counted, placed on a row and marked dealt-with like anything else. Those are
 * preferred whenever the document has any.
 *
 * The audit trail is the fallback, for the documents that predate 1.3 and are sitting
 * in users' project folders right now. It gives one line per STAGE rather than per
 * row, which is exactly the shortcoming 1.3 fixed, but it is what those documents
 * have.
 *
 * `run_health` is deliberately not a third source: every stage in it has an audit
 * entry saying the same thing in the engineer's terms rather than the pipeline's, so
 * reading both prints each gap twice under two names. It decides only whether the
 * heading says "incomplete".
 */
export function coverageGaps(doc: FindingsDoc | null): CoverageGap[] {
  const filed = (doc?.findings ?? []).filter((f) => severityOf(f) === "Not verified");
  if (filed.length) {
    return filed.map((f) => ({
      item: f.title,
      note: briefNote(f.detail ?? ""),
      full: f.detail ?? "",
      refdes: (f.anchors ?? []).flatMap((a) => a.refdes ?? []).join(", "),
      mpn: (f.anchors ?? []).map((a) => a.mpn).find((m) => !!m) ?? "",
    }));
  }
  const out: CoverageGap[] = [];
  for (const a of doc?.bom_audit ?? []) {
    if (a.result === "OK") continue;
    if (NOT_COVERAGE.test(a.item.trim()) || DISMISSED_RULE.test(a.item)) continue;
    const full = a.note ?? "";
    out.push({ item: a.item, note: briefNote(full), full });
  }
  return out;
}

const BRIEF_MAX = 110;

/** One line per miss. The audit notes are paragraphs; this is a scan list. */
function briefNote(note: string): string {
  const first = (/^[\s\S]*?[.!?](?=\s|$)/.exec(note.trim())?.[0] ?? note.trim()).trim();
  if (first.length <= BRIEF_MAX) return first;
  // Cut before the enumeration rather than mid-list: these sentences end in a colon
  // or a dash followed by every part number that missed, and half a part number is
  // worse than none.
  const cut = Math.max(first.lastIndexOf(":", BRIEF_MAX), first.lastIndexOf(" — ", BRIEF_MAX));
  const head = cut > 20 ? first.slice(0, cut) : first.slice(0, first.lastIndexOf(" ", BRIEF_MAX));
  return `${head.replace(/[,;:\s]+$/, "")}…`;
}

/** "N of M part numbers were accounted for" — the coverage sentence, from the audit
 *  entry the assembler writes for it. Null when the review filed no such entry. */
export function partCoverage(doc: FindingsDoc | null): string | null {
  const entry = doc?.bom_audit?.find((a) => a.item.trim().toLowerCase() === "part verification");
  return entry?.note ? briefNote(entry.note) : null;
}

/** The three numbers that head the review: how bad, how minor, and how much of it
 *  nobody checked. The third is the one the app used to leave out, so a review with
 *  twenty blind spots and no findings read as a pass. */
export function reviewTallies(doc: FindingsDoc | null): {
  critical: number;
  nonCritical: number;
  notVerified: number;
  /** True when a whole stage degraded or failed. The gap list is then "incomplete"
   *  rather than merely "not verified". */
  stalled: boolean;
} {
  const findings = doc?.findings ?? [];
  // Counted by predicate, not by subtraction. `length - critical` swept the new
  // `Not verified` findings into the non-critical tally, which said we had found
  // things wrong with the board that we had in fact failed to look at.
  const critical = findings.filter((f) => severityOf(f) === "Critical").length;
  return {
    critical,
    nonCritical: findings.filter((f) => severityOf(f) === "Non-critical").length,
    notVerified: coverageGaps(doc).length,
    stalled: (doc?.run_health ?? []).length > 0,
  };
}

/** Human label for a review stage. The app never shows a raw stage id. */
export const STAGE_LABELS: Record<string, string> = {
  validate_bundle: "Checking the BOM",
  fetch_datasheets: "Collecting datasheets",
  deterministic_rules: "Running the rule pack",
  verify_specs: "Cross-checking every row",
  judgment_pass: "Reviewing against datasheets",
  assemble: "Assembling the report",
};

export function stageLabel(stage: string | null | undefined): string {
  return stage ? (STAGE_LABELS[stage] ?? stage) : "";
}

/**
 * What to tell the user when a review came back incomplete — or null when it did not.
 *
 * A stage that was cut short is the difference between "the BOM is clean" and
 * "nothing checked the BOM", and the app is the only place the user looks.
 */
export function runHealthSummary(doc: FindingsDoc | null): { text: string; detail: string } | null {
  const entries = doc?.run_health ?? [];
  if (!entries.length) return null;
  const lines = entries.map((h) => `${stageLabel(h.stage)} ${h.status}${h.detail ? `: ${h.detail}` : ""}`);
  const worst = entries.find((h) => h.status === "failed") ?? entries[0];
  const text = `${stageLabel(worst?.stage)} ${worst?.status === "failed" ? "failed" : "was cut short"}${
    entries.length > 1 ? ` (+${entries.length - 1} more)` : ""
  }`;
  return { text, detail: lines.join("\n") };
}
