import { useProjectStore } from "../../stores/projectStore";
import { useShellStore } from "../../stores/shellStore";
import { IconSpinZero, IconFolder } from "../icons";

function basename(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? p;
}

/** Home screen: the project list + entry points. Replaces the old "Open Folder"
 *  welcome screen — the app now manages projects, not bare design folders. The
 *  New Project wizard is rendered by ShellDialogs (mounted in the always-present
 *  MenuBar). */
export function Home() {
  const openProject = useProjectStore((s) => s.openProject);
  const recents = useProjectStore((s) => s.recents);
  const busy = useProjectStore((s) => s.busy);
  const errorMsg = useProjectStore((s) => s.errorMsg);
  const openWizard = useShellStore((s) => s.openWizard);
  const openExisting = useShellStore((s) => s.openExisting);
  const shellErr = useShellStore((s) => s.err);

  return (
    <div className="home">
      <div className="home-card">
        <div className="home-brand">
          <span className="home-logo"><IconSpinZero size={26} /></span>
          <div>
            <h1>SpinZero</h1>
            <p className="home-sub">Local-first design review for KiCad</p>
          </div>
        </div>

        <div className="home-actions">
          <button className="home-action primary" onClick={() => openWizard()} disabled={busy}>
            <span className="home-plus" aria-hidden>＋</span>
            <span>New Project</span>
          </button>
          <button className="home-action" onClick={() => void openExisting()} disabled={busy}>
            <IconFolder size={16} />
            <span>Open Project…</span>
          </button>
        </div>

        {(shellErr || errorMsg) && <div className="error-card">{shellErr || errorMsg}</div>}

        {recents.length > 0 && (
          <>
            <div className="side-panel-header" style={{ padding: 0 }}>Recent</div>
            <div className="recent-list">
              {recents.map((r) => (
                <button key={r} onClick={() => void openProject(r).catch(() => {})} title={r}>
                  <span className="recent-name">{basename(r)}</span>
                  <span className="recent-path">{r}</span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
