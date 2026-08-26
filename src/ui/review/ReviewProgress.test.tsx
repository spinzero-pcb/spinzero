import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReviewProgress } from "./ReviewProgress";
import { useDetailedReviewStore } from "../../stores/detailedReviewStore";

// The bar answers "how far along"; the panel behind it answers "where is it stuck",
// which is the question a ten-minute run actually provokes. What is pinned here is
// that the panel opens off the bar at all, and that the one row that answers the
// question — the six minutes where nothing happened — is legible as a gap.

const at = (iso: string, text: string, tone: "step" | "tool" | "waiting" | "error", seq: number) => ({
  seq,
  ts: iso,
  tone,
  text,
});

describe("ReviewProgress activity panel", () => {
  beforeEach(() => {
    useDetailedReviewStore.setState({
      phase: "running",
      step: 2,
      progress: "Reviewing against datasheets",
      reviewProgress: { reviewed: 2, candidates: 16, datasheetsRead: 11 },
      activity: [
        at("2026-08-25T10:09:37.000Z", "read_datasheet - turn 3", "tool", 1),
        at("2026-08-25T10:16:21.000Z", "read_datasheet - turn 5", "tool", 2),
      ],
    });
  });

  it("stays closed until the bar is clicked", () => {
    render(<ReviewProgress />);
    expect(screen.queryByRole("log")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByRole("log")).toBeInTheDocument();
    expect(screen.getAllByText(/read_datasheet/)).toHaveLength(2);
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
    useDetailedReviewStore.setState({ activity: [] });
    render(<ReviewProgress />);
    fireEvent.click(screen.getByRole("button", { name: /progress/i }));
    expect(screen.getByText(/Nothing yet/)).toBeInTheDocument();
  });
});
