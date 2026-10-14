import { describe, expect, it } from "vitest";
import {
  columnWidth,
  findFreeSpot,
  GAP,
  moveBy,
  resizeBy,
  resolveCollisions,
  ROW_H,
  rowsHeight,
  toRect,
  usedRows,
  type Layout,
} from "./layout";

const colW = 72; // a 952px canvas: 12 × 72 + 11 × 8
const cell = colW + GAP;
const row = ROW_H + GAP;

describe("board grid", () => {
  it("shares the width among 12 columns with gutters", () => {
    expect(columnWidth(12 * colW + 11 * GAP)).toBe(colW);
    expect(toRect({ x: 2, y: 1, w: 3, h: 2 }, colW)).toEqual({
      left: 2 * cell,
      top: row,
      width: 3 * colW + 2 * GAP,
      height: 2 * ROW_H + GAP,
    });
    expect(rowsHeight(0)).toBe(0);
    expect(rowsHeight(3)).toBe(3 * ROW_H + 2 * GAP);
  });

  it("snaps a move to the nearest cell and keeps it on the grid", () => {
    const start: Layout = { x: 2, y: 2, w: 4, h: 3 };
    expect(moveBy(start, cell * 0.4, row * 0.6, colW)).toEqual({ ...start, x: 2, y: 3 });
    expect(moveBy(start, cell * 1.6, -row * 1.4, colW)).toEqual({ ...start, x: 4, y: 1 });
    expect(moveBy(start, cell * 20, -row * 20, colW)).toEqual({ ...start, x: 8, y: 0 });
    expect(moveBy(start, -cell * 20, 0, colW)).toEqual({ ...start, x: 0 });
  });

  it("snaps a resize to whole cells within the minimum and the right edge", () => {
    const start: Layout = { x: 8, y: 0, w: 2, h: 2 };
    const min = { w: 2, h: 2 };
    expect(resizeBy(start, cell * 1.5, row * 2.2, colW, min)).toEqual({ ...start, w: 4, h: 4 });
    expect(resizeBy(start, cell * 10, 0, colW, min)).toEqual({ ...start, w: 4 });
    expect(resizeBy(start, -cell * 10, -row * 10, colW, min)).toEqual({ ...start, w: 2, h: 2 });
  });

  it("pushes the objects a moved one lands on down, in a cascade", () => {
    const items = [
      { id: "a", layout: { x: 0, y: 0, w: 6, h: 2 } },
      { id: "b", layout: { x: 0, y: 2, w: 6, h: 2 } },
      { id: "c", layout: { x: 6, y: 0, w: 6, h: 2 } },
      { id: "moved", layout: { x: 0, y: 1, w: 4, h: 2 } },
    ];
    const out = resolveCollisions(items, "moved");
    expect(Object.fromEntries(out.map((i) => [i.id, i.layout.y]))).toEqual({
      a: 3,
      b: 5,
      c: 0,
      moved: 1,
    });
    // Untouched objects keep their identity (no needless re-render / save).
    expect(out[2]).toBe(items[2]);
    expect(resolveCollisions(items, "nope")).toEqual(items);
  });

  it("finds the first free spot and the rows in use", () => {
    const layouts: Layout[] = [
      { x: 0, y: 0, w: 6, h: 2 },
      { x: 6, y: 0, w: 4, h: 1 },
    ];
    expect(findFreeSpot(layouts, 2, 1)).toEqual({ x: 10, y: 0 });
    expect(findFreeSpot(layouts, 3, 2)).toEqual({ x: 6, y: 1 });
    expect(findFreeSpot(layouts, 12, 1)).toEqual({ x: 0, y: 2 });
    expect(findFreeSpot([], 20, 1)).toEqual({ x: 0, y: 0 });
    expect(usedRows(layouts)).toBe(2);
    expect(usedRows([])).toBe(0);
  });
});
