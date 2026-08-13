import { mockIPC } from "@tauri-apps/api/mocks";
import { useSettingsStore, flushSettings } from "./settingsStore";

// Example of the fast frontend layer: drive a zustand store that talks to the
// Rust backend through `ipc`, with `invoke` mocked. No webview, no Rust — just
// the UI logic. `args` for a command match the object passed to `invoke`.
describe("settingsStore", () => {
  beforeEach(() => {
    // Reset the singleton store to its initial shape before each test.
    useSettingsStore.setState({ keymap: null, projectRoot: null, loaded: false });
  });

  it("loads the persisted keymap preset from the backend", async () => {
    mockIPC((cmd) => {
      if (cmd === "get_settings") return { keymap_preset: "kicad" };
      throw new Error(`unexpected command ${cmd}`);
    });

    await useSettingsStore.getState().load();

    const state = useSettingsStore.getState();
    expect(state.loaded).toBe(true);
    expect(state.keymap).toBe("kicad");
  });

  it("falls back to null + loaded when the backend errors", async () => {
    mockIPC(() => {
      throw new Error("no config dir");
    });

    await useSettingsStore.getState().load();

    const state = useSettingsStore.getState();
    expect(state.loaded).toBe(true);
    expect(state.keymap).toBeNull();
  });

  it("optimistically applies a keymap and persists it via set_settings", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      return undefined;
    });

    await useSettingsStore.getState().setKeymap("kicad");

    expect(useSettingsStore.getState().keymap).toBe("kicad");
    // The full settings object is persisted so one setter never clobbers another's
    // field (keymap write preserves project_root, and vice-versa).
    expect(calls).toContainEqual({
      cmd: "set_settings",
      args: {
        settings: expect.objectContaining({ keymap_preset: "kicad", project_root: null }),
      },
    });
  });

  it("persists the project root without clobbering the keymap", async () => {
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      return undefined;
    });
    useSettingsStore.setState({ keymap: "kicad", projectRoot: null, loaded: true });

    await useSettingsStore.getState().setProjectRoot("/home/me/SpinZero Projects");

    expect(useSettingsStore.getState().projectRoot).toBe("/home/me/SpinZero Projects");
    expect(calls).toContainEqual({
      cmd: "set_settings",
      args: {
        settings: expect.objectContaining({
          keymap_preset: "kicad",
          project_root: "/home/me/SpinZero Projects",
        }),
      },
    });
  });

  it("never writes this store's defaults over the file before load()", async () => {
    // A setter firing during boot (the updater's deferral check) used to persist the
    // in-memory defaults — wiping keymap, accent and every project's UI.
    const calls: Array<{ cmd: string; args: unknown }> = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "get_settings") return { keymap_preset: "kicad", accent_color: "#3fb950" };
      return undefined;
    });
    useSettingsStore.setState({ keymap: null, accentColor: null, loaded: false });

    await useSettingsStore.getState().setUpdateDeferred("1.2.3");

    // It read the file first, so the write carries the real prefs, not the defaults —
    // and the new value survives the read that preceded it.
    const write = calls.filter((c) => c.cmd === "set_settings").at(-1)!;
    expect(write.args).toEqual({
      settings: expect.objectContaining({
        keymap_preset: "kicad",
        accent_color: "#3fb950",
        update_deferred: "1.2.3",
      }),
    });
  });

  it("debounces the opacity sliders into one write, and flushes on teardown", async () => {
    const writes: unknown[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "set_settings") writes.push(args);
      return undefined;
    });
    useSettingsStore.setState({ loaded: true });

    // A drag: many ticks, no disk write yet.
    for (let i = 1; i <= 20; i++) {
      useSettingsStore.getState().setPcbOpacity({ tracks: i / 20 });
    }
    expect(writes).toHaveLength(0);
    expect(useSettingsStore.getState().pcbOpacity).toEqual({ tracks: 1 }); // UI is live

    await flushSettings(); // stands in for the trailing timer / pagehide
    expect(writes).toHaveLength(1);
    expect(writes[0]).toEqual({
      settings: expect.objectContaining({ pcb_opacity: { tracks: 1 } }),
    });

    // Nothing pending → flush is a no-op rather than a redundant write.
    await flushSettings();
    expect(writes).toHaveLength(1);
  });

  it("prunes project_ui only past the cap, and only for folders that are gone", async () => {
    const gone = "/p/deleted";
    const offline = "/p/unplugged";
    mockIPC((cmd, args) => {
      if (cmd === "get_settings") {
        // 26 entries: over the cap of 24, so a prune runs.
        const project_ui: Record<string, unknown> = {
          [gone]: { status_tab: "All", last_seen: "2026-01-01T00:00:00.000Z" },
          // Newest, so the LRU trim below can't be what saves it.
          [offline]: { status_tab: "All", last_seen: "2026-12-31T00:00:00.000Z" },
        };
        for (let i = 0; i < 24; i++) {
          project_ui[`/p/live${i}`] = { last_seen: `2026-02-${String(i + 1).padStart(2, "0")}T00:00:00.000Z` };
        }
        return { project_ui };
      }
      if (cmd === "inspect_folder") {
        const path = (args as { path: string }).path;
        if (path === gone) return "unknown"; // folder no longer exists
        if (path === offline) throw new Error("network share unavailable");
        return "project";
      }
      return undefined;
    });

    await useSettingsStore.getState().load();
    await new Promise((r) => setTimeout(r, 0)); // prune runs off the load

    const kept = useSettingsStore.getState().projectUi;
    expect(kept[gone]).toBeUndefined(); // definitively gone → dropped
    // An IPC failure is never itself a reason to delete: the unreachable share
    // survives the existence pass. (Past the cap the LRU trim can still evict it —
    // that is the price of bounding the map, and it takes the oldest first.)
    expect(kept[offline]).toBeDefined();
    expect(kept["/p/live0"]).toBeUndefined(); // oldest survivor, trimmed to the cap
    expect(Object.keys(kept)).toHaveLength(24); // back at the cap
  });

  it("leaves project_ui alone while it is under the cap", async () => {
    const inspected: string[] = [];
    mockIPC((cmd, args) => {
      if (cmd === "get_settings") return { project_ui: { "/p/a": {}, "/p/b": {} } };
      if (cmd === "inspect_folder") {
        inspected.push((args as { path: string }).path);
        return "unknown";
      }
      return undefined;
    });

    await useSettingsStore.getState().load();
    await new Promise((r) => setTimeout(r, 0));

    // No filesystem probing at all for a normal-sized map — and nothing removed, even
    // though both folders would have reported "gone".
    expect(inspected).toEqual([]);
    expect(Object.keys(useSettingsStore.getState().projectUi)).toEqual(["/p/a", "/p/b"]);
  });
});
