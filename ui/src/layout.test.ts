import { describe, expect, it } from "vitest";
import { dividers, fuzzyScore, leaves, neighbor } from "./layout";
import type { LayoutCell } from "./types";

// 81x24{ 40x24[40x12 %1, 40x11 %2], 40x24 %3 }  (as produced by tmux)
const L: LayoutCell = {
  width: 81, height: 24, x: 0, y: 0, type: "horizontal",
  value: [
    { width: 40, height: 24, x: 0, y: 0, type: "vertical", value: [
      { width: 40, height: 12, x: 0, y: 0, type: "pane", value: "%1" },
      { width: 40, height: 11, x: 0, y: 13, type: "pane", value: "%2" },
    ] },
    { width: 40, height: 24, x: 41, y: 0, type: "pane", value: "%3" },
  ],
};

describe("layout", () => {
  it("collects leaves in order", () => {
    expect(leaves(L).map((l) => l.pane)).toEqual(["%1", "%2", "%3"]);
  });

  it("places dividers in the 1-cell gaps with paths", () => {
    const d = dividers(L);
    expect(d).toContainEqual({ path: [0], index: 0, orientation: "row", x: 0, y: 12, width: 40, height: 1 });
    expect(d).toContainEqual({ path: [], index: 0, orientation: "col", x: 40, y: 0, width: 1, height: 24 });
    expect(d).toHaveLength(2);
  });

  it("finds directional neighbours", () => {
    const all = leaves(L);
    expect(neighbor(all, "%1", "right")).toBe("%3");
    expect(neighbor(all, "%1", "down")).toBe("%2");
    expect(neighbor(all, "%3", "left")).toBe("%1"); // overlaps %1 more (12 vs 11 rows)
    expect(neighbor(all, "%1", "up")).toBeNull();
  });

  it("fuzzy matches", () => {
    expect(fuzzyScore("w17", "w17")).not.toBeNull();
    expect(fuzzyScore("dvsrv", "dev-server")).not.toBeNull();
    expect(fuzzyScore("zz", "dev-server")).toBeNull();
    expect(fuzzyScore("ser", "server")!).toBeGreaterThan(fuzzyScore("ser", "s-e-r")!);
  });
});
