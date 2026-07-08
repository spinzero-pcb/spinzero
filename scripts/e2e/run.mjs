#!/usr/bin/env node
// tauri-pilot E2E runner (Layer 2). Drives the LIVE app over tauri-pilot and asserts
// on chrome + the debug render probe (window.__spinzero). See docs/e2e-tauri-pilot.md
// for the full catalog — this file implements the validated cases and is the place to
// add the rest (same shape).
//
// Usage:
//   1. Launch the debug app with a fixture:
//        PCBREVIEW_CACHE_DIR=<fixtureRoot> npm run tauri dev
//   2. npm run e2e
//
// Exits non-zero on the first failed assertion. Env knobs:
//   TAURI_PILOT  path to the tauri-pilot binary (default: ~/.cargo/bin or PATH)
//   PILOT_WINDOW window label (default: main)
//   E2E_QUERY    palette search term that yields ≥1 result (default: GND)

import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir, platform } from "node:os";
import { join } from "node:path";

const WIN = platform() === "win32";
const PILOT =
  process.env.TAURI_PILOT ||
  [join(homedir(), ".cargo", "bin", WIN ? "tauri-pilot.exe" : "tauri-pilot")].find(existsSync) ||
  (WIN ? "tauri-pilot.exe" : "tauri-pilot");
const WINDOW = process.env.PILOT_WINDOW || "main";
const QUERY = process.env.E2E_QUERY || "GND";

/** Run a tauri-pilot subcommand, resolve trimmed stdout (rejects on CLI error). */
function pilot(args) {
  return new Promise((resolve, reject) => {
    execFile(PILOT, ["--window", WINDOW, ...args], { maxBuffer: 16 * 1024 * 1024 }, (err, out, errout) => {
      if (err) return reject(new Error((errout || err.message || "").trim()));
      resolve(String(out).trim());
    });
  });
}
/** eval a statement/expression; returns raw stdout text. */
const ev = (js) => pilot(["eval", js]);
/** eval an EXPRESSION and JSON-parse the result. Wrap multi-statement work in an IIFE. */
const evj = async (expr) => JSON.parse(await ev(`JSON.stringify((${expr}))`));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- tiny test harness -----------------------------------------------------
let pass = 0;
const failures = [];
async function test(id, fn) {
  try {
    await fn();
    pass++;
    console.log(`  ✓ ${id}`);
  } catch (e) {
    failures.push(`${id}: ${e.message}`);
    console.log(`  ✗ ${id} — ${e.message}`);
  }
}
function assert(cond, msg) {
  if (!cond) throw new Error(msg || "assertion failed");
}

// ---- shared UI drivers (mirror docs/e2e-tauri-pilot.md idioms) -------------
const clickTab = (label) =>
  ev(`[...document.querySelectorAll('.view-tab')].find(b=>b.textContent.trim()===${JSON.stringify(label)})?.click()??'no-tab'`);
const key = (k, mods = {}) =>
  ev(`window.dispatchEvent(new KeyboardEvent('keydown',{key:${JSON.stringify(k)},bubbles:true,...${JSON.stringify(mods)}}));'ok'`);

async function main() {
  console.log(`tauri-pilot E2E → ${PILOT} (window=${WINDOW})\n`);

  // Gate: probe must be installed (app booted + a canvas mounted in a debug build).
  const probeType = await ev("typeof window.__spinzero").catch((e) => `__error__ ${e.message}`);
  if (probeType !== "object") {
    console.error(
      `window.__spinzero is "${probeType}" — is the DEBUG app running with PCBREVIEW_CACHE_DIR set?`,
    );
    process.exit(2);
  }

  console.log("A. boot & shell");
  await test("A2 both views registered", async () => {
    const v = await evj("window.__spinzero.views()");
    assert(v.includes("schematic") && v.includes("pcb"), `views()=${JSON.stringify(v)}`);
  });
  await test("A3 no error logs on boot", async () => {
    const logs = await pilot(["logs"]);
    const errs = logs.split("\n").filter((l) => /\]\s*error\b/i.test(l));
    assert(errs.length === 0, `error logs:\n${errs.join("\n")}`);
  });

  console.log("B. schematic canvas (probe)");
  await clickTab("Schematic");
  await test("B1 schematic mounted on a sheet", async () => {
    const s = await evj("window.__spinzero.view('schematic')");
    assert(s.sheetName && s.elements > 0 && s.cam.s > 0, JSON.stringify(s).slice(0, 200));
  });
  await test("B2 viewBox is valid", async () => {
    const s = await evj("window.__spinzero.view('schematic')");
    assert(s.viewBox.length === 4 && s.viewBox[2] > 0 && s.viewBox[3] > 0, JSON.stringify(s.viewBox));
  });

  console.log("E/B3. palette search → selection → highlight");
  await test("E1 Ctrl+F opens search palette", async () => {
    await key("f", { ctrlKey: true });
    await sleep(300);
    const ph = await ev("document.querySelector('.palette-input')?.placeholder ?? 'none'");
    assert(/search/i.test(ph), `placeholder="${ph}"`);
  });
  await test("B3 selecting first net result highlights it", async () => {
    await ev(
      `(()=>{const i=document.querySelector('.palette-input');const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;set.call(i,${JSON.stringify(QUERY)});i.dispatchEvent(new Event('input',{bubbles:true}));return 'typed'})()`,
    );
    await sleep(400);
    const ref = await ev("document.querySelector('.palette-item .palette-ref')?.textContent.trim() ?? ''");
    assert(ref, `no palette results for "${QUERY}" (set E2E_QUERY)`);
    await ev("document.querySelector('.palette-item')?.click()");
    await sleep(600);
    const s = await evj("window.__spinzero.view('schematic')");
    assert(s.highlights.length >= 1, "no highlight after select");
    assert(s.highlights[0].ref === ref, `highlight ${s.highlights[0].ref} != ${ref}`);
    assert(s.overlays >= 1 && s.dimmed, `overlays=${s.overlays} dimmed=${s.dimmed}`);
  });
  await test("B5 Esc clears the selection", async () => {
    await key("Escape");
    await sleep(400);
    const s = await evj("window.__spinzero.view('schematic')");
    assert(s.highlights.length === 0 && s.overlays === 0 && !s.dimmed, JSON.stringify(s).slice(0, 160));
  });

  console.log("C. PCB canvas (probe)");
  await test("C1 layer stack present", async () => {
    const p = await evj("window.__spinzero.view('pcb')");
    assert(p.layers.includes("F.Cu") && p.layers.includes("B.Cu"), JSON.stringify(p.layers));
  });
  await test("C2 first PCB show fits the board", async () => {
    await clickTab("PCB");
    await sleep(600);
    const p = await evj("window.__spinzero.view('pcb')");
    assert(p.visible && p.fitted && p.cam.s > 0, `visible=${p.visible} fitted=${p.fitted} s=${p.cam.s}`);
  });
  await test("C6 object filter toggles (pads)", async () => {
    const before = await evj("window.__spinzero.view('pcb').objects.pads");
    await ev(
      "(()=>{const r=[...document.querySelectorAll('.pcb-objrow')].find(x=>/pad/i.test(x.textContent));const c=r&&r.querySelector('input,button');c&&c.click();return 'ok'})()",
    );
    await sleep(300);
    const after = await evj("window.__spinzero.view('pcb').objects.pads");
    assert(before !== after, `pads stayed ${before}`);
    // restore
    await ev(
      "(()=>{const r=[...document.querySelectorAll('.pcb-objrow')].find(x=>/pad/i.test(x.textContent));const c=r&&r.querySelector('input,button');c&&c.click();return 'ok'})()",
    );
  });

  console.log("H/J. review + menus");
  await clickTab("Schematic");
  await test("H1 C arms comment mode", async () => {
    await key("c");
    await sleep(300);
    const armed = await evj("!!document.querySelector('.canvas-stage.arming')");
    assert(armed, "comment mode not armed");
    await key("Escape"); // disarm
  });
  await test("J1 File menu lists core items", async () => {
    await ev("[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='File')?.click()");
    await sleep(300);
    const items = await evj(
      "[...document.querySelectorAll('[role=menuitem],button')].map(b=>b.textContent.trim())",
    );
    for (const want of ["New Project", "Open Project", "Privacy"])
      assert(items.some((t) => t.includes(want)), `menu missing "${want}"`);
    await key("Escape");
  });

  console.log("M. robustness");
  await test("M4 no error logs after the suite", async () => {
    const logs = await pilot(["logs"]);
    const errs = logs.split("\n").filter((l) => /\]\s*error\b/i.test(l));
    assert(errs.length === 0, `error logs:\n${errs.join("\n")}`);
  });

  // ---- summary -------------------------------------------------------------
  console.log(`\n${pass} passed, ${failures.length} failed`);
  if (failures.length) {
    console.error("\nFailures:\n - " + failures.join("\n - "));
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("runner crashed:", e.message);
  process.exit(2);
});
