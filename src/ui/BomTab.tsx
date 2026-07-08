import { useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { useDesignStore } from "../stores/designStore";
import { useSelectionStore } from "../stores/selectionStore";
import { useViewStore } from "../stores/viewStore";
import { nav } from "./canvas/navigator";
import type { BomLine } from "../lib/types";

// WS7: BOM keeps a tab; it is not canvas content. Row click = select the line's
// first designator (card/status mirror it); double-click = jump to the symbol.

type SortKey = "item" | "qty" | "designators" | "value" | "footprint" | "mpn" | "dnp";

const COLS: { key: SortKey; label: string }[] = [
  { key: "item", label: "Item" },
  { key: "qty", label: "Qty" },
  { key: "designators", label: "Designators" },
  { key: "value", label: "Value" },
  { key: "footprint", label: "Footprint" },
  { key: "mpn", label: "MPN" },
  { key: "dnp", label: "DNP" },
];

export function BomTab() {
  const indexes = useDesignStore((s) => s.indexes);
  const selection = useSelectionStore((s) => s.selection);
  const setSelection = useSelectionStore((s) => s.setSelection);
  const setView = useViewStore((s) => s.setView);
  const [lines, setLines] = useState<BomLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: "item", dir: 1 });

  // Re-fetch per revision: `indexes` is replaced on every design reload.
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

  const rows = useMemo(() => {
    if (!lines) return [];
    const q = filter.trim().toLowerCase();
    const filtered = q
      ? lines.filter((l) =>
          [l.designators.join(","), l.value, l.footprint, l.mpn]
            .join(" ")
            .toLowerCase()
            .includes(q),
        )
      : [...lines];
    const { key, dir } = sort;
    filtered.sort((a, b) => {
      const va = key === "designators" ? a.designators[0] ?? "" : a[key];
      const vb = key === "designators" ? b.designators[0] ?? "" : b[key];
      if (typeof va === "number" && typeof vb === "number") return (va - vb) * dir;
      if (typeof va === "boolean" && typeof vb === "boolean")
        return (Number(va) - Number(vb)) * dir;
      return String(va).localeCompare(String(vb), undefined, { numeric: true }) * dir;
    });
    return filtered;
  }, [lines, filter, sort]);

  function clickHeader(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === 1 ? -1 : 1 } : { key, dir: 1 }));
  }

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
        <span className="bom-count">
          {rows.length}
          {lines && rows.length !== lines.length ? ` / ${lines.length}` : ""} lines
        </span>
      </div>
      {error && <div className="bom-empty">BOM unavailable: {error}</div>}
      {!error && lines && lines.length === 0 && (
        <div className="bom-empty">The crunched bundle has no BOM.</div>
      )}
      <div className="bom-scroll">
        <table className="bom-table">
          <thead>
            <tr>
              {COLS.map((c) => (
                <th key={c.key} onClick={() => clickHeader(c.key)}>
                  {c.label}
                  {sort.key === c.key && <span>{sort.dir === 1 ? " ▲" : " ▼"}</span>}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((l) => {
              const first = l.designators[0];
              const active =
                selection?.kind === "comp" &&
                typeof selection.ref === "string" &&
                l.designators.includes(selection.ref);
              return (
                <tr
                  key={l.item}
                  className={`${active ? "active" : ""}${l.dnp ? " dnp" : ""}`}
                  onClick={() => first && setSelection({ kind: "comp", ref: first })}
                  onDoubleClick={() => {
                    if (!first) return;
                    setView("schematic");
                    nav.goComp(first);
                  }}
                  title="click: select · double-click: jump to symbol"
                >
                  <td className="mono dim">{l.item}</td>
                  <td className="mono">{l.qty}</td>
                  <td className="mono bom-dsg">{l.designators.join(", ")}</td>
                  <td>{l.value}</td>
                  <td className="dim">{l.footprint}</td>
                  <td className="mono">{l.mpn || indexes?.components[first ?? ""]?.mpn || ""}</td>
                  <td>{l.dnp ? "DNP" : ""}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
