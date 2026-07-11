import { useEffect, useRef, useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import {
  COMP_COLOR,
  NET_COLOR,
  colorForNext,
  useSelectionStore,
  type Highlight,
} from "../../stores/selectionStore";
import { sheetMatches, type Selection, type SheetRef } from "../../lib/design";
import { isTypingTarget } from "../../lib/keymap";
import { useViewStore } from "../../stores/viewStore";
import { camBridge, canvasRestore, emphasizeDiffText, nav, type ChipComment } from "./navigator";
import { useDiffStore } from "../../stores/diffStore";
import { relabelInstances } from "./relabel";
import { useReviewStore } from "../../stores/reviewStore";
import { Overview } from "./Overview";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { IconBoard, IconClose, IconComment, IconCopy, IconFit, IconTrash } from "../icons";
import { registerRenderProbe } from "../../lib/renderProbe";
import { ipc } from "../../lib/ipc";

const SVG_NS = "http://www.w3.org/2000/svg";
const CARD_GUTTER = 300; // keep the landing clear of the properties card on the right
const PAD = 24;
const LANDING_WIDTH_MM = 130; // context width when landing on a net (spec §2)
const BADGE_KINDS = ["ports", "labels", "sheet_entries", "power_ports"]; // off-sheet links

interface Cam {
  x: number;
  y: number;
  s: number;
}
interface HistEntry {
  sheet: number;
  cam: Cam;
  highlights: Highlight[];
  selection: Selection;
}
interface BadgeAnchor {
  ux: number;
  uy: number;
  /** screen-space vertical offset for stacked badges (one per destination sheet) */
  dy: number;
  el: HTMLDivElement;
  /** cached layout size — badges never change size after creation */
  w?: number;
  h?: number;
}

/** The hyperlinked schematic canvas (WS1–WS3): live inline-SVG island, smoothed
 *  camera, click-everything selection, multi-net highlight, net-to-net jumps,
 *  browser-style history. */
export function Canvas() {
  const indexes = useDesignStore((s) => s.indexes);
  const getSheetSvg = useDesignStore((s) => s.getSheetSvg);
  const setSelectionStore = useSelectionStore((s) => s.setSelection);
  const setHighlightStore = useSelectionStore((s) => s.setHighlights);
  const setCurrentSheet = useSelectionStore((s) => s.setCurrentSheet);
  // Persistent highlights (item 22) live in the store; repaint when they change
  // (e.g. loaded from disk after mount, or pinned/unpinned via the context menu).
  const pinned = useSelectionStore((s) => s.pinned);
  // Comment mode (C): a crosshair signals you can click an object or drag a box.
  const armed = useReviewStore((s) => s.armed);

  const stageRef = useRef<HTMLDivElement>(null);
  const worldRef = useRef<HTMLDivElement>(null);
  const islandRef = useRef<HTMLDivElement>(null);
  const badgeLayerRef = useRef<HTMLDivElement>(null);
  const commentLayerRef = useRef<HTMLDivElement>(null);

  // Right-click menu — built imperatively inside the canvas effect (where the
  // hit-test + selection helpers live), then rendered from this state.
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);

  // Overview contact-sheet (WS6) — opened by zoom-out-past-fit or the overview key.
  const [overviewOpen, setOverviewOpen] = useState(false);
  const overviewRef = useRef(overviewOpen);
  useEffect(() => {
    overviewRef.current = overviewOpen;
  }, [overviewOpen]);

  // D1 tombstone: the sheet we tried to restore after a reload no longer exists.
  const [tombstone, setTombstone] = useState<string | null>(null);
  useEffect(() => {
    if (!tombstone) return;
    const t = setTimeout(() => setTombstone(null), 6000);
    return () => clearTimeout(t);
  }, [tombstone]);

  const cam = useRef<Cam>({ x: 0, y: 0, s: 1 });
  const tgt = useRef<Cam>({ x: 0, y: 0, s: 1 });
  const vb = useRef<[number, number, number, number]>([0, 0, 297, 210]);
  const curSvg = useRef<SVGSVGElement | null>(null);
  const curSheet = useRef<number | null>(null);
  /** Last interaction point in world (sheet user-unit) coords — badge anchoring. */
  const focusWorld = useRef<{ x: number; y: number } | null>(null);
  /** Per-sheet bbox cache of net geometry, for the nearest-net click fallback. */
  const bboxCache = useRef<Map<string, { x: number; y: number; w: number; h: number }> | null>(null);
  const highlights = useRef<Highlight[]>([]);
  const hiddenSources = useRef<SVGElement[]>([]);
  const badgeAnchors = useRef<BadgeAnchor[]>([]);
  const commentAnchors = useRef<BadgeAnchor[]>([]);
  const comments = useRef<ChipComment[]>([]);
  const hist = useRef<HistEntry[]>([]);
  const fwd = useRef<HistEntry[]>([]);
  /** Exposes the in-effect renderHighlights so a pinned-change effect can repaint. */
  const renderRef = useRef<() => void>(() => {});

  useEffect(() => {
    if (!indexes) return;
    const idx = indexes; // non-null capture so the closures below keep narrowing
    const stage = stageRef.current!;
    const world = worldRef.current!;
    const island = islandRef.current!;
    const badgeLayer = badgeLayerRef.current!;
    const commentLayer = commentLayerRef.current!;

    const sheetName = (n: number) => idx.sheets.find((s) => s.num === n)?.name ?? String(n);
    const esc = (s: string) =>
      window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/["\\]/g, "\\$&");

    // ------------------------------------------- per-sheet uuid resolution
    // Sheets instantiated multiple times from one source file (gate_driver U/V/W)
    // share element uuids, so the GLOBAL svg_to_net/svg_to_comp maps only cover one
    // instance (~30% of gate_driver_U resolves globally). The per-sheet net data in
    // nets[].by_sheet disambiguates — always consult it first.
    const sheetNetMaps = new Map<number, Map<string, string>>();
    function netMapFor(num: number): Map<string, string> {
      let m = sheetNetMaps.get(num);
      if (!m) {
        m = new Map();
        for (const [name, n] of Object.entries(idx.nets)) {
          const uuids = n.by_sheet[String(num)];
          if (uuids) for (const u of uuids) m.set(u, name);
        }
        sheetNetMaps.set(num, m);
      }
      return m;
    }
    function netOf(u: string): string | undefined {
      const num = curSheet.current;
      const local = num != null ? netMapFor(num).get(u) : undefined;
      if (local) return local;
      // Multi-valued map covers instance-sheet wires the local map misses; pick the
      // candidate net that actually lives on this sheet.
      const multi = idx.svg_to_nets[u];
      if (multi?.length) {
        if (multi.length === 1) return multi[0];
        if (num != null) {
          const here = multi.find((nm) => idx.nets[nm]?.sheets.includes(num));
          if (here) return here;
        }
        return multi[0];
      }
      return idx.svg_to_net[u];
    }
    const sheetCompMaps = new Map<number, Map<string, string>>();
    function compOf(u: string): string | undefined {
      const num = curSheet.current;
      if (num != null) {
        let m = sheetCompMaps.get(num);
        if (!m) {
          m = new Map();
          for (const [dsg, c] of Object.entries(idx.components))
            if (c.sheet === num && c.svg_id) m.set(c.svg_id, dsg);
          sheetCompMaps.set(num, m);
        }
        const local = m.get(u);
        if (local) return local;
      }
      return idx.svg_to_comp[u];
    }
    /** Net a power/flag symbol drives. Power symbols (+5V, +3.3V, GND, PWR_FLAG)
     *  aren't net terminals or components, so clicking their body resolves to
     *  neither — but their single pin sits on a net. Resolve via that pin's uuid. */
    function powerNetOf(g: Element): string | undefined {
      for (const pin of g.querySelectorAll('g[data-primitive="pin"][data-uuid]')) {
        const n = netOf((pin as SVGElement).dataset.uuid!);
        if (n) return n;
      }
      return undefined;
    }

    // -------------------------------------------------------------- camera
    // Width reserved on the right for the floating properties card. In diff mode the
    // card is never shown (focusing a change is read-only — nothing gets selected) and
    // the schematic is a half-width B pane, so reserving the full gutter there shrinks
    // the landing until a change reads as "zoomed out to fit". Drop it while diffing.
    const gutter = () => (useDiffStore.getState().active ? 0 : CARD_GUTTER);
    function fitSheet() {
      const r = stage.getBoundingClientRect();
      const usableW = r.width - gutter();
      const s = Math.min(usableW / vb.current[2], (r.height - 2 * PAD) / vb.current[3]);
      tgt.current.s = s;
      tgt.current.x = PAD;
      tgt.current.y = PAD + ((r.height - 2 * PAD) - vb.current[3] * s) / 2;
    }
    function centerOn(ux: number, uy: number, viewW: number) {
      const r = stage.getBoundingClientRect();
      const usableW = r.width - gutter();
      const s = Math.min(60, Math.max(0.2, usableW / viewW));
      tgt.current.s = s;
      tgt.current.x = usableW / 2 - (ux - vb.current[0]) * s;
      tgt.current.y = r.height / 2 - (uy - vb.current[1]) * s;
    }
    /** Land the camera on the union bbox of the given elements (same-sheet focus,
     *  feedback item 2). `viewW` is the *minimum* context width: a lone/clustered set
     *  lands at that readable zoom, a spread-out set zooms out to frame all of it — but
     *  never past whole-sheet fit (a net scattered across the sheet then lands at ~fit,
     *  centred on its centroid, instead of zoomed out past fit with empty margins).
     *  Returns false when nothing measurable is on this sheet. */
    function centerOnUuids(uuids: string[], viewW: number): boolean {
      const svg = curSvg.current;
      if (!svg) return false;
      let minX = Infinity,
        minY = Infinity,
        maxX = -Infinity,
        maxY = -Infinity;
      for (const u of uuids) {
        const el = svg.querySelector(`g[data-uuid="${esc(u)}"]`) as SVGGraphicsElement | null;
        if (!el) continue;
        try {
          const b = el.getBBox();
          if (b.width === 0 && b.height === 0) continue;
          minX = Math.min(minX, b.x);
          minY = Math.min(minY, b.y);
          maxX = Math.max(maxX, b.x + b.width);
          maxY = Math.max(maxY, b.y + b.height);
        } catch {
          /* detached/hidden */
        }
      }
      if (!isFinite(minX)) return false;
      frameWorldBox(minX, minY, maxX, maxY, viewW);
      return true;
    }
    /** Land the shared camera on an explicit world-space bbox with the same padding/fit
     *  rules as centerOnUuids — shared by the same-sheet uuid focus and camBridge's
     *  A-island landing (a removed object framed from the A side). */
    function frameWorldBox(minX: number, minY: number, maxX: number, maxY: number, viewW: number) {
      const r = stage.getBoundingClientRect();
      const usableW = r.width - gutter();
      const usableH = r.height - 2 * PAD;
      const aspect = usableW / usableH; // px aspect, to weigh the Y span as an X-width
      const PADDING = 1.3; // breathing room so the framed set isn't flush to the edges
      // Context width that frames the (padded) span on BOTH axes — the old code only
      // looked at the X span (× 1.6), so a net spread tall-but-narrow was cut off and a
      // wide one zoomed out past the sheet (feedback batch 2).
      let need = Math.max(maxX - minX, aspect * (maxY - minY)) * PADDING;
      need = Math.max(need, viewW); // floor: readable zoom for a lone/clustered set
      // Cap at the width that fits the whole sheet, so a sheet-wide net lands at ~fit
      // centred on its centroid rather than zoomed out past fit with empty margins.
      need = Math.min(need, Math.max(vb.current[2], aspect * vb.current[3]));
      centerOn((minX + maxX) / 2, (minY + maxY) / 2, need);
      focusWorld.current = { x: (minX + maxX) / 2, y: (minY + maxY) / 2 };
    }

    // -------------------------------------------------------------- sheet load
    // Monotonic token: two rapid navigations (double-clicking sheet symbols, spamming
    // back/forward, a reveal racing a badge jump) both await getSheetSvg; without this the
    // slower earlier fetch resolves last and stomps the DOM + curSheet with the older sheet.
    let sheetGen = 0;
    async function loadSheet(num: number) {
      const gen = ++sheetGen;
      const txt = await getSheetSvg(num);
      if (gen !== sheetGen) return; // a newer loadSheet started while we awaited — drop this one
      island.innerHTML = txt;
      const svg = island.querySelector("svg") as SVGSVGElement | null;
      if (!svg) return;
      const raw = (svg.getAttribute("viewBox") ?? "0 0 297 210").split(/\s+/).map(Number);
      vb.current = [raw[0] || 0, raw[1] || 0, raw[2] || 297, raw[3] || 210];
      svg.setAttribute("width", String(vb.current[2])); // 1px = 1mm world units
      svg.setAttribute("height", String(vb.current[3]));
      svg.style.display = "block";
      curSvg.current = svg;
      curSheet.current = num;
      relabelInstances(svg, compOf); // re-used sheet instances: base ref → this instance's designator
      hiddenSources.current = []; // new DOM; previous hidden refs are gone
      bboxCache.current = null;
      focusWorld.current = null;
      setCurrentSheet(num);
      renderCommentChips(); // re-anchor comment chips to the new sheet's DOM
    }

    // ------------------------------------------------- world-space helpers
    function worldFromClient(cx: number, cy: number) {
      const r = stage.getBoundingClientRect();
      const c = cam.current;
      return {
        x: (cx - r.left - c.x) / c.s + vb.current[0],
        y: (cy - r.top - c.y) / c.s + vb.current[1],
      };
    }
    /** Lazy bbox index over this sheet's net geometry (wires/junctions/labels/pins).
     *  Built once per sheet on first fallback use; getBBox ignores visibility, so
     *  highlighted (hidden-original) nets stay findable. */
    function ensureBBoxCache() {
      if (bboxCache.current) return bboxCache.current;
      const m = new Map<string, { x: number; y: number; w: number; h: number }>();
      const svg = curSvg.current;
      if (svg) {
        for (const g of svg.querySelectorAll("g[data-uuid]")) {
          const u = (g as SVGElement).dataset.uuid!;
          if (!netOf(u)) continue; // net geometry only — symbols would swallow clicks
          try {
            const b = (g as SVGGraphicsElement).getBBox();
            m.set(u, { x: b.x, y: b.y, w: b.width, h: b.height });
          } catch {
            /* detached/empty groups */
          }
        }
      }
      bboxCache.current = m;
      return m;
    }
    /** Snap to the nearest net element within maxWorld mm of the point.
     *  Ties (overlapping bboxes at wire crossings) go to: an already-highlighted net
     *  first — so clicking a highlighted wire toggles *it*, not a neighbor — then to
     *  the smaller (more specific) bbox. */
    function nearestNetAt(wx: number, wy: number, maxWorld: number): Selection {
      const selected = new Set(
        highlights.current.filter((h) => h.kind === "net").map((h) => h.ref),
      );
      type Cand = { name: string; d: number; area: number };
      let best: Cand | null = null;
      let bestSel: Cand | null = null;
      for (const [u, b] of ensureBBoxCache()) {
        const dx = Math.max(b.x - wx, 0, wx - (b.x + b.w));
        const dy = Math.max(b.y - wy, 0, wy - (b.y + b.h));
        const d = Math.hypot(dx, dy);
        if (d > maxWorld) continue;
        const cand: Cand = { name: netOf(u)!, d, area: b.w * b.h };
        const beats = (cur: Cand | null) =>
          !cur || d < cur.d - 1e-9 || (Math.abs(d - cur.d) < 1e-9 && cand.area < cur.area);
        if (beats(best)) best = cand;
        if (selected.has(cand.name) && beats(bestSel)) bestSel = cand;
      }
      // Prefer the highlighted net only when it genuinely ties the nearest hit —
      // a fresh wire that merely passes nearby must still win.
      const pick = bestSel && best && bestSel.d <= best.d + 0.05 ? bestSel : best;
      return pick ? { kind: "net", ref: pick.name } : null;
    }

    // -------------------------------------------------------------- highlight render
    function clearPaint() {
      const svg = curSvg.current;
      svg?.querySelectorAll(".hl-overlay, .hl-scrim").forEach((n) => n.remove());
      for (const el of hiddenSources.current) el.style.visibility = "";
      hiddenSources.current = [];
      badgeLayer.innerHTML = "";
      badgeAnchors.current = [];
    }
    /** Translucent paper-wash scrim — de-emphasis WITHOUT group opacity (which would
     *  rasterize the base and pixellate on zoom). Sits above base, below overlays. */
    function dimBase() {
      const svg = curSvg.current;
      if (!svg) return;
      const scrim = document.createElementNS(SVG_NS, "rect");
      scrim.setAttribute("class", "hl-scrim");
      scrim.setAttribute("x", String(vb.current[0]));
      scrim.setAttribute("y", String(vb.current[1]));
      scrim.setAttribute("width", String(vb.current[2]));
      scrim.setAttribute("height", String(vb.current[3]));
      svg.appendChild(scrim);
    }
    /** Clone members into a colored overlay AND hide the originals, so each highlighted
     *  element renders exactly once — no double-paint blur (user feedback). */
    function paint(uuids: string[], color: string) {
      const svg = curSvg.current;
      if (!svg) return;
      const ov = document.createElementNS(SVG_NS, "g");
      ov.setAttribute("class", "hl-overlay");
      ov.style.setProperty("--ov", color);
      for (const u of uuids) {
        const src = svg.querySelector(`g[data-uuid="${esc(u)}"]`) as SVGElement | null;
        if (!src) continue;
        ov.appendChild(src.cloneNode(true));
        src.style.visibility = "hidden";
        hiddenSources.current.push(src);
      }
      svg.appendChild(ov);
    }
    /** Visual-diff tint: clone the changed uuids into a `.hl-diff` overlay coloured by
     *  the change's role (err/ok/warn CSS vars) and scrim the rest, so the focused change
     *  reads against a dimmed sheet (§4). Kept in its own class so the normal
     *  clearPaint/renderHighlights cycle never wipes it; cleared explicitly via clearDiff. */
    function paintDiff(uuids: string[], role: "err" | "ok" | "warn", emph?: string) {
      clearDiffPaint();
      const svg = curSvg.current;
      if (!svg) return;
      // Dim the unchanged base (own scrim class, so it survives the highlight cycle).
      const scrim = document.createElementNS(SVG_NS, "rect");
      scrim.setAttribute("class", "hl-diff-scrim");
      scrim.setAttribute("x", String(vb.current[0]));
      scrim.setAttribute("y", String(vb.current[1]));
      scrim.setAttribute("width", String(vb.current[2]));
      scrim.setAttribute("height", String(vb.current[3]));
      svg.appendChild(scrim);
      if (uuids.length === 0) return;
      const ov = document.createElementNS(SVG_NS, "g");
      ov.setAttribute("class", `hl-diff hl-diff-${role} hl-diff-pulse`);
      for (const u of uuids) {
        const src = svg.querySelector(`g[data-uuid="${esc(u)}"]`) as SVGElement | null;
        if (!src) continue;
        ov.appendChild(src.cloneNode(true));
      }
      // B side shows the NEW state: colour the changed text (e.g. the value string)
      // green inside the cloned overlay so the exact edit stands out.
      emphasizeDiffText(ov, emph, "hl-diff-emph-ok");
      svg.appendChild(ov);
    }
    function clearDiffPaint() {
      curSvg.current?.querySelectorAll(".hl-diff, .hl-diff-scrim").forEach((n) => n.remove());
    }
    function netMembersInDom(name: string): string[] {
      const svg = curSvg.current;
      if (!svg) return [];
      const out = [...svg.querySelectorAll("g[data-uuid]")]
        .map((g) => (g as SVGElement).dataset.uuid!)
        .filter((u) => netOf(u) === name);
      // A power/flag symbol's BODY isn't a net member — only its (zero-length) pin is —
      // so the flag never painted with its net and lent the landing no measurable
      // geometry. Pull in any power symbol whose pin drives this net so the body colors
      // with the rest of the net and the jump can frame it (user feedback, +12V).
      const seen = new Set(out);
      for (const sym of svg.querySelectorAll('g[data-primitive="power-symbol"][data-uuid]')) {
        const u = (sym as SVGElement).dataset.uuid!;
        if (!seen.has(u) && powerNetOf(sym) === name) out.push(u);
      }
      return out;
    }


    /** Object-anchored comment chips (Phase 2): one numbered chip per comment whose
     *  anchored object is on the current sheet. Lives in its own layer so the
     *  highlight clearPaint cycle never wipes it; de-overlaps with the off-sheet
     *  badges in the shared rAF pass. */
    function renderCommentChips() {
      commentLayer.innerHTML = "";
      commentAnchors.current = [];
      const svg = curSvg.current;
      if (!svg) return;
      // Box-select region outlines live in the SVG (world coords) so they scale + pan
      // with the art; stroke compensated per-frame via --cmt-stroke on this group only.
      // Re-created here each pass; survives the highlight clearPaint cycle (own group).
      svg.querySelector(".cmt-regions")?.remove();
      const regions = document.createElementNS(SVG_NS, "g");
      regions.setAttribute("class", "cmt-regions");
      svg.appendChild(regions);
      cmtRegionsGroup = regions;
      const curName = curSheet.current != null ? sheetName(curSheet.current) : null;
      for (const c of comments.current) {
        let ux: number, uy: number;
        if (c.anchor.type === "region") {
          const rect = c.anchor.rect;
          if (!rect) continue;
          // Region comments are sheet-scoped (stamped with the sheet drawn on).
          if (c.anchor.sheet && curName && c.anchor.sheet !== curName) continue;
          const box = document.createElementNS(SVG_NS, "rect");
          box.setAttribute(
            "class",
            `cmt-region st-${c.status}${c.severity ? ` sev-${c.severity}` : ""}`,
          );
          box.setAttribute("x", String(rect.x));
          box.setAttribute("y", String(rect.y));
          box.setAttribute("width", String(rect.w));
          box.setAttribute("height", String(rect.h));
          regions.appendChild(box);
          ux = rect.x + rect.w; // chip rides the top-right corner
          uy = rect.y;
        } else if (c.anchor.type === "component") {
          const u = idx.components[c.anchor.ref]?.svg_id;
          const el = u
            ? (svg.querySelector(`g[data-uuid="${esc(u)}"]`) as SVGGraphicsElement | null)
            : null;
          if (!el) continue;
          let b;
          try {
            b = el.getBBox();
          } catch {
            continue;
          }
          if (b.width === 0 && b.height === 0) continue;
          ux = b.x + b.width;
          uy = b.y;
        } else {
          const members = netMembersInDom(c.anchor.ref);
          const landing =
            members.find((u) => BADGE_KINDS.includes(idx.elem_kind[u])) ?? members[0];
          const el = landing
            ? (svg.querySelector(`g[data-uuid="${esc(landing)}"]`) as SVGGraphicsElement | null)
            : null;
          if (!el) continue;
          let b;
          try {
            b = el.getBBox();
          } catch {
            continue;
          }
          ux = b.x + b.width;
          uy = b.y;
        }
        const div = document.createElement("div");
        div.className = `cmt-chip st-${c.status}${c.severity ? ` sev-${c.severity}` : ""}`;
        div.textContent =
          c.status === "resolved" ? "✓" : c.status === "recheck" ? "⟳" : String(c.number);
        div.title = `Comment ${c.number} on ${c.anchor.ref} — ${c.status}`;
        div.onpointerdown = (e) => e.stopPropagation();
        div.onclick = (e) => {
          // Item 8: clicking a comment just opens its thread — no select/highlight.
          e.stopPropagation();
          const r = div.getBoundingClientRect();
          useReviewStore.getState().openThread(c.id, { x: r.right + 8, y: r.top });
        };
        commentLayer.appendChild(div);
        commentAnchors.current.push({ ux, uy, dy: 0, el: div });
      }
    }

    /** Repaint the persistent highlights (item 22) + the transient click-selection on
     *  the current sheet, then sync the store. Pinned objects always render in their
     *  saved color; the off-sheet badges + PCB chip belong only to the active
     *  selection's primary, so pinned-only items just get the color wash. */
    function renderHighlights() {
      clearPaint();
      const svg = curSvg.current;
      const transient = highlights.current;
      const pinnedSet = useSelectionStore.getState().pinned;
      const isTransient = (h: { kind: string; ref: string }) =>
        transient.some((t) => t.kind === h.kind && t.ref === h.ref);
      // Pinned objects not currently selected, then the selection on top.
      const combined: Highlight[] = [
        ...pinnedSet.filter((p) => !isTransient(p)),
        ...transient,
      ];
      const plans = combined.map((h) => {
        if (h.kind === "net") return { h, members: netMembersInDom(h.ref) };
        const u = idx.components[h.ref]?.svg_id;
        const present = u && svg?.querySelector(`g[data-uuid="${esc(u)}"]`) ? [u] : [];
        return { h, members: present };
      });
      if (plans.some((p) => p.members.length)) {
        // Dim the rest of the sheet only for an active click-selection (the compare
        // view). A right-click net color is a persistent (pinned-only) highlight — it
        // just recolors that net, so leave the other nets at full strength.
        if (transient.length > 0) dimBase();
        for (const p of plans) {
          paint(p.members, p.h.color);
        }
      }
      const primary = transient[transient.length - 1];
      setHighlightStore([...transient], "sch");
      setSelectionStore(primary ? { kind: primary.kind, ref: primary.ref } : null, "sch");
    }

    // -------------------------------------------------------------- selection ops
    // Selection reuses a pinned object's color when one exists (item 23), else the
    // default net/component color.
    const selColor = (h: { kind: "net" | "comp"; ref: string }) =>
      useSelectionStore.getState().pinnedColor(h.kind, h.ref) ??
      (h.kind === "net" ? NET_COLOR : COMP_COLOR);
    function setSingle(h: { kind: "net" | "comp"; ref: string }) {
      highlights.current = [{ ...h, color: selColor(h) }];
      renderHighlights();
    }
    function toggleAdd(h: { kind: "net" | "comp"; ref: string }) {
      const i = highlights.current.findIndex((x) => x.kind === h.kind && x.ref === h.ref);
      if (i >= 0) highlights.current.splice(i, 1);
      else highlights.current.push({ ...h, color: useSelectionStore.getState().pinnedColor(h.kind, h.ref) ?? colorForNext(highlights.current) });
      renderHighlights();
    }
    function clearAll() {
      highlights.current = [];
      renderHighlights();
    }

    // -------------------------------------------------------------- jumps + history
    // Cap the nav stacks: each entry snapshots the highlight set, so an unbounded stack
    // grows without limit over a long review session. Oldest entries drop first.
    const HIST_MAX = 200;
    const pushEntry = (stack: HistEntry[], entry: HistEntry) => {
      stack.push(entry);
      if (stack.length > HIST_MAX) stack.shift();
    };
    function pushHistory() {
      if (curSheet.current == null) return;
      const st = useSelectionStore.getState();
      pushEntry(hist.current, {
        sheet: curSheet.current,
        cam: { ...tgt.current },
        highlights: [...highlights.current],
        selection: st.selection,
      });
      fwd.current = [];
    }
    async function jumpToNet(name: string, dest: number, replace = false) {
      pushHistory();
      await loadSheet(dest);
      const there = netMembersInDom(name);
      // Frame the net's geometry on the destination sheet via the union bbox, exactly
      // like same-sheet goNet — centerOnUuids is exception-safe (a single landing
      // element's unguarded getBBox could throw and abort the whole jump, leaving the
      // camera put — the "doesn't transition" symptom on large sheets). Prefer the
      // off-sheet connection points (ports/labels/power symbols) so a big multi-sheet
      // net lands where it ENTERS this sheet instead of zooming out over its full spread.
      // But fall through to the full geometry when those links measure nothing: a power
      // net's only links are its flag symbols, whose pin polyline is zero-length, so
      // framing them returned false and the jump fell back to a whole-sheet fit far too
      // zoomed out (feedback 25.PNG → 26.PNG). `there` keeps the wires + flag bodies.
      const links = there.filter((u) => BADGE_KINDS.includes(idx.elem_kind[u]));
      if (!centerOnUuids(links, LANDING_WIDTH_MM) && !centerOnUuids(there, LANDING_WIDTH_MM))
        fitSheet();
      if (replace) {
        highlights.current = [{ kind: "net", ref: name, color: NET_COLOR }];
      } else {
        // Badge / "go" jumps keep the comparison set; just make this net primary.
        const i = highlights.current.findIndex((x) => x.kind === "net" && x.ref === name);
        if (i >= 0) highlights.current.push(...highlights.current.splice(i, 1));
        else highlights.current.push({ kind: "net", ref: name, color: colorForNext(highlights.current) });
      }
      renderHighlights();
    }
    async function jumpToComp(dsg: string, dest: number) {
      pushHistory();
      await loadSheet(dest);
      const c = idx.components[dsg];
      const el = c?.svg_id
        ? (curSvg.current!.querySelector(`g[data-uuid="${esc(c.svg_id)}"]`) as SVGGraphicsElement | null)
        : null;
      if (el) {
        const b = el.getBBox();
        centerOn(b.x + b.width / 2, b.y + b.height / 2, 110);
      } else fitSheet();
      setSingle({ kind: "comp", ref: dsg });
    }
    async function restore(h: HistEntry) {
      await loadSheet(h.sheet);
      tgt.current = { ...h.cam };
      highlights.current = [...h.highlights];
      renderHighlights();
    }
    async function back() {
      const h = hist.current.pop();
      if (!h || curSheet.current == null) return;
      const st = useSelectionStore.getState();
      pushEntry(fwd.current, { sheet: curSheet.current, cam: { ...tgt.current }, highlights: [...highlights.current], selection: st.selection });
      await restore(h);
    }
    async function forward() {
      const h = fwd.current.pop();
      if (!h || curSheet.current == null) return;
      const st = useSelectionStore.getState();
      pushEntry(hist.current, { sheet: curSheet.current, cam: { ...tgt.current }, highlights: [...highlights.current], selection: st.selection });
      await restore(h);
    }
    /** Sidebar-driven sheet open: keep the active highlight set, re-resolved here.
     *  Sidebar numbering (manifest) ≠ design-JSON numbering, so match by filename. */
    async function openSheet(ref: SheetRef) {
      const sheet = idx.sheets.find((s) => s.svg && sheetMatches(s, ref));
      if (!sheet || sheet.num === curSheet.current) return;
      pushHistory();
      await loadSheet(sheet.num);
      fitSheet();
      renderHighlights(); // selection persists across sheet switches (feedback #5)
    }

    // -------------------------------------------------------------- hit-testing
    function resolveAt(cx: number, cy: number, wantPin: boolean): Selection {
      const spiral = [[0, 0], [3, 0], [-3, 0], [0, 3], [0, -3], [5, 5], [-5, -5], [5, -5], [-5, 5]];
      for (const [dx, dy] of spiral) {
        for (const el of document.elementsFromPoint(cx + dx, cy + dy)) {
          const pin = (el as Element).closest?.("[data-designator][data-pin]") as HTMLElement | null;
          if (pin && wantPin)
            return { kind: "pin", ref: { designator: pin.dataset.designator!, pin: pin.dataset.pin! } };
          let g = (el as Element).closest?.("g[data-uuid]") as SVGElement | null;
          while (g) {
            const u = g.dataset.uuid!;
            const net = netOf(u);
            if (net) return { kind: "net", ref: net };
            const comp = compOf(u);
            if (comp) return { kind: "comp", ref: comp };
            // Power/flag symbol body → the net its pin drives.
            if (g.dataset.primitive === "power-symbol") {
              const pnet = powerNetOf(g);
              if (pnet) return { kind: "net", ref: pnet };
            }
            g = (g.parentElement?.closest("g[data-uuid]") as SVGElement) ?? null;
          }
          if (pin) {
            const dsg = pin.dataset.designator!;
            const pn = pin.dataset.pin!;
            for (const [name, n] of Object.entries(idx.nets))
              if (n.terminals.some((t) => t.d === dsg && t.p === pn)) return { kind: "net", ref: name };
            return { kind: "comp", ref: dsg };
          }
        }
      }
      return null;
    }
    /** Double-click on a hierarchical sheet symbol → descend into that sheet
     *  (KiCad-style navigation from the root sheet). Point-in-rect in world coords:
     *  the symbol body is fill="none", so DOM hit-testing never sees its interior. */
    function enterSheetAt(cx: number, cy: number): boolean {
      const svg = curSvg.current;
      if (!svg) return false;
      const w = worldFromClient(cx, cy);
      const norm = (s: string) => s.toLowerCase().replace(/[\s_]+/g, "");
      for (const symEl of svg.querySelectorAll('[data-primitive="sheet-symbol"]')) {
        const sym = symEl as SVGGraphicsElement;
        const d = sym.dataset;
        let x: number, y: number, wd: number, ht: number;
        if (d.atXNm && d.atYNm && d.sizeXNm && d.sizeYNm) {
          x = +d.atXNm / 1e6; // nm → mm, same space as the viewBox
          y = +d.atYNm / 1e6;
          wd = +d.sizeXNm / 1e6;
          ht = +d.sizeYNm / 1e6;
        } else {
          const b = sym.getBBox();
          ({ x, y } = b);
          wd = b.width;
          ht = b.height;
        }
        if (w.x < x || w.x > x + wd || w.y < y || w.y > y + ht) continue;
        const name = d.sheetName;
        if (!name) continue;
        const sheet = idx.sheets.find((s) => s.svg && norm(s.name) === norm(name));
        if (!sheet || sheet.num === curSheet.current) continue;
        pushHistory();
        void (async () => {
          await loadSheet(sheet.num);
          fitSheet();
          renderHighlights();
        })();
        return true;
      }
      return false;
    }

    function handleTap(cx: number, cy: number, mods: { shift: boolean; pin: boolean }) {
      // A schematic text note may carry a KiCad hyperlink (emitted as data-href on its
      // group). Clicking the text opens the link instead of selecting (batch3 item 5).
      const linkEl = (document.elementFromPoint(cx, cy) as Element | null)?.closest?.("[data-href]");
      const href = linkEl?.getAttribute("data-href");
      if (href) {
        void ipc.openExternal(href).catch(() => {});
        return;
      }
      const w = worldFromClient(cx, cy);
      focusWorld.current = w;
      let hit = resolveAt(cx, cy, mods.pin);
      // DOM probe missed (hairline wires) — snap to the nearest net geometry,
      // with a tolerance that shrinks as you zoom in (~12 screen px, max 3 mm).
      if (!hit) hit = nearestNetAt(w.x, w.y, Math.min(3, 12 / cam.current.s));
      // Comment mode (C armed): the click picks the object to anchor a new comment
      // to, instead of selecting (docs/phase2-ui-plan.md §3).
      const review = useReviewStore.getState();
      if (review.armed && hit) {
        const sheet =
          curSheet.current != null ? sheetName(curSheet.current) : undefined;
        const anchor =
          hit.kind === "net"
            ? { type: "net" as const, ref: hit.ref }
            : { type: "component" as const, ref: hit.kind === "pin" ? hit.ref.designator : hit.ref, sheet };
        review.beginCompose({ anchor, pos: { x: cx + 12, y: cy } });
        return;
      }
      if (!hit) {
        if (!mods.shift) {
          clearAll();
          useSelectionStore.getState().setAnchor(null);
        }
        return;
      }
      // Open the info panel next to where the user clicked (viewport coords).
      useSelectionStore.getState().setAnchor({ x: cx, y: cy });
      if (hit.kind === "pin") {
        const c = idx.components[hit.ref.designator];
        if (c?.svg_id) {
          highlights.current = [{ kind: "comp", ref: hit.ref.designator, color: COMP_COLOR }];
          renderHighlights();
        }
        setSelectionStore(hit, "sch"); // pin specifics in the card
        return;
      }
      if (hit.kind === "comp") {
        const c = idx.components[hit.ref];
        const here = c?.svg_id && curSvg.current?.querySelector(`g[data-uuid="${esc(c.svg_id)}"]`);
        if (!here && c?.sheet != null && c.sheet !== curSheet.current) {
          jumpToComp(hit.ref, c.sheet);
          return;
        }
      }
      const target = { kind: hit.kind, ref: hit.ref } as { kind: "net" | "comp"; ref: string };
      mods.shift ? toggleAdd(target) : setSingle(target);
    }

    // --------------------------------------------- box-select rubber-band
    // Drawn as an SVG rect in world coords (like the persistent region outlines) so it
    // tracks the art; only present while a comment-mode drag is in progress.
    let rubberEl: SVGRectElement | null = null;
    // Tracks the <g class="cmt-regions"> group so the tick loop can update --cmt-stroke
    // on just that subtree rather than the entire world (avoids per-frame full-SVG cascade).
    let cmtRegionsGroup: SVGGElement | null = null;
    function updateRubber(x0: number, y0: number, x1: number, y1: number) {
      const svg = curSvg.current;
      if (!svg) return;
      if (!rubberEl) {
        rubberEl = document.createElementNS(SVG_NS, "rect");
        rubberEl.setAttribute("class", "cmt-rubber");
        svg.appendChild(rubberEl);
      }
      const a = worldFromClient(x0, y0);
      const b = worldFromClient(x1, y1);
      rubberEl.setAttribute("x", String(Math.min(a.x, b.x)));
      rubberEl.setAttribute("y", String(Math.min(a.y, b.y)));
      rubberEl.setAttribute("width", String(Math.abs(a.x - b.x)));
      rubberEl.setAttribute("height", String(Math.abs(a.y - b.y)));
    }
    function hideRubber() {
      rubberEl?.remove();
      rubberEl = null;
    }

    // -------------------------------------------------------------- input
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const r = stage.getBoundingClientRect();
      const mx = e.clientX - r.left;
      const my = e.clientY - r.top;
      const f = Math.exp(-e.deltaY * 0.0016);
      const ns = Math.min(60, Math.max(0.2, tgt.current.s * f));
      // Zoom-out past the sheet edge → overview contact-sheet (spec WS6). The camera
      // stays put so closing the overview returns exactly where you were.
      if (e.deltaY > 0 && !overviewRef.current) {
        const fitS = Math.min(
          (r.width - gutter()) / vb.current[2],
          (r.height - 2 * PAD) / vb.current[3],
        );
        if (ns < fitS * 0.5) {
          setOverviewOpen(true);
          return;
        }
      }
      // Pivot the cursor-anchor on the LIVE camera (what's on screen), not tgt: while a
      // zoom ease is in flight cam.s ≠ tgt.s, so anchoring on tgt drifts the point under
      // the cursor on rapid scroll. ns still grows from tgt.s, so zoom speed is unchanged.
      const c = cam.current;
      const ratio = ns / c.s;
      tgt.current.x = mx - (mx - c.x) * ratio;
      tgt.current.y = my - (my - c.y) * ratio;
      tgt.current.s = ns;
    };
    let drag: { px: number; py: number; ox: number; oy: number; text: boolean; box?: boolean; btn: number } | null = null;
    const onPointerDown = (e: PointerEvent) => {
      if (e.button === 3) { back(); return; }
      if (e.button === 4) { forward(); return; }
      if (e.button === 2) return; // right-click → context menu (its own handler)
      // Scoped text-select (no tool mode, item 4): a left-drag that *starts on a text
      // glyph* runs native selection — don't capture the pointer or pan. Everywhere
      // else (and the middle button) pans.
      const onText = e.button === 0 && !!(e.target as Element).closest?.("text, tspan");
      // Box-select for a region comment: when comment mode is armed (C), a left-drag
      // rubber-bands a rectangle and opens the composer anchored to that region instead
      // of panning. A click with no drag still picks the object under it (on pointer-up).
      if (useReviewStore.getState().armed && e.button === 0 && !onText) {
        drag = { px: e.clientX, py: e.clientY, ox: 0, oy: 0, text: false, box: true, btn: e.button };
        stage.setPointerCapture(e.pointerId);
        updateRubber(e.clientX, e.clientY, e.clientX, e.clientY);
        return;
      }
      drag = { px: e.clientX, py: e.clientY, ox: tgt.current.x, oy: tgt.current.y, text: onText, btn: e.button };
      if (onText) return; // leave the drag to the browser's text selection
      // Item 1: a press anywhere that isn't text deselects any active text
      // selection — without a pan/text tool there was no other way to drop it.
      const selnow = window.getSelection();
      if (selnow && !selnow.isCollapsed) selnow.removeAllRanges();
      stage.classList.add("panning");
      stage.setPointerCapture(e.pointerId);
    };
    const onPointerMove = (e: PointerEvent) => {
      if (!drag) return;
      if (drag.box) { updateRubber(drag.px, drag.py, e.clientX, e.clientY); return; }
      if (drag.text) return;
      tgt.current.x = drag.ox + e.clientX - drag.px;
      tgt.current.y = drag.oy + e.clientY - drag.py;
      cam.current.x = tgt.current.x;
      cam.current.y = tgt.current.y;
    };
    let lastTap = { t: 0, x: 0, y: 0 };
    const onPointerUp = (e: PointerEvent) => {
      if (!drag) return;
      const moved = Math.hypot(e.clientX - drag.px, e.clientY - drag.py);
      // Comment-mode box-select: open the composer anchored to the dragged region. A
      // tiny drag is treated as an armed click → the object under it (existing path).
      if (drag.box) {
        const { px, py } = drag;
        drag = null;
        if (moved < 6) {
          hideRubber();
          handleTap(e.clientX, e.clientY, { shift: e.shiftKey, pin: e.ctrlKey || e.metaKey });
          return;
        }
        // Keep the rubber-band visible as the pending region while the composer is open
        // (batch2); the compose subscription below clears it once composing ends.
        const a = worldFromClient(px, py);
        const b = worldFromClient(e.clientX, e.clientY);
        const rect = {
          x: Math.min(a.x, b.x),
          y: Math.min(a.y, b.y),
          w: Math.abs(a.x - b.x),
          h: Math.abs(a.y - b.y),
        };
        const sheet = curSheet.current != null ? sheetName(curSheet.current) : undefined;
        useReviewStore.getState().beginCompose({
          anchor: { type: "region", ref: "region", sheet, rect },
          pos: { x: e.clientX + 12, y: e.clientY },
        });
        return;
      }
      const btn = drag.btn;
      drag = null;
      stage.classList.remove("panning");
      if (moved >= 5) return; // a real drag (pan, or a native text selection) — not a tap
      if (btn !== 0) return; // middle-button release must not synthesize a select/tap
      // Pointer capture eats child click/dblclick — both are synthesized here.
      const now = performance.now();
      const dbl =
        now - lastTap.t < 400 && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 6;
      lastTap = { t: dbl ? 0 : now, x: e.clientX, y: e.clientY };
      if (dbl && enterSheetAt(e.clientX, e.clientY)) return;
      handleTap(e.clientX, e.clientY, { shift: e.shiftKey, pin: e.ctrlKey || e.metaKey });
    };
    const onKey = (e: KeyboardEvent) => {
      // The canvas stays mounted across views and this is a window listener, so scope it:
      // ignore keys while typing in a field (Esc would wipe the highlight set from the
      // comment composer / BOM filter) and when the schematic isn't the active view (Alt+←
      // would silently rewind schematic history from the PCB/BOM tab).
      if (isTypingTarget(e)) return;
      if (useViewStore.getState().view !== "schematic") return;
      if (e.key === "Escape") {
        const selnow = window.getSelection();
        if (selnow && !selnow.isCollapsed) selnow.removeAllRanges(); // item 1: drop text selection
        if (overviewRef.current) setOverviewOpen(false);
        else clearAll();
      } else if (e.altKey && e.key === "ArrowLeft") { e.preventDefault(); back(); }
      else if (e.altKey && e.key === "ArrowRight") { e.preventDefault(); forward(); }
    };

    // Right-click menu (items 'figure out right-click' + highlight-in-color + fit).
    // Built here so it can reuse resolveAt / setSingle / renderHighlights / fitSheet.
    const onContextMenu = (e: MouseEvent) => {
      e.preventDefault();
      const items: MenuItem[] = [];
      const w = worldFromClient(e.clientX, e.clientY);
      let hit = resolveAt(e.clientX, e.clientY, false);
      if (!hit) hit = nearestNetAt(w.x, w.y, Math.min(3, 12 / cam.current.s));
      if (hit && hit.kind !== "pin") {
        const { kind, ref } = hit;
        // Item 22: right-click no longer selects — highlighting is a separate,
        // persistent action. "Highlight in <color>" pins the object (survives reopen).
        const store = useSelectionStore.getState();
        const isPinned = store.pinned.some((p) => p.kind === kind && p.ref === ref);
        items.push({
          label: `Highlight ${ref}`,
          colorPicker: {
            onPick: (color) => {
              void store.pinHighlight({ kind, ref, color });
              renderHighlights();
            },
          },
        });
        if (isPinned)
          items.push({
            label: "Remove highlight",
            icon: <IconTrash size={14} />,
            onClick: () => {
              void store.unpinHighlight(kind, ref);
              renderHighlights();
            },
          });
        items.push({
          label: kind === "net" ? "Copy net name" : "Copy designator",
          icon: <IconCopy size={14} />,
          onClick: () => void navigator.clipboard?.writeText(ref),
        });
        if (kind === "comp") {
          const val = idx.components[ref]?.value;
          if (val)
            items.push({ label: "Copy value", icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(val) });
        }
        if (idx.layers.length)
          items.push({
            label: "Show on PCB",
            icon: <IconBoard size={14} />,
            onClick: () => {
              setSingle({ kind, ref });
              useViewStore.getState().setView("pcb");
            },
          });
        // Item 18: start a comment thread anchored to this object.
        items.push({
          label: "Add comment",
          icon: <IconComment size={14} />,
          onClick: () => {
            const sheet = curSheet.current != null ? sheetName(curSheet.current) : undefined;
            const anchor =
              kind === "net"
                ? { type: "net" as const, ref }
                : { type: "component" as const, ref, sheet };
            useReviewStore
              .getState()
              .beginCompose({ anchor, pos: { x: e.clientX + 12, y: e.clientY } });
          },
        });
        items.push({ separator: true });
      } else {
        const txt = (e.target as Element).closest?.("text, tspan")?.textContent?.trim();
        if (txt) {
          items.push({ label: "Copy text", icon: <IconCopy size={14} />, onClick: () => void navigator.clipboard?.writeText(txt) });
          items.push({ separator: true });
        }
      }
      items.push({ label: "Fit to screen", icon: <IconFit size={14} />, onClick: fitSheet });
      {
        const store = useSelectionStore.getState();
        const hasAny = highlights.current.length > 0 || store.pinned.length > 0;
        items.push({
          label: "Clear highlights",
          icon: <IconTrash size={14} />,
          disabled: !hasAny,
          onClick: () => {
            void store.clearPinned(); // persistent highlights (item 22)
            clearAll(); // transient selection
          },
        });
      }
      // In comparison mode, expose the same exit the banner's × does — a right-click on
      // the canvas is where users reach for it (batch1).
      if (useDiffStore.getState().active) {
        items.push({ separator: true });
        items.push({ label: "Exit comparison", icon: <IconClose size={14} />, onClick: () => useDiffStore.getState().exitDiff() });
      }
      setCtxMenu({ x: e.clientX, y: e.clientY, items });
    };

    // Keep the box-select rubber-band visible while the comment composer is open, then
    // clear it the instant composing ends — submit or cancel (batch2). Self-contained so
    // the imperative rubber lifecycle stays inside this effect.
    const unsubCompose = useReviewStore.subscribe((s, prev) => {
      if (prev.compose && !s.compose) hideRubber();
    });

    stage.addEventListener("wheel", onWheel, { passive: false });
    stage.addEventListener("pointerdown", onPointerDown);
    stage.addEventListener("pointermove", onPointerMove);
    stage.addEventListener("pointerup", onPointerUp);
    stage.addEventListener("contextmenu", onContextMenu);
    window.addEventListener("keydown", onKey);

    // -------------------------------------------------------------- nav bridge
    // Palette / cross-probe / BOM drive the canvas right after switching the view to
    // "schematic" — while it is still display:none, where getBBox returns zeros and
    // badges would anchor at the origin. Defer until the stage has extent again.
    // Tracked so the self-rescheduling poll can be cancelled on cleanup — otherwise an
    // orphaned loop keeps running against the torn-down closure (old idx / old DOM) after
    // a design reload or unmount while the schematic tab is hidden.
    const visRafs = new Set<number>();
    const whenVisible = (fn: () => void) => {
      if (stage.clientWidth > 0) {
        fn();
        return;
      }
      const id = requestAnimationFrame(() => {
        visRafs.delete(id);
        whenVisible(fn);
      });
      visRafs.add(id);
    };
    nav.goNet = (name) => whenVisible(() => {
      const n = idx.nets[name];
      if (!n) return;
      const members = netMembersInDom(name);
      if (members.length) {
        setSingle({ kind: "net", ref: name });
        centerOnUuids(members, LANDING_WIDTH_MM); // same-sheet auto-focus (item 2)
      } else {
        const best = Object.entries(n.by_sheet).sort((a, b) => b[1].length - a[1].length)[0];
        jumpToNet(name, best ? Number(best[0]) : n.sheets[0], true);
      }
    });
    nav.goComp = (dsg) => whenVisible(() => {
      const c = idx.components[dsg];
      if (!c) return;
      const here = c.svg_id && curSvg.current?.querySelector(`g[data-uuid="${esc(c.svg_id)}"]`);
      if (here || c.sheet == null || c.sheet === curSheet.current) {
        setSingle({ kind: "comp", ref: dsg });
        if (c.svg_id) centerOnUuids([c.svg_id], 110); // same-sheet auto-focus (item 2)
      } else jumpToComp(dsg, c.sheet);
    });
    // PCB pad → schematic pin (item 5): land on the pin geometry itself.
    nav.goPin = (dsg, pin) => whenVisible(() => {
      const c = idx.components[dsg];
      if (!c) return;
      const land = () => {
        setSingle({ kind: "comp", ref: dsg });
        setSelectionStore({ kind: "pin", ref: { designator: dsg, pin } }, "sch");
        const el = curSvg.current?.querySelector(
          `[data-designator="${esc(dsg)}"][data-pin="${esc(pin)}"]`,
        ) as SVGGraphicsElement | null;
        if (el) {
          const b = el.getBBox();
          centerOn(b.x + b.width / 2, b.y + b.height / 2, 70);
          focusWorld.current = { x: b.x + b.width / 2, y: b.y + b.height / 2 };
        } else if (c.svg_id) centerOnUuids([c.svg_id], 110);
      };
      const here = c.svg_id && curSvg.current?.querySelector(`g[data-uuid="${esc(c.svg_id)}"]`);
      if (here || c.sheet == null || c.sheet === curSheet.current) land();
      else {
        pushHistory();
        void (async () => {
          await loadSheet(c.sheet!);
          land();
        })();
      }
    });
    // Cross-view continuity (item 6): the PCB view hands its highlight set over when
    // the user switches back to the schematic tab; land on the primary's first home.
    nav.applySelection = (list) => whenVisible(() => {
      highlights.current = list.filter((h) =>
        h.kind === "net" ? !!idx.nets[h.ref] : !!idx.components[h.ref],
      );
      const primary = highlights.current[highlights.current.length - 1];
      if (!primary) {
        renderHighlights();
        return;
      }
      const landHere = () => {
        renderHighlights();
        const ok =
          primary.kind === "net"
            ? centerOnUuids(netMembersInDom(primary.ref), LANDING_WIDTH_MM)
            : !!idx.components[primary.ref]?.svg_id &&
              centerOnUuids([idx.components[primary.ref].svg_id], 110);
        if (!ok) fitSheet();
      };
      const presentHere =
        primary.kind === "net"
          ? netMembersInDom(primary.ref).length > 0
          : !!(idx.components[primary.ref]?.svg_id &&
              curSvg.current?.querySelector(`g[data-uuid="${esc(idx.components[primary.ref].svg_id)}"]`));
      if (presentHere) {
        landHere();
        return;
      }
      const dest =
        primary.kind === "net"
          ? (() => {
              const n = idx.nets[primary.ref];
              const best = Object.entries(n.by_sheet).sort((a, b) => b[1].length - a[1].length)[0];
              return best ? Number(best[0]) : n.sheets[0];
            })()
          : idx.components[primary.ref].sheet;
      if (dest == null) {
        renderHighlights();
        return;
      }
      pushHistory();
      void (async () => {
        await loadSheet(dest);
        landHere();
      })();
    });
    nav.jumpToNet = (name, dest) => whenVisible(() => void jumpToNet(name, dest));
    nav.openSheet = openSheet;
    nav.goSheetNum = (num) => {
      if (num === curSheet.current) return;
      pushHistory();
      void (async () => {
        await loadSheet(num);
        fitSheet();
        renderHighlights();
      })();
    };
    nav.toggleHighlight = (kind, ref) => toggleAdd({ kind, ref });
    renderRef.current = renderHighlights; // let the pinned-change effect repaint
    nav.fitView = fitSheet;
    nav.zoomBy = (factor) => {
      const r = stage.getBoundingClientRect();
      const mx = (r.width - gutter()) / 2;
      const my = r.height / 2;
      const ns = Math.min(60, Math.max(0.2, tgt.current.s * factor));
      const ratio = ns / tgt.current.s;
      tgt.current.x = mx - (mx - tgt.current.x) * ratio;
      tgt.current.y = my - (my - tgt.current.y) * ratio;
      tgt.current.s = ns;
    };
    nav.toggleOverview = () => setOverviewOpen((o) => !o);
    nav.setComments = (cs) => {
      comments.current = cs;
      renderCommentChips();
    };
    // Item 8: go to a comment's object without selecting/highlighting it — load its
    // sheet if needed, then just center the camera.
    nav.reveal = (anchor) =>
      whenVisible(() => {
        if (anchor.type === "region") {
          const rect = anchor.rect;
          if (!rect) return;
          const land = () =>
            centerOn(rect.x + rect.w / 2, rect.y + rect.h / 2, Math.max(rect.w * 1.5, 40));
          const target = anchor.sheet
            ? idx.sheets.find((s) => s.svg && sheetName(s.num) === anchor.sheet)
            : null;
          if (target && target.num !== curSheet.current) {
            pushHistory();
            void (async () => {
              await loadSheet(target.num);
              land();
            })();
          } else {
            land();
          }
          return;
        }
        if (anchor.type === "component") {
          const c = idx.components[anchor.ref];
          if (!c) return;
          const here =
            c.svg_id && curSvg.current?.querySelector(`g[data-uuid="${esc(c.svg_id)}"]`);
          if (here || c.sheet == null || c.sheet === curSheet.current) {
            if (c.svg_id) centerOnUuids([c.svg_id], 110);
          } else {
            pushHistory();
            void (async () => {
              await loadSheet(c.sheet!);
              if (c.svg_id) centerOnUuids([c.svg_id], 110);
              else fitSheet();
            })();
          }
          return;
        }
        const n = idx.nets[anchor.ref];
        if (!n) return;
        const members = netMembersInDom(anchor.ref);
        if (members.length) {
          centerOnUuids(members, LANDING_WIDTH_MM);
          return;
        }
        const best = Object.entries(n.by_sheet).sort((a, b) => b[1].length - a[1].length)[0];
        const dest = best ? Number(best[0]) : n.sheets[0];
        if (dest == null) return;
        pushHistory();
        void (async () => {
          await loadSheet(dest);
          const m = netMembersInDom(anchor.ref);
          if (m.length) centerOnUuids(m, LANDING_WIDTH_MM);
          else fitSheet();
        })();
      });
    nav.getViewState = () =>
      curSheet.current == null
        ? null
        : {
            sheetName: sheetName(curSheet.current),
            cam: { ...tgt.current },
            highlights: [...highlights.current],
          };

    // Visual diff (§4): load the change's sheet if needed, centre on its uuids, and
    // paint the diff tint. Read-only — no pushHistory / selection writes, so exiting
    // diff mode leaves the normal viewing state exactly as it was.
    nav.revealDiff = (sheet, uuids, role, emph, aOnly) =>
      whenVisible(() => {
        const land = () => {
          if (aOnly) {
            // The object exists only on A (removed): this (B) side has no geometry to
            // frame, so leave the camera to the A island (camBridge.centerWorld) and just
            // dim the sheet. Don't fit — that would fight the A-side landing.
          } else if (uuids.length && centerOnUuids(uuids, 110)) {
            /* framed the changed set */
          } else {
            fitSheet(); // added/removed sheet, or uuids not on this sheet → show context
          }
          paintDiff(uuids, role, emph);
        };
        if (sheet !== curSheet.current && idx.sheets.some((s) => s.num === sheet)) {
          void (async () => {
            await loadSheet(sheet);
            land();
          })();
        } else {
          land();
        }
      });
    nav.clearDiff = () => clearDiffPaint();

    // Shared-camera driver for the A-island: it forwards its own pan (screen px) and
    // wheel-zoom (factor about the cursor, in *this* canvas's screen space) here so
    // panning/zooming either side moves both (§4).
    camBridge.drive = (dx, dy, zoomFactor, anchorX, anchorY) => {
      if (dx || dy) {
        tgt.current.x += dx;
        tgt.current.y += dy;
      }
      if (zoomFactor !== 1) {
        const ns = Math.min(60, Math.max(0.2, tgt.current.s * zoomFactor));
        const ratio = ns / tgt.current.s;
        tgt.current.x = anchorX - (anchorX - tgt.current.x) * ratio;
        tgt.current.y = anchorY - (anchorY - tgt.current.y) * ratio;
        tgt.current.s = ns;
      }
    };
    // Absolute landing for the A island: frame a removed object's A-side bbox on the
    // shared camera (this B side has no such geometry to centre on). Same world units.
    camBridge.centerWorld = (box) =>
      whenVisible(() => frameWorldBox(box.x, box.y, box.x + box.width, box.y + box.height, 110));

    // Debug-only render probe (Layer-2 E2E): tauri-pilot can't read the painted SVG
    // island, so expose the live render state for it to assert on via `eval`. No-op
    // outside dev builds. See lib/renderProbe.ts + docs/testing.md.
    const unregisterProbe = registerRenderProbe("schematic", () => {
      const svg = curSvg.current;
      return {
        sheet: curSheet.current,
        sheetName: curSheet.current == null ? null : sheetName(curSheet.current),
        viewBox: [...vb.current],
        cam: { ...cam.current },
        tgt: { ...tgt.current },
        elements: svg?.querySelectorAll("g[data-uuid]").length ?? 0,
        highlights: highlights.current.map((h) => ({ kind: h.kind, ref: h.ref, color: h.color })),
        overlays: svg?.querySelectorAll(".hl-overlay").length ?? 0,
        hiddenSources: hiddenSources.current.length,
        dimmed: !!svg?.querySelector(".hl-scrim"),
        comments: commentAnchors.current.length,
        badges: badgeAnchors.current.length,
        overviewOpen: overviewRef.current,
      };
    });

    // -------------------------------------------------------------- rAF loop
    let raf = 0;
    const tick = () => {
      const k = 0.32;
      const c = cam.current;
      const t = tgt.current;
      c.x += (t.x - c.x) * k;
      c.y += (t.y - c.y) * k;
      c.s += (t.s - c.s) * k;
      world.style.transform = `translate(${c.x}px,${c.y}px) scale(${c.s})`;
      // Visual diff: publish the live camera + viewBox + sheet so the read-only A-island
      // follows this (B) side (shared camera, §4). Cheap; only meaningful in diff mode.
      if (useDiffStore.getState().active) {
        camBridge.cam.x = c.x;
        camBridge.cam.y = c.y;
        camBridge.cam.s = c.s;
        camBridge.vb = [...vb.current];
        if (camBridge.sheet !== curSheet.current) {
          camBridge.sheet = curSheet.current;
          camBridge.epoch++;
        }
      }
      // Region/rubber outlines: compensate stroke for zoom (see .cmt-region css).
      // Set on the tiny group/element only — NOT on world — to avoid cascading a style
      // recalculation over the entire schematic SVG every frame (causes text jank).
      const sw = String(1.2 / c.s);
      cmtRegionsGroup?.style.setProperty("--cmt-stroke", sw);
      if (rubberEl) rubberEl.style.setProperty("--cmt-stroke", sw);
      // Screen-space de-overlap (7.PNG): badges from different anchors (off-sheet
      // stacks + the copper-layer chips) collide when their anchors sit close
      // together — push later ones below whatever is already placed.
      const placed: { x: number; y: number; w: number; h: number }[] = [];
      // Comment chips placed first (persistent across selections), then the
      // off-sheet / copper-layer badges flow below whatever is already there.
      for (const a of [...commentAnchors.current, ...badgeAnchors.current]) {
        const sx = c.x + (a.ux - vb.current[0]) * c.s + 4;
        const sy = c.y + (a.uy - vb.current[1]) * c.s + a.dy;
        a.w ??= a.el.offsetWidth;
        a.h ??= a.el.offsetHeight;
        const r = { x: sx, y: sy - a.h / 2, w: a.w, h: a.h };
        for (let guard = 0; guard < placed.length; guard++) {
          const hit = placed.find(
            (p) => r.x < p.x + p.w + 4 && p.x < r.x + r.w + 4 && r.y < p.y + p.h + 4 && p.y < r.y + r.h + 4,
          );
          if (!hit) break;
          r.y = hit.y + hit.h + 4;
        }
        placed.push(r);
        a.el.style.transform = `translate(${r.x}px,${r.y}px)`;
      }
      raf = requestAnimationFrame(tick);
    };

    // -------------------------------------------------------------- boot
    // Open the root sheet by default (KiCad's top-level page, the lowest sheet
    // number). The design JSON lists sheets root-first, so sheets[0] is the root;
    // the min-num scan is a guard in case that order ever changes.
    const start =
      idx.sheets.reduce<number | null>((lo, s) => (lo == null || s.num < lo ? s.num : lo), null) ??
      idx.sheets[0]?.num ??
      1;
    (async () => {
      try {
        // D1 reload restore: selection and viewport survive a re-crunch; the sheet
        // re-resolves by NAME (numbering may shift), highlights by ref.
        const restore = canvasRestore.state;
        canvasRestore.state = null;
        const norm = (s: string) => s.toLowerCase().replace(/[\s_]+/g, "");
        const restoreSheet =
          restore && idx.sheets.find((s) => s.svg && norm(s.name) === norm(restore.sheetName));
        if (restore && restoreSheet) {
          await loadSheet(restoreSheet.num);
          tgt.current = { ...restore.cam };
          highlights.current = restore.highlights.filter((h) =>
            h.kind === "net" ? !!idx.nets[h.ref] : !!idx.components[h.ref],
          );
          renderHighlights();
        } else {
          if (restore) setTombstone(restore.sheetName); // deleted-sheet tombstone
          await loadSheet(start);
          fitSheet();
          renderHighlights(); // paint any persistent highlights for this project (item 22)
        }
      } catch {
        // Initial sheet read failed (extraction deleted/moved mid-session) — getSheetSvg
        // already raised the "extraction missing" toast with a Reload action. Swallow here
        // so the render loop below still starts: the canvas keeps panning/zooming instead
        // of dying until the user reloads.
      } finally {
        cam.current = { ...tgt.current };
        raf = requestAnimationFrame(tick);
      }
    })();

    return () => {
      cancelAnimationFrame(raf);
      for (const id of visRafs) cancelAnimationFrame(id); // stop any pending whenVisible poll
      unsubCompose();
      unregisterProbe();
      stage.removeEventListener("wheel", onWheel);
      stage.removeEventListener("pointerdown", onPointerDown);
      stage.removeEventListener("pointermove", onPointerMove);
      stage.removeEventListener("pointerup", onPointerUp);
      stage.removeEventListener("contextmenu", onContextMenu);
      window.removeEventListener("keydown", onKey);
      nav.goNet = nav.goComp = nav.goPin = nav.jumpToNet = nav.openSheet = nav.goSheetNum =
        nav.toggleHighlight = nav.fitView = nav.zoomBy = nav.toggleOverview =
        nav.applySelection = nav.setComments = () => {};
      nav.reveal = () => {};
      nav.revealDiff = () => {};
      nav.clearDiff = () => {};
      nav.getViewState = () => null;
      camBridge.drive = () => {};
      camBridge.centerWorld = () => {};
    };
  }, [indexes, getSheetSvg, setSelectionStore, setHighlightStore, setCurrentSheet]);

  // Repaint when persistent highlights change (loaded async, or pinned/unpinned).
  useEffect(() => {
    renderRef.current();
  }, [pinned]);

  // Crosshair cursor while comment mode is armed (click an object or drag a region).
  useEffect(() => {
    stageRef.current?.classList.toggle("arming", armed);
  }, [armed]);

  return (
    <div className="canvas-host">
      {/* The top nav bar (Fit / overview / hint text) was removed (item 4): Fit = F
          key or right-click; overview = O key or zoom-out past the sheet edge. */}
      <div ref={stageRef} className="canvas-stage">
        <div ref={worldRef} className="canvas-world">
          <div ref={islandRef} className="canvas-island" />
        </div>
        <div ref={badgeLayerRef} className="canvas-badges" />
        <div ref={commentLayerRef} className="canvas-badges comment-layer" />
        {overviewOpen && (
          <Overview
            onPick={(num) => {
              setOverviewOpen(false);
              nav.goSheetNum(num);
            }}
          />
        )}
        {tombstone && (
          <div className="canvas-toast">
            Sheet “{tombstone}” no longer exists in this revision.
          </div>
        )}
      </div>
      {ctxMenu && (
        <ContextMenu {...ctxMenu} onClose={() => setCtxMenu(null)} />
      )}
    </div>
  );
}
