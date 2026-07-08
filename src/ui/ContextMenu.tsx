import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { HL_PALETTE } from "../stores/selectionStore";

/** One row in a right-click menu. A bare `{}` (or `separator`) renders a divider.
 *  `submenu` opens a nested menu on hover; `colorPicker` renders the highlight
 *  swatch row (the 6 compare colors + a native custom-color input). `icon` is the
 *  leading glyph — every action row carries one so the menu reads at a glance. */
export interface MenuItem {
  label?: string;
  icon?: ReactNode;
  /** A CSS colour: renders a small filled square as the leading glyph (used by the
   *  severity submenu so each level reads by its colour). Takes the icon slot. */
  swatch?: string;
  /** Marks the currently-selected choice in a submenu — the row is highlighted and
   *  carries a trailing check so the active option is obvious at a glance. */
  active?: boolean;
  separator?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  submenu?: MenuItem[];
  colorPicker?: { onPick: (color: string) => void };
}

interface Props {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}

/** Floating right-click menu: fixed-position, clamped into the viewport, dismissed
 *  by click-outside / Escape / any action. Shared by the schematic canvas and the
 *  PCB view so both get the same highlight-in-color + copy + fit actions. */
export function ContextMenu({ x, y, items, onClose }: Props) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });

  // Keep the menu on-screen: nudge up/left when it would overflow the window.
  useLayoutEffect(() => {
    const el = menuRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let nx = x;
    let ny = y;
    if (x + r.width > window.innerWidth) nx = Math.max(4, window.innerWidth - r.width - 4);
    if (y + r.height > window.innerHeight) ny = Math.max(4, window.innerHeight - r.height - 4);
    if (nx !== pos.x || ny !== pos.y) setPos({ x: nx, y: ny });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [x, y]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  return (
    <div
      className="ctx-backdrop"
      onPointerDown={onClose}
      onContextMenu={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      <div
        ref={menuRef}
        className="ctx-menu"
        style={{ left: pos.x, top: pos.y }}
        onPointerDown={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.preventDefault()}
      >
        <MenuList items={items} onClose={onClose} />
      </div>
    </div>
  );
}

function MenuList({ items, onClose }: { items: MenuItem[]; onClose: () => void }) {
  const [openSub, setOpenSub] = useState<number | null>(null);
  return (
    <>
      {items.map((it, i) =>
        it.separator || (!it.label && !it.colorPicker) ? (
          <div key={i} className="ctx-sep" />
        ) : it.colorPicker ? (
          <div key={i} className="ctx-colors">
            {HL_PALETTE.map((c) => (
              <button
                key={c}
                className="ctx-swatch"
                style={{ background: c }}
                title={`Highlight in ${c}`}
                onClick={() => {
                  it.colorPicker!.onPick(c);
                  onClose();
                }}
              />
            ))}
            <label className="ctx-swatch ctx-swatch-custom" title="Custom color…">
              +
              <input
                type="color"
                onChange={(e) => {
                  it.colorPicker!.onPick(e.target.value);
                  onClose();
                }}
              />
            </label>
          </div>
        ) : (
          <div
            key={i}
            className={`ctx-item ${it.disabled ? "disabled" : ""} ${it.submenu ? "has-sub" : ""} ${it.active ? "active" : ""}`}
            onMouseEnter={() => setOpenSub(it.submenu ? i : null)}
            onClick={() => {
              if (it.disabled || it.submenu) return;
              it.onClick?.();
              onClose();
            }}
          >
            <span className="ctx-icon">
              {it.swatch ? (
                <span className="ctx-swatch-mini" style={{ background: it.swatch }} />
              ) : (
                it.icon
              )}
            </span>
            <span className="ctx-label">{it.label}</span>
            {it.active && !it.submenu && <span className="ctx-check">✓</span>}
            {it.submenu && <span className="ctx-arrow">▸</span>}
            {it.submenu && openSub === i && (
              <div className="ctx-sub">
                <MenuList items={it.submenu} onClose={onClose} />
              </div>
            )}
          </div>
        ),
      )}
    </>
  );
}
