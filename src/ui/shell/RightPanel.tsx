import { useState } from "react";
import { useProjectStore } from "../../stores/projectStore";
import { useViewStore } from "../../stores/viewStore";
import { SheetsPanel } from "./SheetsPanel";
import { AppearancePanel } from "../pcb/AppearancePanel";
import { NetClassPanel } from "../pcb/NetClassPanel";
import { useDesignStore } from "../../stores/designStore";
import { listNetClasses } from "../../lib/netClasses";
import { IconChevron, IconSheet } from "../icons";

// The RIGHT panel is the per-view navigator/tools (docs/phase2-ui-plan.md §1):
// `Sheets` on the schematic (moved off the old left Explorer), `Appearance` on the
// PCB, a filters placeholder on the BOM. The LEFT rail owns Review/AI; revisions
// moved to the clickable footer picker (item 21). Layers and review never collide.

function Section({
  title,
  icon,
  count,
  defaultOpen = true,
  children,
}: {
  title: string;
  icon?: React.ReactNode;
  count?: number;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="tree-section">
      <button className="tree-label" onClick={() => setOpen(!open)}>
        <span className={`chevron ${open ? "open" : ""}`}>
          <IconChevron size={12} />
        </span>
        {icon}
        <span>{title}</span>
        {count !== undefined && <span className="dim">({count})</span>}
      </button>
      {open && children}
    </div>
  );
}

export function RightPanel() {
  const project = useProjectStore((s) => s.project);
  const summary = useProjectStore((s) => s.summary);
  const sheets = useProjectStore((s) => s.sheets);
  const view = useViewStore((s) => s.view);
  const indexes = useDesignStore((s) => s.indexes);
  const netClasses = view === "pcb" ? listNetClasses(indexes) : [];

  if (!project) return null;

  const projectName = summary?.name ?? project.name ?? "Project";

  return (
    <>
      <div className="side-panel-header explorer-header">
        <span className="tree-name">{projectName}</span>
      </div>
      <div className="right-panel-scroll">
        {view === "schematic" && (
          <Section title="Sheets" icon={<IconSheet size={13} />} count={sheets.length}>
            <SheetsPanel />
          </Section>
        )}
        {view === "pcb" && <AppearancePanel />}
        {view === "pcb" && netClasses.length > 0 && <NetClassPanel />}
        {/* BOM has no right panel — the whole <aside> is hidden on BOM (see App.tsx). */}
      </div>
    </>
  );
}
