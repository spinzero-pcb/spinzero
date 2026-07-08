import { useState } from "react";
import { useDesignStore } from "../../stores/designStore";
import type { LayerLite } from "../../lib/design";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { IconLayers } from "../icons";
import {
  PCB_OBJECT_KEYS,
  PCB_OBJECT_LABELS,
  isWorksheetLayer,
  layerColorVar,
  usePcbViewStore,
  type PcbObjectKey,
} from "../../stores/pcbViewStore";

// KiCad-style Appearance controls for the PCB view: layer visibility + an active
// layer painted on top (others translucent), object-class toggles + per-class
// opacity. Pure store consumer — the canvas reacts to the same store. Lives in the
// per-view RIGHT panel (docs/phase2-ui-plan.md §1); it used to be an <aside> inside
// PcbView.
// Stable empty array so the selector never returns a fresh `[]` (which would defeat
// zustand's Object.is bail-out and re-render the panel on every unrelated store change).
const EMPTY_LAYERS: LayerLite[] = [];

export function AppearancePanel() {
  const layers = useDesignStore((s) => s.indexes?.layers ?? EMPTY_LAYERS);
  const active = usePcbViewStore((s) => s.active);
  const hidden = usePcbViewStore((s) => s.hidden);
  const objects = usePcbViewStore((s) => s.objects);
  const opacity = usePcbViewStore((s) => s.opacity);
  const setActive = usePcbViewStore((s) => s.setActive);
  const toggleLayer = usePcbViewStore((s) => s.toggleLayer);
  const setObject = usePcbViewStore((s) => s.setObject);
  const setOpacity = usePcbViewStore((s) => s.setOpacity);
  const showAllLayers = usePcbViewStore((s) => s.showAllLayers);
  const setHidden = usePcbViewStore((s) => s.setHidden);

  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);

  // The drawing sheet is page context, not a board layer — it's always painted as a
  // background by the canvas, so it never appears as a toggleable/selectable row here.
  const boardLayers = layers.filter((l) => !isWorksheetLayer(l));

  if (!boardLayers.length)
    return <div className="panel-empty">No PCB layers in this design.</div>;

  // Right-click presets (replace the old Select-all/Deselect-all buttons). "but active"
  // keeps the active layer visible — or the right-clicked one when nothing is active.
  const allNames = boardLayers.map((l) => l.name);
  const cuNames = allNames.filter((n) => n.endsWith(".Cu"));
  const layerMenu = (clicked: string): MenuItem[] => {
    const keep = active ?? clicked;
    const layerIcon = <IconLayers size={14} />;
    return [
      {
        label: "Hide all layers but active",
        icon: layerIcon,
        onClick: () => {
          setHidden(allNames.filter((n) => n !== keep));
          if (!active) setActive(clicked);
        },
      },
      { label: "Hide all layers", icon: layerIcon, onClick: () => setHidden(allNames) },
      { label: "Show all layers", icon: layerIcon, onClick: () => showAllLayers() },
      { separator: true },
      { label: "Show only Cu layers", icon: layerIcon, onClick: () => setHidden(allNames.filter((n) => !n.endsWith(".Cu"))) },
      { label: "Hide all Cu layers", icon: layerIcon, onClick: () => setHidden([...hidden, ...cuNames]) },
    ];
  };

  return (
    <div className="pcb-appearance">
      <div className="pcb-layers-head">
        <h2>Layers</h2>
      </div>
      {boardLayers.map((l) => (
        <div
          key={l.name}
          className={`pcb-layerrow ${active === l.name ? "active" : ""}`}
          onClick={() => setActive(active === l.name ? null : l.name)}
          onContextMenu={(e) => {
            e.preventDefault();
            setMenu({ x: e.clientX, y: e.clientY, items: layerMenu(l.name) });
          }}
          title={
            active === l.name
              ? "active (painted on top) — click to restore board order — right-click for layer presets"
              : "click to paint this layer on top — right-click for layer presets"
          }
        >
          <input
            type="checkbox"
            checked={!hidden.has(l.name)}
            onClick={(e) => e.stopPropagation()}
            onChange={() => toggleLayer(l.name)}
            title={`show/hide ${l.name}`}
          />
          <span className="pcb-swatch" style={{ background: layerColorVar(l.name, l.color) }} />
          <span className="tree-name" title={l.user_name && l.user_name !== l.name ? l.name : undefined}>
            {l.user_name || l.name}
          </span>
        </div>
      ))}
      <h2>Objects</h2>
      {PCB_OBJECT_KEYS.map((key: PcbObjectKey) => (
        <div key={key} className="pcb-layerrow pcb-objrow">
          <input
            type="checkbox"
            checked={objects[key]}
            onChange={(e) => setObject(key, e.target.checked)}
          />
          <span className="pcb-objname">{PCB_OBJECT_LABELS[key]}</span>
          <input
            type="range"
            min={10}
            max={100}
            value={Math.round(opacity[key] * 100)}
            onChange={(e) => setOpacity(key, Number(e.target.value) / 100)}
            title={`${PCB_OBJECT_LABELS[key]} opacity`}
          />
        </div>
      ))}
      {menu && <ContextMenu {...menu} onClose={() => setMenu(null)} />}
    </div>
  );
}
