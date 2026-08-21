import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { useDesignStore } from "../stores/designStore";
import { useSelectionStore } from "../stores/selectionStore";
import { useViewStore } from "../stores/viewStore";
import { useDiffStore } from "../stores/diffStore";
import { useToastStore } from "../stores/toastStore";
import {
  displayInfo,
  numberMap,
  useReviewStore,
  type DisplayStatus,
} from "../stores/reviewStore";
import { bomNav, nav } from "./canvas/navigator";
import { IconChecklist, IconComment, IconCopy } from "./icons";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { BomCheckBar } from "./BomCheckBar";
import { useBomCheckStore } from "../stores/bomCheckStore";
import {
  bomChanges,
  bomDeltaCsv,
  changesFirstCompare,
  decorateBomRows,
  bomOldValues,
  looseField,
  type DiffBomRow,
} from "../lib/bomDiff";
import {
  DEFAULT_COLS,
  DEFAULT_GROUP_BY,
  MIXED_VALUES,
  STATUS_COL,
  customFieldValue,
  groupLines,
  presetColumns,
  type BomCol,
} from "../lib/bomColumns";
import type { BomLine, BomPreset, Comment, CommentSeverity } from "../lib/types";
import type { Change } from "../lib/diff";

/** Cell content in diff mode: "old → new" — the old value struck through in red, the new
 *  one in green — or just the value when there is no extractable old one. */
const isChange = (c: Change | undefined): c is Change => !!c;

function wasCell(old: string | number | undefined, now: string | number) {
  if (old === undefined || String(old) === String(now)) return plainCell(now);
  return (
    <>
      <span className="bom-was">{String(old) || "∅"}</span>
      <span className="bom-was-arrow">→</span>
      <span className="bom-now">{plainCell(now)}</span>
    </>
  );
}

/** A cell's value, with the grouped-line "mixed members" marker highlighted rather than
 *  reading as an ordinary value. */
function plainCell(v: string | number) {
  return v === MIXED_VALUES ? <span className="bom-mixed">{v}</span> : v;
}

// WS7: BOM keeps a tab; it is not canvas content. Row click = select the line's
// first designator (card/status mirror it); double-click = jump to the symbol.
//
// Diff mode (visual-diff plan §8): the table gains a Status column + row tinting
// (added green / removed red / changed amber; removed rows struck-through, shown
// from revision A), a changes-first default sort, a "Copy delta CSV" action, and
// per-row designator chips that link to the underlying component changes. The
// Changes panel steps into the table via bomNav (row scroll + flash).

// Column sets come from src/lib/bomColumns: the built-in "Default" set, or the visible
// fields of a KiCad BOM preset read out of the project (custom columns land in line.fields).
//
// Long BOMs are windowed by hand (no dependency): fixed row height, spacer rows top and
// bottom, only the visible slice ± overscan mounted. Short ones render whole.
const ROW_H = 26;
const OVERSCAN = 10;
const VIRT_MIN = 100;
/** Narrowest a column can be dragged, so a mis-drag can't make one unclickable. */
const MIN_COL_W = 40;
/** Width for a column that appears after the table was sized (the diff-mode Δ column, a
 *  column un-hidden later) — fixed layout would otherwise leave it degenerate. */
const DEFAULT_COL_W = 120;
/** Every BOM row a comment covers. A checker finding that groups many rows into one
 *  point (AEC-Q, duplicate refdes) anchors on the first designator but carries the whole
 *  set in its evidence anchors — every one of those rows earns a marker, not just the
 *  anchor row. Human comments have no evidence, so they stay on their anchor alone. */
function coveredRefs(c: Comment): string[] {
  const refs = c.anchor.type === "component" ? [c.anchor.ref] : [];
  const anchors = (c.evidence as { anchors?: unknown } | null)?.anchors;
  if (Array.isArray(anchors)) {
    for (const a of anchors) {
      // The findings schema names the field `type` (bom-rules' `Anchor.kind` is
      // `#[serde(rename = "type")]`), so that is what the stored evidence carries.
      const anchor = a as { type?: unknown; refdes?: unknown } | null;
      if (anchor?.type !== "bom_row" || !Array.isArray(anchor.refdes)) continue;
      for (const r of anchor.refdes) if (typeof r === "string" && r) refs.push(r);
    }
  }
  return refs;
}

/** Severity order, so the row marker shows the worst comment on a line. */
const SEVERITY_RANK: Record<CommentSeverity, number> = { info: 0, minor: 1, major: 2, critical: 3 };

/** The leading comment gutter's width — mirrors .bom-cmt-col in app.css. */
const CMT_COL_W = 24;

const STATUS_LABEL = { added: "Added", removed: "Removed", changed: "Changed" } as const;
const STATUS_ROLE = { added: "ok", removed: "err", changed: "warn" } as const;

/** Plain-text value of one cell, taken from the row data (not the DOM) so diff
 *  decoration / chips / badges don't leak into the clipboard or the sort. Custom preset
 *  columns read line.fields; `mpnFallback` supplies the component-index MPN. */
function cellText(
  r: DiffBomRow,
  col: BomCol | undefined,
  mpnFallback: (designator: string) => string,
): string {
  if (!col) return "";
  const l = r.line;
  switch (col.builtin) {
    case "status":
      return r.status ? STATUS_LABEL[r.status] : "";
    case "item":
      return r.synthetic ? "—" : String(l.item);
    case "qty":
      return String(l.qty);
    case "designators":
      return l.designators.join(", ");
    case "mpn":
      return l.mpn || mpnFallback(l.designators[0] ?? "") || "";
    case "dnp":
      return l.dnp ? "DNP" : "";
    case "value":
      return l.value;
    case "footprint":
      return l.footprint;
    default:
      return col.field ? customFieldValue(l, col.field, col.label) : "";
  }
}

/** Stable React/expansion key for a BOM row (matches the old inline key expression). */
const rowKeyOf = (r: DiffBomRow) => (r.synthetic ? `removed-${r.key}` : String(r.line.item));

/** One row of the rendered table: a BOM line, or a per-designator child of one. */
type DisplayRow = { row: DiffBomRow; key: string; child: boolean };

export function BomTab() {
  const indexes = useDesignStore((s) => s.indexes);
  const selection = useSelectionStore((s) => s.selection);
  const setSelection = useSelectionStore((s) => s.setSelection);
  const setView = useViewStore((s) => s.setView);
  const bomChips = useViewStore((s) => s.bomChips);
  const toggleBomChip = useViewStore((s) => s.toggleBomChip);
  const setBomChip = useViewStore((s) => s.setBomChip);
  const bomLayout = useViewStore((s) => s.bomLayout);
  const setBomPreset = useViewStore((s) => s.setBomPreset);
  const toggleBomColumn = useViewStore((s) => s.toggleBomColumn);
  const setBomSort = useViewStore((s) => s.setBomSort);
  const setBomColWidths = useViewStore((s) => s.setBomColWidths);
  const diffActive = useDiffStore((s) => s.active);
  const diffDoc = useDiffStore((s) => s.doc);
  const focusChange = useDiffStore((s) => s.focusChange);
  // Review comments live in the BOM tab too (view: "bom"): the composer/thread popover
  // float over this tab already, so we only wire up per-row create + the existing-comment
  // markers here. `armed` is the app-wide comment mode (C key / the toolbar toggle).
  const armed = useReviewStore((s) => s.armed);
  const comments = useReviewStore((s) => s.comments);
  const activeSessionId = useReviewStore((s) => s.activeSessionId);
  const [lines, setLines] = useState<BomLine[] | null>(null);
  const [presets, setPresets] = useState<BomPreset[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<{ key: string; dir: 1 | -1 }>(
    () => bomLayout.sort ?? { key: "item", dir: 1 },
  );
  const [flashKey, setFlashKey] = useState<string | null>(null);
  // Expanded grouped lines (KiCad's Symbol Fields Table behaviour): a row key here means
  // "show one child row per designator". Local + transient — collapses on filter/preset
  // change, never persisted.
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [colMenu, setColMenu] = useState<{ x: number; y: number } | null>(null);

  // Re-fetch per revision: `indexes` is replaced on every design reload, and diff
  // mode pins the active revision to B — so the table always shows the B-side BOM.
  useEffect(() => {
    let cancelled = false;
    ipc
      .getBomLines()
      .then((l) => !cancelled && (setLines(l), setError(null)))
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [indexes]);

  // The project's KiCad BOM presets (column sets). Purely additive: a project without
  // them — or a backend that errors — just leaves the "Default" set selected.
  useEffect(() => {
    let cancelled = false;
    ipc
      .getBomPresets()
      .then((p) => !cancelled && setPresets(Array.isArray(p) ? p : []))
      .catch((e) => {
        if (cancelled) return;
        console.warn("BOM presets unavailable; using the default columns", e);
        setPresets([]);
      });
    return () => {
      cancelled = true;
    };
  }, [indexes]);

  // "" = the built-in Default set. A remembered preset that this project doesn't have
  // (the user switched projects) falls back to Default without complaining. Until the user
  // picks one (preset === null), the project's own KiCad selection wins.
  const projectDefault = presets.find((p) => p.is_project_default)?.name ?? "";
  const presetName =
    bomLayout.preset === null
      ? projectDefault
      : presets.some((p) => p.name === bomLayout.preset)
        ? bomLayout.preset
        : "";
  const activePreset = presets.find((p) => p.name === presetName);

  // Grouping belongs to the preset, not to the extractor: the backend hands us one line
  // per component, and we fold those onto the fields KiCad flags `group_by` in the active
  // preset. No preset (built-in Default set) → the default key; a preset that flags
  // nothing → one row per component, exactly as KiCad shows it.
  const grouped = useMemo(
    () =>
      lines ? groupLines(lines, activePreset ? (activePreset.fields ?? []) : DEFAULT_GROUP_BY) : null,
    [lines, activePreset],
  );

  /** Every column of the active set (before the user's hide list), Δ first in diff mode. */
  const allCols = useMemo<BomCol[]>(() => {
    const base = activePreset ? presetColumns(activePreset) : DEFAULT_COLS;
    const cols = base.length > 0 ? base : DEFAULT_COLS;
    return diffActive ? [STATUS_COL, ...cols] : cols;
  }, [activePreset, diffActive]);

  const hidden = bomLayout.hidden[presetName] ?? [];
  const cols = useMemo(
    () => allCols.filter((c) => c.id === "status" || !hidden.includes(c.id)),
    [allCols, hidden],
  );

  // ---- column widths -----------------------------------------------------------
  // Per-preset and persisted. The table stays in auto layout until the user drags a
  // header edge: the first drag freezes every column at its measured width and flips the
  // table to fixed layout, so pulling one edge doesn't reflow the neighbours.
  const [dragWidths, setDragWidths] = useState<Record<string, number> | null>(null);
  const widths = dragWidths ?? bomLayout.widths[presetName] ?? {};
  const sized = Object.keys(widths).length > 0;
  // The sized table is laid out at exactly the sum of its columns (see .bom-table.sized):
  // any narrower and .bom-scroll scrolls; any wider and the trailing filler column eats
  // the slack, so a column the user drags narrower stays narrow.
  const totalW =
    CMT_COL_W + cols.reduce((n, c) => n + (widths[c.id] ?? DEFAULT_COL_W), 0);
  const thRefs = useRef<Map<string, HTMLTableCellElement>>(new Map());
  const drag = useRef<{ id: string; x: number; base: Record<string, number> } | null>(null);
  const dragLive = useRef<Record<string, number> | null>(null);

  function startResize(e: React.PointerEvent<HTMLSpanElement>, id: string) {
    e.preventDefault();
    e.stopPropagation();
    const base: Record<string, number> = { ...widths };
    for (const c of cols) {
      if (base[c.id] === undefined)
        base[c.id] = thRefs.current.get(c.id)?.offsetWidth ?? MIN_COL_W * 2;
    }
    drag.current = { id, x: e.clientX, base };
    dragLive.current = base;
    setDragWidths(base);
    e.currentTarget.setPointerCapture(e.pointerId);
  }

  function moveResize(e: React.PointerEvent<HTMLSpanElement>) {
    const d = drag.current;
    if (!d) return;
    const next = { ...d.base, [d.id]: Math.max(MIN_COL_W, d.base[d.id] + (e.clientX - d.x)) };
    dragLive.current = next;
    setDragWidths(next);
  }

  function endResize() {
    if (!drag.current) return;
    drag.current = null;
    if (dragLive.current) setBomColWidths(presetName, dragLive.current);
    dragLive.current = null;
    setDragWidths(null);
  }

  // Entering diff mode defaults to changes-first ("status"); leaving restores Item.
  // The user's own header clicks still win afterwards (this only fires on the flip).
  // The first run is skipped so the persisted sort survives a remount.
  const firstDiffFlip = useRef(true);
  useEffect(() => {
    if (firstDiffFlip.current) {
      firstDiffFlip.current = false;
      return;
    }
    setSort(diffActive ? { key: "status", dir: 1 } : { key: "item", dir: 1 });
    // Entering a comparison also re-arms "Changed only": the affected lines are what the
    // comparison is about. Only on the flip — the user's own chip click still wins, and a
    // tab switch mid-comparison doesn't undo it. (Its remembered default is on too.)
    if (diffActive) setBomChip("changedOnly", true);
  }, [diffActive, setBomChip]);

  // A persisted sort column can vanish (preset switch, column hidden) — fall back to the
  // first live column rather than silently sorting on nothing.
  const sortKey = cols.some((c) => c.id === sort.key) ? sort.key : (cols[0]?.id ?? "item");
  const sortCol = cols.find((c) => c.id === sortKey);

  const changes = useMemo(
    () => (diffActive && diffDoc ? bomChanges(diffDoc.changes) : []),
    [diffActive, diffDoc],
  );

  const changeById = useMemo(() => new Map(changes.map((c) => [c.id, c])), [changes]);

  const rows = useMemo<DiffBomRow[]>(() => {
    if (!grouped) return [];
    const decorated =
      diffActive && changes.length > 0
        ? decorateBomRows(grouped, changes)
        : grouped.map((line) => ({
            line,
            status: null,
            changeIds: [],
            key: "",
            synthetic: false,
          }));
    const q = filter.trim().toLowerCase();
    // Chips AND-combine with each other and with the text filter. "Changed only" is
    // inert outside diff mode even when its remembered state is on.
    const changedOnly = diffActive && bomChips.changedOnly;
    const mpnFallback = (d: string) => indexes?.components[d]?.mpn ?? "";
    const filtered = decorated.filter((r) => {
      // Match against every visible column, so search follows the active preset.
      if (q && !cols.some((c) => cellText(r, c, mpnFallback).toLowerCase().includes(q)))
        return false;
      if (bomChips.dnpOnly && !r.line.dnp) return false;
      // The MPN cell falls back to the component index, so a row is only "missing"
      // when both the BOM line and the indexed part carry nothing.
      if (bomChips.missingMpn && (r.line.mpn || indexes?.components[r.line.designators[0] ?? ""]?.mpn))
        return false;
      if (changedOnly && !r.status) return false;
      return true;
    });
    const dir = sort.dir;
    return [...filtered].sort((a, b) => {
      if (sortCol?.builtin === "status") return changesFirstCompare(a, b) * dir;
      if (sortCol?.builtin === "designators")
        return (
          (a.line.designators[0] ?? "").localeCompare(b.line.designators[0] ?? "", undefined, {
            numeric: true,
          }) * dir
        );
      const va = cellText(a, sortCol, mpnFallback);
      const vb = cellText(b, sortCol, mpnFallback);
      return va.localeCompare(vb, undefined, { numeric: true }) * dir;
    });
  }, [grouped, changes, diffActive, filter, sort.dir, sortCol, bomChips, indexes, cols]);

  // Collapse everything when the row set is rebuilt by a filter/preset switch: the keys
  // would still match, but the groups on screen have changed under the user.
  useEffect(() => {
    setExpanded((s) => (s.size === 0 ? s : new Set()));
  }, [filter, presetName, diffActive]);

  /** The ungrouped (one-per-component) line each designator came from, so an expanded
   *  child can show that component's own field values instead of the group's merged
   *  ones — which read MIXED_VALUES wherever the members disagree. */
  const lineByDesignator = useMemo(() => {
    const m = new Map<string, BomLine>();
    for (const l of lines ?? []) for (const d of l.designators) if (!m.has(d)) m.set(d, l);
    return m;
  }, [lines]);

  /** The flattened list actually rendered: every parent row, each followed by its
   *  designator children while expanded. All windowing arithmetic runs on this list, so
   *  spacer heights, rowIndex and scrollToKey stay in step with what is on screen. */
  const display = useMemo<DisplayRow[]>(() => {
    const out: DisplayRow[] = [];
    for (const r of rows) {
      const key = rowKeyOf(r);
      out.push({ row: r, key, child: false });
      if (r.line.designators.length < 2 || !expanded.has(key)) continue;
      for (const d of r.line.designators) {
        // A child is one component's own line narrowed to that designator, so
        // cellText/renderCell (and therefore copy and the MPN fallback) just work — and
        // the fields the group folded to MIXED_VALUES read as this member's values. A
        // designator the ungrouped set doesn't carry (a synthetic removed row) keeps the
        // old behaviour: the group's line, narrowed.
        const own = lineByDesignator.get(d) ?? r.line;
        out.push({
          row: {
            ...r,
            status: null,
            changeIds: [],
            line: {
              ...own,
              item: r.line.item,
              designators: [d],
              mpn: own.mpn || indexes?.components[d]?.mpn || "",
            },
          },
          key: `${key}:${d}`,
          child: true,
        });
      }
    }
    return out;
  }, [rows, expanded, indexes, lineByDesignator]);

  function toggleExpand(key: string) {
    setExpanded((s) => {
      const next = new Set(s);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }

  // Open/unaddressed BOM comments keyed by every designator they cover, scoped to the
  // active review session (mirrors the canvas chip filter in CommentBridge: resolved and
  // dismissed comments carry no marker). A row shows the marker for the first of its
  // designators that carries one.
  const commentByRef = useMemo(() => {
    const numbers = numberMap(comments);
    const m = new Map<
      string,
      { id: string; number: number; status: DisplayStatus; severity: CommentSeverity }
    >();
    for (const c of comments) {
      if (c.view !== "bom" || c.anchor.type !== "component") continue;
      if (activeSessionId !== null && c.session_id !== activeSessionId) continue;
      const status = displayInfo(c, indexes ?? null).status;
      if (status === "resolved" || status === "dismissed") continue;
      const number = numbers.get(c.id) ?? 0;
      const severity = c.severity ?? "info";
      for (const ref of coveredRefs(c)) {
        const prev = m.get(ref);
        // The marker now shows severity, so when a row carries several comments the most
        // severe one wins (oldest breaks a tie) — a critical must never hide behind an info.
        const better =
          !prev ||
          SEVERITY_RANK[severity] > SEVERITY_RANK[prev.severity] ||
          (SEVERITY_RANK[severity] === SEVERITY_RANK[prev.severity] && number < prev.number);
        if (better) m.set(ref, { id: c.id, number, status, severity });
      }
    }
    return m;
  }, [comments, activeSessionId, indexes]);

  const rowComment = (l: BomLine) => {
    for (const d of l.designators) {
      const hit = commentByRef.get(d);
      if (hit) return hit;
    }
    return undefined;
  };

  /** Anchor a new BOM comment to the line's first designator (a "component" anchor,
   *  stamped with the current "bom" view by the composer). The popover floats where the
   *  click landed. Removed (synthetic) lines have no live object to comment on. */
  function addComment(l: BomLine, synthetic: boolean, e: React.MouseEvent) {
    e.stopPropagation();
    const ref = l.designators[0];
    if (synthetic || !ref) return;
    useReviewStore
      .getState()
      .beginCompose({ anchor: { type: "component", ref }, pos: { x: e.clientX + 12, y: e.clientY } });
  }

  function openRowThread(id: string, e: React.MouseEvent) {
    e.stopPropagation();
    useReviewStore.getState().openThread(id, { x: e.clientX + 12, y: e.clientY });
  }

  // ---- windowing ---------------------------------------------------------------
  // Only long BOMs are virtualized; under VIRT_MIN rows everything renders (a short
  // table shouldn't pay for spacer arithmetic, and row heights can stay natural).
  const scrollRef = useRef<HTMLDivElement>(null);
  const headRef = useRef<HTMLTableSectionElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(600);
  const virt = display.length > VIRT_MIN;

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    setViewportH(el.clientHeight);
    const ro = new ResizeObserver(() => setViewportH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const headH = headRef.current?.offsetHeight ?? ROW_H;
  const firstVisible = virt ? Math.floor(Math.max(0, scrollTop - headH) / ROW_H) : 0;
  const start = virt ? Math.max(0, firstVisible - OVERSCAN) : 0;
  const end = virt
    ? Math.min(display.length, firstVisible + Math.ceil(viewportH / ROW_H) + OVERSCAN)
    : display.length;
  const windowRows = virt ? display.slice(start, end) : display;

  /** Row index by lookup key (diff row key + first designator), so a scroll target that
   *  is currently outside the window can still be reached — by scrollTop, not by DOM.
   *  Built over the flattened list; parents come first, so a designator inside a group
   *  resolves to the parent row whether or not the group is expanded. */
  const rowIndex = useMemo(() => {
    const m = new Map<string, number>();
    display.forEach((e, i) => {
      if (e.child) return;
      const r = e.row;
      for (const k of [r.key, ...r.line.designators]) if (k && !m.has(k)) m.set(k, i);
    });
    return m;
  }, [display]);

  /** Designator → the key its row is registered under in rowRefs (diff key or first
   *  designator), so any designator of a grouped line can reach the mounted row. */
  const refKeyByDesignator = useMemo(() => {
    const m = new Map<string, string>();
    for (const r of rows) {
      const k = r.key || r.line.designators[0];
      if (!k) continue;
      for (const d of r.line.designators) if (!m.has(d)) m.set(d, k);
    }
    return m;
  }, [rows]);

  // Latest values for the imperative scroll helper, which lives in a mount-only effect.
  const scrollState = useRef({ rowIndex, virt, headH });
  scrollState.current = { rowIndex, virt, headH };

  /** Centre the row for `key`: scrollIntoView when it is mounted, otherwise compute its
   *  offset from the fixed row height (windowed rows may not exist in the DOM yet). */
  function scrollToKey(key: string, smooth = false) {
    const el = rowRefs.current.get(key);
    if (el) {
      el.scrollIntoView({ block: "center", behavior: smooth ? "smooth" : "auto" });
      return;
    }
    const { rowIndex: idx, virt: on, headH: h } = scrollState.current;
    const i = idx.get(key);
    const sc = scrollRef.current;
    if (!on || i === undefined || !sc) return;
    sc.scrollTop = Math.max(0, h + i * ROW_H - sc.clientHeight / 2);
  }

  // Stepper landing (bomNav): scroll the row into view and flash it. Registered
  // while mounted; a flash requested during the view switch is queued by the bridge.
  const rowRefs = useRef<Map<string, HTMLTableRowElement>>(new Map());
  const flashTimer = useRef<number | null>(null);
  useEffect(() => {
    const unregister = bomNav.register((key) => {
      setFlashKey(null); // restart the CSS animation even for the same row
      requestAnimationFrame(() => {
        setFlashKey(key);
        scrollToKey(key);
      });
      if (flashTimer.current) window.clearTimeout(flashTimer.current);
      flashTimer.current = window.setTimeout(() => setFlashKey(null), 1600);
    });
    return () => {
      unregister();
      if (flashTimer.current) window.clearTimeout(flashTimer.current);
    };
  }, []);

  // Reverse cross-probe: canvas selection scrolls its BOM row into view, but only
  // when the row is off-screen (clicking a row sets selection; don't jerk the table).
  //
  // The tab mounts fresh on every switch into it and its lines arrive asynchronously, so
  // a selection made on the canvas is already set when this first runs, with no rows to
  // scroll to yet. Hence `display` in the deps — the scroll is honoured as soon as the
  // rows exist — and `honoured`, so it happens once per selection instead of on every
  // later filter/expand that rebuilds the list under the same selected part.
  const honoured = useRef<string | null>(null);
  useEffect(() => {
    if (selection?.kind !== "comp" || typeof selection.ref !== "string") {
      honoured.current = null;
      return;
    }
    const ref = selection.ref;
    if (honoured.current === ref || display.length === 0) return;
    const refKey = refKeyByDesignator.get(ref) ?? ref;
    const row = rowRefs.current.get(refKey);
    if (row) {
      honoured.current = ref;
      const scroller = row.closest(".bom-scroll");
      if (scroller) {
        const r = row.getBoundingClientRect();
        const s = scroller.getBoundingClientRect();
        if (r.top >= s.top && r.bottom <= s.bottom) return;
      }
      row.scrollIntoView({ block: "center", behavior: "smooth" });
      return;
    }
    // Off-window (virtualized): jump by index instead. A ref the current rows don't
    // carry (filtered out) stays unhonoured, so clearing the filter still lands it.
    if (rowIndex.get(ref) === undefined) return;
    honoured.current = ref;
    scrollToKey(ref, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selection, display]);

  function clickHeader(key: string) {
    const next: { key: string; dir: 1 | -1 } =
      sort.key === key ? { key, dir: sort.dir === 1 ? -1 : 1 } : { key, dir: 1 };
    setSort(next);
    setBomSort(next);
  }

  async function copyDeltaCsv() {
    try {
      await navigator.clipboard.writeText(bomDeltaCsv(changes));
      useToastStore.getState().push({
        kind: "info",
        title: "BOM delta copied",
        message: `${changes.length} change${changes.length === 1 ? "" : "s"} as CSV.`,
      });
    } catch (e) {
      useToastStore.getState().push({
        kind: "error",
        title: "Couldn’t copy BOM delta",
        message: String(e),
      });
    }
  }

  const text = (r: DiffBomRow, col: BomCol | undefined) =>
    cellText(r, col, (d) => indexes?.components[d]?.mpn ?? "");

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      useToastStore.getState().push({
        kind: "error",
        title: "Couldn’t copy",
        message: String(e),
      });
    }
  }

  /** Right-click on a data cell: copy this cell, the row (visible columns,
   *  tab-separated) or the column across all currently visible rows. */
  function openCellMenu(e: React.MouseEvent, r: DiffBomRow, col: BomCol) {
    e.preventDefault();
    e.stopPropagation();
    const copyCols = cols.filter((c) => c.builtin !== "status");
    setCtxMenu({
      x: e.clientX,
      y: e.clientY,
      items: [
        {
          label: "Copy cell",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(text(r, col)),
        },
        {
          label: "Copy row",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(copyCols.map((c) => text(r, c)).join("\t")),
        },
        {
          label: "Copy column",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(rows.map((row) => text(row, col)).join("\n")),
        },
        {
          label: "Run BOM check",
          icon: <IconChecklist size={14} />,
          onClick: () => void useBomCheckStore.getState().run(),
        },
      ],
    });
  }

  /** A designator chip in a tinted row links to the underlying component change
   *  (its schematic/PCB anchors, §8); parts without one fall back to a plain jump. */
  function goDesignator(d: string) {
    const underlying = diffDoc?.changes.find(
      (c) =>
        c.group === "component" &&
        (c.anchors.pcb?.comp === d || c.title === d || c.title.startsWith(`${d} `)),
    );
    if (underlying) {
      focusChange(underlying.id);
      return;
    }
    setView("schematic");
    nav.goComp(d);
  }

  const changedCount = rows.filter((r) => r.status).length;

  /** One data cell. Diff decoration (old → new, designator chips) stays attached to the
   *  built-in columns wherever the active preset happens to place them. */
  function renderCell(
    r: DiffBomRow,
    col: BomCol,
    old: ReturnType<typeof bomOldValues>,
    displayKey: string,
    child: boolean,
  ) {
    const l = r.line;
    switch (col.builtin) {
      case "status":
        return (
          <td key={col.id} className="bom-status-cell">
            {r.status && (
              <span className={`bom-status bom-status-${STATUS_ROLE[r.status]}`}>
                {STATUS_LABEL[r.status]}
              </span>
            )}
          </td>
        );
      case "item":
        return (
          <td key={col.id} className="dim" onContextMenu={(e) => openCellMenu(e, r, col)}>
            {r.synthetic ? "—" : l.item}
          </td>
        );
      case "qty":
        return (
          <td key={col.id} onContextMenu={(e) => openCellMenu(e, r, col)}>
            {wasCell(old.qty, l.qty)}
          </td>
        );
      case "designators": {
        const groupKey = child ? "" : displayKey;
        const expandable = !child && l.designators.length > 1;
        const open = expandable && expanded.has(groupKey);
        return (
          <td
            key={col.id}
            className={`bom-dsg${child ? " bom-dsg-child" : ""}`}
            // A grouped line can carry dozens of designators: the cell clips to one line
            // (see .bom-dsg-text) and the full list lives on the tooltip / the expander.
            title={l.designators.join(", ")}
            onContextMenu={(e) => openCellMenu(e, r, col)}
          >
            {expandable && (
              <button
                className="bom-dsg-toggle"
                aria-expanded={open}
                title={open ? "Collapse the grouped designators" : "Expand the grouped designators"}
                onClick={(e) => {
                  e.stopPropagation();
                  toggleExpand(groupKey);
                }}
                onDoubleClick={(e) => e.stopPropagation()}
              >
                {open ? "▾" : "▸"}
              </button>
            )}
            <span className="bom-dsg-text">
              {diffActive && r.status
                ? l.designators.map((d) => (
                    <button
                      key={d}
                      className="bom-dsg-chip"
                      title={`Jump to ${d} (the underlying schematic/PCB change)`}
                      onClick={(e) => {
                        e.stopPropagation();
                        goDesignator(d);
                      }}
                    >
                      {d}
                    </button>
                  ))
                : l.designators.join(", ")}
            </span>
          </td>
        );
      }
      case "value":
        return (
          <td key={col.id} onContextMenu={(e) => openCellMenu(e, r, col)}>
            {wasCell(old.value, l.value)}
          </td>
        );
      case "footprint":
        return (
          <td key={col.id} className="dim" onContextMenu={(e) => openCellMenu(e, r, col)}>
            {wasCell(old.footprint, l.footprint)}
          </td>
        );
      case "mpn":
        return (
          <td key={col.id} onContextMenu={(e) => openCellMenu(e, r, col)}>
            {wasCell(old.mpn, l.mpn || indexes?.components[l.designators[0] ?? ""]?.mpn || "")}
          </td>
        );
      case "dnp":
        return (
          <td key={col.id} onContextMenu={(e) => openCellMenu(e, r, col)}>
            {l.dnp ? "DNP" : ""}
          </td>
        );
      default: {
        // Custom preset columns carry the long values (Description, Manufacturer, …) and
        // every cell now clips with an ellipsis, so the full text lives on the tooltip.
        // A symbol property the comparison changed (MSL, Automotive Grade, …) reads
        // old → new here just like the built-in columns.
        const v = text(r, col);
        const was = old.fields[looseField(col.field ?? col.label)];
        return (
          <td key={col.id} title={v} onContextMenu={(e) => openCellMenu(e, r, col)}>
            {wasCell(was, v)}
          </td>
        );
      }
    }
  }

  return (
    <div className="bom-tab">
      <div className="bom-bar">
        <input
          className="bom-filter"
          placeholder="Filter rows"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          spellCheck={false}
        />
        {presets.length > 0 && (
          <select
            className="bom-select"
            value={presetName}
            title="BOM column set — the project's KiCad BOM presets"
            onChange={(e) => setBomPreset(e.target.value)}
          >
            <option value="">Default</option>
            {presets.map((p) => (
              <option key={p.name} value={p.name}>
                {p.name}
              </option>
            ))}
          </select>
        )}
        <button
          className="btn-ghost bom-chip"
          title="Show or hide individual columns"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setColMenu(colMenu ? null : { x: r.left, y: r.bottom + 4 });
          }}
        >
          Columns ▾
        </button>
        <button
          className={`btn-ghost bom-chip ${bomChips.dnpOnly ? "on" : ""}`}
          aria-pressed={bomChips.dnpOnly}
          title="Show only Do-Not-Populate lines"
          onClick={() => toggleBomChip("dnpOnly")}
        >
          DNP only
        </button>
        <button
          className={`btn-ghost bom-chip ${bomChips.missingMpn ? "on" : ""}`}
          aria-pressed={bomChips.missingMpn}
          title="Show only lines with no manufacturer part number"
          onClick={() => toggleBomChip("missingMpn")}
        >
          Missing MPN
        </button>
        {diffActive && (
          <button
            className={`btn-ghost bom-chip ${bomChips.changedOnly ? "on" : ""}`}
            aria-pressed={bomChips.changedOnly}
            title="Show only lines affected by this comparison"
            onClick={() => toggleBomChip("changedOnly")}
          >
            Changed only
          </button>
        )}
        <span className="bom-count">
          {rows.length} line{rows.length === 1 ? "" : "s"}
          {diffActive && changedCount > 0 ? ` · ${changedCount} affected` : ""}
        </span>
        {diffActive && changes.length > 0 && (
          <button
            className="btn-ghost bom-csv-btn"
            title="Copy the BOM delta (added / removed / changed lines) as CSV for the purchasing conversation"
            onClick={() => void copyDeltaCsv()}
          >
            Copy delta CSV
          </button>
        )}
        <button
          className={`btn-ghost bom-comment-btn ${armed ? "on" : ""}`}
          aria-pressed={armed}
          title="Comment mode (C) — then click a row to add a review comment"
          onClick={() => useReviewStore.getState().arm(!armed)}
        >
          <IconComment size={14} />
        </button>
      </div>
      <BomCheckBar />
      {error && <div className="bom-empty">BOM unavailable: {error}</div>}
      {!error && lines && lines.length === 0 && (
        <div className="bom-empty">The crunched bundle has no BOM.</div>
      )}
      <div
        className="bom-scroll"
        ref={scrollRef}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <table
          className={`bom-table ${armed ? "arming" : ""} ${virt ? "virt" : ""} ${sized ? "sized" : ""}`}
          style={sized ? { width: totalW } : undefined}
        >
          <colgroup>
            <col className="bom-cmt-col" />
            {cols.map((c) => (
              <col
                key={c.id}
                style={sized ? { width: widths[c.id] ?? DEFAULT_COL_W } : undefined}
              />
            ))}
            {/* Slack eater: the only column with no width, so in fixed layout it absorbs
                whatever is left over to the pane edge. Without it the surplus is shared
                out over the real columns and dragging one narrower does nothing. */}
            <col className="bom-fill-col" />
          </colgroup>
          <thead ref={headRef}>
            <tr>
              <th className="bom-cmt-th" aria-hidden />
              {cols.map((c) => (
                <th
                  key={c.id}
                  // The designators header carries the same expander gutter its cells do,
                  // so the label sits over the designator text rather than left of it.
                  className={c.builtin === "designators" ? "bom-dsg-th" : undefined}
                  ref={(el) => {
                    if (el) thRefs.current.set(c.id, el);
                    else thRefs.current.delete(c.id);
                  }}
                  onClick={() => clickHeader(c.id)}
                >
                  {c.label}
                  {sortKey === c.id && (
                    <span className="bom-sort-arrow">{sort.dir === 1 ? " ▲" : " ▼"}</span>
                  )}
                  <span
                    className="bom-col-resize"
                    role="separator"
                    aria-orientation="vertical"
                    title={`Drag to resize the ${c.label} column`}
                    onClick={(e) => e.stopPropagation()}
                    onPointerDown={(e) => startResize(e, c.id)}
                    onPointerMove={moveResize}
                    onPointerUp={endResize}
                    onPointerCancel={endResize}
                    onLostPointerCapture={endResize}
                  />
                </th>
              ))}
              <th className="bom-fill-th" aria-hidden />
            </tr>
          </thead>
          <tbody>
            {start > 0 && (
              <tr className="bom-spacer" style={{ height: start * ROW_H }} aria-hidden>
                <td colSpan={cols.length + 2} />
              </tr>
            )}
            {windowRows.map(({ row: r, key: displayKey, child }) => {
              const l = r.line;
              const first = l.designators[0];
              const active =
                selection?.kind === "comp" &&
                typeof selection.ref === "string" &&
                l.designators.includes(selection.ref);
              const statusCls = r.status ? ` bom-${r.status}` : "";
              const flash =
                !child &&
                flashKey !== null &&
                flashKey !== "" &&
                (flashKey === r.key || flashKey === first)
                  ? " bom-flash"
                  : "";
              const cmt = rowComment(l);
              // Inline old → new, so a "changed" row reads without opening the panel.
              const old =
                r.status === "changed"
                  ? bomOldValues(r.changeIds.map((id) => changeById.get(id)).filter(isChange))
                  : { fields: {} };
              return (
                <tr
                  key={displayKey}
                  ref={(el) => {
                    // Register under the diff row key (stepper landing) and the first
                    // designator (review-comment landing); keys never collide — the diff
                    // key is a value/footprint/mpn hash, not a designator. Child rows
                    // register nothing, so cross-probe always lands on the parent.
                    if (child) return;
                    for (const k of [r.key, first]) {
                      if (!k) continue;
                      if (el) rowRefs.current.set(k, el);
                      else rowRefs.current.delete(k);
                    }
                  }}
                  className={`${active ? "active" : ""}${l.dnp ? " dnp" : ""}${statusCls}${flash}${child ? " bom-child" : ""}`}
                  onClick={(e) => {
                    if (useReviewStore.getState().armed) return addComment(l, r.synthetic, e);
                    if (r.changeIds.length > 0) focusChange(r.changeIds[0]);
                    else if (first && !r.synthetic) setSelection({ kind: "comp", ref: first });
                  }}
                  onDoubleClick={() => {
                    if (!first || r.synthetic) return;
                    setView("schematic");
                    nav.goComp(first);
                  }}
                  title={
                    armed
                      ? r.synthetic
                        ? "Removed line — nothing live to comment on"
                        : "click: add a review comment on this line"
                      : r.synthetic
                        ? "Removed line (from the older revision)"
                        : r.status
                          ? "click: focus this change · designators jump to the part"
                          : "click: select · double-click: jump to symbol"
                  }
                >
                  <td className="bom-comment-cell">
                    {child ? null : cmt ? (
                      <button
                        className={`bom-cmt-badge sev-${cmt.severity} st-${cmt.status}`}
                        title={`Open the ${cmt.severity} review comment on this line`}
                        aria-label={`${cmt.severity} review comment`}
                        onClick={(e) => openRowThread(cmt.id, e)}
                      />
                    ) : (
                      !r.synthetic &&
                      first && (
                        <button
                          className="bom-cmt-add"
                          title="Add a review comment on this line"
                          onClick={(e) => addComment(l, r.synthetic, e)}
                        >
                          <IconComment size={12} />
                        </button>
                      )
                    )}
                  </td>
                  {cols.map((c) => renderCell(r, c, old, displayKey, child))}
                  <td className="bom-fill-cell" />
                </tr>
              );
            })}
            {end < display.length && (
              <tr className="bom-spacer" style={{ height: (display.length - end) * ROW_H }} aria-hidden>
                <td colSpan={cols.length + 2} />
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {colMenu && (
        <div className="bom-colmenu-backdrop" onPointerDown={() => setColMenu(null)}>
          <div
            className="bom-colmenu"
            style={{ left: colMenu.x, top: colMenu.y }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            {allCols
              .filter((c) => c.builtin !== "status")
              .map((c) => (
                <label key={c.id} className="bom-colmenu-row">
                  <input
                    type="checkbox"
                    checked={!hidden.includes(c.id)}
                    onChange={() => toggleBomColumn(presetName, c.id)}
                  />
                  {c.label}
                </label>
              ))}
          </div>
        </div>
      )}
      {ctxMenu && <ContextMenu {...ctxMenu} onClose={() => setCtxMenu(null)} />}
    </div>
  );
}
