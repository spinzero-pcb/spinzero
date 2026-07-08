import { useMemo, useState } from "react";
import { useProjectStore } from "../../stores/projectStore";
import { useSelectionStore } from "../../stores/selectionStore";
import { useDesignStore } from "../../stores/designStore";
import { sheetMatches } from "../../lib/design";
import { useViewStore } from "../../stores/viewStore";
import { nav } from "../canvas/navigator";
import type { SheetInfo } from "../../lib/types";
import { IconChevron, IconSheet } from "../icons";

// The schematic sheet hierarchy (KiCad-style: nested sheets, page numbers). Moved
// off the LEFT Explorer into the per-view RIGHT panel as the `Sheets` tab — the
// left rail is now the global Review surface (docs/phase2-ui-plan.md §1).

interface SheetNode {
  sheet: SheetInfo;
  children: SheetNode[];
}

export function buildSheetTree(sheets: SheetInfo[]): SheetNode[] {
  const byPath = new Map<string, SheetNode>();
  for (const s of sheets) byPath.set(s.sheet_path, { sheet: s, children: [] });
  const roots: SheetNode[] = [];
  for (const node of byPath.values()) {
    const p = node.sheet.sheet_path;
    if (p === "/") {
      roots.unshift(node);
      continue;
    }
    // A sheet NAME can itself contain "/" (freedom's "User Input/Output" → path
    // "/User Input/Output/"), so strip the whole name rather than the last "/…/"
    // run — otherwise the slash forges a phantom parent and the node falls out to
    // the top level instead of nesting under Root.
    const suffix = `${node.sheet.name}/`;
    const parentPath = p.endsWith(suffix)
      ? p.slice(0, p.length - suffix.length)
      : p.replace(/[^/]+\/$/, "");
    const parent = byPath.get(parentPath);
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  const sortRec = (nodes: SheetNode[]) => {
    nodes.sort(compareSheets);
    nodes.forEach((n) => sortRec(n.children));
  };
  sortRec(roots);
  return roots;
}

/** Sort key for a sheet: KiCad's page number (the label the panel shows), parsed into
 *  numeric segments so hierarchical pages ("2", "2.1", "10") order naturally. Falls back
 *  to the sequential sheet number when the project uses automatic numbering (empty page).
 *  Sorting by page — the same value we display — keeps the panel in KiCad's page order
 *  instead of the extraction/DFS order (which can diverge when pages are renumbered). */
function pageKey(s: SheetInfo): number[] {
  if (!s.page) return [s.number];
  return s.page
    .split(/[.\-_]/)
    .map((seg) => {
      const n = parseInt(seg, 10);
      return Number.isNaN(n) ? 0 : n;
    });
}

function compareSheets(a: SheetNode, b: SheetNode): number {
  const ka = pageKey(a.sheet);
  const kb = pageKey(b.sheet);
  for (let i = 0; i < Math.max(ka.length, kb.length); i++) {
    const d = (ka[i] ?? 0) - (kb[i] ?? 0);
    if (d !== 0) return d;
  }
  return a.sheet.number - b.sheet.number; // stable tiebreak
}

function SheetBranch({ node, depth }: { node: SheetNode; depth: number }) {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  const currentSheet = useSelectionStore((s) => s.currentSheet);
  // Active check goes through filename matching: sidebar (manifest) numbering and
  // canvas (design JSON) numbering are different sequences.
  const designSheet = useDesignStore((s) =>
    s.indexes?.sheets.find((d) => d.num === currentSheet),
  );
  const active =
    !!designSheet &&
    sheetMatches(designSheet, {
      number: node.sheet.number,
      name: node.sheet.name,
      svg_path: node.sheet.svg_path,
    });
  return (
    <>
      <div className="tree-row" style={{ paddingLeft: 8 + depth * 14 }}>
        {hasChildren ? (
          <button
            className={`chevron ${expanded ? "open" : ""}`}
            onClick={() => setExpanded(!expanded)}
            aria-label={expanded ? "collapse" : "expand"}
          >
            <IconChevron size={12} />
          </button>
        ) : (
          <span className="chevron-spacer" />
        )}
        <button
          className={`tree-item ${active ? "active" : ""}`}
          title={node.sheet.svg_path}
          onClick={() => {
            useViewStore.getState().setView("schematic");
            nav.openSheet({
              number: node.sheet.number,
              name: node.sheet.name,
              svg_path: node.sheet.svg_path,
            });
          }}
        >
          <IconSheet size={13} />
          <span className="tree-name">
            {node.sheet.sheet_path === "/" ? "Root" : node.sheet.name}
          </span>
          {/* KiCad page label when the project sets one (hierarchical designs);
              otherwise the sequential sheet number. */}
          <span className="dim mono">p{node.sheet.page || node.sheet.number}</span>
        </button>
      </div>
      {expanded &&
        node.children.map((c) => (
          <SheetBranch key={c.sheet.sheet_path} node={c} depth={depth + 1} />
        ))}
    </>
  );
}

export function SheetsPanel() {
  const sheets = useProjectStore((s) => s.sheets);
  const tree = useMemo(() => buildSheetTree(sheets), [sheets]);

  if (!sheets.length)
    return <div className="panel-empty">No schematic sheets in this design.</div>;

  return (
    <div className="tree">
      {tree.map((n) => (
        <SheetBranch key={n.sheet.sheet_path} node={n} depth={0} />
      ))}
    </div>
  );
}
