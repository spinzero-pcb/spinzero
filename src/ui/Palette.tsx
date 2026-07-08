import { useEffect, useMemo, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { useDesignStore } from "../stores/designStore";
import { useProjectStore } from "../stores/projectStore";
import { useShellStore } from "../stores/shellStore";
import { useViewStore } from "../stores/viewStore";
import { useMeasureStore } from "../stores/measureStore";
import { useReviewStore } from "../stores/reviewStore";
import { nav } from "./canvas/navigator";

// Ctrl+P palette (WS5). Plain query = find nets/components in the in-memory design
// indexes (works in vault AND dev-cache mode, where the SQLite FTS doesn't exist);
// `>` prefix = command mode.

interface Hit {
  kind: "net" | "comp" | "command";
  ref: string;
  detail: string;
  run: () => void;
}

const MAX_HITS = 40;

/** startsWith beats word-boundary beats substring; ties break on shorter ref. */
function score(ref: string, q: string): number {
  const r = ref.toLowerCase();
  if (r === q) return 0;
  if (r.startsWith(q)) return 1;
  const i = r.indexOf(q);
  if (i < 0) return -1;
  return /[a-z0-9]/.test(r[i - 1] ?? "/") ? 3 : 2;
}

export function Palette({ onClose, initial = "" }: { onClose: () => void; initial?: string }) {
  const indexes = useDesignStore((s) => s.indexes);
  const loadDesign = useDesignStore((s) => s.load);
  const openProject = useProjectStore((s) => s.openProject);
  const recents = useProjectStore((s) => s.recents);
  const openWizard = useShellStore((s) => s.openWizard);
  const openExisting = useShellStore((s) => s.openExisting);
  const setView = useViewStore((s) => s.setView);
  const [query, setQuery] = useState(initial);
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  const hits = useMemo<Hit[]>(() => {
    const goSchematic = () => setView("schematic");
    if (query.startsWith(">")) {
      const q = query.slice(1).trim().toLowerCase();
      const commands: Hit[] = [
        {
          kind: "command",
          ref: "Reload design",
          detail: "re-read the crunched bundle",
          run: () => void loadDesign(),
        },
        {
          kind: "command",
          ref: "Extract Now",
          detail: "re-import the design from source files",
          run: () => void ipc.crunchNow().catch(() => {}),
        },
        {
          kind: "command",
          ref: "Rebuild index",
          detail: "drop and refill the search index from extractions",
          run: () => void ipc.rebuildIndex().catch(() => {}),
        },
        {
          kind: "command",
          ref: "Measure tool",
          detail: "measure distances on the PCB (Ctrl+Shift+M)",
          run: () => {
            setView("pcb");
            useReviewStore.getState().arm(false); // measure ⇄ comment are exclusive
            useMeasureStore.getState().setActive(true);
          },
        },
        {
          kind: "command",
          ref: "New project…",
          detail: "create a project from a design folder",
          run: () => openWizard(),
        },
        {
          kind: "command",
          ref: "Open project…",
          detail: "open another project folder",
          run: () => void openExisting(),
        },
        // File → Open Recent (item 11): previously opened projects.
        ...recents.map<Hit>((r) => ({
          kind: "command",
          ref: `Open recent: ${r.split(/[\\/]/).filter(Boolean).pop() ?? r}`,
          detail: r,
          run: () => void openProject(r).catch(() => {}),
        })),
      ];
      return commands.filter((c) => c.ref.toLowerCase().includes(q));
    }

    const q = query.trim().toLowerCase();
    if (!q || !indexes) return [];
    const out: { hit: Hit; s: number }[] = [];
    for (const [name, n] of Object.entries(indexes.nets)) {
      const s = score(name, q);
      if (s < 0) continue;
      out.push({
        s,
        hit: {
          kind: "net",
          ref: name,
          detail: `${n.class} · ${n.terminals.length} pins · ${n.sheets.length} sheet${n.sheets.length === 1 ? "" : "s"}`,
          run: () => {
            goSchematic();
            nav.goNet(name);
          },
        },
      });
    }
    for (const [dsg, c] of Object.entries(indexes.components)) {
      let s = score(dsg, q);
      if (s < 0 && c.value && score(c.value, q) >= 0) s = score(c.value, q) + 4;
      if (s < 0 && c.mpn && score(c.mpn, q) >= 0) s = score(c.mpn, q) + 4;
      if (s < 0) continue;
      out.push({
        s,
        hit: {
          kind: "comp",
          ref: dsg,
          detail: [c.value, c.mpn].filter(Boolean).join(" · "),
          run: () => {
            goSchematic();
            nav.goComp(dsg);
          },
        },
      });
    }
    out.sort((a, b) => a.s - b.s || a.hit.ref.length - b.hit.ref.length);
    return out.slice(0, MAX_HITS).map((o) => o.hit);
  }, [query, indexes, loadDesign, openProject, openWizard, openExisting, recents, setView]);

  useEffect(() => setSel(0), [query]);
  useEffect(() => {
    listRef.current
      ?.querySelector(".palette-item.sel")
      ?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  function onKeyDown(e: React.KeyboardEvent) {
    e.stopPropagation(); // keep the app keymap out while typing
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, hits.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter" && hits[sel]) {
      hits[sel].run();
      onClose();
    }
  }

  return (
    <div className="palette-backdrop" onPointerDown={onClose}>
      <div className="palette" onPointerDown={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Search nets and components — > for commands"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          spellCheck={false}
        />
        <div className="palette-list" ref={listRef}>
          {hits.map((h, i) => (
            <button
              key={h.kind + h.ref}
              className={`palette-item ${i === sel ? "sel" : ""}`}
              onClick={() => {
                h.run();
                onClose();
              }}
              onPointerMove={() => setSel(i)}
            >
              <span className={`palette-kind ${h.kind}`}>
                {h.kind === "net" ? "N" : h.kind === "comp" ? "C" : "›"}
              </span>
              <span className="palette-ref mono">{h.ref}</span>
              <span className="palette-detail">{h.detail}</span>
            </button>
          ))}
          {hits.length === 0 && query && (
            <div className="palette-none">no matches</div>
          )}
        </div>
      </div>
    </div>
  );
}
