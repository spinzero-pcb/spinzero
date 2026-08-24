import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { useProjectStore } from "../../stores/projectStore";
import { useShellStore } from "../../stores/shellStore";
import { useSettingsStore, ACCENT_PRESETS, ACCENT_DEFAULT } from "../../stores/settingsStore";
import { useViewStore } from "../../stores/viewStore";
import { useShortcutsDialog } from "./KeyboardShortcuts";
import { ipc } from "../../lib/ipc";
import { ShellDialogs } from "./ShellDialogs";
import { reExtract } from "./StatusBar";
import { IconSpinZero } from "../icons";

function basename(p: string): string {
  const parts = p.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? p;
}

const appWindow = getCurrentWindow();

/** Public releases repo — README, install guide, downloads, changelog (opened from About). */
const RELEASES_URL = "https://github.com/spinzero-pcb/spinzero";

/** Prefilled new-issue URL in the releases repo. The `version`/`os` query params
 *  match the issue-form field ids, so they arrive already filled in. */
function newIssueUrl(template: "bug_report.yml" | "feedback.yml", version: string | null): string {
  const ua = navigator.userAgent;
  const os = ua.includes("Windows") ? "Windows" : ua.includes("Mac") ? "macOS" : "Linux";
  const params = new URLSearchParams({ template, version: version ?? "unknown", os });
  return `${RELEASES_URL}/issues/new?${params.toString()}`;
}

type OpenMenu = "file" | "view" | "help" | null;

/** Custom title bar (item 3): the window is decorationless, so this bar IS the title
 *  bar — app icon + File/Help menus on the left, a draggable middle, and the window
 *  controls on the right. No "SpinZero" text; the project name shows in the status bar. */
export function MenuBar() {
  const openProject = useProjectStore((s) => s.openProject);
  const project = useProjectStore((s) => s.project);
  const summary = useProjectStore((s) => s.summary);
  const recents = useProjectStore((s) => s.recents);
  const busy = useProjectStore((s) => s.busy);
  const openWizard = useShellStore((s) => s.openWizard);
  const openExisting = useShellStore((s) => s.openExisting);
  const fullscreen = useViewStore((s) => s.fullscreen);
  const toggleFullscreen = useViewStore((s) => s.toggleFullscreen);
  const openShortcuts = useShortcutsDialog((s) => s.setOpen);
  const [openMenu, setOpenMenu] = useState<OpenMenu>(null);
  const [recentOpen, setRecentOpen] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const [appearanceOpen, setAppearanceOpen] = useState(false);
  const accentColor = useSettingsStore((s) => s.accentColor);
  const setAccentColor = useSettingsStore((s) => s.setAccentColor);
  const authorName = useSettingsStore((s) => s.authorName);
  const setAuthorName = useSettingsStore((s) => s.setAuthorName);
  // Local buffer so typing doesn't rewrite the settings file on every keystroke — we
  // persist on blur / dialog close.
  const [nameDraft, setNameDraft] = useState("");
  // Telemetry consent for the Data Privacy dialog. `null` until loaded; defaults ON.
  const [shareDiagnostics, setShareDiagnostics] = useState<boolean | null>(null);
  const [maximized, setMaximized] = useState(false);
  // The one place the project is named. `summary` is the extracted design's own name
  // and wins when present; `project.name` covers a project whose first extraction has
  // not landed yet.
  const projectName = summary?.name ?? project?.name ?? null;
  const [version, setVersion] = useState<string | null>(null);
  // The OS-derived identity slug — shown as the placeholder / fallback for the name field.
  const [reviewAuthorSlug, setReviewAuthorSlug] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!openMenu) return;
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpenMenu(null);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpenMenu(null);
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [openMenu]);

  // App version (item 16): sourced from tauri.conf.json at runtime, shown in About.
  useEffect(() => {
    let active = true;
    void getVersion()
      .then((v) => active && setVersion(v))
      .catch(() => {});
    void ipc
      .getReviewAuthor()
      .then((a) => active && setReviewAuthorSlug(a))
      .catch(() => {});
    return () => {
      active = false;
    };
  }, []);

  // Track maximize state so the middle control shows the right glyph.
  useEffect(() => {
    let active = true;
    void appWindow.isMaximized().then((m) => active && setMaximized(m));
    const un = appWindow.onResized(() => {
      void appWindow.isMaximized().then((m) => active && setMaximized(m));
    });
    return () => {
      active = false;
      void un.then((fn) => fn());
    };
  }, []);

  function newProject() {
    setOpenMenu(null);
    openWizard();
  }

  function openExistingProject() {
    setOpenMenu(null);
    void openExisting();
  }

  function openPrivacy() {
    setOpenMenu(null);
    setPrivacyOpen(true);
    // Load current consent (defaults ON if telemetry info can't be read).
    void ipc
      .getTelemetryInfo()
      .then((info) => setShareDiagnostics(info.enabled))
      .catch(() => setShareDiagnostics(true));
  }

  return (
    <>
    <div className="menu-bar" data-tauri-drag-region>
      <span className="titlebar-icon" aria-hidden>
        <IconSpinZero size={16} />
      </span>
      <div ref={rootRef} style={{ display: "contents" }}>
        <div className="menu-root">
          <button
            className={`menu-title ${openMenu === "file" ? "on" : ""}`}
            onClick={() => setOpenMenu((o) => (o === "file" ? null : "file"))}
          >
            File
          </button>
          {openMenu === "file" && (
            <div className="menu-dropdown">
              <button className="menu-entry" onClick={newProject} disabled={busy}>
                New Project…
              </button>
              <button className="menu-entry" onClick={openExistingProject} disabled={busy}>
                Open Project…
              </button>
              <div
                className="menu-entry has-sub"
                onMouseEnter={() => setRecentOpen(true)}
                onMouseLeave={() => setRecentOpen(false)}
              >
                <span>Open Recent</span>
                <span className="menu-arrow">▸</span>
                {recentOpen && (
                  <div className="menu-sub">
                    {recents.length === 0 ? (
                      <div className="menu-empty">No recent projects</div>
                    ) : (
                      recents.map((r) => (
                        <button
                          key={r}
                          className="menu-entry"
                          title={r}
                          onClick={() => {
                            setOpenMenu(null);
                            void openProject(r).catch(() => {});
                          }}
                        >
                          {basename(r)}
                        </button>
                      ))
                    )}
                  </div>
                )}
              </div>
              <div className="menu-sep" />
              {/* The manual re-extract. It used to be a permanent footer button, which
                  was a no-op whenever nothing had changed; extraction is automatic, so
                  the escape hatch belongs in a menu — for a missed file-watch event or
                  after a failure. */}
              <button
                className="menu-entry"
                disabled={!project || busy}
                onClick={() => {
                  setOpenMenu(null);
                  void reExtract();
                }}
              >
                Re-extract Design
              </button>
            </div>
          )}
        </div>

        <div className="menu-root">
          <button
            className={`menu-title ${openMenu === "view" ? "on" : ""}`}
            onClick={() => setOpenMenu((o) => (o === "view" ? null : "view"))}
          >
            View
          </button>
          {openMenu === "view" && (
            <div className="menu-dropdown">
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  toggleFullscreen();
                }}
              >
                {fullscreen ? "Exit Full Screen" : "Full Screen"}
                <span className="menu-shortcut">F11</span>
              </button>
              <div className="menu-sep" />
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  setNameDraft(authorName ?? "");
                  setAppearanceOpen(true);
                }}
              >
                Appearance…
              </button>
            </div>
          )}
        </div>

        <div className="menu-root">
          <button
            className={`menu-title ${openMenu === "help" ? "on" : ""}`}
            onClick={() => setOpenMenu((o) => (o === "help" ? null : "help"))}
          >
            Help
          </button>
          {openMenu === "help" && (
            <div className="menu-dropdown">
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  openShortcuts(true);
                }}
              >
                Keyboard Shortcuts
                <span className="menu-shortcut">?</span>
              </button>
              <div className="menu-sep" />
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  void ipc.openExternal(newIssueUrl("feedback.yml", version)).catch(() => {});
                }}
              >
                Send Feedback…
              </button>
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  void ipc.openExternal(newIssueUrl("bug_report.yml", version)).catch(() => {});
                }}
              >
                Report a Bug…
              </button>
              <button
                className="menu-entry"
                onClick={() => {
                  setOpenMenu(null);
                  setAboutOpen(true);
                }}
              >
                About SpinZero
              </button>
              <button className="menu-entry" onClick={openPrivacy}>
                Data Privacy
              </button>
            </div>
          )}
        </div>
      </div>

      {/* Draggable middle — pointer events that fall here move the window. It also
          carries the project name: this strip is the window's title bar and the name is
          the document's title, so it belongs here rather than repeated in the footer and
          the right panel (2026-08-24). Reverses the earlier "no text in the title bar"
          call, which is what left the name duplicated in two panels. */}
      <div className="titlebar-drag" data-tauri-drag-region>
        {projectName && (
          <span className="titlebar-project" data-tauri-drag-region title={projectName}>
            {projectName}
          </span>
        )}
      </div>

      <div className="window-controls">
        <button
          className="win-btn"
          title="Minimize"
          onClick={() => void appWindow.minimize()}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <path d="M1 5h8" stroke="currentColor" strokeWidth="1" />
          </svg>
        </button>
        <button
          className="win-btn"
          title={maximized ? "Restore" : "Maximize"}
          onClick={() => void appWindow.toggleMaximize()}
        >
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden>
              <rect x="1.5" y="2.5" width="6" height="6" stroke="currentColor" strokeWidth="1" />
              <path d="M3.5 2.5V1.5h5v5h-1" stroke="currentColor" strokeWidth="1" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden>
              <rect x="1.5" y="1.5" width="7" height="7" stroke="currentColor" strokeWidth="1" />
            </svg>
          )}
        </button>
        <button
          className="win-btn win-close"
          title="Close"
          onClick={() => void appWindow.close()}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" strokeWidth="1" />
          </svg>
        </button>
      </div>
    </div>
    {aboutOpen && (
      <div
        className="wizard-overlay about-overlay"
        onPointerDown={(e) => e.target === e.currentTarget && setAboutOpen(false)}
      >
        <div className="about-card" role="dialog" aria-label="About SpinZero">
          <div className="about-head">
            <span className="about-icon" aria-hidden>
              <IconSpinZero size={30} />
            </span>
            <div>
              <div className="about-name">SpinZero</div>
              <div className="about-version mono">version {version ?? "—"}</div>
            </div>
          </div>
          <p className="about-blurb">
            You can read everything about this app on the github page:
          </p>
          <a
            className="about-link"
            href={RELEASES_URL}
            onClick={(e) => {
              e.preventDefault();
              void ipc.openExternal(RELEASES_URL).catch(() => {});
            }}
          >
            github.com/spinzero-pcb/spinzero ↗
          </a>
          <div className="about-actions">
            <button className="btn-primary" onClick={() => setAboutOpen(false)}>
              Close
            </button>
          </div>
        </div>
      </div>
    )}
    {privacyOpen && (
      <div
        className="wizard-overlay about-overlay"
        onPointerDown={(e) => e.target === e.currentTarget && setPrivacyOpen(false)}
      >
        <div className="about-card" role="dialog" aria-label="Data Privacy">
          <div className="about-name">Data Privacy</div>
          <p className="about-blurb">
            SpinZero can anonymously report crashes and usage events to help us fix
            bugs and improve the app. A completely random identifier is generated for
            this — it holds no personally identifiable information (PII) and is used
            only for diagnostics.
          </p>
          <p className="about-blurb">
            Your design files, such as schematics or PCB layouts, are never shared.
          </p>
          <label className="privacy-toggle">
            <input
              type="checkbox"
              checked={shareDiagnostics ?? true}
              disabled={shareDiagnostics === null}
              onChange={(e) => {
                const next = e.target.checked;
                const prev = shareDiagnostics;
                setShareDiagnostics(next);
                void ipc
                  .setTelemetryEnabled(next)
                  .then((v) => setShareDiagnostics(v))
                  // Persist failed — revert to the truth so the toggle doesn't show a
                  // state that was never saved.
                  .catch(() => setShareDiagnostics(prev));
              }}
            />
            <span>Share anonymous diagnostics</span>
          </label>
          <div className="about-actions">
            <button className="btn-primary" onClick={() => setPrivacyOpen(false)}>
              Close
            </button>
          </div>
        </div>
      </div>
    )}
    {appearanceOpen && (
      <div
        className="wizard-overlay about-overlay"
        onPointerDown={(e) => e.target === e.currentTarget && setAppearanceOpen(false)}
      >
        <div className="about-card" role="dialog" aria-label="Appearance">
          <div className="about-name">Appearance</div>
          <p className="about-blurb">
            Pick an accent colour for buttons, selection, and highlights. This is a
            personal preference — it doesn’t change the SpinZero logo.
          </p>
          <div className="appearance-field">
            <label htmlFor="author-name-input">Your name on review comments</label>
            <input
              id="author-name-input"
              className="text-input"
              type="text"
              maxLength={60}
              placeholder={reviewAuthorSlug ?? "your name"}
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onBlur={() => void setAuthorName(nameDraft)}
            />
            <p className="about-blurb dim appearance-hint">
              Shown as the author of comments you post. Leave blank to use your system
              username{reviewAuthorSlug ? ` (${reviewAuthorSlug})` : ""}. Your identity is
              tracked separately, so this name doesn’t need to be unique.
            </p>
          </div>
          <div className="accent-picker" role="group" aria-label="Accent colour">
            {ACCENT_PRESETS.map((p) => {
              const selected = (p.value ?? null) === (accentColor ?? null);
              return (
                <button
                  key={p.label}
                  className={`ctx-swatch accent-swatch ${selected ? "on" : ""}`}
                  style={{ background: p.value ?? ACCENT_DEFAULT }}
                  title={p.label}
                  aria-label={p.label}
                  aria-pressed={selected}
                  onClick={() => void setAccentColor(p.value)}
                />
              );
            })}
            <label className="ctx-swatch ctx-swatch-custom accent-swatch" title="Custom colour…">
              +
              <input
                type="color"
                value={accentColor ?? ACCENT_DEFAULT}
                onChange={(e) => void setAccentColor(e.target.value)}
              />
            </label>
          </div>
          <div className="about-actions accent-actions">
            <button className="btn-ghost" onClick={() => void setAccentColor(null)}>
              Reset to default
            </button>
            <button className="btn-primary" onClick={() => setAppearanceOpen(false)}>
              Close
            </button>
          </div>
        </div>
      </div>
    )}
    <ShellDialogs />
    </>
  );
}
