import { useMemo } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useNetClassStore } from "../../stores/netClassStore";
import { listNetClasses, netClassColor } from "../../lib/netClasses";

// PCB "Net Classes" panel (right sidebar). Click a class to highlight its nets in
// the render — each class keeps its own colour — and isolate the copper layers those
// nets run on. Multiple classes can be active at once; a header "Clear" drops them
// all and restores the pre-isolation layer view. Pure store consumer, like the
// Appearance panel above it.
export function NetClassPanel() {
  const indexes = useDesignStore((s) => s.indexes);
  const selected = useNetClassStore((s) => s.selected);
  const toggle = useNetClassStore((s) => s.toggle);
  const clear = useNetClassStore((s) => s.clear);

  const classes = useMemo(() => listNetClasses(indexes), [indexes]);
  const ordered = useMemo(() => classes.map((c) => c.name), [classes]);

  if (!classes.length)
    return <div className="panel-empty">No net classes in this design.</div>;

  return (
    <div className="pcb-netclasses">
      {selected.length > 0 && (
        <div className="pcb-nc-head">
          <span className="dim">
            {selected.length} selected
          </span>
          <button className="btn-ghost pcb-nc-clear" onClick={() => clear()}>
            Clear
          </button>
        </div>
      )}
      {classes.map((c) => {
        const on = selected.includes(c.name);
        const color = netClassColor(c.name, ordered);
        return (
          <div
            key={c.name}
            className={`pcb-layerrow pcb-ncrow ${on ? "on" : ""}`}
            onClick={() => toggle(c.name)}
            title={
              on
                ? `${c.name} highlighted — click to clear`
                : `highlight ${c.name} (${c.nets.length} net${c.nets.length === 1 ? "" : "s"})`
            }
          >
            <input
              type="checkbox"
              checked={on}
              onClick={(e) => e.stopPropagation()}
              onChange={() => toggle(c.name)}
              title={`highlight ${c.name}`}
            />
            <span className="pcb-swatch" style={{ background: color }} />
            <span className="tree-name">{c.name}</span>
            <span className="dim pcb-nc-count">{c.nets.length}</span>
          </div>
        );
      })}
    </div>
  );
}
