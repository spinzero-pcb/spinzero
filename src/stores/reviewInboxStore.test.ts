import { mockIPC } from "@tauri-apps/api/mocks";
import { useReviewInboxStore } from "./reviewInboxStore";
import { useBomCheckStore } from "./bomCheckStore";
import { useReviewStore } from "./reviewStore";
import { useToastStore } from "./toastStore";
import type { CheckOutcome, FindingsDoc } from "../lib/findings";

function doc(partial: Partial<FindingsDoc> = {}): FindingsDoc {
  return {
    schema_version: "1.0",
    engine_version: "engine-0.2.0",
    pipeline: "bom-detailed",
    profile: "default",
    findings: [],
    bom_audit: [],
    stats: { item_count: 3, finding_count: 0, duration_ms: 2 },
    ...partial,
  };
}

function outcome(partial: Partial<CheckOutcome> = {}): CheckOutcome {
  return {
    findings: doc(),
    session_id: "s_mcp",
    filed: 2,
    reopened: 0,
    unchanged: 1,
    auto_resolved: 0,
    unmapped_columns: [],
    comments: [],
    ...partial,
  };
}

describe("reviewInboxStore", () => {
  beforeEach(() => {
    useReviewInboxStore.setState({ entries: [], importing: null, error: null });
    useToastStore.setState({ toasts: [] });
    useBomCheckStore.getState().clear();
  });

  it("lists what is waiting, including what it cannot import", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_review_inbox")
        return [
          { name: "bom-detailed-1.json", pipeline: "bom-detailed", engine_version: "e", finding_count: 4, error: null },
          { name: "junk.json", pipeline: "", engine_version: "", finding_count: 0, error: "not valid JSON" },
        ];
      return undefined;
    });
    await useReviewInboxStore.getState().load();
    // A file that cannot be imported is kept in the list: a review the user believes
    // ran and cannot find anywhere is worse than an error line.
    expect(useReviewInboxStore.getState().entries).toHaveLength(2);
    expect(useReviewInboxStore.getState().error).toBeNull();
  });

  it("a failed listing is recorded, not thrown at the user", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_review_inbox") throw new Error("no project open");
      return undefined;
    });
    await useReviewInboxStore.getState().load();
    expect(useReviewInboxStore.getState().entries).toEqual([]);
    expect(useReviewInboxStore.getState().error).toContain("no project open");
    // Listing happens whenever the launcher opens, so it must never interrupt someone
    // who was doing something else.
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it("imports through the same landing path a hosted review takes", async () => {
    const calls: string[] = [];
    let listed = [
      { name: "bom-detailed-1.json", pipeline: "bom-detailed", engine_version: "e", finding_count: 4, error: null },
    ];
    mockIPC((cmd, args) => {
      calls.push(cmd);
      if (cmd === "list_review_inbox") return listed;
      if (cmd === "import_review_inbox") {
        expect((args as { name: string }).name).toBe("bom-detailed-1.json");
        // The backend archives the file out of the drop-box once its findings are
        // comments, so the re-listing that follows is what removes the row.
        listed = [];
        return outcome({ findings: doc({ findings: [] }) });
      }
      if (cmd === "list_comments") return [];
      if (cmd === "list_review_sessions") return [];
      return undefined;
    });

    await useReviewInboxStore.getState().importOne("bom-detailed-1.json");

    expect(calls).toContain("import_review_inbox");
    expect(useReviewInboxStore.getState().entries).toEqual([]);
    expect(useReviewInboxStore.getState().importing).toBeNull();
    // The BOM strip renders whatever ran last, whichever surface produced it.
    expect(useBomCheckStore.getState().sessionId).toBe("s_mcp");
    expect(useReviewStore.getState().activeSessionId).toBe("s_mcp");
    expect(useToastStore.getState().toasts[0]?.title).toContain("Imported review");
  });

  it("says a review is incomplete rather than reporting it clean", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_review_inbox") return [];
      if (cmd === "import_review_inbox")
        return outcome({
          findings: doc({
            run_health: [{ stage: "judgment_pass", status: "degraded", detail: "4 of 6 checks unreviewed" }],
          }),
        });
      if (cmd === "list_comments") return [];
      if (cmd === "list_review_sessions") return [];
      return undefined;
    });
    await useReviewInboxStore.getState().importOne("partial.json");
    const toast = useToastStore.getState().toasts[0];
    expect(toast?.kind).toBe("error");
    expect(toast?.title).toContain("incomplete");
  });

  it("surfaces a failed import and leaves the file in the drop-box", async () => {
    mockIPC((cmd) => {
      if (cmd === "list_review_inbox")
        return [{ name: "x.json", pipeline: "bom-detailed", engine_version: "e", finding_count: 1, error: null }];
      if (cmd === "import_review_inbox") throw new Error("findings schema_version 2.0 is not supported");
      return undefined;
    });
    await useReviewInboxStore.getState().importOne("x.json");
    expect(useToastStore.getState().toasts[0]?.kind).toBe("error");
    expect(useReviewInboxStore.getState().entries).toHaveLength(1);
  });
});
