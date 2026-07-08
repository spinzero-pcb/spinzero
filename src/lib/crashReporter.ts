import { ipc } from "./ipc";
import { useToastStore } from "../stores/toastStore";

/** Format an unknown thrown value into a single log-friendly string. */
function describe(value: unknown): string {
  if (value instanceof Error) {
    return `${value.name}: ${value.message}\n${value.stack ?? "(no stack)"}`;
  }
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

let lastReport = "";

/** Forward an error to the backend log and surface a single, deduped toast.
 *  Swallows everything: the reporter must never throw from inside an error
 *  handler (that would replace one crash with another). */
function report(source: string, value: unknown) {
  try {
    const detail = describe(value);
    // Collapse a storm of identical errors (e.g. a render loop) into one report.
    if (detail === lastReport) return;
    lastReport = detail;

    void ipc.logError(`${source}: ${detail}`);

    const message =
      value instanceof Error ? value.message : String(value).slice(0, 200);
    // Deferred: in dev React replays render errors through a synthetic window
    // "error" event dispatched synchronously INSIDE the render phase, so pushing
    // the toast here would itself be a setState-in-render ("Cannot update Toaster
    // while rendering …"). A macrotask hop lands the push safely outside render.
    setTimeout(() => {
      useToastStore.getState().push({
        kind: "error",
        key: "uncaught-error", // one slot — later errors replace, never stack
        title: "Something went wrong",
        message: message || "An unexpected error occurred. The app is still running.",
      });
    }, 0);
  } catch {
    // Last resort: there is nowhere safe left to report to.
  }
}

let installed = false;

/** Install process-wide guards so an uncaught error or rejected promise is
 *  logged and surfaced instead of silently breaking the app. Idempotent. */
export function installGlobalErrorHandlers() {
  if (installed) return;
  installed = true;

  window.addEventListener("error", (e: ErrorEvent) => {
    report("Uncaught error", e.error ?? e.message);
  });

  window.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
    report("Unhandled promise rejection", e.reason);
  });
}
