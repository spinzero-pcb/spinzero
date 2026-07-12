import { useEffect } from "react";
import { useProjectStore } from "../stores/projectStore";

/** Confirmation shown when "Update KiCad files" would overwrite un-captured on-disk
 *  edits. Resolves `projectStore.checkoutPrompt` with the choice; Cancel / Escape /
 *  backdrop = false, "Save & update" = true (which captures the working tree as a
 *  checkpoint first, then writes the selected revision to disk). */
export function CheckoutConfirm() {
  const prompt = useProjectStore((s) => s.checkoutPrompt);

  useEffect(() => {
    if (!prompt) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") prompt.resolve(false);
    };
    window.addEventListener("keydown", onKey);
    // Settle on unmount / prompt change so a teardown mid-prompt can't strand
    // projectStore.busy = true (which would block every later revision switch).
    // Idempotent — a no-op if the user already resolved via cancel/confirm/Escape.
    return () => {
      window.removeEventListener("keydown", onKey);
      prompt.resolve(false);
    };
  }, [prompt]);

  if (!prompt) return null;
  const cancel = () => prompt.resolve(false);
  const confirm = () => prompt.resolve(true);

  return (
    <div
      className="wizard-overlay"
      onPointerDown={(e) => e.target === e.currentTarget && cancel()}
    >
      <div className="wizard-card" role="dialog" aria-label="Confirm KiCad files update">
        <div className="wizard-head">
          <div>
            <div className="wizard-title">Update the KiCad files?</div>
            <div className="wizard-step">You have un-captured changes</div>
          </div>
        </div>
        <div className="wizard-body">
          <p className="wizard-hint">
            Your design folder has edits that aren’t in any saved version yet. Updating
            will first save them as a local checkpoint, then write the selected version
            into the design folder so KiCad shows the same thing as the app. If the
            design is open in KiCad, close it first and reopen it after the update.
          </p>
          <div className="wizard-actions">
            <button className="btn-ghost" onClick={cancel}>
              Cancel
            </button>
            <button className="btn-primary" onClick={confirm}>
              Save &amp; update
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
