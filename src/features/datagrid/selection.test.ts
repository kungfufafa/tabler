import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  isCellSelected,
  selectedRows,
  selectionBounds,
  selectionReducer,
  type SelectionState,
} from "./selection";

const b = { rows: 10, cols: 5 };
const run = (
  actions: Parameters<typeof selectionReducer>[1][],
  start: SelectionState = EMPTY_SELECTION,
) => actions.reduce((s, a) => selectionReducer(s, a, b), start);

describe("selectionReducer", () => {
  it("click, shift-click and meta-click", () => {
    let s = run([{ type: "click", pos: { r: 1, c: 1 }, shift: false, meta: false }]);
    expect(s.focus).toEqual({ r: 1, c: 1 });
    s = run([{ type: "click", pos: { r: 3, c: 3 }, shift: true, meta: false }], s);
    expect(selectionBounds(s)).toEqual({ r0: 1, c0: 1, r1: 3, c1: 3 });
    expect(isCellSelected(s, 2, 2)).toBe(true);
    expect(isCellSelected(s, 4, 4)).toBe(false);
    s = run([{ type: "click", pos: { r: 8, c: 0 }, shift: false, meta: true }], s);
    expect(s.ranges).toHaveLength(2);
    expect(selectedRows(s)).toEqual([1, 2, 3, 8]);
  });

  it("keyboard movement clamps, extends with shift and jumps with meta", () => {
    let s = run([
      { type: "set", pos: { r: 0, c: 0 } },
      { type: "move", dr: -1, dc: 0, shift: false, jump: false },
    ]);
    expect(s.focus).toEqual({ r: 0, c: 0 });
    s = run(
      [
        { type: "move", dr: 1, dc: 0, shift: true, jump: false },
        { type: "move", dr: 1, dc: 0, shift: true, jump: false },
      ],
      s,
    );
    expect(selectionBounds(s)).toEqual({ r0: 0, c0: 0, r1: 2, c1: 0 });
    s = run([{ type: "move", dr: 0, dc: 1, shift: false, jump: true }], s);
    expect(s.focus).toEqual({ r: 2, c: 4 });
    expect(s.ranges).toHaveLength(1);
  });

  it("row/column/all selection", () => {
    let s = run([{ type: "selectRow", r: 2, shift: false, meta: false }]);
    expect(selectionBounds(s)).toEqual({ r0: 2, c0: 0, r1: 2, c1: 4 });
    s = run([{ type: "selectRow", r: 4, shift: true, meta: false }], s);
    expect(selectedRows(s)).toEqual([2, 3, 4]);
    s = run([{ type: "selectColumn", c: 1, shift: false, meta: false }]);
    expect(selectionBounds(s)).toEqual({ r0: 0, c0: 1, r1: 9, c1: 1 });
    s = run([{ type: "selectAll" }]);
    expect(selectionBounds(s)).toEqual({ r0: 0, c0: 0, r1: 9, c1: 4 });
    expect(selectionReducer(s, { type: "clamp" }, { rows: 3, cols: 2 }).ranges[0]?.focus).toEqual({
      r: 2,
      c: 1,
    });
    expect(selectionReducer(s, { type: "selectAll" }, { rows: 0, cols: 5 })).toBe(EMPTY_SELECTION);
  });
});
