import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ackReview,
  describeProgress,
  fetchFindings,
  health,
  runHealthSummary,
  stageLabel,
  streamProgress,
  submitReview,
  ReviewServiceError,
  type ReviewProgress,
} from "./reviewService";
import type { FindingsDoc } from "./findings";

/** The em dash the labels use, spelled out so this file stays ASCII. */
const DASH = String.fromCharCode(0x2014);

// The client's job is to make a remote, optional, failure-prone service feel like a
// local capability: a dead service must produce an explainable message rather than an
// unhandled rejection, and an SSE stream must survive being split across chunks the
// way a real socket delivers it.

const config = { baseUrl: "http://localhost:8787/", token: "tok" };

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/** An SSE body delivered in arbitrary chunks — frames WILL straddle chunk edges. */
function sseResponse(chunks: string[]): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      const encoder = new TextEncoder();
      for (const c of chunks) controller.enqueue(encoder.encode(c));
      controller.close();
    },
  });
  return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
}

const fetchMock = vi.fn();

beforeEach(() => {
  fetchMock.mockReset();
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => vi.unstubAllGlobals());

describe("reviewService client", () => {
  it("sends the bundle with bearer auth and a normalized URL", async () => {
    fetchMock.mockResolvedValue(jsonResponse({ job_id: "j1" }, 202));
    const out = await submitReview(config, { profile: "automotive", files: { "bom_enriched.csv": "a,b\n1,2\n" } });
    expect(out.job_id).toBe("j1");
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    // The trailing slash in the configured base must not produce a double slash.
    expect(url).toBe("http://localhost:8787/v1/reviews");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer tok");
    const body = JSON.parse(init.body as string) as { pipeline: string; profile: string; files: object };
    expect(body.pipeline).toBe("bom-detailed");
    expect(body.profile).toBe("automotive");
    expect(Object.keys(body.files)).toEqual(["bom_enriched.csv"]);
  });

  it("turns an unreachable service into an explainable error, not a raw throw", async () => {
    fetchMock.mockRejectedValue(new TypeError("fetch failed"));
    await expect(submitReview(config, { profile: "default", files: {} })).rejects.toThrow(
      /cannot reach the review service at http:\/\/localhost:8787/,
    );
    // health() never throws — the button uses it to decide whether to explain itself.
    expect(await health(config)).toMatchObject({ ok: false });
  });

  it("surfaces the server's error message and status", async () => {
    fetchMock.mockResolvedValue(jsonResponse({ error: "invalid token" }, 401));
    const err = await fetchFindings(config, "j1").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ReviewServiceError);
    expect((err as ReviewServiceError).status).toBe(401);
    expect((err as Error).message).toBe("invalid token");
  });

  it("never lets a failed ack become the user's problem", async () => {
    fetchMock.mockResolvedValue(jsonResponse({ error: "gone" }, 500));
    expect(await ackReview(config, "j1")).toBe(false);
  });

  it("reassembles SSE frames split across chunks and stops at the terminal event", async () => {
    const frames = [
      'event: stage_started\ndata: {"type":"stage_started","ts":"t","stage":"determ',
      'inistic_rules"}\n\nevent: stage_progress\ndata: {"type":"stage_progress","ts":"t","stage":"judgment_pass","data":{"findings":2}}\n\n',
      ': ping\n\n',
      'event: completed\ndata: {"type":"completed","ts":"t","data":{"findings":7}}\n\n',
      'event: log\ndata: {"type":"log","ts":"t","message":"after the end"}\n\n',
    ];
    fetchMock.mockResolvedValue(sseResponse(frames));
    const seen: ReviewProgress[] = [];
    const terminal = await streamProgress(config, "j1", (e) => seen.push(e));
    expect(terminal.type).toBe("completed");
    expect(seen.map((e) => e.type)).toEqual(["stage_started", "stage_progress", "completed"]);
    expect(seen[0]?.stage).toBe("deterministic_rules");
    expect((seen[1]?.data as { findings: number }).findings).toBe(2);
  });

  it("resolves — not rejects — when the review itself failed", async () => {
    fetchMock.mockResolvedValue(
      sseResponse(['event: failed\ndata: {"type":"failed","ts":"t","data":{"error":"engine exited 1"}}\n\n']),
    );
    const terminal = await streamProgress(config, "j1", () => {});
    expect(terminal.type).toBe("failed");
  });

  it("rejects when the stream cannot be opened at all", async () => {
    fetchMock.mockResolvedValue(new Response("nope", { status: 502 }));
    await expect(streamProgress(config, "j1", () => {})).rejects.toThrow(/progress stream failed/);
  });

  it("summarises an incomplete review, and says nothing about a clean one", () => {
    // The regression this guards: a rate-limited review came back as findings at
    // Low confidence with no visible reason — the run looked complete.
    const doc = {
      run_health: [
        { stage: "judgment_pass", status: "failed" as const, detail: "429 Too Many Requests — rate limited" },
        { stage: "deterministic_rules", status: "degraded" as const, detail: "ended early (cost_cap)" },
      ],
    } as unknown as FindingsDoc;
    const summary = runHealthSummary(doc);
    expect(summary?.text).toBe("Reviewing against datasheets failed (+1 more)");
    expect(summary?.detail).toContain("429 Too Many Requests");
    expect(summary?.detail).toContain("Running the rule pack degraded");

    expect(runHealthSummary(null)).toBeNull();
    expect(runHealthSummary({ run_health: [] } as unknown as FindingsDoc)).toBeNull();
    expect(runHealthSummary({} as FindingsDoc)).toBeNull();
  });

  it("labels stages in human words", () => {
    // Stage labels are customer-facing: "judgment pass" is our word for it, not theirs.
    expect(stageLabel("judgment_pass")).toBe("Reviewing against datasheets");
    // An unknown stage still renders something rather than blanking the strip.
    expect(stageLabel("future_stage")).toBe("future_stage");
    expect(stageLabel(undefined)).toBe("");
  });

  it("renders every event as an activity line, including the ones the bar ignores", () => {
    // The regression this guards: a run spent six minutes inside one model turn and
    // the app showed nothing at all, because `log` events were dropped on the floor.
    // The heartbeat is the row that distinguishes a slow run from a dead one, so it
    // must survive the trip from the engine to the feed.
    const beat = describeProgress({
      type: "log",
      ts: "2026-08-25T10:20:01.000Z",
      stage: "judgment_pass",
      message: "waiting on the model - turn 6 of 30, 214s",
      data: { waiting: true, turn: 6, max_turns: 30, elapsed_ms: 214000 },
    });
    expect(beat?.tone).toBe("waiting");
    expect(beat?.text).toContain("turn 6 of 30");

    const tool = describeProgress({
      type: "log",
      ts: "2026-08-25T10:20:03.000Z",
      stage: "judgment_pass",
      data: { tool: "read_datasheet", turn: 6, duration_ms: 1234, ok: true },
    });
    expect(tool?.tone).toBe("tool");
    expect(tool?.text).toMatch(/^read_datasheet .* turn 6 .* 1\.2s$/);
    // Arguments ride only under SPINZERO_TRACE; without them there is no detail line.
    expect(tool?.detail).toBeUndefined();

    const failed = describeProgress({
      type: "log",
      ts: "2026-08-25T10:20:04.000Z",
      data: { tool: "web_search", turn: 6, duration_ms: 90, ok: false, input: '{"q":"x"}' },
    });
    expect(failed?.tone).toBe("error");
    expect(failed?.detail).toContain('{"q":"x"}');

    // Stages keep their human labels here too, and a duration when they finish.
    expect(
      describeProgress({ type: "stage_done", ts: "t", stage: "judgment_pass", data: { duration_ms: 3185 } })?.text,
    ).toBe(`${stageLabel("judgment_pass")} ${DASH} done (3.2s)`);
    // An event type this app has not been taught still gets a row: an unknown event
    // is exactly when someone is staring at the feed asking what is going on.
    expect(describeProgress({ type: "invented" as never, ts: "t", message: "hello" })?.text).toBe("hello");
  });
});
