import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { PcbGlView } from "./PcbGlView";
import { pcbNav } from "../canvas/navigator";
import { useReviewStore } from "../../stores/reviewStore";

// Layer-1 smoke coverage for the batch-4 port (comment chips + right-click menu).
// jsdom has no WebGL, so the renderer stays null — which is exactly the path that
// exercises the React/store wiring: the context menu still opens with its always-on
// actions, and a chip pushed through the pcbNav bridge lands in the comment layer.
describe("PcbGlView batch-4 wiring", () => {
  beforeEach(() => {
    // The render loop and renderer need browser APIs jsdom lacks; stub them so the
    // component mounts cleanly with no GL renderer.
    vi.stubGlobal("requestAnimationFrame", () => 0);
    vi.stubGlobal("cancelAnimationFrame", () => {});
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    useReviewStore.setState({ armed: false });
  });

  it("opens the right-click menu with the always-present actions", () => {
    const { container } = render(<PcbGlView visible={true} />);
    fireEvent.contextMenu(container.firstChild as HTMLElement, { clientX: 80, clientY: 80 });
    expect(screen.getByText("Fit to screen")).toBeInTheDocument();
    expect(screen.getByText("Clear highlights")).toBeInTheDocument();
  });

  it("renders a comment chip pushed through the pcbNav bridge", () => {
    render(<PcbGlView visible={true} />);
    act(() => {
      pcbNav.setComments([
        { id: "c1", number: 7, status: "open", severity: null, anchor: { type: "net", ref: "GND", at: { x: 5, y: 5 } } },
      ]);
    });
    const chip = screen.getByText("7");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveClass("cmt-chip");
  });
});
