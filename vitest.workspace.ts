import { defineWorkspace } from "vitest/config";

// Suite runtime is dominated by standing up a fresh jsdom per test file, not by the
// assertions (~1s of actual test time). Splitting by file type lets the bulk of the
// suite skip that cost:
//
//   components — `.test.tsx` files mount React trees whose module-level state leaks
//                across files, so they keep the default per-file isolation.
//   logic      — `.test.ts` files are pure logic/store tests; they share one
//                environment per worker (src/test/setup.ts still tears the DOM and
//                the IPC mock down after every test).
//
// Everything else (plugins, jsdom, setup file, thread pool) is inherited from
// vitest.config.ts.
export default defineWorkspace([
  {
    extends: "./vitest.config.ts",
    test: { name: "components", include: ["src/**/*.{test,spec}.tsx"] },
  },
  {
    extends: "./vitest.config.ts",
    test: { name: "logic", include: ["src/**/*.{test,spec}.ts"], isolate: false },
  },
]);
