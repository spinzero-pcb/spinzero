import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ackReview,
  fetchFindings,
  health,
  stageLabel,
  streamProgress,
  submitReview,
  ReviewServiceError,
  type ReviewProgress,
} from "./reviewService";

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
      'event: stage_started\ndata: {"type":"stage_started","ts":"t","stage":"fp_val',
      'idation"}\n\nevent: stage_progress\ndata: {"type":"stage_progress","ts":"t","stage":"judgment_pass","data":{"findings":2}}\n\n',
      ': ping\n\n',
      'event: completed\ndata: {"type":"completed","ts":"t","data":{"findings":7}}\n\n',
      'event: log\ndata: {"type":"log","ts":"t","message":"after the end"}\n\n',
    ];
    fetchMock.mockResolvedValue(sseResponse(frames));
    const seen: ReviewProgress[] = [];
    const terminal = await streamProgress(config, "j1", (e) => seen.push(e));
    expect(terminal.type).toBe("completed");
    expect(seen.map((e) => e.type)).toEqual(["stage_started", "stage_progress", "completed"]);
    expect(seen[0]?.stage).toBe("fp_validation");
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

  it("labels stages in human words", () => {
    expect(stageLabel("judgment_pass")).toBe("Judgment pass");
    // An unknown stage still renders something rather than blanking the strip.
    expect(stageLabel("future_stage")).toBe("future_stage");
    expect(stageLabel(undefined)).toBe("");
  });
});
