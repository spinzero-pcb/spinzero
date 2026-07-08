import { Fragment, useEffect, useRef, useState } from "react";
import { useDesignStore } from "../stores/designStore";
import { useSelectionStore } from "../stores/selectionStore";
import { layerColorVar, usePcbViewStore } from "../stores/pcbViewStore";
import { useViewStore } from "../stores/viewStore";
import { nav, pcbNav } from "./canvas/navigator";

/** Floating, draggable info panel (phase-2 §3, consolidated): the single surface for
 *  a selection. It opens next to where you clicked (the selection store's `anchor`),
 *  can be dragged anywhere, and carries the cross-view navigation — schematic sheets
 *  and PCB layers in two side-by-side columns — that used to live as separate
 *  on-canvas chips. Reflects the selection store; click-pivots drive the canvas via
 *  the nav bridge. */
const W = 340; // panel width (keep in sync with .props-card width)

/** Lay out cross-view destinations as side-by-side chip groups (e.g. Sheets |
 *  Layers). Each group wraps its chips, capped at 4 per row via a flex break, so
 *  a net on many sheets fills extra rows instead of one cramped line. */
function ChipRail({
  groups,
}: {
  groups: { label: string; empty?: string; chips: React.ReactNode[] }[];
}) {
  return (
    <div className="xnav">
      {groups.map((g, gi) => (
        <Fragment key={gi}>
          {gi > 0 && <div className="xnav-div" aria-hidden />}
          <div className="xnav-group">
            <div className="xnav-label">{g.label}</div>
            {g.chips.length ? (
              <div className="xnav-chips">
                {g.chips.map((chip, i) => (
                  <Fragment key={i}>
                    {chip}
                    {(i + 1) % 4 === 0 && i < g.chips.length - 1 && (
                      <span className="xnav-break" aria-hidden />
                    )}
                  </Fragment>
                ))}
              </div>
            ) : (
              <div className="xnav-empty">{g.empty ?? "—"}</div>
            )}
          </div>
        </Fragment>
      ))}
    </div>
  );
}

export function PropertiesCard() {
  const selection = useSelectionStore((s) => s.selection);
  const highlights = useSelectionStore((s) => s.highlights);
  const setSelection = useSelectionStore((s) => s.setSelection);
  const currentSheet = useSelectionStore((s) => s.currentSheet);
  const anchor = useSelectionStore((s) => s.anchor);
  const indexes = useDesignStore((s) => s.indexes);
  const pcbIndex = useDesignStore((s) => s.pcbIndex);
  const setView = useViewStore((s) => s.setView);

  const cardRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ dx: number; dy: number } | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  // Re-place next to the selection whenever a fresh click sets a new anchor. A
  // cross-probe (no anchor) parks the panel in the top-right corner.
  useEffect(() => {
    const clamp = (x: number, y: number) => ({
      x: Math.min(Math.max(8, x), window.innerWidth - W - 8),
      y: Math.min(Math.max(48, y), window.innerHeight - 120),
    });
    if (anchor) setPos(clamp(anchor.x + 18, anchor.y - 24));
    else setPos(clamp(window.innerWidth - W - 16, 64));
  }, [anchor]);

  if (!selection || !indexes) return null;
  const sheetName = (n: number) => indexes.sheets.find((s) => s.num === n)?.name ?? String(n);

  // ----- drag (header is the handle) -----
  function onPointerDown(e: React.PointerEvent) {
    const card = cardRef.current;
    if (!card) return;
    const r = card.getBoundingClientRect();
    drag.current = { dx: e.clientX - r.left, dy: e.clientY - r.top };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  }
  function onPointerMove(e: React.PointerEvent) {
    if (!drag.current) return;
    const w = cardRef.current?.offsetWidth ?? W;
    const h = cardRef.current?.offsetHeight ?? 200;
    setPos({
      x: Math.min(Math.max(8, e.clientX - drag.current.dx), window.innerWidth - w - 8),
      y: Math.min(Math.max(8, e.clientY - drag.current.dy), window.innerHeight - h - 8),
    });
  }
  function onPointerUp(e: React.PointerEvent) {
    drag.current = null;
    (e.currentTarget as Element).releasePointerCapture?.(e.pointerId);
  }

  /** Open the PCB view focused on the current selection, optionally activating a layer. */
  function goPcb(layer?: string) {
    if (layer) {
      const pv = usePcbViewStore.getState();
      pv.showLayer(layer);
      pv.setActive(layer);
    }
    setView("pcb");
    // Center the PCB camera on the selection — without this the net/part is only in
    // frame if the camera happened to be there already (unlike the view-tab handler,
    // which reveals). Mirrors App.tsx's schematic→PCB cross-probe.
    if (selection) {
      const anchor =
        selection.kind === "net"
          ? { type: "net" as const, ref: selection.ref }
          : {
              type: "component" as const,
              ref: selection.kind === "pin" ? selection.ref.designator : selection.ref,
            };
      pcbNav.reveal(anchor);
    }
  }
  function goSchematicNet(name: string, sheet?: number) {
    setView("schematic");
    if (sheet != null && sheet !== currentSheet) nav.jumpToNet(name, sheet);
    else nav.goNet(name);
  }
  function goSchematicComp(dsg: string) {
    setView("schematic");
    nav.goComp(dsg);
  }

  // Color chips for the active highlight set; click focuses a member in the card.
  const chips =
    highlights.length > 1 ? (
      <div className="chips">
        {highlights.map((h) => (
          <button
            key={h.kind + h.ref}
            className={`chip ${selection.kind === h.kind && selection.ref === h.ref ? "on" : ""}`}
            onClick={() => setSelection({ kind: h.kind, ref: h.ref })}
            title={h.ref}
          >
            <span className="dot" style={{ background: h.color }} />
            {h.ref}
            <span
              className="x"
              title="Remove from selection"
              onClick={(e) => {
                e.stopPropagation();
                nav.toggleHighlight(h.kind, h.ref);
              }}
            >
              ×
            </span>
          </button>
        ))}
      </div>
    ) : null;

  // Common chrome: a draggable header + the floating shell. `title`/`sub` vary by kind.
  const shell = (title: React.ReactNode, sub: string, body: React.ReactNode) => (
    <div
      ref={cardRef}
      className="props-card"
      style={pos ? { left: pos.x, top: pos.y } : undefined}
    >
      <div className="props-head" onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp}>
        <span className="props-grip" aria-hidden>⋮⋮</span>
        <div className="props-title">{title}</div>
        <span className="props-close" title="Clear selection" onPointerDown={(e) => e.stopPropagation()} onClick={() => setSelection(null)}>×</span>
      </div>
      <div className="props-sub">{sub}</div>
      <div className="props-body">
        {chips}
        {body}
      </div>
    </div>
  );

  if (selection.kind === "net") {
    const n = indexes.nets[selection.ref];
    if (!n) return null;
    const pn = pcbIndex?.nets[selection.ref];
    // Class only when meaningful — "Default" is noise (and was the symptom of the
    // old, broken net-class resolution).
    const classLabel = n.class && n.class !== "Default" ? ` · ${n.class}` : "";
    const sheetCount = n.sheets.length;
    return shell(
      <h1 className="net mono">{selection.ref}</h1>,
      `net · ${sheetCount} sheet${sheetCount === 1 ? "" : "s"}${classLabel}`,
      <>
        {/* Cross-view nav as a chip rail: schematic sheets and PCB layers side by
            side (not stacked), each row capped at 4 chips and wrapping below. */}
        <ChipRail
          groups={[
            {
              label: `Sheets${sheetCount > 1 ? ` · ${sheetCount}` : ""}`,
              empty: "not on any sheet",
              chips: n.sheets.map((x) => (
                <button
                  key={x}
                  className={`xchip${x === currentSheet ? " cur" : ""}`}
                  onClick={() => goSchematicNet(selection.ref, x)}
                  title={x === currentSheet ? `${sheetName(x)} (current)` : sheetName(x)}
                >
                  <span className="txt">{sheetName(x)}</span>
                  {x === currentSheet && <span className="cur-dot" aria-hidden />}
                </button>
              )),
            },
            {
              label: `Layers${pn && pn.layers.length > 1 ? ` · ${pn.layers.length}` : ""}`,
              empty: "not routed",
              chips: (pn?.layers ?? []).map((l) => (
                <button key={l} className="xchip" onClick={() => goPcb(l)} title={l}>
                  <span className="layer-dot" style={{ background: layerColorVar(l) }} />
                  <span className="txt">{l}</span>
                </button>
              )),
            },
          ]}
        />
        {pn && (pn.widths.length > 0 || pn.vias > 0) && (
          <div className="props-stats">
            {pn.widths.length > 0 && (
              <span className="stat">
                <span className="k">width</span>
                {pn.widths.length === 1
                  ? `${pn.widths[0]}mm`
                  : `${pn.widths[0]}–${pn.widths[pn.widths.length - 1]}mm`}
              </span>
            )}
            {pn.vias > 0 && (
              <span className="stat">
                <span className="k">vias</span>
                {pn.vias}
              </span>
            )}
          </div>
        )}
      </>,
    );
  }

  if (selection.kind === "comp") {
    const c = indexes.components[selection.ref];
    if (!c) return null;
    const side = pcbIndex?.compSide[selection.ref];
    const kv = (k: string, v: string) =>
      v ? (
        <div className="kv">
          <span className="k">{k}</span>
          <span className="v">{v}</span>
        </div>
      ) : null;
    return shell(
      <h1 className="comp mono">{selection.ref}</h1>,
      `component · ${c.sheet != null ? sheetName(c.sheet) : "—"}${c.dnp ? " · DNP" : ""}`,
      <>
        {kv("Value", c.value)}
        {kv("MPN", c.mpn)}
        {kv("Mfr", c.mfr)}
        {kv("Footprint", c.fp)}
        {kv("Desc", c.desc)}
        <ChipRail
          groups={[
            {
              label: `Nets · ${c.nets.length}`,
              empty: "no nets",
              chips: c.nets.map((nn) => (
                <button key={nn} className="xchip" onClick={() => goSchematicNet(nn)} title={nn}>
                  <span className="txt">{nn}</span>
                </button>
              )),
            },
            {
              label: "PCB",
              empty: "not placed",
              chips: side
                ? [
                    <button
                      key="side"
                      className="xchip"
                      onClick={() => goPcb(side === "back" ? "B.Cu" : "F.Cu")}
                      title="Open on the board"
                    >
                      <span className="txt">
                        {side === "front" ? "Front" : side === "back" ? "Back" : "Front + back"}
                      </span>
                    </button>,
                  ]
                : [],
            },
          ]}
        />
      </>,
    );
  }

  // pin
  const { designator, pin } = selection.ref;
  const c = indexes.components[designator];
  let netName = "—";
  let pinType = "";
  let pinName = "";
  for (const [name, n] of Object.entries(indexes.nets)) {
    const t = n.terminals.find((t) => t.d === designator && t.p === pin);
    if (t) {
      netName = name;
      pinType = t.pt;
      pinName = t.pn;
      break;
    }
  }
  return shell(
    <h1 className="mono">
      {designator}.{pin}
    </h1>,
    `pin${pinType ? ` · ${pinType}` : ""}${pinName ? ` · ${pinName}` : ""}`,
    <ChipRail
      groups={[
        {
          label: "On net",
          empty: "no net",
          chips:
            netName !== "—"
              ? [
                  <button key="net" className="xchip" onClick={() => goSchematicNet(netName)} title={netName}>
                    <span className="txt">{netName}</span>
                  </button>,
                ]
              : [],
        },
        {
          label: "Component",
          empty: "—",
          chips: [
            <button
              key="comp"
              className="xchip"
              onClick={() => goSchematicComp(designator)}
              title={`${designator} ${c?.value ?? ""}`}
            >
              <span className="txt">
                {designator}
                {c?.value ? ` · ${c.value}` : ""}
              </span>
            </button>,
          ],
        },
      ]}
    />,
  );
}
