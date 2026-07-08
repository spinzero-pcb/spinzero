import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach } from "vitest";

// Tear down the DOM and any installed IPC mock between tests so state from one
// test can never leak into the next.
afterEach(() => {
  cleanup();
  clearMocks();
});
