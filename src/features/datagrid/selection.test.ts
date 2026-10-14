import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  isCellSelected,
  isRowSelected,
  mergeSpans,
  selectedColumns,
  selectedRowCount,
  selectedRows,
  selectionReducer,
  selectionRect,
  type SelectionAction,
  type SelectionState,
} from "./selection";

const b = { rows: 10, cols: 5 };
const run = (actions: SelectionAction[], start: SelectionState = EMPTY_SELECTION) =>
  actions.reduce((s, a) => selectionReducer(s, a, b), start);
const click = (r: number, c: number, mods: { shift?: boolean; meta?: boolean } = {}) =>
  ({ type: "click", pos: { r, c }, shift: !!mods.shift, meta: !!mods.meta }) as const;
const move = (dr: number, dc: number, mods: { shift?: boolean; jump?: boolean } = {}) =>
  ({ type: "move", dr, dc, shift: !!mods.shift, jump: !!mods.jump }) as const;

describe("selectionReducer (rows, TablePlus)", () => {
  it("a click selects the row and focuses the clicked cell", () => {
    const s = run([click(1, 3)]);
    expect(s.focus).toEqual({ r: 1, c: 3 });
    expect(selectedRows(s)).toEqual([1]);
    // Every cell of the row is selected, none of another row.
    expect(isCellSelected(s, 1, 0)).toBe(true);
    expect(isCellSelected(s, 1, 4)).toBe(true);
    expect(isCellSelected(s, 2, 3)).toBe(false);
  });

  it("⇧-click extends the row range, ⌘-click toggles rows", () => {
    let s = run([click(1, 1), click(3, 4, { shift: true })]);
    expect(selectedRows(s)).toEqual([1, 2, 3]);
    expect(s.focus).toEqual({ r: 3, c: 4 });
    // ⇧ again re-extends from the same anchor.
    s = run([click(2, 0, { shift: true })], s);
    expect(selectedRows(s)).toEqual([1, 2]);
    // ⌘ adds a row, then removes rows (splitting a range around it).
    s = run([click(8, 0, { meta: true })], s);
    expect(selectedRows(s)).toEqual([1, 2, 8]);
    s = run([click(1, 2, { meta: true })], s);
    expect(selectedRows(s)).toEqual([2, 8]);
    expect(s.focus).toEqual({ r: 1, c: 2 });
    s = run([click(5, 0), click(7, 0, { shift: true }), click(6, 0, { meta: true })]);
    expect(selectedRows(s)).toEqual([5, 7]);
    // ⇧ after ⌘ keeps the earlier rows and extends the last range (Finder).
    s = run([click(1, 0), click(5, 0, { meta: true }), click(7, 0, { shift: true })]);
    expect(selectedRows(s)).toEqual([1, 5, 6, 7]);
  });

  it("a drag extends the pressed row range and moves the focus along", () => {
    let s = run([click(4, 1), { type: "dragTo", pos: { r: 6, c: 2 } }]);
    expect(selectedRows(s)).toEqual([4, 5, 6]);
    expect(s.focus).toEqual({ r: 6, c: 2 });
    s = run([{ type: "dragTo", pos: { r: 2, c: 2 } }], s);
    expect(selectedRows(s)).toEqual([2, 3, 4]);
    expect(selectionReducer(s, { type: "dragTo", pos: { r: 2, c: 2 } }, b)).toBe(s);
  });

  it("↑↓ move the row selection, ⇧ extends it, ←→ only move the focused cell", () => {
    let s = run([{ type: "set", pos: { r: 0, c: 0 } }, move(-1, 0)]);
    expect(s.focus).toEqual({ r: 0, c: 0 });
    s = run([move(1, 0, { shift: true }), move(1, 0, { shift: true })], s);
    expect(selectedRows(s)).toEqual([0, 1, 2]);
    expect(s.focus).toEqual({ r: 2, c: 0 });
    // → keeps the selected rows and moves the focused cell; ⌘→ jumps to the last column.
    s = run([move(0, 1)], s);
    expect(selectedRows(s)).toEqual([0, 1, 2]);
    expect(s.focus).toEqual({ r: 2, c: 1 });
    s = run([move(0, 1, { jump: true })], s);
    expect(s.focus).toEqual({ r: 2, c: 4 });
    expect(run([move(0, 1)], s)).toBe(s);
    // A plain ↓ collapses to the next row; ⌘↓ jumps to the last.
    s = run([move(1, 0)], s);
    expect(selectedRows(s)).toEqual([3]);
    expect(s.focus).toEqual({ r: 3, c: 4 });
    s = run([move(1, 0, { jump: true })], s);
    expect(s.focus).toEqual({ r: 9, c: 4 });
    // ⇧↑ shrinks back towards the anchor and past it.
    s = run([click(5, 0), move(1, 0, { shift: true }), move(-1, 0, { shift: true })]);
    expect(selectedRows(s)).toEqual([5]);
    s = run([move(-1, 0, { shift: true })], s);
    expect(selectedRows(s)).toEqual([4, 5]);
  });

  it("selects every row, a row by its number and rows just added", () => {
    let s = run([click(2, 3), { type: "selectAll" }]);
    expect(selectedRowCount(s)).toBe(10);
    expect(s.focus).toEqual({ r: 2, c: 3 });
    // A row number keeps the focused column.
    s = run([{ type: "selectRow", r: 6, shift: false, meta: false }], s);
    expect(selectedRows(s)).toEqual([6]);
    expect(s.focus).toEqual({ r: 6, c: 3 });
    s = run([{ type: "selectRow", r: 8, shift: true, meta: false }], s);
    expect(selectedRows(s)).toEqual([6, 7, 8]);
    s = run([{ type: "selectRows", r0: 7, r1: 9, focus: { r: 7, c: 0 } }], s);
    expect(selectedRows(s)).toEqual([7, 8, 9]);
    expect(s.focus).toEqual({ r: 7, c: 0 });
  });

  it("focus moves the focused cell inside the selection; collapse keeps its row", () => {
    let s = run([
      click(1, 0),
      click(4, 0, { shift: true }),
      { type: "focus", pos: { r: 2, c: 3 } },
    ]);
    expect(selectedRows(s)).toEqual([1, 2, 3, 4]);
    expect(s.focus).toEqual({ r: 2, c: 3 });
    s = run([{ type: "collapse" }], s);
    expect(selectedRows(s)).toEqual([2]);
    expect(run([{ type: "clear" }], s)).toBe(EMPTY_SELECTION);
  });

  it("whole columns (⌥-click a header) replace the row selection", () => {
    let s = run([click(3, 0), { type: "selectColumn", c: 1, shift: false, meta: false }]);
    expect(selectedRows(s)).toEqual([]);
    expect(selectedColumns(s)).toEqual([1]);
    expect(s.focus).toEqual({ r: 3, c: 1 });
    expect(isCellSelected(s, 9, 1)).toBe(true);
    expect(isCellSelected(s, 3, 0)).toBe(false);
    s = run([{ type: "selectColumn", c: 3, shift: true, meta: false }], s);
    expect(selectedColumns(s)).toEqual([1, 2, 3]);
    s = run([{ type: "selectColumn", c: 2, shift: false, meta: true }], s);
    expect(selectedColumns(s)).toEqual([1, 3]);
    // A click goes back to rows; so does ↓ (from the focused row).
    expect(selectedRows(run([click(0, 0)], s))).toEqual([0]);
    const down = run([move(1, 0)], s);
    expect(down.cols).toEqual([]);
    expect(selectedRows(down)).toEqual([4]);
  });

  it("clamps to new bounds, dropping rows past the end", () => {
    const s = run([click(2, 4), click(8, 4, { meta: true }), click(9, 4, { shift: true })]);
    const fewer = selectionReducer(s, { type: "clamp" }, { rows: 5, cols: 3 });
    expect(selectedRows(fewer)).toEqual([2]);
    expect(fewer.focus).toEqual({ r: 4, c: 2 });
    // Every selected row gone: the focused row is selected.
    const gone = selectionReducer(run([click(8, 0)]), { type: "clamp" }, { rows: 3, cols: 5 });
    expect(selectedRows(gone)).toEqual([2]);
    expect(selectionReducer(s, { type: "clamp" }, b)).toBe(s);
    expect(selectionReducer(s, { type: "selectAll" }, { rows: 0, cols: 5 })).toBe(EMPTY_SELECTION);
  });

  it("counts and merges runs without listing them", () => {
    const s = run([click(1, 0), click(3, 0, { shift: true }), click(2, 0, { meta: true })]);
    expect(isRowSelected(s, 2)).toBe(false);
    expect(selectedRowCount(s)).toBe(2);
    expect(
      mergeSpans([
        { anchor: 5, focus: 3 },
        { anchor: 6, focus: 6 },
        { anchor: 0, focus: 1 },
      ]),
    ).toEqual([
      [0, 1],
      [3, 6],
    ]);
  });
});

describe("selectionReducer (cell ranges)", () => {
  const drag = (r: number, c: number) => ({ type: "dragTo", pos: { r, c } }) as const;

  it("a drag that stays in its column selects rows; across columns it selects a cell range", () => {
    let s = run([click(1, 1), drag(3, 1)]);
    expect(selectionRect(s)).toBeNull();
    expect(selectedRows(s)).toEqual([1, 2, 3]);
    s = run([drag(3, 3)], s);
    expect(selectionRect(s)).toEqual({ r0: 1, r1: 3, c0: 1, c1: 3 });
    expect(selectedRows(s)).toEqual([1, 2, 3]);
    expect(isCellSelected(s, 2, 2)).toBe(true);
    expect(isCellSelected(s, 2, 0)).toBe(false);
    expect(isCellSelected(s, 4, 2)).toBe(false);
    // Back over the start: still a range (one cell wide), until a plain click.
    s = run([drag(0, 1)], s);
    expect(selectionRect(s)).toEqual({ r0: 0, r1: 1, c0: 1, c1: 1 });
    s = run([click(5, 2)], s);
    expect(selectionRect(s)).toBeNull();
    expect(selectedRows(s)).toEqual([5]);
  });

  it("a drag down the row numbers stays a row selection", () => {
    const s = run([click(1, 1), { type: "dragTo", pos: { r: 4, c: 3 }, rowsOnly: true }]);
    expect(selectionRect(s)).toBeNull();
    expect(selectedRows(s)).toEqual([1, 2, 3, 4]);
  });

  it("⇧← / ⇧→ start a range from the selected rows; ⇧↑ / ⇧↓ grow it; plain arrows end it", () => {
    let s = run([click(2, 1), move(1, 0, { shift: true }), move(0, 1, { shift: true })]);
    expect(selectionRect(s)).toEqual({ r0: 2, r1: 3, c0: 1, c1: 2 });
    s = run([move(1, 0, { shift: true }), move(0, -2, { shift: true })], s);
    expect(selectionRect(s)).toEqual({ r0: 2, r1: 4, c0: 0, c1: 1 });
    expect(selectedRows(s)).toEqual([2, 3, 4]);
    s = run([move(0, 1)], s);
    expect(selectionRect(s)).toBeNull();
    expect(selectedRows(s)).toEqual([4]);
  });

  it("⇧-click grows a range; focusing a cell inside it keeps it; clamp cuts it", () => {
    let s = run([click(0, 0), move(0, 1, { shift: true }), click(2, 3, { shift: true })]);
    expect(selectionRect(s)).toEqual({ r0: 0, r1: 2, c0: 0, c1: 3 });
    s = run([{ type: "focus", pos: { r: 1, c: 1 } }], s);
    expect(selectionRect(s)).toEqual({ r0: 0, r1: 2, c0: 0, c1: 3 });
    expect(s.focus).toEqual({ r: 1, c: 1 });
    s = selectionReducer(s, { type: "clamp" }, { rows: 2, cols: 2 });
    expect(selectionRect(s)).toEqual({ r0: 0, r1: 1, c0: 0, c1: 1 });
    expect(selectedRows(s)).toEqual([0, 1]);
    expect(selectionReducer(s, { type: "clamp" }, { rows: 2, cols: 2 })).toBe(s);
  });

  it("selecting all rows or a column ends a range", () => {
    const s = run([click(0, 0), move(0, 1, { shift: true })]);
    expect(selectionRect(run([{ type: "selectAll" }], s))).toBeNull();
    expect(
      selectionRect(run([{ type: "selectColumn", c: 2, shift: false, meta: false }], s)),
    ).toBeNull();
  });
});
