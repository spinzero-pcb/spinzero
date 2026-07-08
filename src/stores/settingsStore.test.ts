import { mockIPC } from "@tauri-apps/api/mocks";
import { useSettingsStore } from "./settingsStore";

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
});
