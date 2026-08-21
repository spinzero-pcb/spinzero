import { useEffect, useState } from "react";
import {
  DEFAULT_SERVICE_URL,
  serviceConfig,
  useDetailedReviewStore,
} from "../stores/detailedReviewStore";
import { useSettingsStore } from "../stores/settingsStore";
import { BOM_PROFILES } from "../lib/findings";
import { useBomCheckStore } from "../stores/bomCheckStore";

// Pre-flight for the detailed (paid) review.
//
// This dialog exists to make one promise checkable: the review uploads the enriched
// BOM, its mapping sidecar and a small metadata blob — and nothing else (plan §4.2).
// So it lists the actual files, their sizes, what is deliberately left behind, and
// the retention policy, before the user can press the button. The list is not a
// description of the upload; it IS the upload, straight from `build_review_bundle`.

/** Retention wording shown verbatim, because it is a promise, not a summary. */
const RETENTION =
  "The service deletes your bundle as soon as the review finishes, and deletes the " +
  "findings once this app confirms it has them. Prompts and completions are never " +
  "stored. What is kept is job metadata with no BOM content in it.";

export function DetailedReviewDialog() {
  const phase = useDetailedReviewStore((s) => s.phase);
  const bundle = useDetailedReviewStore((s) => s.bundle);
  const bundleError = useDetailedReviewStore((s) => s.bundleError);
  const serviceOk = useDetailedReviewStore((s) => s.serviceOk);
  const close = useDetailedReviewStore((s) => s.closePreflight);
  const start = useDetailedReviewStore((s) => s.start);
  const profile = useBomCheckStore((s) => s.profile);
  const saved = useSettingsStore((s) => s.reviewService);

  const [baseUrl, setBaseUrl] = useState(saved?.base_url ?? DEFAULT_SERVICE_URL);
  const [token, setToken] = useState(saved?.token ?? "");
  const [editing, setEditing] = useState(!saved);

  useEffect(() => {
    if (phase !== "preflight") return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [phase, close]);

  useEffect(() => {
    setBaseUrl(saved?.base_url ?? DEFAULT_SERVICE_URL);
    setToken(saved?.token ?? "");
    setEditing(!saved);
  }, [saved]);

  if (phase !== "preflight") return null;

  const configured = Boolean(serviceConfig());
  const ready = Boolean(bundle) && configured;
  const profileLabel = BOM_PROFILES.find((p) => p.id === profile)?.label ?? profile;

  async function saveService() {
    const url = baseUrl.trim().replace(/\/+$/, "");
    if (!/^https?:\/\//i.test(url)) return;
    await useSettingsStore.getState().setReviewService({ base_url: url, token: token.trim() });
    setEditing(false);
    void useDetailedReviewStore.getState().checkService();
  }

  return (
    <div className="wizard-overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div className="wizard-card review-preflight" role="dialog" aria-label="Detailed BOM review">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">Run a detailed BOM review</div>
            <div className="wizard-step">Profile: {profileLabel}</div>
          </div>
        </div>

        <div className="wizard-body">
          {bundleError ? (
            <p className="wizard-hint review-preflight-error">
              This project has nothing to review yet: {bundleError}
            </p>
          ) : !bundle ? (
            <p className="wizard-hint">Collecting the files…</p>
          ) : (
            <>
              <p className="wizard-hint">
                These {Object.keys(bundle.files).length} files — and only these — are sent to the
                review service:
              </p>
              <ul className="review-file-list">
                {Object.keys(bundle.files).map((name) => (
                  <li key={name}>
                    <span className="review-file-name">{name}</span>
                    <span className="review-file-size">{formatBytes(bundle.sizes[name] ?? 0)}</span>
                  </li>
                ))}
              </ul>
              <p className="wizard-hint">
                Not sent: {bundle.excluded.join(", ")}. The BOM carries {bundle.bom_rows} line
                items.
              </p>
              <p className="wizard-hint review-retention">{RETENTION}</p>
            </>
          )}

          {editing || !configured ? (
            <div className="review-service-config">
              <label className="review-field">
                <span>Service URL</span>
                <input
                  className="wizard-input"
                  value={baseUrl}
                  spellCheck={false}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder={DEFAULT_SERVICE_URL}
                />
              </label>
              <label className="review-field">
                <span>Token</span>
                <input
                  className="wizard-input"
                  type="password"
                  value={token}
                  spellCheck={false}
                  onChange={(e) => setToken(e.target.value)}
                  placeholder="SPINZERO_DEV_TOKEN"
                />
              </label>
              <button className="btn-ghost" onClick={() => void saveService()}>
                Save service
              </button>
            </div>
          ) : (
            <p className="wizard-hint">
              Service: <code>{saved?.base_url}</code>{" "}
              {serviceOk === false ? (
                <span className="bom-check-warn">unreachable</span>
              ) : serviceOk ? (
                <span className="review-ok">reachable</span>
              ) : null}{" "}
              <button className="btn-ghost review-change" onClick={() => setEditing(true)}>
                change
              </button>
            </p>
          )}

          <div className="wizard-actions">
            <button className="btn-ghost" onClick={close}>
              Cancel
            </button>
            <button className="btn-primary" disabled={!ready} onClick={() => void start()}>
              Send and review
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} kB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}
