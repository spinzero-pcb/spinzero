import { useMemo, useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import { useNetClassStore } from "../../stores/netClassStore";
import { listNetClasses } from "../../lib/netClasses";

// PCB "Net Classes" panel (right sidebar). Click a class to highlight its nets in
// the render — in the nets' own PCB layer colours unless a colour is picked here —
// and isolate the copper layers those nets run on. Each class expands to its net
// names (collapsed by default) so single nets can be toggled and coloured on their
// own. Multiple classes can be active at once; a header "Clear" drops them all and
// restores the pre-isolation layer view. Pure store consumer, like the Appearance
// panel above it.

/** Swatch + native colour input. Empty (dashed) until a colour is picked; the ×
 *  clears it, putting the net/class back on the PCB layer colours. */
function ColorPick({
  color,
  onPick,
  label,
}: {
  color: string | undefined;
  onPick: (c: string | null) => void;
  label: string;
}) {
  return (
    <span className="pcb-nc-color" onClick={(e) => e.stopPropagation()}>
      <label
        className={`pcb-nc-swatch ${color ? "" : "empty"}`}
        style={color ? { background: color } : undefined}
        title={color ? `${label}: ${color} — click to change` : `${label}: pick a colour`}
      >
        <input
          type="color"
          value={color ?? "#4f8cff"}
          onChange={(e) => onPick(e.target.value)}
        />
      </label>
      {color && (
        <button
          className="pcb-nc-colorclear"
          title={`${label}: use PCB layer colours`}
          onClick={() => onPick(null)}
        >
          ×
        </button>
      )}
    </span>
  );
}

export function NetClassPanel() {
  const indexes = useDesignStore((s) => s.indexes);
  const selected = useNetClassStore((s) => s.selected);
  const classColors = useNetClassStore((s) => s.classColors);
  const netOverride = useNetClassStore((s) => s.netOverride);
  const netColors = useNetClassStore((s) => s.netColors);
  const toggle = useNetClassStore((s) => s.toggle);
  const toggleNet = useNetClassStore((s) => s.toggleNet);
  const setClassColor = useNetClassStore((s) => s.setClassColor);
  const setNetColor = useNetClassStore((s) => s.setNetColor);
  const clear = useNetClassStore((s) => s.clear);

  // Which classes have their net list expanded — view-only, so local state.
  const [expanded, setExpanded] = useState<string[]>([]);

  const classes = useMemo(() => listNetClasses(indexes), [indexes]);

  if (!classes.length)
    return <div className="panel-empty">No net classes in this design.</div>;

  return (
    <div className="pcb-netclasses">
      <h2>Net Classes</h2>
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
      <div className="pcb-scrolllist">
      {classes.map((c) => {
        const on = selected.includes(c.name);
        const open = expanded.includes(c.name);
        return (
          <div key={c.name}>
            <div
              className={`pcb-layerrow pcb-ncrow ${on ? "on" : ""}`}
              onClick={() => toggle(c.name)}
              title={
                on
                  ? `${c.name} highlighted — click to clear`
                  : `highlight ${c.name} (${c.nets.length} net${c.nets.length === 1 ? "" : "s"})`
              }
            >
              <button
                className={`pcb-nc-caret ${open ? "open" : ""}`}
                title={open ? `hide ${c.name} nets` : `show ${c.name} nets`}
                onClick={(e) => {
                  e.stopPropagation();
                  setExpanded((xs) =>
                    xs.includes(c.name) ? xs.filter((n) => n !== c.name) : [...xs, c.name],
                  );
                }}
              >
                ▸
              </button>
              <input
                type="checkbox"
                checked={on}
                onClick={(e) => e.stopPropagation()}
                onChange={() => toggle(c.name)}
                title={`highlight ${c.name}`}
              />
              <span className="tree-name">{c.name}</span>
              <ColorPick
                color={classColors[c.name]}
                onPick={(col) => setClassColor(c.name, col)}
                label={c.name}
              />
              <span className="dim pcb-nc-count">{c.nets.length}</span>
            </div>
            {open &&
              c.nets.map((net) => {
                const netOn = netOverride[net] ?? on;
                return (
                  <div
                    key={net}
                    className={`pcb-layerrow pcb-ncrow pcb-ncnet ${netOn ? "on" : ""}`}
                    onClick={() => toggleNet(net)}
                    title={netOn ? `${net} highlighted — click to clear` : `highlight ${net}`}
                  >
                    <input
                      type="checkbox"
                      checked={netOn}
                      onClick={(e) => e.stopPropagation()}
                      onChange={() => toggleNet(net)}
                      title={`highlight ${net}`}
                    />
                    <span className="tree-name">{net}</span>
                    <ColorPick
                      color={netColors[net]}
                      onPick={(col) => setNetColor(net, col)}
                      label={net}
                    />
                  </div>
                );
              })}
          </div>
        );
      })}
      </div>
    </div>
  );
}
