import { useEffect, useMemo, useRef, useState } from "react";
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
import { IconComment, IconCopy } from "./icons";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import {
  bomChanges,
  bomDeltaCsv,
  changesFirstCompare,
  decorateBomRows,
  bomOldValues,
  type DiffBomRow,
} from "../lib/bomDiff";
import type { BomLine } from "../lib/types";
import type { Change } from "../lib/diff";

/** Cell content in diff mode: "old → new" with the old value dimmed, or just the
 *  new value when there is no extractable old one. */
const isChange = (c: Change | undefined): c is Change => !!c;

function wasCell(old: string | number | undefined, now: string | number) {
  if (old === undefined || String(old) === String(now)) return now;
  return (
    <>
      <span className="bom-was">{String(old) || "∅"}</span>
      <span className="bom-was-arrow">→</span>
      {now}
    </>
  );
}

// WS7: BOM keeps a tab; it is not canvas content. Row click = select the line's
// first designator (card/status mirror it); double-click = jump to the symbol.
//
// Diff mode (visual-diff plan §8): the table gains a Status column + row tinting
// (added green / removed red / changed amber; removed rows struck-through, shown
// from revision A), a changes-first default sort, a "Copy delta CSV" action, and
// per-row designator chips that link to the underlying component changes. The
// Changes panel steps into the table via bomNav (row scroll + flash).

type SortKey = "status" | "item" | "qty" | "designators" | "value" | "footprint" | "mpn" | "dnp";

const COLS: { key: SortKey; label: string; diffOnly?: boolean }[] = [
  { key: "status", label: "Δ", diffOnly: true },
  { key: "item", label: "Item" },
  { key: "qty", label: "Qty" },
  { key: "designators", label: "Designators" },
  { key: "value", label: "Value" },
  { key: "footprint", label: "Footprint" },
  { key: "mpn", label: "MPN" },
  { key: "dnp", label: "DNP" },
];

const STATUS_LABEL = { added: "Added", removed: "Removed", changed: "Changed" } as const;
const STATUS_ROLE = { added: "ok", removed: "err", changed: "warn" } as const;

export function BomTab() {
  const indexes = useDesignStore((s) => s.indexes);
  const selection = useSelectionStore((s) => s.selection);
  const setSelection = useSelectionStore((s) => s.setSelection);
  const setView = useViewStore((s) => s.setView);
  const bomChips = useViewStore((s) => s.bomChips);
  const toggleBomChip = useViewStore((s) => s.toggleBomChip);
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
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: "item", dir: 1 });
  const [flashKey, setFlashKey] = useState<string | null>(null);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);

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

  // Entering diff mode defaults to changes-first ("status"); leaving restores Item.
  // The user's own header clicks still win afterwards (this only fires on the flip).
  useEffect(() => {
    setSort(diffActive ? { key: "status", dir: 1 } : { key: "item", dir: 1 });
  }, [diffActive]);

  const changes = useMemo(
    () => (diffActive && diffDoc ? bomChanges(diffDoc.changes) : []),
    [diffActive, diffDoc],
  );

  const changeById = useMemo(() => new Map(changes.map((c) => [c.id, c])), [changes]);

  const rows = useMemo<DiffBomRow[]>(() => {
    if (!lines) return [];
    const decorated =
      diffActive && changes.length > 0
        ? decorateBomRows(lines, changes)
        : lines.map((line) => ({
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
    const filtered = decorated.filter((r) => {
      if (
        q &&
        ![r.line.designators.join(","), r.line.value, r.line.footprint, r.line.mpn]
          .join(" ")
          .toLowerCase()
          .includes(q)
      )
        return false;
      if (bomChips.dnpOnly && !r.line.dnp) return false;
      // The MPN cell falls back to the component index, so a row is only "missing"
      // when both the BOM line and the indexed part carry nothing.
      if (bomChips.missingMpn && (r.line.mpn || indexes?.components[r.line.designators[0] ?? ""]?.mpn))
        return false;
      if (changedOnly && !r.status) return false;
      return true;
    });
    const { key, dir } = sort;
    return [...filtered].sort((a, b) => {
      if (key === "status") return changesFirstCompare(a, b) * dir;
      const va = key === "designators" ? a.line.designators[0] ?? "" : a.line[key];
      const vb = key === "designators" ? b.line.designators[0] ?? "" : b.line[key];
      if (typeof va === "number" && typeof vb === "number") return (va - vb) * dir;
      if (typeof va === "boolean" && typeof vb === "boolean")
        return (Number(va) - Number(vb)) * dir;
      return String(va).localeCompare(String(vb), undefined, { numeric: true }) * dir;
    });
  }, [lines, changes, diffActive, filter, sort, bomChips, indexes]);

  // Open/unaddressed BOM comments keyed by their anchored designator, scoped to the
  // active review session (mirrors the canvas chip filter in CommentBridge: resolved and
  // dismissed comments carry no marker). A row shows the marker for the first of its
  // designators that carries one.
  const commentByRef = useMemo(() => {
    const numbers = numberMap(comments);
    const m = new Map<string, { id: string; number: number; status: DisplayStatus }>();
    for (const c of comments) {
      if (c.view !== "bom" || c.anchor.type !== "component") continue;
      if (activeSessionId !== null && c.session_id !== activeSessionId) continue;
      const status = displayInfo(c, indexes ?? null).status;
      if (status === "resolved" || status === "dismissed") continue;
      const number = numbers.get(c.id) ?? 0;
      const prev = m.get(c.anchor.ref);
      if (!prev || number < prev.number) m.set(c.anchor.ref, { id: c.id, number, status });
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

  // Stepper landing (bomNav): scroll the row into view and flash it. Registered
  // while mounted; a flash requested during the view switch is queued by the bridge.
  const rowRefs = useRef<Map<string, HTMLTableRowElement>>(new Map());
  const flashTimer = useRef<number | null>(null);
  useEffect(() => {
    const unregister = bomNav.register((key) => {
      setFlashKey(null); // restart the CSS animation even for the same row
      requestAnimationFrame(() => {
        setFlashKey(key);
        rowRefs.current.get(key)?.scrollIntoView({ block: "center" });
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
  useEffect(() => {
    if (selection?.kind !== "comp" || typeof selection.ref !== "string") return;
    const row = rowRefs.current.get(selection.ref);
    if (!row) return;
    const scroller = row.closest(".bom-scroll");
    if (scroller) {
      const r = row.getBoundingClientRect();
      const s = scroller.getBoundingClientRect();
      if (r.top >= s.top && r.bottom <= s.bottom) return;
    }
    row.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [selection]);

  function clickHeader(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === 1 ? -1 : 1 } : { key, dir: 1 }));
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

  /** Plain-text value of one cell, taken from the row data (not the DOM) so diff
   *  decoration / chips / badges don't leak into the clipboard. */
  function cellText(r: DiffBomRow, key: SortKey): string {
    const l = r.line;
    switch (key) {
      case "item":
        return r.synthetic ? "—" : String(l.item);
      case "qty":
        return String(l.qty);
      case "designators":
        return l.designators.join(", ");
      case "mpn":
        return l.mpn || indexes?.components[l.designators[0] ?? ""]?.mpn || "";
      case "dnp":
        return l.dnp ? "DNP" : "";
      case "value":
        return l.value;
      case "footprint":
        return l.footprint;
      default:
        return "";
    }
  }

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
  function openCellMenu(e: React.MouseEvent, r: DiffBomRow, key: SortKey) {
    e.preventDefault();
    e.stopPropagation();
    const copyKeys = cols.filter((c) => c.key !== "status").map((c) => c.key);
    setCtxMenu({
      x: e.clientX,
      y: e.clientY,
      items: [
        {
          label: "Copy cell",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(cellText(r, key)),
        },
        {
          label: "Copy row",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(copyKeys.map((k) => cellText(r, k)).join("\t")),
        },
        {
          label: "Copy column",
          icon: <IconCopy size={14} />,
          onClick: () => void copyText(rows.map((row) => cellText(row, key)).join("\n")),
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

  const cols = COLS.filter((c) => !c.diffOnly || diffActive);
  const changedCount = rows.filter((r) => r.status).length;

  return (
    <div className="bom-tab">
      <div className="bom-bar">
        <input
          className="bom-filter"
          placeholder="Filter — designator, value, footprint, MPN"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          spellCheck={false}
        />
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
      {error && <div className="bom-empty">BOM unavailable: {error}</div>}
      {!error && lines && lines.length === 0 && (
        <div className="bom-empty">The crunched bundle has no BOM.</div>
      )}
      <div className="bom-scroll">
        <table className={`bom-table ${armed ? "arming" : ""}`}>
          <thead>
            <tr>
              <th className="bom-cmt-th" aria-hidden />
              {cols.map((c) => (
                <th key={c.key} onClick={() => clickHeader(c.key)}>
                  {c.label}
                  {sort.key === c.key && (
                    <span className="bom-sort-arrow">{sort.dir === 1 ? " ▲" : " ▼"}</span>
                  )}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => {
              const l = r.line;
              const first = l.designators[0];
              const active =
                selection?.kind === "comp" &&
                typeof selection.ref === "string" &&
                l.designators.includes(selection.ref);
              const statusCls = r.status ? ` bom-${r.status}` : "";
              const flash =
                flashKey !== null && flashKey !== "" && (flashKey === r.key || flashKey === first)
                  ? " bom-flash"
                  : "";
              const cmt = rowComment(l);
              // Inline old → new, so a "changed" row reads without opening the panel.
              const old =
                r.status === "changed"
                  ? bomOldValues(r.changeIds.map((id) => changeById.get(id)).filter(isChange))
                  : {};
              return (
                <tr
                  key={r.synthetic ? `removed-${r.key}` : l.item}
                  ref={(el) => {
                    // Register under the diff row key (stepper landing) and the first
                    // designator (review-comment landing); keys never collide — the diff
                    // key is a value/footprint/mpn hash, not a designator.
                    for (const k of [r.key, first]) {
                      if (!k) continue;
                      if (el) rowRefs.current.set(k, el);
                      else rowRefs.current.delete(k);
                    }
                  }}
                  className={`${active ? "active" : ""}${l.dnp ? " dnp" : ""}${statusCls}${flash}`}
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
                    {cmt ? (
                      <button
                        className={`bom-cmt-badge st-${cmt.status}`}
                        title="Open the review comment on this line"
                        onClick={(e) => openRowThread(cmt.id, e)}
                      >
                        {cmt.number}
                      </button>
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
                  {diffActive && (
                    <td className="bom-status-cell">
                      {r.status && (
                        <span className={`bom-status bom-status-${STATUS_ROLE[r.status]}`}>
                          {STATUS_LABEL[r.status]}
                        </span>
                      )}
                    </td>
                  )}
                  <td className="mono dim" onContextMenu={(e) => openCellMenu(e, r, "item")}>
                    {r.synthetic ? "—" : l.item}
                  </td>
                  <td className="mono" onContextMenu={(e) => openCellMenu(e, r, "qty")}>
                    {wasCell(old.qty, l.qty)}
                  </td>
                  <td className="bom-dsg" onContextMenu={(e) => openCellMenu(e, r, "designators")}>
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
                  </td>
                  <td onContextMenu={(e) => openCellMenu(e, r, "value")}>
                    {wasCell(old.value, l.value)}
                  </td>
                  <td className="dim" onContextMenu={(e) => openCellMenu(e, r, "footprint")}>
                    {wasCell(old.footprint, l.footprint)}
                  </td>
                  <td className="mono" onContextMenu={(e) => openCellMenu(e, r, "mpn")}>
                    {wasCell(old.mpn, l.mpn || indexes?.components[first ?? ""]?.mpn || "")}
                  </td>
                  <td onContextMenu={(e) => openCellMenu(e, r, "dnp")}>{l.dnp ? "DNP" : ""}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {ctxMenu && <ContextMenu {...ctxMenu} onClose={() => setCtxMenu(null)} />}
    </div>
  );
}
