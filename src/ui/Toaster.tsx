import { useToastStore } from "../stores/toastStore";

/** Bottom-right notification stack. The app's single place for transient,
 *  app-wide messages (extraction failures, open errors, missing data) that must
 *  be seen no matter which view is up — the bottom Output panel and the status
 *  bar are easy to miss. */
export function Toaster() {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);
  if (toasts.length === 0) return null;
  return (
    <div className="toaster" role="region" aria-label="Notifications">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`} role="status">
          <div className="toast-body">
            <div className="toast-title">{t.title}</div>
            {t.message && <div className="toast-msg">{t.message}</div>}
          </div>
          {t.action && (
            <button
              className="toast-action"
              onClick={() => {
                t.action!.onClick();
                dismiss(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          <button
            className="toast-close"
            aria-label="Dismiss"
            title="Dismiss"
            onClick={() => dismiss(t.id)}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
