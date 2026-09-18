import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewProgress } from "./ReviewProgress";
import { useAgentReviewStore, type ActivityEntry } from "../../stores/agentReviewStore";
import type { RunStatus } from "../../lib/ipc";

// The bar answers "how far along"; the panel behind it answers "where is it stuck",
// which is the question a ten-minute run actually provokes. What is pinned here is
// that the bar reads off the review server's own counts, that a run which stopped
// reporting says so rather than holding a number, and that the one row that answers
// "where did the time go" is legible as a gap.

const at = (iso: string, text: string, tone: ActivityEntry["tone"], seq: number): ActivityEntry => ({
  seq,
  ts: iso,
  tone,
  text,
});

function status(patch: Partial<RunStatus> = {}): RunStatus {
  return {
    status_version: 1,
    review_id: "r1",
    pipeline: "bom-detailed",
    profile: "commercial",
    project_dir: "C:/boards/MC-02",
    phase: "step_open",
    stage: null,
    steps_done: 1,
    steps_total: 4,
    parts_done: 44,
    parts_total: 88,
    datasheets_read: 88,
    datasheets_total: 88,
    started_ts: "2026-08-25T10:00:00.000Z",
    updated_ts: new Date().toISOString(),
    findings_path: null,
    report_path: null,
    error: null,
    ...patch,
  };
}

describe("ReviewProgress", () => {
  beforeEach(() => {
    useAgentReviewStore.setState({
      phase: "running",
      line: "",
      error: null,
      status: status(),
      activity: [
        at("2026-08-25T10:09:37.000Z", "Collecting datasheets", "step", 1),
        at("2026-08-25T10:16:21.000Z", "Reviewing 88 parts", "step", 2),
      ],
    });
  });

  it("fills the middle of the bar on parts accounted for", () => {
    render(<ReviewProgress />);
    // Half the parts is the middle of the 30-to-95 span, not half the bar: the
    // deterministic layer really did happen before it.
    expect(screen.getByText("63%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "63");
  });

  it("says a run that stopped reporting has stalled instead of holding a number", () => {
    useAgentReviewStore.setState({ status: status({ updated_ts: "2026-08-25T10:00:00.000Z" }) });
    render(<ReviewProgress />);
    expect(screen.getByText("stalled")).toBeInTheDocument();
  });

  it("stays closed until the bar is clicked", () => {
    render(<ReviewProgress />);
    expect(screen.queryByRole("log")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByRole("log")).toBeInTheDocument();
    expect(screen.getByText("Collecting datasheets")).toBeInTheDocument();
  });

  it("prints the gap that names the time nobody could account for", () => {
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    // 10:09:37 to 10:16:21 is where the run went; a run of same-millisecond events
    // gets no gap at all, so the number only ever appears where it means something.
    expect(screen.getByText("+6m44s")).toBeInTheDocument();
    expect(screen.queryByText(/^\+0s$/)).toBeNull();
  });

  it("says so when the run has not reported in yet", () => {
    useAgentReviewStore.setState({ activity: [] });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByText(/Nothing yet/)).toBeInTheDocument();
  });
});
