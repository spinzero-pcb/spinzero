import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { homeDir, join } from "@tauri-apps/api/path";
import { useProjectStore } from "../../stores/projectStore";
import { useSettingsStore } from "../../stores/settingsStore";
import type { DetectedDesign, ProjectClass } from "../../lib/types";
import { ipc } from "../../lib/ipc";
import { PROJECT_CLASSES } from "../../lib/projectClass";
import { IconBoard, IconFolder } from "../icons";

function basename(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? p;
}

function slugify(s: string): string {
  return (
    s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "project"
  );
}


export function NewProjectWizard({
  onClose,
  initialFolder,
}: {
  onClose: () => void;
  initialFolder?: string | null;
}) {
  const createProject = useProjectStore((s) => s.createProject);
  const busy = useProjectStore((s) => s.busy);
  const projectRoot = useSettingsStore((s) => s.projectRoot);
  const setProjectRoot = useSettingsStore((s) => s.setProjectRoot);

  const [step, setStep] = useState<1 | 2>(1);
  const [designFolder, setDesignFolder] = useState<string | null>(null);
  const [detected, setDetected] = useState<DetectedDesign | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [detectErr, setDetectErr] = useState<string | null>(null);
  // A pre-KiCad-6 project we can detect but not import. When set we block the wizard:
  // no Next button, only Cancel — there is nothing to proceed to.
  const [legacyBlocked, setLegacyBlocked] = useState(false);

  const [name, setName] = useState("");
  const [cls, setCls] = useState<ProjectClass>("general");
  const [root, setRoot] = useState<string | null>(projectRoot);
  const [locationPreview, setLocationPreview] = useState("");
  const [createErr, setCreateErr] = useState<string | null>(null);

  // Resolve the default parent folder: remembered root, else ~/PCBReview Projects.
  useEffect(() => {
    if (root) return;
    void (async () => {
      try {
        const home = await homeDir();
        setRoot(await join(home, "spinzero-projects"));
      } catch {
        setRoot("spinzero-projects");
      }
    })();
  }, [root]);

  // Keep the previewed project folder in sync with root + name.
  useEffect(() => {
    if (!root) return;
    void (async () => {
      try {
        setLocationPreview(await join(root, slugify(name)));
      } catch {
        setLocationPreview(`${root}/${slugify(name)}`);
      }
    })();
  }, [root, name]);

  async function runDetect(dir: string, advanceOnHit = false) {
    setDetectErr(null);
    setLegacyBlocked(false);
    setDesignFolder(dir);
    setDetecting(true);
    try {
      const d = await ipc.detectDesign(dir);
      if (d?.legacy) {
        // Pre-KiCad-6 (≤5): detected but not importable — the board is still in the old
        // format even if a newer KiCad rewrote the project file, so the extraction would
        // be incomplete. Block the wizard (no Next) and point the user at KiCad.
        setDetected(null);
        setLegacyBlocked(true);
        setDetectErr(
          `“${basename(d.file)}” was built with KiCad 5 or older. SpinZero imports ` +
            `KiCad 6 and newer. Open the board in KiCad’s PCB editor and choose ` +
            `File → Save to upgrade it to the current format, then import the folder again.`,
        );
        return;
      }
      setDetected(d);
      if (!d) {
        setDetectErr(
          "No KiCad (.kicad_pro) project file found in that folder.",
        );
      } else {
        if (!name) setName(d.name || basename(dir));
        if (advanceOnHit) setStep(2);
      }
    } catch (e) {
      setDetectErr(String(e));
    } finally {
      setDetecting(false);
    }
  }

  async function pickDesignFolder() {
    const dir = await openDialog({ directory: true, title: "Select the design folder" });
    if (typeof dir !== "string") return;
    await runDetect(dir);
  }

  // Opened from "Open Project…" pointed at a raw design folder (e.g. a KiCad demo):
  // pre-fill it, detect, and skip straight to step 2 when it's a valid design.
  useEffect(() => {
    if (initialFolder) void runDetect(initialFolder, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialFolder]);

  // The projects folder is common and remembered — picking here updates the
  // default for every future project, not just this one (asked once, reused).
  async function changeRoot() {
    const dir = await openDialog({ directory: true, title: "Where to store all projects" });
    if (typeof dir === "string") {
      setRoot(dir);
      void setProjectRoot(dir);
    }
  }

  async function create() {
    if (!designFolder || !detected) return;
    setCreateErr(null);
    if (root) void setProjectRoot(root); // remember for next time
    try {
      await createProject({
        name: name.trim() || detected.name,
        designPath: designFolder,
        projectDir: locationPreview,
        designTool: detected.kind,
        class: cls,
      });
      onClose(); // App transitions to the viewer once the project is open
    } catch (e) {
      setCreateErr(String(e));
    }
  }

  return (
    <div className="wizard-overlay" onPointerDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="wizard-card" role="dialog" aria-label="New project">
        <div className="wizard-head">
          <span className="wizard-icon"><IconBoard size={18} /></span>
          <div>
            <div className="wizard-title">New Project</div>
            <div className="wizard-step">Step {step} of 2</div>
          </div>
        </div>

        {step === 1 && (
          <div className="wizard-body">
            <label className="wizard-label">Design folder</label>
            <p className="wizard-hint">
              Point at your KiCad project folder. The folder stays untouched —
              all review data is kept in a separate project folder.
            </p>
            <button className="wizard-pick" onClick={pickDesignFolder} disabled={detecting}>
              <IconFolder size={14} />
              <span>{designFolder ? basename(designFolder) : "Choose folder…"}</span>
            </button>
            {designFolder && (
              <div className="wizard-path mono">{designFolder}</div>
            )}
            {detecting && <div className="wizard-hint">Detecting design…</div>}
            {detected && (
              <div className="wizard-detected">
                <span className={`tool-badge ${detected.kind}`}>{detected.kind}</span>
                <span className="mono">{basename(detected.file)}</span>
              </div>
            )}
            {detectErr && <div className="error-card">{detectErr}</div>}

            <div className="wizard-actions">
              <button className="btn-ghost" onClick={onClose}>Cancel</button>
              {!legacyBlocked && (
                <button
                  className="btn-primary"
                  disabled={!detected}
                  onClick={() => setStep(2)}
                >
                  Next
                </button>
              )}
            </div>
          </div>
        )}

        {step === 2 && (
          <div className="wizard-body">
            <label className="wizard-label" htmlFor="np-name">Project name</label>
            <input
              id="np-name"
              className="wizard-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={detected?.name ?? "My Board"}
            />

            <label className="wizard-label">Project class</label>
            
            <div className="wizard-class">
              {PROJECT_CLASSES.map((c) => (
                <button
                  key={c.value}
                  className={`class-chip ${cls === c.value ? "on" : ""}`}
                  title={c.hint}
                  onClick={() => setCls(c.value)}
                >
                  {c.label}
                </button>
              ))}
            </div>

            <label className="wizard-label">Project location</label>
            <p className="wizard-hint">
              {projectRoot
                ? "Saved in your projects folder — chosen once and reused for every project."
                : "Choose where all your projects are stored. You’ll only be asked this once."}
            </p>
            <div className="wizard-loc">
              <div className="wizard-path mono">{locationPreview || "…"}</div>
              <button className="btn-ghost" onClick={changeRoot} disabled={busy}>
                {projectRoot ? "Change default…" : "Choose folder…"}
              </button>
            </div>

            {createErr && <div className="error-card">{createErr}</div>}

            <div className="wizard-actions">
              <button className="btn-ghost" onClick={() => setStep(1)} disabled={busy}>
                Back
              </button>
              <button
                className="btn-primary"
                disabled={busy || !locationPreview}
                onClick={() => void create()}
              >
                {busy ? "Creating & extracting…" : "Create Project"}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
