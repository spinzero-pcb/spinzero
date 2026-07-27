import { mockIPC, clearMocks } from "@tauri-apps/api/mocks";
import { useProjectStore } from "./projectStore";

/** Benign fan-out mocks for the post-switch refresh (refreshIndex + design load):
 *  summary + a sheet so the retry loop exits on the first attempt. */
function fanOut(cmd: string): unknown {
  if (cmd === "get_project_summary") return { name: "board" };
  if (cmd === "list_sheets") return [{ num: 1, name: "root", path: "/", parent: null }];
  if (cmd === "list_layers") return [];
  if (cmd === "list_extractions") return [];
  if (cmd === "get_highlights") return null;
  if (cmd === "get_design_indexes") throw new Error("no bundle in tests");
  if (cmd === "log_frontend_warn" || cmd === "log_frontend_error") return undefined;
  return null;
}

/** Wait until the store exposes a pending checkout prompt (or fail fast). */
async function waitForPrompt() {
  for (let i = 0; i < 50; i++) {
    const prompt = useProjectStore.getState().checkoutPrompt;
    if (prompt) return prompt;
    await new Promise((r) => setTimeout(r, 5));
  }
  throw new Error("checkoutPrompt never appeared");
}

describe("projectStore revision switching", () => {
  beforeEach(() => {
    useProjectStore.setState({
      activeExtraction: null,
      busy: false,
      checkoutPrompt: null,
    });
  });
  afterEach(() => clearMocks());

  it("setActiveExtraction is a pure viewer switch — never calls update_design_files", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "set_active_extraction") return undefined;
      return fanOut(cmd);
    });

    await useProjectStore.getState().setActiveExtraction("rev1");

    expect(useProjectStore.getState().activeExtraction).toBe("rev1");
    expect(calls).toContainEqual({ cmd: "set_active_extraction", args: { id: "rev1" } });
    expect(calls.some((c) => c.cmd === "update_design_files")).toBe(false);
    // No confirmation dialog for a view-only switch.
    expect(useProjectStore.getState().checkoutPrompt).toBeNull();
  });

  it("updateDesignFiles confirms a dirty tree, then retries with confirmed=true", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "update_design_files") {
        const a = args as { id: string; confirmed: boolean };
        return a.confirmed
          ? { status: "switched", captured: "cp1" }
          : { status: "dirty", captured: null };
      }
      return fanOut(cmd);
    });

    const done = useProjectStore.getState().updateDesignFiles("rev2");
    const prompt = await waitForPrompt();
    prompt.resolve(true);
    await done;

    expect(calls).toContainEqual({ cmd: "update_design_files", args: { id: "rev2", confirmed: false } });
    expect(calls).toContainEqual({ cmd: "update_design_files", args: { id: "rev2", confirmed: true } });
    expect(useProjectStore.getState().activeExtraction).toBe("rev2");
    expect(useProjectStore.getState().checkoutPrompt).toBeNull();
    expect(useProjectStore.getState().busy).toBe(false);
  });

  it("cancelling the dirty confirmation leaves disk and viewer untouched", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "update_design_files") return { status: "dirty", captured: null };
      return fanOut(cmd);
    });

    const done = useProjectStore.getState().updateDesignFiles("rev3");
    const prompt = await waitForPrompt();
    prompt.resolve(false);
    await done;

    // Only the probing (unconfirmed) call went out — no confirmed overwrite.
    expect(calls.filter((c) => c.cmd === "update_design_files")).toEqual([
      { cmd: "update_design_files", args: { id: "rev3", confirmed: false } },
    ]);
    expect(useProjectStore.getState().activeExtraction).toBeNull();
    expect(useProjectStore.getState().busy).toBe(false);
  });
});
