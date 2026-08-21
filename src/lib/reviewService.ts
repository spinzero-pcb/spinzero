// Client for the SpinZero review service (the paid detailed review).
//
// The service is a real HTTP API from day one; "no server yet" only means it
// listens on localhost. So this client speaks the final contract — bearer auth,
// job submit, SSE progress, findings fetch, ack-then-delete — and moving to a
// hosted URL later is a setting change, not a rewrite.
//
// Two rules this file exists to keep:
//
//  * **The service is optional.** Every failure returns a message the BOM tab can
//    show; nothing here throws into a render path, and the free local check keeps
//    working when the service is unreachable.
//  * **Nothing is uploaded that the user has not seen.** The bundle is built in
//    Rust (`build_review_bundle`) and shown in the pre-flight dialog; this client
//    sends exactly that map and never reaches for a file itself.

import type { FindingsDoc } from "./findings";

/** Where the service lives and how we authenticate to it. Phase 1: a static dev
 *  token (plan §5). Phase 2 replaces the token with a Clerk-issued JWT — the
 *  request shape does not change. */
export interface ReviewServiceConfig {
  baseUrl: string;
  token: string;
}

export const DEFAULT_BASE_URL = "http://localhost:8787";

/** The bundle as Rust built it: the file map plus what the dialog renders. */
export interface ReviewBundle {
  files: Record<string, string>;
  sizes: Record<string, number>;
  bom_rows: number;
  excluded: string[];
}

export interface ServiceIdentity {
  org_id: string;
  plan: string;
  /** null = unmetered (local dev); a number once billing exists. */
  credits: number | null;
}

/** One progress event, relayed verbatim from the engine (see the engine's
 *  `ProgressEvent`). Carries counters and stage names only — never BOM content. */
export interface ReviewProgress {
  type:
    | "queued"
    | "run_started"
    | "stage_started"
    | "stage_progress"
    | "stage_done"
    | "log"
    | "completed"
    | "failed";
  ts: string;
  stage?: string;
  message?: string;
  data?: Record<string, unknown>;
}

export class ReviewServiceError extends Error {
  readonly status: number;
  constructor(message: string, status = 0) {
    super(message);
    this.name = "ReviewServiceError";
    this.status = status;
  }
}

function url(config: ReviewServiceConfig, path: string): string {
  return `${config.baseUrl.replace(/\/+$/, "")}${path}`;
}

function headers(config: ReviewServiceConfig): Record<string, string> {
  return config.token ? { authorization: `Bearer ${config.token}` } : {};
}

async function request<T>(
  config: ReviewServiceConfig,
  path: string,
  init: RequestInit = {},
  timeoutMs = 30_000,
): Promise<T> {
  let res: Response;
  try {
    res = await fetch(url(config, path), {
      ...init,
      headers: { ...headers(config), ...(init.headers ?? {}) },
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (e) {
    // A dead service is the expected case on a laptop, not an exception worth a
    // stack: say what would fix it.
    throw new ReviewServiceError(
      `cannot reach the review service at ${config.baseUrl} (${String(e)}). Is it running?`,
    );
  }
  const text = await res.text();
  if (!res.ok) {
    let message = text.slice(0, 300);
    try {
      message = (JSON.parse(text) as { error?: string }).error ?? message;
    } catch {
      /* not JSON — use the raw text */
    }
    throw new ReviewServiceError(message || `${res.status} ${res.statusText}`, res.status);
  }
  return (text ? JSON.parse(text) : {}) as T;
}

/** Is the service up? Used to enable the button, never to gate the free check. */
export async function health(config: ReviewServiceConfig): Promise<{ ok: boolean; error?: string }> {
  try {
    await request<{ ok: boolean }>(config, "/healthz", {}, 5_000);
    return { ok: true };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

export function whoAmI(config: ReviewServiceConfig): Promise<ServiceIdentity> {
  return request<ServiceIdentity>(config, "/v1/me");
}

export function submitReview(
  config: ReviewServiceConfig,
  input: { profile: string; files: Record<string, string> },
): Promise<{ job_id: string }> {
  return request<{ job_id: string }>(
    config,
    "/v1/reviews",
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pipeline: "bom-detailed", profile: input.profile, files: input.files }),
    },
    60_000,
  );
}

export function fetchFindings(config: ReviewServiceConfig, jobId: string): Promise<FindingsDoc> {
  return request<FindingsDoc>(config, `/v1/reviews/${jobId}/findings`);
}

/** Tell the service we have the findings so it deletes them (plan §6.1). Never
 *  throws: a failed ack costs a 24 h TTL, not the user's result. */
export async function ackReview(config: ReviewServiceConfig, jobId: string): Promise<boolean> {
  try {
    await request(config, `/v1/reviews/${jobId}/ack`, { method: "POST" });
    return true;
  } catch {
    return false;
  }
}

export async function cancelReview(config: ReviewServiceConfig, jobId: string): Promise<boolean> {
  try {
    await request(config, `/v1/reviews/${jobId}`, { method: "DELETE" });
    return true;
  } catch {
    return false;
  }
}

/**
 * Stream a job's progress until it ends, calling `onProgress` for every event.
 *
 * Hand-rolled over `fetch` rather than `EventSource` for one reason: EventSource
 * cannot send an Authorization header. Resolves with the terminal event; rejects
 * only if the stream cannot be opened at all — a `failed` event resolves, because
 * "the review failed" is a result the UI must render, not an exception.
 */
export async function streamProgress(
  config: ReviewServiceConfig,
  jobId: string,
  onProgress: (event: ReviewProgress) => void,
  signal?: AbortSignal,
): Promise<ReviewProgress> {
  let res: Response;
  try {
    res = await fetch(url(config, `/v1/reviews/${jobId}/events`), {
      headers: { ...headers(config), accept: "text/event-stream" },
      ...(signal ? { signal } : {}),
    });
  } catch (e) {
    throw new ReviewServiceError(`progress stream failed: ${String(e)}`);
  }
  if (!res.ok || !res.body) {
    throw new ReviewServiceError(`progress stream failed: ${res.status} ${res.statusText}`, res.status);
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let last: ReviewProgress = { type: "queued", ts: new Date().toISOString() };
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    // SSE frames are separated by a blank line; a partial frame waits for more.
    let split = buffer.indexOf("\n\n");
    while (split >= 0) {
      const frame = buffer.slice(0, split);
      buffer = buffer.slice(split + 2);
      split = buffer.indexOf("\n\n");
      const dataLine = frame.split("\n").find((l) => l.startsWith("data:"));
      if (!dataLine) continue; // a heartbeat comment
      try {
        const event = JSON.parse(dataLine.slice(5).trim()) as ReviewProgress;
        last = event;
        onProgress(event);
        if (event.type === "completed" || event.type === "failed") {
          void reader.cancel();
          return event;
        }
      } catch {
        /* a frame we cannot parse is not worth failing a paid run over */
      }
    }
  }
  return last;
}

/** Human label for a stage id — the app never shows raw stage ids. */
export const STAGE_LABELS: Record<string, string> = {
  validate_bundle: "Checking the bundle",
  deterministic_rules: "Running the rule pack",
  fp_validation: "Validating rule findings",
  judgment_pass: "Judgment pass",
  assemble: "Assembling the report",
};

export function stageLabel(stage: string | undefined): string {
  return stage ? (STAGE_LABELS[stage] ?? stage) : "";
}
