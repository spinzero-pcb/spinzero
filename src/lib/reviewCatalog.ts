// The review catalog — one entry per review the app can run, ready or not.
//
// This is the registry the "Run a review" launcher renders and the only place a new
// review type has to be declared. Everything the UI needs to draw a row lives here;
// the review's own setup dialog supplies its scope, so a row is one line no matter
// how elaborate that scope turns out to be.
//
// Unbuilt reviews are listed deliberately (decision, 2026-08-24): the picker doubles
// as the coverage readout, and a review that is hidden reads like a review that
// passed. They carry `ready: false` and render as "coming soon".

/** What a review reads. Staleness is per-input, not global: editing the schematic
 *  does not invalidate a datasheet check, so each review is only stale when the
 *  inputs IT read have moved. */
export type ReviewInput = "bom" | "datasheets" | "schematic" | "pcb";

export type ReviewTier = "included" | "premium";

export type ReviewKindId = "bom" | "datasheets" | "schematic" | "tolerance" | "layout";

export interface ReviewKind {
  id: ReviewKindId;
  label: string;
  /** One line, present tense, for the picker row's tooltip and the setup sheet. */
  blurb: string;
  /** The cheapest tier this review can run at — a review with both tiers says
   *  "included" here and offers the depth choice inside its setup sheet. */
  tier: ReviewTier;
  /** false = declared but not built; the row renders disabled with a "soon" tag. */
  ready: boolean;
  inputs: ReviewInput[];
}

export const REVIEW_KINDS: ReviewKind[] = [
  {
    id: "bom",
    label: "BOM Review",
    blurb:
      "Deterministic rules over the BOM columns; a detailed run also reads the datasheets behind the part numbers.",
    tier: "included",
    ready: true,
    inputs: ["bom"],
  },
  {
    id: "datasheets",
    label: "Datasheet checker",
    blurb: "Checks each part against the document that describes it — ratings, qualification, lifecycle.",
    tier: "premium",
    ready: false,
    inputs: ["bom", "datasheets"],
  },
  {
    id: "schematic",
    label: "Schematic Review",
    blurb: "Connectivity, power and reference-designator checks over the extracted schematic.",
    tier: "premium",
    ready: false,
    inputs: ["schematic"],
  },
  {
    id: "tolerance",
    label: "Tolerance analysis",
    blurb: "Worst-case and RSS analysis over the rails the design depends on.",
    tier: "premium",
    ready: false,
    inputs: ["bom", "schematic"],
  },
  {
    id: "layout",
    label: "Layout Review",
    blurb: "Placement, clearance and return-path checks over the board geometry.",
    tier: "premium",
    ready: false,
    inputs: ["pcb"],
  },
];

export function reviewKind(id: ReviewKindId): ReviewKind | undefined {
  return REVIEW_KINDS.find((k) => k.id === id);
}

/** Record of one completed run, persisted per project (ProjectUi.review_runs).
 *  `inputs` holds the digest of each input AT RUN TIME — comparing them to the
 *  current digests is what makes staleness per-review rather than global. */
export interface ReviewRun {
  /** ISO timestamp the run finished. */
  ts: string;
  /** Extraction the run read, for the record; staleness uses the digests. */
  extraction_id?: string | null;
  /** input name → digest at run time. An input with no digest is simply not tracked. */
  inputs?: Partial<Record<ReviewInput, string>>;
}

/** Sanitize one persisted run record. Settings are hand-editable, so nothing is
 *  trusted; an unreadable record reads as "never run" rather than throwing. */
export function sanitizeRun(v: unknown): ReviewRun | null {
  if (!v || typeof v !== "object") return null;
  const r = v as Record<string, unknown>;
  if (typeof r.ts !== "string" || !r.ts) return null;
  const inputs: Partial<Record<ReviewInput, string>> = {};
  if (r.inputs && typeof r.inputs === "object") {
    for (const [k, val] of Object.entries(r.inputs as Record<string, unknown>)) {
      if (typeof val === "string" && (["bom", "datasheets", "schematic", "pcb"] as string[]).includes(k))
        inputs[k as ReviewInput] = val;
    }
  }
  return {
    ts: r.ts,
    extraction_id: typeof r.extraction_id === "string" ? r.extraction_id : null,
    inputs,
  };
}

export function sanitizeRuns(v: unknown): Partial<Record<ReviewKindId, ReviewRun>> {
  const out: Partial<Record<ReviewKindId, ReviewRun>> = {};
  if (!v || typeof v !== "object") return out;
  for (const k of REVIEW_KINDS) {
    const run = sanitizeRun((v as Record<string, unknown>)[k.id]);
    if (run) out[k.id] = run;
  }
  return out;
}

/**
 * Is this review stale — have the inputs it read moved since it ran?
 *
 * Only inputs present in BOTH the run record and `current` participate. An input we
 * cannot digest yet (datasheets, schematic, pcb) is therefore never a reason to call
 * a review stale: the launcher must not cry wolf, and a missing digest is ignorance,
 * not evidence of change.
 */
export function isStale(
  kind: ReviewKind,
  run: ReviewRun | undefined,
  current: Partial<Record<ReviewInput, string>>,
): boolean {
  if (!run) return false; // never run is not stale, it is unrun
  for (const input of kind.inputs) {
    const then = run.inputs?.[input];
    const now = current[input];
    if (then && now && then !== now) return true;
  }
  return false;
}

/** Order-independent, allocation-cheap digest of a list of strings (FNV-1a, hex).
 *  Used to fingerprint a review's inputs; it only ever has to detect change. */
export function digestOf(parts: readonly string[]): string {
  let h = 0x811c9dc5;
  for (const p of parts) {
    for (let i = 0; i < p.length; i++) {
      h ^= p.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    h ^= 0x0a;
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}
