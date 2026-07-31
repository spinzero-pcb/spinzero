import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit/integration tests run in jsdom with the Tauri IPC layer mocked
// (see src/test/setup.ts + @tauri-apps/api/mocks). This exercises React/zustand
// UI logic without a native webview or the Rust backend — the fast, deterministic
// layer. For real end-to-end coverage against the live app, use tauri-pilot
// (docs/testing.md), which drives the actual webview + backend.
//
// The per-project split that makes the suite fast lives in vitest.workspace.ts;
// this file holds the settings both projects inherit.
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    // `include` is set per project in vitest.workspace.ts — the split is by file type.
    // Spawning worker threads is much cheaper than forking a process per test file.
    pool: "threads",
  },
});
