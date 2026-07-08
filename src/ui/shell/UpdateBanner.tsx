import { useUpdateStore } from "../../stores/updateStore";

/** Persistent update notice pinned to the bottom of the left rail (batch1). Replaces
 *  the old transient "Relaunch to update" toast: an update applies only on the user's
 *  nod, so the prompt stays put until they take it — there is intentionally no dismiss
 *  control, and no changelog detail (just the version). Renders nothing until a signed
 *  build has been downloaded and is ready to install. */
export function UpdateBanner() {
  const phase = useUpdateStore((s) => s.phase);
  const version = useUpdateStore((s) => s.version);
  const error = useUpdateStore((s) => s.error);
  const apply = useUpdateStore((s) => s.apply);
  if (phase === "idle") return null;

  const installing = phase === "installing";
  const failed = phase === "error";
  return (
    <div className={`update-banner ${failed ? "update-banner--error" : ""}`}>
      <div className="update-banner-text">
        <span className="update-banner-title">
          {failed ? "Update failed" : `Update ready${version ? ` : v${version}` : ""}`}
        </span>
        {failed && error && <span className="update-banner-sub">{error}</span>}
      </div>
      <button
        className="btn-primary update-banner-btn"
        disabled={installing}
        onClick={() => void apply()}
      >
        {installing ? "Updating…" : failed ? "Retry" : "Relaunch to update"}
      </button>
    </div>
  );
}
