// Pure DAG layout for the revision history graph — no React, so it is unit-testable.
//
// Input is the picker's revision list (newest-first, as the backend returns it). We
// assign each revision a lane (column) with a git-log-style sweep that reuses lanes
// for linear history and branches a lane per fork, then emit edges between node
// positions. EDA history is overwhelmingly linear with the occasional fork, so a
// handful of lanes suffice.

import type { ExtractionMeta } from "../../lib/types";

export interface DagNode {
  meta: ExtractionMeta;
  lane: number;
  row: number;
}

export interface DagEdge {
  fromLane: number;
  fromRow: number;
  toLane: number;
  toRow: number;
  /** "normal" linear link, "merge" when the child has >1 parent (convergence),
   *  "pending" stub to a parent not present locally (not yet synced). */
  kind: "normal" | "merge" | "pending";
}

export interface DagLayout {
  nodes: DagNode[];
  edges: DagEdge[];
  laneCount: number;
}

/** Lay out `revs` (newest-first) into lanes + edges. Hidden revisions are dropped
 *  unless `showHidden`; an edge to a hidden (filtered) parent is omitted, while an
 *  edge to a genuinely-absent parent (not in the log at all) becomes a pending stub. */
export function layoutDag(revs: ExtractionMeta[], showHidden: boolean): DagLayout {
  const visible = revs.filter((r) => showHidden || !r.hidden);
  const present = new Set(visible.map((r) => r.id));
  const allIds = new Set(revs.map((r) => r.id));
  const rowOf = new Map(visible.map((r, i) => [r.id, i] as const));

  // lanes[k] = the (not-yet-reached, older) revision id reserved for lane k.
  const lanes: (string | null)[] = [];
  const laneOf = new Map<string, number>();
  const takeFree = (): number => {
    const k = lanes.indexOf(null);
    if (k >= 0) return k;
    lanes.push(null);
    return lanes.length - 1;
  };

  for (const r of visible) {
    let lane = lanes.indexOf(r.id);
    if (lane < 0) {
      lane = takeFree(); // a tip — no drawn child reserved a lane for it
    } else {
      // Other lanes reserved for the same id are children converging on a shared
      // parent (a fork); collapse them into this one.
      for (let k = 0; k < lanes.length; k++) {
        if (k !== lane && lanes[k] === r.id) lanes[k] = null;
      }
    }
    lanes[lane] = null; // consume the reservation
    laneOf.set(r.id, lane);

    // Reserve lanes for this revision's present parents (older, drawn below): the
    // first reuses this lane, each additional one branches to a fresh lane.
    const parents = r.parents.filter((p) => present.has(p));
    parents.forEach((pid, idx) => {
      const plane = idx === 0 ? lane : takeFree();
      lanes[plane] = pid;
    });
  }

  const laneCount = Math.max(1, lanes.length, ...Array.from(laneOf.values(), (l) => l + 1));

  const nodes: DagNode[] = visible.map((r, row) => ({ meta: r, lane: laneOf.get(r.id)!, row }));

  const edges: DagEdge[] = [];
  visible.forEach((r, row) => {
    const cl = laneOf.get(r.id)!;
    const multi = r.parents.filter((p) => present.has(p)).length > 1;
    for (const pid of r.parents) {
      if (present.has(pid)) {
        edges.push({
          fromLane: cl,
          fromRow: row,
          toLane: laneOf.get(pid)!,
          toRow: rowOf.get(pid)!,
          kind: multi ? "merge" : "normal",
        });
      } else if (!allIds.has(pid)) {
        // Parent isn't in the log on this machine yet — render a short pending stub.
        edges.push({ fromLane: cl, fromRow: row, toLane: cl, toRow: row + 0.6, kind: "pending" });
      }
      // else: parent exists but is hidden/filtered — omit the edge.
    }
  });

  return { nodes, edges, laneCount };
}
