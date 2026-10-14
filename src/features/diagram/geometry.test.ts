import { describe, expect, it } from "vitest";
import {
  crowsFootPath,
  fitText,
  oneEndMarks,
  relationSides,
  roundedPolyline,
  routeRelation,
  rowCenterY,
  STUB,
  type CardBox,
} from "./geometry";
import { CARD } from "./layout";
import { fitRow } from "./scene";

const box = (x: number, y: number, columns = ["id", "user_id", "name"], w = 200): CardBox => ({
  x,
  y,
  w,
  h: CARD.header + columns.length * CARD.row + CARD.padBottom,
  columns,
});

/** Every `x y` pair of a path. */
function points(d: string): Array<[number, number]> {
  const nums = d.match(/-?\d+(?:\.\d+)?/g)!.map(Number);
  const out: Array<[number, number]> = [];
  for (let i = 0; i + 1 < nums.length; i += 2) out.push([nums[i]!, nums[i + 1]!]);
  return out;
}

describe("rowCenterY", () => {
  it("is the middle of the column's row, or of the header", () => {
    const b = box(0, 100);
    expect(rowCenterY(b, "id")).toBe(100 + CARD.header + CARD.row / 2);
    expect(rowCenterY(b, "name")).toBe(100 + CARD.header + 2 * CARD.row + CARD.row / 2);
    expect(rowCenterY(b, "missing")).toBe(100 + CARD.header / 2);
    expect(rowCenterY(b, undefined)).toBe(100 + CARD.header / 2);
  });
});

describe("relationSides", () => {
  it("uses facing sides when the cards stand apart, else loops on the right", () => {
    expect(relationSides(box(0, 0), box(400, 0))).toEqual({ from: 1, to: -1 });
    expect(relationSides(box(400, 0), box(0, 0))).toEqual({ from: -1, to: 1 });
    // Stacked (overlapping horizontally) or too close for the markers.
    expect(relationSides(box(0, 0), box(50, 300))).toEqual({ from: 1, to: 1 });
    expect(relationSides(box(0, 0), box(200 + STUB, 0))).toEqual({ from: 1, to: 1 });
    // A self reference always loops.
    expect(relationSides(box(0, 0), box(400, 0), true)).toEqual({ from: 1, to: 1 });
  });
});

describe("routeRelation", () => {
  it("attaches the ends to the column rows on the facing sides", () => {
    const child = box(400, 0);
    const parent = box(0, 200);
    for (const style of ["curved", "orthogonal"] as const) {
      const g = routeRelation(child, "user_id", parent, "id", style);
      expect(g.from).toEqual({ x: 400, y: rowCenterY(child, "user_id"), dir: -1 });
      expect(g.to).toEqual({ x: 200, y: rowCenterY(parent, "id"), dir: 1 });
      const pts = points(g.path);
      expect(pts[0]).toEqual([g.from.x, g.from.y]);
      expect(pts[pts.length - 1]).toEqual([g.to.x, g.to.y]);
      // The label sits between the two cards.
      expect(g.mid.x).toBeGreaterThan(200);
      expect(g.mid.x).toBeLessThan(400);
    }
  });

  it("leaves and enters horizontally", () => {
    const g = routeRelation(box(0, 0), "user_id", box(500, 300), "id", "orthogonal");
    const pts = points(g.path);
    // First segment and last segment are horizontal.
    expect(pts[1]![1]).toBe(pts[0]![1]);
    expect(pts[pts.length - 2]![1]).toBe(pts[pts.length - 1]![1]);
    // Orthogonal: every point lies on a horizontal or vertical line with a neighbour.
    const curved = routeRelation(box(0, 0), "user_id", box(500, 300), "id", "curved");
    const [p0, c1, c2, p3] = points(curved.path);
    expect(c1![1]).toBe(p0![1]);
    expect(c2![1]).toBe(p3![1]);
  });

  it("loops a self reference out on the right", () => {
    const b = box(0, 0, ["id", "parent_id"]);
    for (const style of ["curved", "orthogonal"] as const) {
      const g = routeRelation(b, "parent_id", b, "id", style, true);
      expect(g.from.dir).toBe(1);
      expect(g.to.dir).toBe(1);
      expect(g.from.x).toBe(200);
      expect(Math.max(...points(g.path).map(([x]) => x))).toBeGreaterThan(200 + STUB);
    }
  });

  it("keeps a loop into the same row apart", () => {
    const b = box(0, 0, ["id"]);
    const g = routeRelation(b, "id", b, "id", "orthogonal", true);
    expect(Math.min(...points(g.path).map(([, y]) => y))).toBeLessThan(g.from.y);
  });
});

describe("markers", () => {
  it("draws a crow's foot spreading onto the card side", () => {
    const pts = points(crowsFootPath({ x: 100, y: 50, dir: 1 }));
    expect(pts).toContainEqual([100, 44]);
    expect(pts).toContainEqual([100, 56]);
    expect(pts).toContainEqual([114, 50]);
    // Pointing left, the tip is on the left.
    expect(points(crowsFootPath({ x: 100, y: 50, dir: -1 }))).toContainEqual([86, 50]);
  });

  it("draws two bars for a mandatory parent, a bar and a circle for an optional one", () => {
    const mandatory = oneEndMarks({ x: 0, y: 0, dir: 1 }, false);
    expect(mandatory.circle).toBeNull();
    expect(mandatory.bars.match(/M/g)).toHaveLength(2);
    const optional = oneEndMarks({ x: 0, y: 0, dir: -1 }, true);
    expect(optional.circle).toEqual({ x: -16, y: 0 });
    expect(optional.bars.match(/M/g)).toHaveLength(1);
  });
});

describe("roundedPolyline", () => {
  it("rounds corners and keeps straight runs", () => {
    expect(
      roundedPolyline([
        { x: 0, y: 0 },
        { x: 10, y: 0 },
      ]),
    ).toBe("M 0 0 L 10 0");
    const d = roundedPolyline(
      [
        { x: 0, y: 0 },
        { x: 40, y: 0 },
        { x: 40, y: 40 },
      ],
      8,
    );
    expect(d).toBe("M 0 0 L 32 0 Q 40 0 40 8 L 40 40");
    // A corner shorter than twice the radius gets a smaller one.
    expect(
      roundedPolyline(
        [
          { x: 0, y: 0 },
          { x: 4, y: 0 },
          { x: 4, y: 40 },
        ],
        8,
      ),
    ).toBe("M 0 0 L 2 0 Q 4 0 4 2 L 4 40");
    expect(roundedPolyline([])).toBe("");
  });
});

describe("text fitting", () => {
  it("cuts with an ellipsis", () => {
    expect(fitText("orders", 10)).toBe("orders");
    expect(fitText("order_items", 6)).toBe("order…");
    expect(fitText("abc", 1)).toBe("…");
    expect(fitText("abc", 0)).toBe("");
  });

  it("shares a row between the name and the type", () => {
    expect(fitRow("id", "bigint", 200)).toEqual(["id", "bigint"]);
    const [name, type] = fitRow("a_very_long_column_name_here", "character varying(255)", 200);
    expect(name.endsWith("…")).toBe(true);
    expect(type.endsWith("…")).toBe(true);
    const width = (s: string, c: number) => s.length * c;
    expect(width(name, CARD.nameChar) + width(type, CARD.typeChar)).toBeLessThanOrEqual(
      200 - CARD.padX * 2 - CARD.iconSlot - CARD.gap,
    );
    // A short name leaves the rest to the type.
    expect(fitRow("id", "character varying(255)", 200)[0]).toBe("id");
  });
});
