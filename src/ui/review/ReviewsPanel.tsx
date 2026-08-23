import { useBomCheckStore } from "../../stores/bomCheckStore";
import { useBomMappingStore } from "../../stores/bomMappingStore";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";
import { useReviewStore } from "../../stores/reviewStore";
import { BOM_PROFILES, isBomProfile } from "../../lib/findings";
import { IconChecklist, IconSparkle } from "../icons";

// Reviews — where checks are launched, one entry per check the app can run.
//
// It is a rail surface rather than a view tab because a review is not a view of one
// artifact: the BOM check is here today and the schematic check lands beside it, and
// neither belongs to whichever canvas happens to be on screen. The findings go to the
// Review rail as comments; nothing about a result is rendered twice here.
//
// The premium case for the detailed tier is computed from this project's own comments,
// not written as copy. What a detailed review demonstrably added is the set of findings
// it filed that the deterministic rules never produced — and `bomcheck::ingest` makes
// that set readable directly: a paid finding matching a free one by fingerprint UPDATES
// the free comment (which keeps `source: "rule"`), so a comment still carrying
// `source: "agent"` is one the rules could not reach. Before a detailed run exists
// there is nothing to claim, and the panel says what the tier does instead of
// inventing numbers.

/** Severity mix of a set of comments, worst first — the shape of what a tier found. */
const SEV_ORDER = ["critical", "major", "minor", "info"] as const;
const SEV_LABEL: Record<string, string> = {
  critical: "Critical",
  major: "Major",
  minor: "Minor",
  info: "Info",
};

export function ReviewsPanel() {
  const running = useBomCheckStore((s) => s.running);
  const profile = useBomCheckStore((s) => s.profile);
  const setProfile = useBomCheckStore((s) => s.setProfile);
  const run = useBomCheckStore((s) => s.run);
  const checkError = useBomCheckStore((s) => s.error);

  const detailedPhase = useDetailedReviewStore((s) => s.phase);
  const detailedProgress = useDetailedReviewStore((s) => s.progress);
  const detailedError = useDetailedReviewStore((s) => s.error);
  const liveFindings = useDetailedReviewStore((s) => s.liveFindings);
  const detailedBusy =
    detailedPhase === "submitting" || detailedPhase === "running" || detailedPhase === "ingesting";

  const comments = useReviewStore((s) => s.comments);
  const setLeftTab = useReviewStore((s) => s.setLeftTab);
  const setActiveSession = useReviewStore((s) => s.setActiveSession);
  const setFilterSeverity = useReviewStore((s) => s.setFilterSeverity);

  // Open findings by producer. Resolved and dismissed ones are excluded: this is the
  // case for running the tier again, not a lifetime tally.
  const open = comments.filter((c) => c.status === "open");
  const byRules = open.filter((c) => c.source === "rule");
  const byDetailed = open.filter((c) => c.source === "agent");

  const detailedMix = SEV_ORDER.map((sev) => ({
    sev,
    n: byDetailed.filter((c) => c.severity === sev).length,
  })).filter((s) => s.n > 0);

  /** Show one producer's findings in the Review rail. */
  function showFindings(source: "rule" | "agent") {
    const ids = new Set(
      open.filter((c) => c.source === source).map((c) => c.session_id ?? ""),
    );
    // One session (the normal case) → land on it; several → the all-comments pool,
    // since no single session holds them.
    setActiveSession(ids.size === 1 ? ([...ids][0] || null) : null);
    setFilterSeverity("all");
    setLeftTab("review");
  }

  return (
    <div className="reviews-panel">
      <div className="reviews-head">
        <span className="reviews-title">Reviews</span>
        <select
          className="bom-select"
          value={profile}
          disabled={running || detailedBusy}
          title="End application — decides which rules apply and how severe a gap is"
          onChange={(e) => isBomProfile(e.target.value) && setProfile(e.target.value)}
        >
          {BOM_PROFILES.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      </div>

      <div className="reviews-card">
        <div className="reviews-card-head">
          <IconChecklist size={14} />
          <span className="reviews-card-title">Bill of materials</span>
        </div>

        <div className="reviews-tier">
          <div className="reviews-tier-head">
            <span className="reviews-tier-name">Checks</span>
            <span className="reviews-tier-tag">included</span>
          </div>
          <p className="reviews-tier-what">
            Deterministic rules over the BOM columns — missing part numbers, DNP
            conflicts, spec mismatches against the value and footprint.
          </p>
          <div className="reviews-tier-actions">
            <button className="btn-primary reviews-run" disabled={running} onClick={() => void run()}>
              {running ? "Checking…" : "Run checks"}
            </button>
            {byRules.length > 0 && (
              <button className="btn-ghost reviews-open" onClick={() => showFindings("rule")}>
                {byRules.length} open
              </button>
            )}
          </div>
        </div>

        <div className="reviews-tier">
          <div className="reviews-tier-head">
            <IconSparkle size={12} />
            <span className="reviews-tier-name">Detailed review</span>
            <span className="reviews-tier-tag premium">premium</span>
          </div>
          <p className="reviews-tier-what">
            Reads the datasheets behind the part numbers and judges what a column cannot
            state: whether a part is really qualified, really in production, really the
            one the design needs.
          </p>

          {/* The case for the tier, from this project's own runs. */}
          {byDetailed.length > 0 ? (
            <div className="reviews-delta">
              <span className="reviews-delta-n">
                +{byDetailed.length} found here that the rules could not
              </span>
              {detailedMix.length > 0 && (
                <span className="reviews-delta-mix">
                  {detailedMix.map((s) => `${s.n} ${SEV_LABEL[s.sev]}`).join(" · ")}
                </span>
              )}
              <button className="btn-ghost reviews-open" onClick={() => showFindings("agent")}>
                Show them
              </button>
            </div>
          ) : (
            <p className="reviews-delta empty">
              Not run on this project yet — so there is nothing here it found that the
              checks above did not. Run it once and this becomes that number.
            </p>
          )}

          <div className="reviews-tier-actions">
            <button
              className="btn-primary reviews-run"
              disabled={running || detailedBusy}
              onClick={() => void useDetailedReviewStore.getState().openPreflight()}
            >
              {detailedBusy ? "Reviewing…" : "Run detailed review"}
            </button>
            {detailedBusy && (
              <button
                className="btn-ghost reviews-open"
                onClick={() => void useDetailedReviewStore.getState().cancel()}
              >
                Cancel
              </button>
            )}
          </div>
          {detailedBusy && (
            <p className="reviews-progress">
              {detailedProgress}
              {liveFindings > 0 ? ` · ${liveFindings} so far` : ""}
            </p>
          )}
        </div>

        <div className="reviews-card-foot">
          <button
            className="btn-ghost"
            title="Show which BOM column each check reads, and correct it"
            onClick={() => void useBomMappingStore.getState().openDialog(profile)}
          >
            Column mapping
          </button>
        </div>
      </div>

      {/* Announced, not built: the panel is a list of checks, and a BOM-only list would
          read as a BOM page that happens to live in the rail. */}
      <div className="reviews-card soon">
        <div className="reviews-card-head">
          <IconChecklist size={14} />
          <span className="reviews-card-title">Schematic</span>
          <span className="reviews-tier-tag">soon</span>
        </div>
        <p className="reviews-tier-what">
          Connectivity, power and reference-designator checks over the extracted
          schematic.
        </p>
      </div>

      {checkError && <p className="bom-check-warn">Checks failed: {checkError}</p>}
      {detailedError && <p className="bom-check-warn">Detailed review: {detailedError}</p>}
    </div>
  );
}
