import { mockIPC } from "@tauri-apps/api/mocks";
import { beforeEach, describe, expect, it } from "vitest";
import type { MappingView } from "../lib/findings";
import { useBomMappingStore } from "./bomMappingStore";
import { useRunLauncherStore } from "./runLauncherStore";

// The mapping is edited inside the BOM Review window, and pressing Run there is the
// approval. What is pinned: Run writes the mapping only when there is something to
// record, it writes the whole resolved mapping (not just the edits), and a review
// started elsewhere on an unapproved mapping opens the window instead of running.

function view(approved: boolean): MappingView {
  return {
    fields: [
      { logical: "mpn", column: "MPN", auto: "MPN", overridden: false },
      { logical: "lifecycle", column: "", auto: "", overridden: false },
    ],
    columns: [
      { name: "MPN", fill_rate: 1, sample: "GRM155" },
      { name: "Status", fill_rate: 1, sample: "Active" },
    ],
    unmapped_columns: [{ column: "Status", fill_rate: 1 }],
    row_count: 12,
    approved,
  };
}

let saved: Record<string, string>[] = [];
let current: MappingView;

beforeEach(() => {
  saved = [];
  current = view(false);
  useBomMappingStore.getState().reset();
  useRunLauncherStore.setState({ setupFor: null, menuOpen: false });
  mockIPC((cmd, args) => {
    if (cmd === "get_bom_mapping") return current;
    if (cmd === "set_bom_mapping") {
      saved.push((args as { overrides: Record<string, string> }).overrides);
      return null;
    }
    return null;
  });
});

describe("bomMappingStore", () => {
  it("expands the mapping the first time, so the guess is seen", async () => {
    await useBomMappingStore.getState().load("industrial");
    expect(useBomMappingStore.getState().expanded).toBe(true);
  });

  it("stays folded once approved", async () => {
    current = view(true);
    await useBomMappingStore.getState().load("industrial");
    expect(useBomMappingStore.getState().expanded).toBe(false);
  });

  it("records the whole resolved mapping, edits included, on first Run", async () => {
    await useBomMappingStore.getState().load("industrial");
    useBomMappingStore.getState().setField("lifecycle", "Status");
    expect(await useBomMappingStore.getState().save()).toBe(true);
    expect(saved).toEqual([{ mpn: "MPN", lifecycle: "Status" }]);
    expect(useBomMappingStore.getState().view?.approved).toBe(true);
  });

  it("writes nothing when an approved mapping was left alone", async () => {
    current = view(true);
    await useBomMappingStore.getState().load("industrial");
    expect(await useBomMappingStore.getState().save()).toBe(true);
    expect(saved).toEqual([]);
  });

  it("opens the BOM Review window instead of running on an unapproved mapping", async () => {
    expect(await useBomMappingStore.getState().ensureApproved("industrial")).toBe(false);
    expect(useRunLauncherStore.getState().setupFor).toBe("bom");
    expect(useBomMappingStore.getState().expanded).toBe(true);
  });

  it("lets an approved mapping through", async () => {
    current = view(true);
    expect(await useBomMappingStore.getState().ensureApproved("industrial")).toBe(true);
    expect(useRunLauncherStore.getState().setupFor).toBeNull();
  });
});
