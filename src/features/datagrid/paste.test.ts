import { describe, expect, it } from "vitest";
import { planPaste } from "./paste";

const rect = (r0: number, c0: number, r1: number, c1: number) => ({ r0, c0, r1, c1 });

describe("planPaste", () => {
  it("fills every selected cell with a single value", () => {
    const plan = planPaste([["x"]], [rect(1, 1, 2, 2)], { r: 2, c: 2 }, 10, 5);
    expect(plan.targets).toEqual([
      { r: 1, c: 1, text: "x" },
      { r: 1, c: 2, text: "x" },
      { r: 2, c: 1, text: "x" },
      { r: 2, c: 2, text: "x" },
    ]);
    expect(plan.clippedRows).toBe(0);
  });

  it("pastes a single value into a single cell", () => {
    expect(planPaste([["x"]], [rect(3, 0, 3, 0)], { r: 3, c: 0 }, 10, 5).targets).toEqual([
      { r: 3, c: 0, text: "x" },
    ]);
  });

  it("anchors a block at the top-left of the first range and clips it", () => {
    const block = [
      ["a", "b", "c"],
      ["d", "e", "f"],
      ["g", "h", "i"],
    ];
    const plan = planPaste(block, [rect(8, 3, 9, 4)], { r: 9, c: 4 }, 10, 5);
    expect(plan.targets).toEqual([
      { r: 8, c: 3, text: "a" },
      { r: 8, c: 4, text: "b" },
      { r: 9, c: 3, text: "d" },
      { r: 9, c: 4, text: "e" },
    ]);
    expect(plan.clippedRows).toBe(1);
    expect(plan.clippedCols).toBe(1);
  });

  it("anchors at the focus when nothing is selected", () => {
    const plan = planPaste([["a", "b"]], [], { r: 0, c: 1 }, 2, 5);
    expect(plan.targets).toEqual([
      { r: 0, c: 1, text: "a" },
      { r: 0, c: 2, text: "b" },
    ]);
    expect(planPaste([["a"]], [], null, 2, 2).targets).toEqual([]);
  });

  it("fills every range of a multi-range selection once, or anchors a block at the first", () => {
    const ranges = [rect(0, 0, 0, 1), rect(0, 1, 1, 1), rect(5, 0, 5, 0)];
    const fill = planPaste([["z"]], ranges, { r: 5, c: 0 }, 10, 3);
    expect(fill.targets.map((t) => `${t.r}:${t.c}`)).toEqual(["0:0", "0:1", "1:1", "5:0"]);
    const block = planPaste([["1", "2"]], [rect(4, 1, 4, 1), rect(0, 0, 0, 0)], null, 10, 3);
    expect(block.targets).toEqual([
      { r: 4, c: 1, text: "1" },
      { r: 4, c: 2, text: "2" },
    ]);
  });
});
