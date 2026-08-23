import { create } from "zustand";
import { ipc } from "../lib/ipc";
import { anchorState, metaDiff } from "../lib/objectHash";
import { useBomCheckStore } from "./bomCheckStore";
import { useDesignStore } from "./designStore";
import { useProjectStore } from "./projectStore";
import { useSettingsStore } from "./settingsStore";
import { useToastStore } from "./toastStore";
import type {
  Comment,
  CommentAnchor,
  CommentSeverity,
  CommentStatus,
  CommentView,
  ReviewSession,
} from "../lib/types";
import type { DesignIndexes } from "../lib/design";

/** Which surface the left rail shows. "changes" is only reachable while a visual diff
 *  is active (the Changes tab appears then); diff enter/exit drive it. "reviews" is
 *  where checks are launched, as opposed to "review" where their findings are read.
 *  This is the one source of truth — ActivityBar, LeftPanel, and diffStore all
 *  read/write it. */
export type LeftTab = "review" | "changes" | "reviews";

/** Derived display state. ⟳ re-check is never persisted — it is computed from the
 *  anchored object's live hash vs the hash stored when the comment was filed. */
export type DisplayStatus = "open" | "recheck" | "addressed" | "resolved" | "dismissed";

export interface DisplayInfo {
  status: DisplayStatus;
  /** object changed since the comment's base revision (the §4 drift guard) */
  drift: boolean;
  /** human-readable field changes ("value 10k→4.7k") for the mini-diff */
  diff: string[];
}

/** Stable 1-based numbers for chips/rows: oldest comment is #1 so numbers don't
 *  shuffle as new comments arrive. */
export function numberMap(comments: Comment[]): Map<string, number> {
  const byAge = [...comments].sort((a, b) => a.created_ts.localeCompare(b.created_ts));
  const m = new Map<string, number>();
  byAge.forEach((c, i) => m.set(c.id, i + 1));
  return m;
}

/** Short human label for a comment's anchor — the object ref, or "▭ <sheet>" for a region
 *  box (shared by the review list rows and the thread popover header). */
export function refLabel(c: Comment): string {
  if (c.anchor.type === "region") return `▭ ${c.anchor.sheet ?? "region"}`;
  return c.anchor.type === "net"
    ? c.anchor.ref
    : `${c.anchor.ref}${c.anchor.sheet ? ` · ${c.anchor.sheet}` : ""}`;
}

/** Next default "Review N" title. Uses the max existing number (not the count) so it
 *  doesn't collide after a delete — deleting "Review 2" then adding gives "Review 3". */
export function nextSessionTitle(sessions: ReviewSession[]): string {
  const max = sessions.reduce((m, s) => {
    const n = /^Review (\d+)$/.exec(s.title);
    return n ? Math.max(m, Number(n[1])) : m;
  }, 0);
  return `Review ${max + 1}`;
}

/** Fold the persisted status + live design into the status the UI actually shows. */
export function displayInfo(c: Comment, indexes: DesignIndexes | null): DisplayInfo {
  const current = indexes ? anchorState(c.anchor, indexes) : null;
  // Drift = the anchored object's electrical fields changed (or it vanished)
  // since the comment was filed.
  const drift =
    !!c.object_hash &&
    (current === null || current.hash !== c.object_hash) &&
    indexes !== null;
  const diff = current ? metaDiff(c.object_meta, current.meta) : [];
  if (c.status === "resolved") return { status: "resolved", drift, diff };
  if (c.status === "dismissed") return { status: "dismissed", drift, diff };
  if (c.status === "addressed") return { status: "addressed", drift, diff };
  // open: drift promotes it to ⟳ re-check automatically.
  return { status: drift ? "recheck" : "open", drift, diff };
}

export type ComposeTarget = { anchor: CommentAnchor; pos?: { x: number; y: number } };

interface ReviewState {
  author: string;
  comments: Comment[];
  loaded: boolean;
  /** Review sessions (item 9), oldest-first. */
  sessions: ReviewSession[];
  /** The selected session; null = the "All comments" pool (shows every comment). */
  activeSessionId: string | null;

  leftTab: LeftTab;
  /** Independent filters (items 16/17/19): status chips, severity, and view scope. */
  filterStatus: "all" | DisplayStatus;
  filterSeverity: "all" | CommentSeverity;
  filterView: "all" | CommentView;
  /** open thread popover (existing comment) + where to float it */
  openThreadId: string | null;
  threadPos: { x: number; y: number } | null;
  /** new-comment composer target (C + click on an object) */
  compose: ComposeTarget | null;
  /** C armed — the next canvas object click opens the composer */
  armed: boolean;

  load: () => Promise<void>;
  /** Drop this project's comments/sessions (project switch: never bleed A into B). */
  clear: () => void;
  setLeftTab: (t: LeftTab) => void;
  setFilterStatus: (f: ReviewState["filterStatus"]) => void;
  setFilterSeverity: (f: ReviewState["filterSeverity"]) => void;
  setFilterView: (f: ReviewState["filterView"]) => void;
  arm: (on: boolean) => void;
  beginCompose: (t: ComposeTarget) => void;
  cancelCompose: () => void;
  openThread: (id: string | null, pos?: { x: number; y: number } | null) => void;

  create: (
    anchor: CommentAnchor,
    body: string,
    severity: CommentSeverity,
    view: CommentView,
  ) => Promise<void>;
  reply: (id: string, body: string) => Promise<void>;
  setStatus: (id: string, status: CommentStatus, reason?: string) => Promise<void>;
  setSeverity: (id: string, severity: CommentSeverity) => Promise<void>;
  assign: (id: string, assignee: string | null) => Promise<void>;
  del: (id: string) => Promise<void>;

  setActiveSession: (id: string | null) => void;
  createSession: (title: string) => Promise<void>;
  /** Delete a review session AND its comments (the user's model: a session owns its
   *  comments — "All comments" is just a combined view, not a real session). Deleting
   *  the active session falls back to the next session (or "All comments" if none left). */
  deleteSession: (id: string) => Promise<void>;
}

// Monotonic token so an out-of-order load() (a slow project A resolving after the user
// switched to B, or overlapping crunch-event reloads) can't commit stale comments over
// newer ones — only the most recently started load wins. Mirrors designStore.loadGen.
let loadGen = 0;

/** Run a review mutation, surfacing any backend rejection (locked file, project torn
 *  down mid-action) as an error toast instead of an unhandled rejection → generic crash
 *  toast. Every UI call site fires these and-forgets, so the store owns the error path. */
async function guard(errTitle: string, run: () => Promise<void>) {
  try {
    await run();
  } catch (e) {
    useToastStore.getState().push({ kind: "error", title: errTitle, message: String(e) });
  }
}

/** The status tabs the panel exposes (subset of DisplayStatus + "all"). */
const STATUS_TABS: ReviewState["filterStatus"][] = [
  "all",
  "open",
  "recheck",
  "resolved",
  "dismissed",
  "addressed",
];
function isStatusTab(v: unknown): v is ReviewState["filterStatus"] {
  return typeof v === "string" && (STATUS_TABS as string[]).includes(v);
}

/** Remember a per-project review UI choice (last session / tab) machine-locally, so the
 *  next open lands where the user left off. No-op when no project is open. */
function rememberProjectUi(patch: { session_id?: string | null; status_tab?: string }) {
  const dir = useProjectStore.getState().project?.project_dir;
  if (!dir) return;
  void useSettingsStore.getState().setProjectUi(dir, patch);
}

export const useReviewStore = create<ReviewState>((set, get) => ({
  author: "you",
  comments: [],
  loaded: false,
  sessions: [],
  activeSessionId: null,

  leftTab: "review",
  filterStatus: "all",
  filterSeverity: "all",
  filterView: "all",
  openThreadId: null,
  threadPos: null,
  compose: null,
  armed: false,

  load: async () => {
    const gen = ++loadGen;
    // The last-used session/tab live in machine-local settings; make sure they're loaded
    // before we pick what to restore on the first open of this project.
    if (!useSettingsStore.getState().loaded) {
      try {
        await useSettingsStore.getState().load();
      } catch {
        /* fall back to defaults if settings can't be read */
      }
    }
    try {
      const [author, comments, sessions] = await Promise.all([
        ipc.getReviewAuthor(),
        ipc.listComments(),
        ipc.listReviewSessions(),
      ]);
      // Every project gets a default "Review 1" session (item 4): new comments land in
      // a named session, not the unnamed "All comments" pool. Auto-created once, the
      // first time a project is opened with no sessions yet.
      let list = sessions;
      if (list.length === 0) {
        try {
          list = await ipc.applySessionAction({ action: "create", title: "Review 1" });
        } catch {
          // No project open yet — leave empty; the panel renders its empty state.
        }
      }
      if (gen !== loadGen) return; // a newer load()/clear() ran while we awaited
      const wasLoaded = get().loaded;
      const current = get().activeSessionId;
      const projectDir = useProjectStore.getState().project?.project_dir;
      const remembered = projectDir
        ? useSettingsStore.getState().projectUi[projectDir]
        : undefined;
      // First load restores the last-used session for THIS project (item: remember the
      // last session). A stored null = the explicit "All comments" pool; undefined (never
      // chosen) falls back to the oldest session ("Review 1", item 4). A later reload
      // (e.g. after a crunch) keeps the current pick as long as it still resolves.
      let activeSessionId: string | null;
      if (!wasLoaded) {
        const remId = remembered?.session_id;
        activeSessionId =
          remId === null
            ? null
            : remId && list.some((s) => s.id === remId)
              ? remId
              : (list[0]?.id ?? null);
      } else {
        activeSessionId =
          current === null || list.some((s) => s.id === current)
            ? current
            : (list[0]?.id ?? null);
      }
      // Restore the last-active status tab on first load too (item: remember the tab).
      const remTab = !wasLoaded ? remembered?.status_tab : undefined;
      const filterStatus = isStatusTab(remTab) ? remTab : get().filterStatus;
      set({ author, comments, sessions: list, activeSessionId, filterStatus, loaded: true });
    } catch {
      if (gen !== loadGen) return; // superseded — don't clobber the newer load's state
      // No vault yet — leave empty; the panel renders its empty state.
      set({ loaded: true });
    }
  },

  clear: () => {
    loadGen++; // cancel any in-flight load so it can't commit after we clear
    set({
      comments: [],
      sessions: [],
      activeSessionId: null,
      loaded: false,
      compose: null,
      openThreadId: null,
      threadPos: null,
      armed: false,
    });
  },

  setLeftTab: (leftTab) => set({ leftTab }),
  setFilterStatus: (filterStatus) => {
    set({ filterStatus });
    rememberProjectUi({ status_tab: filterStatus });
  },
  setFilterSeverity: (filterSeverity) => set({ filterSeverity }),
  setFilterView: (filterView) => set({ filterView }),
  arm: (armed) => set({ armed }),
  beginCompose: (compose) => set({ compose, armed: false, openThreadId: null }),
  cancelCompose: () => set({ compose: null }),
  openThread: (openThreadId, pos) =>
    set({ openThreadId, threadPos: pos ?? null, compose: null }),

  create: (anchor, body, severity, view) =>
    // On failure the composer stays open (compose: null is only set on success), so the
    // user's draft isn't lost — the toast tells them the save didn't land.
    guard("Couldn’t add comment", async () => {
      const indexes = useDesignStore.getState().indexes;
      const base = indexes ? anchorState(anchor, indexes) : null;
      const baseRevision = useProjectStore.getState().summary?.revision_id ?? "";
      const before = new Set(get().comments.map((c) => c.id));
      const comments = await ipc.applyReviewAction({
        action: "create",
        anchor,
        view,
        session_id: get().activeSessionId,
        body,
        severity,
        source: "human",
        base_revision: baseRevision,
        object_hash: base?.hash,
        object_meta: base?.meta,
        author_name: useSettingsStore.getState().authorName,
      });
      const created = comments.find((c) => !before.has(c.id));
      set({
        comments,
        compose: null,
        openThreadId: created?.id ?? null,
        threadPos: get().compose?.pos ?? null,
      });
    }),

  reply: (id, body) =>
    guard("Couldn’t post reply", async () => {
      const comments = await ipc.applyReviewAction({
        action: "reply",
        comment_id: id,
        body,
        author_name: useSettingsStore.getState().authorName,
      });
      set({ comments });
    }),
  setStatus: (id, status, reason) =>
    guard("Couldn’t update status", async () => {
      const comments = await ipc.applyReviewAction({ action: "status", comment_id: id, status, reason });
      set({ comments });
    }),
  setSeverity: (id, severity) =>
    guard("Couldn’t change severity", async () => {
      const comments = await ipc.applyReviewAction({ action: "severity", comment_id: id, severity });
      set({ comments });
    }),
  assign: (id, assignee) =>
    guard("Couldn’t change assignee", async () => {
      const comments = await ipc.applyReviewAction({ action: "assign", comment_id: id, assignee });
      set({ comments });
    }),
  del: (id) =>
    guard("Couldn’t delete comment", async () => {
      const comments = await ipc.applyReviewAction({ action: "delete", comment_id: id });
      set((s) => ({
        comments,
        openThreadId: s.openThreadId === id ? null : s.openThreadId,
      }));
    }),

  setActiveSession: (activeSessionId) => {
    set({ activeSessionId });
    rememberProjectUi({ session_id: activeSessionId });
  },
  createSession: (title) =>
    guard("Couldn’t create session", async () => {
      const before = new Set(get().sessions.map((s) => s.id));
      const sessions = await ipc.applySessionAction({ action: "create", title });
      const created = sessions.find((s) => !before.has(s.id));
      set({ sessions, activeSessionId: created?.id ?? get().activeSessionId });
    }),
  deleteSession: async (id) => {
    // Delete every comment that belongs to this session first (a session owns its
    // comments). One batch action, not one call per comment: each call rewrites the
    // whole event log and re-folds every log, so a big session used to cost N of both.
    try {
      const ids = get().comments.filter((c) => c.session_id === id).map((c) => c.id);
      let comments = get().comments;
      if (ids.length) {
        comments = await ipc.applyReviewAction({ action: "delete_many", comment_ids: ids });
      }
      const sessions = await ipc.applySessionAction({ action: "delete", session_id: id });
      // The BOM check strip summarises the session it filed into — those counts describe
      // comments that no longer exist, so drop them with the session.
      useBomCheckStore.getState().clearForSession(id);
      set((s) => ({
        sessions,
        comments,
        // Deleting the active session lands on the next remaining session, or the
        // "All comments" view (null) when none are left.
        activeSessionId: s.activeSessionId === id ? (sessions[0]?.id ?? null) : s.activeSessionId,
      }));
    } catch (e) {
      useToastStore.getState().push({ kind: "error", title: "Couldn’t delete session", message: String(e) });
      // The delete may have applied partially (comments gone, session still present) —
      // resync from the backend so the panel reflects what actually persisted.
      void get().load();
    }
  },
}));
