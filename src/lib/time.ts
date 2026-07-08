// Comment timestamps are stored as RFC3339 UTC by the backend (reviews.rs uses
// OffsetDateTime::now_utc). The UI must show them in the viewer's LOCAL timezone
// (item 14) — `new Date(rfc3339)` parses the offset and renders local.

/** Absolute local time, e.g. "Jun 14, 09:30" (drops the year unless it differs). */
export function formatLocalTime(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleString([], {
    year: sameYear ? undefined : "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Compact relative time ("just now", "5m", "3h", "2d") for thread/row footers,
 *  falling back to the absolute local date past a week. */
export function formatRelative(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const secs = Math.round((Date.now() - d.getTime()) / 1000);
  if (secs < 45) return "just now";
  if (secs < 3600) return `${Math.round(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h ago`;
  if (secs < 604800) return `${Math.round(secs / 86400)}d ago`;
  return formatLocalTime(ts);
}
