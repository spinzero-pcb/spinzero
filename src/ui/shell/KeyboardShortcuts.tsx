import { useEffect } from "react";
import { create } from "zustand";
import { SHORTCUTS, isMacPlatform, modToken, type ShortcutScope } from "../../lib/keymap";

// Keyboard Shortcuts cheat-sheet. Opened from Help → Keyboard Shortcuts or the "?"
// hotkey. The list is driven entirely by lib/keymap's SHORTCUTS table so it can
// never drift from the bindings resolveKey actually implements.

interface ShortcutsDialog {
  open: boolean;
  setOpen: (v: boolean) => void;
}
export const useShortcutsDialog = create<ShortcutsDialog>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));

const SCOPES: ShortcutScope[] = ["Global", "Schematic", "PCB"];

/** Render one combo (e.g. ["Mod","F"]) as <kbd> chips, "Mod" swapped per platform. */
function Combo({ tokens, mac }: { tokens: string[]; mac: boolean }) {
  return (
    <span className="kbd-combo">
      {tokens.map((t, i) => (
        <kbd key={i} className="kbd">
          {t === "Mod" ? modToken(mac) : t}
        </kbd>
      ))}
    </span>
  );
}

export function KeyboardShortcuts() {
  const open = useShortcutsDialog((s) => s.open);
  const setOpen = useShortcutsDialog((s) => s.setOpen);
  const mac = isMacPlatform();

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);

  if (!open) return null;

  return (
    <div
      className="wizard-overlay about-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && setOpen(false)}
    >
      <div className="about-card shortcuts-card" role="dialog" aria-label="Keyboard Shortcuts">
        <div className="about-name">Keyboard Shortcuts</div>
        {SCOPES.map((scope) => {
          const rows = SHORTCUTS.filter((s) => s.scope === scope);
          if (!rows.length) return null;
          return (
            <div key={scope} className="shortcuts-group">
              <div className="shortcuts-scope">{scope}</div>
              {rows.map((s) => (
                <div key={s.action} className="shortcuts-row">
                  <span className="shortcuts-action">{s.action}</span>
                  <span className="shortcuts-keys">
                    {s.combos.map((combo, i) => (
                      <span key={i} className="shortcuts-alt">
                        {i > 0 && <span className="shortcuts-or">or</span>}
                        <Combo tokens={combo} mac={mac} />
                      </span>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          );
        })}
        <p className="about-blurb dim shortcuts-note">
          Shortcuts follow the KiCad-style preset and aren’t customisable yet.
        </p>
        <div className="about-actions">
          <button className="btn-primary" onClick={() => setOpen(false)}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
