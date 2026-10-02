import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewProgress } from "./ReviewProgress";
import { useAgentReviewStore, type ActivityEntry } from "../../stores/agentReviewStore";
import type { RunStatus } from "../../lib/ipc";

// The bar answers "how far along"; the panel behind it answers "where is it stuck",
// which is the question a ten-minute run actually provokes. What is pinned here is
// that the bar reads off the review server's own counts, that a run which stopped
// reporting says so rather than holding a number, and that each open step is a live
// row in the feed, with a clock only while a sub-agent works it.

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
      stepStartedAt: null,
      activity: [
        at("2026-08-25T10:09:37.000Z", "Collecting datasheets", "step", 1),
        at("2026-08-25T10:16:21.000Z", "Reviewing 88 parts", "step", 2),
      ],
    });
  });

  it("fills the middle of the bar on parts accounted for", () => {
    render(<ReviewProgress />);
    // Half the parts is the middle of the 15-to-88 span, not half the bar: the
    // deterministic layer really did happen before it.
    expect(screen.getByText("52%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "52");
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

  it("prints no gap between rows", () => {
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.queryByText(/^\+/)).toBeNull();
  });

  it("shows how long the open step has been running, so a slow step does not look frozen", () => {
    useAgentReviewStore.setState({ stepStartedAt: Date.now() - 192_000 });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByText(/on this step for 3m12s/)).toBeInTheDocument();
    expect(screen.getByText(/Reviewing parts · 44 of 88/)).toBeInTheDocument();
  });

  it("shows each open step as a live feed row, with a clock only while it is worked", () => {
    const base = useAgentReviewStore.getState().status;
    if (!base) throw new Error("the fixture has no status");
    const ts = (agoMs: number) => new Date(Date.now() - agoMs).toISOString();
    useAgentReviewStore.setState({
      status: {
        ...base,
        steps_done: 1,
        open_steps: [
          { step: "verify_parts#2", index: 2, opened_ts: ts(300_000), handed_out_ts: ts(192_000) },
          { step: "verify_parts#3", index: 3, opened_ts: ts(30_000), handed_out_ts: null },
        ],
      },
    });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    const log = screen.getByRole("log");
    expect(screen.getByText("Step 2 in progress").parentElement).toHaveTextContent(/Step 2 in progress3m12s$/);
    expect(screen.getByText("Step 3 waiting to start").parentElement).not.toHaveTextContent(/\d+s$/);
    expect(log).toHaveTextContent("Reviewing parts · 44 of 88");
    expect(log).not.toHaveTextContent(/Steps? 2 and 3/);
  });

  it("gives a single open step its own row too", () => {
    const base = useAgentReviewStore.getState().status;
    if (!base) throw new Error("the fixture has no status");
    useAgentReviewStore.setState({
      status: {
        ...base,
        open_steps: [
          { step: "verify_parts#2", index: 2, opened_ts: new Date().toISOString(), handed_out_ts: null },
        ],
      },
    });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByText("Step 2 waiting to start")).toBeInTheDocument();
  });

  it("says so when the run has not reported in yet", () => {
    useAgentReviewStore.setState({ activity: [] });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByText(/Nothing yet/)).toBeInTheDocument();
  });
});
