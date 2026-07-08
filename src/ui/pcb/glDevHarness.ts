// Standalone dev harness for the WebGL PCB renderer (served at /gldev.html).
//
// The full app needs Tauri IPC to load a design, which a plain browser can't provide.
// This page loads a geometry IR fixture directly (public/dev-geometry.json) so the
// renderer can be developed and visually verified in a browser/preview in isolation.
// Generate the (gitignored) fixture with:
//   pcb-extract design <project.kicad_pro> -o <out> && cp <out>/pcb/geometry.json public/dev-geometry.json
//
// Controls: drag = pan, wheel = zoom (toward cursor), F = fit, H = toggle GND highlight.

import "../../styles/tokens.css";
import { parsePcbGeometry } from "../../lib/pcbGeometry";
import { PcbGlRenderer, type Camera, type ObjectState } from "./glRenderer";

const canvas = document.getElementById("c") as HTMLCanvasElement;
const hud = document.getElementById("hud") as HTMLDivElement;

const objState: ObjectState = {
  objects: { tracks: true, vias: true, pads: true, zones: true, footprints: true, text: true },
  opacity: { tracks: 1, vias: 1, pads: 1, zones: 0.6, footprints: 1, text: 1 },
};

async function main() {
  // preserveDrawingBuffer keeps the last frame readable for screenshots (the harness
  // renders on demand, not in a perpetual loop, so the buffer must persist).
  const gl = canvas.getContext("webgl2", {
    antialias: true,
    premultipliedAlpha: true, // match the app (avoids the dark AA fringe); see PcbGlView
    preserveDrawingBuffer: true,
  });
  if (!gl) {
    hud.textContent = "WebGL2 not available";
    return;
  }
  const text = await fetch("/dev-geometry.json").then((r) => r.text());
  const geom = parsePcbGeometry(text);
  const renderer = new PcbGlRenderer(gl, geom);
  renderer.setLayerState(new Set(), null);
  // Expose the renderer + geometry for headless verification (readPixels), mirroring the
  // app's window.__spinzero probe. Dev harness only — never bundled into the app.
  Object.assign(window as unknown as Record<string, unknown>, { __glr: renderer, __geom: geom });

  let cam: Camera = { x: 0, y: 0, scale: 1 };
  const fit = () => {
    const b = renderer.bbox;
    const bw = Math.max(b.maxx - b.minx, 1);
    const bh = Math.max(b.maxy - b.miny, 1);
    const cw = canvas.clientWidth || 1;
    const ch = canvas.clientHeight || 1;
    cam = { x: (b.minx + b.maxx) / 2, y: (b.miny + b.maxy) / 2, scale: Math.min(cw / bw, ch / bh) * 0.92 };
  };
  fit();

  // ---- interaction ----
  let dragging = false;
  let lastX = 0;
  let lastY = 0;
  canvas.addEventListener("pointerdown", (e) => {
    dragging = true;
    lastX = e.clientX;
    lastY = e.clientY;
    canvas.setPointerCapture(e.pointerId);
    canvas.style.cursor = "grabbing";
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    cam.x -= (e.clientX - lastX) / cam.scale;
    cam.y -= (e.clientY - lastY) / cam.scale;
    lastX = e.clientX;
    lastY = e.clientY;
    draw();
  });
  const endDrag = () => {
    dragging = false;
    canvas.style.cursor = "default";
  };
  canvas.addEventListener("pointerup", endDrag);
  canvas.addEventListener("pointercancel", endDrag);
  canvas.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left - rect.width / 2;
      const my = e.clientY - rect.top - rect.height / 2;
      const wx = cam.x + mx / cam.scale;
      const wy = cam.y + my / cam.scale;
      cam.scale *= Math.exp(-e.deltaY * 0.0015);
      cam.scale = Math.max(0.05, Math.min(cam.scale, 400));
      cam.x = wx - mx / cam.scale;
      cam.y = wy - my / cam.scale;
      draw();
    },
    { passive: false },
  );

  let hl = false;
  window.addEventListener("keydown", (e) => {
    if (e.key === "f" || e.key === "F") fit();
    if (e.key === "h" || e.key === "H") {
      hl = !hl;
      const gnd = renderer.netIndexByName.get("GND");
      // #4f8cff (the default net-highlight colour) so toggling 'h' shows the recolour.
      renderer.setSelection(hl && gnd != null ? [{ id: gnd, color: [0.31, 0.55, 1] }] : [], []);
    }
    draw();
  });

  // ---- render loop + fps ----
  let frames = 0;
  let lastFps = performance.now();
  let fps = 0;
  const counts = {
    tracks: geom.tracks.seg.w.length + geom.tracks.arc.w.length,
    vias: geom.vias.length,
    pads: geom.pads.length,
    zones: geom.zones.length,
    layers: geom.layers.length,
    nets: geom.nets.length - 1,
  };

  const draw = () => {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.round(canvas.clientWidth * dpr);
    const h = Math.round(canvas.clientHeight * dpr);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    renderer.setDpr(dpr);
    renderer.render(cam, w, h, objState);

    frames++;
    const now = performance.now();
    if (now - lastFps >= 500) {
      fps = Math.round((frames * 1000) / (now - lastFps));
      frames = 0;
      lastFps = now;
    }
    hud.textContent =
      `${fps} fps   zoom ${cam.scale.toFixed(1)} px/mm\n` +
      `tracks ${counts.tracks}  pads ${counts.pads}  vias ${counts.vias}  zones ${counts.zones}\n` +
      `layers ${counts.layers}  nets ${counts.nets}   [drag pan · wheel zoom · F fit · H GND]`;
  };
  // Render on demand (initial frame + after each interaction). A perpetual loop would
  // keep the page from ever going idle, which blocks headless screenshots; on-demand
  // rendering + preserveDrawingBuffer keeps the last frame visible for capture.
  draw();
  window.addEventListener("resize", draw);
  // Expose for preview-driven probing.
  (window as unknown as { __pcbgl: unknown }).__pcbgl = { renderer, geom, cam: () => cam, fit, draw };
}

main().catch((e) => {
  hud.textContent = `error: ${String(e)}`;
  console.error(e);
});
