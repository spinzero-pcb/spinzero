import { mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDetailedReviewStore } from "./detailedReviewStore";
import { useBomCheckStore } from "./bomCheckStore";
import { useReviewStore } from "./reviewStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";
import type { CheckOutcome, FindingsDoc } from "../lib/findings";

// What is worth pinning here is the *sequence* the user is promised: nothing is
// uploaded before the pre-flight bundle exists, the findings land through the free
// tier's ingestion path (so comments reconcile), the service is told to delete its
// copy afterwards, and a failure leaves the free results untouched.

const SERVICE = { base_url: "http://localhost:8787", token: "tok" };

function paidDoc(): FindingsDoc {
  return {
    schema_version: "1.0",
    engine_version: "engine-0.1.0",
    pipeline: "bom-detailed",
    profile: "industrial",
    findings: [
      {
        id: "B01",
        section: "BOM · Sourcing",
        severity: "Major",
        confidence: "High",
        rule_id: "bom.manufacturer_mpn_unpaired",
        title: "Manufacturer without a part number",
        anchors: [{ type: "bom_row", refdes: ["R1"] }],
        fingerprint: "aaaa111122223333",
      },
    ],
    bom_audit: [],
    stats: { item_count: 4, finding_count: 1, duration_ms: 900 },
  };
}

function outcome(): CheckOutcome {
  return {
    findings: paidDoc(),
    session_id: "s_paid",
    filed: 0,
    reopened: 0,
    unchanged: 1,
    auto_resolved: 0,
    unmapped_columns: [],
    comments: [],
  };
}

const bundle = {
  files: { "bom_enriched.csv": "Reference,Value\nR1,10k\n", "design_meta.json": "{}" },
  sizes: { "bom_enriched.csv": 26, "design_meta.json": 2 },
  bom_rows: 1,
  excluded: ["schematic and PCB geometry"],
};

const fetchMock = vi.fn();
let ipcCalls: { cmd: string; args: unknown }[] = [];

function sse(...frames: string[]): Response {
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      const enc = new TextEncoder();
      for (const f of frames) c.enqueue(enc.encode(f));
      c.close();
    },
  });
  return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
}

function route(url: string): Response {
  if (url.endsWith("/healthz")) return new Response(JSON.stringify({ ok: true }), { status: 200 });
  if (url.endsWith("/v1/reviews")) return new Response(JSON.stringify({ job_id: "j1" }), { status: 202 });
  if (url.endsWith("/events")) {
    return sse(
      'event: stage_started\ndata: {"type":"stage_started","ts":"t","stage":"judgment_pass"}\n\n',
      'event: completed\ndata: {"type":"completed","ts":"t","data":{"findings":1}}\n\n',
    );
  }
  if (url.endsWith("/findings")) return new Response(JSON.stringify(paidDoc()), { status: 200 });
  if (url.endsWith("/ack")) return new Response(JSON.stringify({ ok: true }), { status: 200 });
  return new Response("not found", { status: 404 });
}

beforeEach(() => {
  useDetailedReviewStore.getState().reset();
  useBomCheckStore.getState().clear();
  useToastStore.setState({ toasts: [] });
  useSettingsStore.setState({ loaded: true, reviewService: SERVICE });
  useReviewStore.setState({ ...useReviewStore.getState(), comments: [] });
  vi.spyOn(useReviewStore.getState(), "load").mockResolvedValue(undefined);
  ipcCalls = [];
  mockIPC((cmd, args) => {
    ipcCalls.push({ cmd, args });
    if (cmd === "build_review_bundle") return bundle;
    if (cmd === "ingest_findings") return outcome();
    return null;
  });
  fetchMock.mockReset();
  fetchMock.mockImplementation((url: string) => Promise.resolve(route(String(url))));
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => vi.unstubAllGlobals());

describe("detailedReviewStore", () => {
  it("shows the bundle before anything is uploaded", async () => {
    await useDetailedReviewStore.getState().openPreflight();
    const s = useDetailedReviewStore.getState();
    expect(s.phase).toBe("preflight");
    expect(Object.keys(s.bundle?.files ?? {})).toEqual(["bom_enriched.csv", "design_meta.json"]);
    // The critical assertion: opening the dialog must not have posted anything.
    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith("/v1/reviews"))).toBe(false);
  });

  it("refuses to start without a pre-flight bundle", async () => {
    await useDetailedReviewStore.getState().start();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(useDetailedReviewStore.getState().phase).toBe("idle");
  });

  it("runs the whole flow: submit → progress → ingest → ack", async () => {
    useBomCheckStore.setState({ profile: "industrial" });
    await useDetailedReviewStore.getState().openPreflight();
    await useDetailedReviewStore.getState().start();

    const s = useDetailedReviewStore.getState();
    expect(s.phase).toBe("done");
    expect(s.doc?.pipeline).toBe("bom-detailed");

    // Findings go through the SAME ingestion command as the free check.
    const ingest = ipcCalls.find((c) => c.cmd === "ingest_findings");
    expect(ingest).toBeTruthy();
    expect((ingest?.args as { doc: FindingsDoc }).doc.pipeline).toBe("bom-detailed");

    // The BOM strip now summarizes the paid run.
    expect(useBomCheckStore.getState().doc?.pipeline).toBe("bom-detailed");
    expect(useBomCheckStore.getState().sessionId).toBe("s_paid");
    // …and the rail lands on that run's session, so the findings are on screen.
    expect(useReviewStore.getState().activeSessionId).toBe("s_paid");

    // Ack: the service is told to delete its copy.
    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith("/ack"))).toBe(true);
  });

  it("reports a failed review without disturbing the free tier's results", async () => {
    const freeDoc = { ...paidDoc(), pipeline: "bom-rules" };
    useBomCheckStore.setState({ doc: freeDoc });
    fetchMock.mockImplementation((url: string) =>
      String(url).endsWith("/events")
        ? Promise.resolve(sse('event: failed\ndata: {"type":"failed","ts":"t","data":{"error":"engine exited 1"}}\n\n'))
        : Promise.resolve(route(String(url))),
    );

    await useDetailedReviewStore.getState().openPreflight();
    await useDetailedReviewStore.getState().start();

    expect(useDetailedReviewStore.getState().phase).toBe("idle");
    expect(useDetailedReviewStore.getState().error).toContain("engine exited 1");
    expect(useToastStore.getState().toasts[0]?.kind).toBe("error");
    // The free check's findings are still on screen.
    expect(useBomCheckStore.getState().doc?.pipeline).toBe("bom-rules");
    expect(ipcCalls.some((c) => c.cmd === "ingest_findings")).toBe(false);
  });

  it("collects the result anyway when only the progress stream dies", async () => {
    fetchMock.mockImplementation((url: string) =>
      String(url).endsWith("/events")
        ? Promise.reject(new TypeError("socket closed"))
        : Promise.resolve(route(String(url))),
    );
    await useDetailedReviewStore.getState().openPreflight();
    await useDetailedReviewStore.getState().start();
    // Losing progress is not losing the review — the job had already been submitted.
    expect(useDetailedReviewStore.getState().phase).toBe("done");
    expect(ipcCalls.some((c) => c.cmd === "ingest_findings")).toBe(true);
  });

  it("explains an unconfigured service instead of posting nowhere", async () => {
    useSettingsStore.setState({ reviewService: null });
    await useDetailedReviewStore.getState().openPreflight();
    await useDetailedReviewStore.getState().start();
    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith("/v1/reviews"))).toBe(false);
    expect(useDetailedReviewStore.getState().phase).toBe("preflight");
  });
});
