import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend unit/integration tests run in jsdom with the Tauri IPC layer mocked
// (see src/test/setup.ts + @tauri-apps/api/mocks). This exercises React/zustand
// UI logic without a native webview or the Rust backend — the fast, deterministic
// layer. For real end-to-end coverage against the live app, use tauri-pilot
// (docs/testing.md), which drives the actual webview + backend.
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
