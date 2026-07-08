import { describe, expect, it } from "vitest";
import { netLabelRows } from "./glRenderer";

// Pure-logic coverage for the net-label row layout (feedback 30.png): a pad's pin number must
// be larger than its net name (issue 1) and pad/via text must scale with zoom — uncapped —
// while tracks stay capped so a wide power track never gets giant letters (issue 2). Pad and
// track names are sized bigger than the original tuning (feedback: net names too small).

describe("netLabelRows", () => {
  it("a pad with a number and a net stacks them, number larger and on top", () => {
    const rows = netLabelRows({ key: "pads", num: "2", net: "Net-(D2-A)" }, 200);
    expect(rows.map((r) => r.text)).toEqual(["2", "Net-(D2-A)"]);
    // Pin number reads clearly larger than the net name (KiCad draws it larger).
    expect(rows[0].size).toBeGreaterThan(rows[1].size);
    expect(rows[0].size / rows[1].size).toBeCloseTo(0.34 / 0.26, 5);
    // Number sits in the top half, net name in the bottom half.
    expect(rows[0].cy).toBeLessThan(0);
    expect(rows[1].cy).toBeGreaterThan(0);
  });

  it("pad and via text is uncapped, so it grows with zoom", () => {
    // At a large on-screen size the pad number keeps scaling (no cap on pads).
    expect(netLabelRows({ key: "pads", num: "1", net: "+3V3" }, 400)[0].size).toBeCloseTo(136, 5);
    // Vias are unchanged (0.42 of the body) — they already read well.
    expect(netLabelRows({ key: "vias", net: "+5V" }, 300)[0].size).toBeCloseTo(126, 5);
  });

  it("track text is capped so a wide power track never gets giant letters", () => {
    expect(netLabelRows({ key: "tracks", net: "GND" }, 4000)[0].size).toBe(22);
  });

  it("a pad missing one of number/net shows the single label centred", () => {
    const numOnly = netLabelRows({ key: "pads", num: "7", net: "" }, 100);
    expect(numOnly).toHaveLength(1);
    expect(numOnly[0]).toMatchObject({ text: "7", cy: 0 });
    const netOnly = netLabelRows({ key: "pads", net: "VBUS" }, 100);
    expect(netOnly.map((r) => r.text)).toEqual(["VBUS"]);
  });

  it("an empty net with no number yields no rows", () => {
    expect(netLabelRows({ key: "tracks", net: "" }, 100)).toEqual([]);
  });
});
