// Pure helpers shared by the two version-control surfaces (rail panel + workspace) —
// no React, so they are unit-testable alongside `layout.ts`.

import { formatLocalTime } from "../../lib/time";
import type { ExtractionMeta } from "../../lib/types";

/** Short, git-style revision id — the trailing ref and the last-resort row label. */
export const shortId = (id: string) => id.slice(0, 10);

/** A row's subject reads like a commit subject: a manual rename wins, else the publish
 *  changelog, else the timestamp. Shared so the rail, the workspace and the compare
 *  nudge all name a revision the same way. */
export const rowText = (r: ExtractionMeta) =>
  r.label ?? r.message ?? formatLocalTime(r.created_at);

/** Free-text filter over the fields a reader would actually search by: subject, author,
 *  tag and the short id. Empty/blank query = everything (no filtering cost). */
export function filterRevisions(revs: ExtractionMeta[], query: string): ExtractionMeta[] {
  const q = query.trim().toLowerCase();
  if (!q) return revs;
  return revs.filter((r) =>
    [rowText(r), r.author ?? "", shortId(r.id), ...r.tags]
      .some((f) => f.toLowerCase().includes(q)),
  );
}

/** How many ref chips ("viewing", "on disk") the widest row in this list carries.
 *  Both surfaces reserve `refColumns() * chipWidth` for the refs slot so the subject
 *  text starts at the same x on every row instead of shifting per-row — the specific
 *  raggedness the old inline badges caused. 0 when neither pointer is in view (e.g.
 *  the filter excluded them), so an unfiltered list wastes no width. */
export function refColumns(
  rows: ExtractionMeta[],
  activeId: string | null,
  designHead: string | null,
): number {
  let max = 0;
  for (const r of rows) {
    const n = (r.id === activeId ? 1 : 0) + (r.id === designHead ? 1 : 0);
    if (n > max) max = n;
  }
  return max;
}

/** DAG tips = revisions nobody claims as a parent (the branch heads). Two or more is
 *  the fork case: the workspace draws it, and the rail — which has no room to render
 *  lanes honestly — points the reader at the workspace instead of faking a graph. */
export function branchTips(revs: ExtractionMeta[]): ExtractionMeta[] {
  const isParent = new Set(revs.flatMap((e) => e.parents));
  return revs.filter((e) => !e.hidden && !isParent.has(e.id));
}

/** The nearest present parent — "compare with previous" (first parent = nearest ancestor
 *  on the active lane, per the VC plan §8). null for a root revision. */
export function parentOf(r: ExtractionMeta, all: ExtractionMeta[]): string | null {
  return r.parents.find((p) => all.some((e) => e.id === p)) ?? null;
}

/** Author initials for the row avatar. Handles "priya", "Priya Nair" and "p.nair@x.com"
 *  without pretending to parse names properly — two letters, uppercased. */
export function initials(author: string | null): string {
  const name = (author ?? "").trim();
  if (!name) return "?";
  const parts = name.split(/[\s._@-]+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}
