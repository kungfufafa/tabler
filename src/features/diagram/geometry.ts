import { CARD, type Point, type Rect } from "./layout";

/** How relation lines are drawn. */
export type LineStyle = "curved" | "orthogonal";

/** A card as the relation router sees it: its rectangle and its columns in display order. */
export interface CardBox extends Rect {
  columns: readonly string[];
}

/** A line end on a card's side: `dir` is +1 when the line leaves to the right, -1 to the left. */
export interface LineEnd extends Point {
  dir: 1 | -1;
}

export interface RelationGeometry {
  /** SVG path of the line. */
  path: string;
  /** The child (many) end, on the referencing column's row. */
  from: LineEnd;
  /** The parent (one) end, on the referenced column's row. */
  to: LineEnd;
  /** Where the hover label goes. */
  mid: Point;
}

/** Room for the end markers before a line may turn. */
export const STUB = 22;
/** Corner radius of orthogonal lines. */
const CORNER = 8;

/** Vertical middle of `column`'s row, or of the header when the card has no such column. */
export function rowCenterY(box: CardBox, column: string | undefined): number {
  const i = column === undefined ? -1 : box.columns.indexOf(column);
  if (i < 0) return box.y + CARD.header / 2;
  return box.y + CARD.header + i * CARD.row + CARD.row / 2;
}

/**
 * The sides a relation leaves and enters by: facing sides when the cards stand apart, otherwise
 * both on the right (a loop), which is also how a table referencing itself is drawn.
 */
export function relationSides(from: Rect, to: Rect, self = false): { from: 1 | -1; to: 1 | -1 } {
  if (!self) {
    if (from.x + from.w + STUB * 2 <= to.x) return { from: 1, to: -1 };
    if (to.x + to.w + STUB * 2 <= from.x) return { from: -1, to: 1 };
  }
  return { from: 1, to: 1 };
}

const r1 = (n: number) => Math.round(n * 10) / 10;
const pt = (p: Point) => `${r1(p.x)} ${r1(p.y)}`;

/** An orthogonal polyline through `points` with rounded corners. */
export function roundedPolyline(points: readonly Point[], radius = CORNER): string {
  if (points.length === 0) return "";
  let d = `M ${pt(points[0]!)}`;
  for (let i = 1; i < points.length - 1; i++) {
    const prev = points[i - 1]!;
    const cur = points[i]!;
    const next = points[i + 1]!;
    const lenIn = Math.hypot(cur.x - prev.x, cur.y - prev.y);
    const lenOut = Math.hypot(next.x - cur.x, next.y - cur.y);
    const r = Math.min(radius, lenIn / 2, lenOut / 2);
    if (r <= 0.5) {
      d += ` L ${pt(cur)}`;
      continue;
    }
    const a = {
      x: cur.x - ((cur.x - prev.x) / lenIn) * r,
      y: cur.y - ((cur.y - prev.y) / lenIn) * r,
    };
    const b = {
      x: cur.x + ((next.x - cur.x) / lenOut) * r,
      y: cur.y + ((next.y - cur.y) / lenOut) * r,
    };
    d += ` L ${pt(a)} Q ${pt(cur)} ${pt(b)}`;
  }
  return `${d} L ${pt(points[points.length - 1]!)}`;
}

/**
 * Routes a relation from the child card's `fromColumn` row to the parent card's `toColumn` row.
 * Lines leave and enter horizontally, so the end markers always sit on a straight stub.
 */
export function routeRelation(
  from: CardBox,
  fromColumn: string | undefined,
  to: CardBox,
  toColumn: string | undefined,
  style: LineStyle,
  self = false,
): RelationGeometry {
  const sides = relationSides(from, to, self);
  const a: LineEnd = {
    x: sides.from === 1 ? from.x + from.w : from.x,
    y: rowCenterY(from, fromColumn),
    dir: sides.from,
  };
  const b: LineEnd = {
    x: sides.to === 1 ? to.x + to.w : to.x,
    y: rowCenterY(to, toColumn),
    dir: sides.to,
  };
  const sameSide = a.dir === b.dir;
  // A loop back into the same row still needs some height.
  const spread = sameSide && Math.abs(a.y - b.y) < 1 ? 14 : 0;

  if (style === "orthogonal") {
    const turnX = sameSide
      ? Math.max(a.x, b.x) + STUB + 12
      : a.dir === 1
        ? Math.max(a.x + STUB, Math.min(b.x - STUB, (a.x + b.x) / 2))
        : Math.min(a.x - STUB, Math.max(b.x + STUB, (a.x + b.x) / 2));
    if (spread > 0) {
      // Back into the same row: out, up, back and down onto the stub.
      const top = a.y - spread;
      const stubX = b.x + STUB;
      return {
        path: roundedPolyline(
          [
            a,
            { x: turnX, y: a.y },
            { x: turnX, y: top },
            { x: stubX, y: top },
            { x: stubX, y: b.y },
            b,
          ],
          5,
        ),
        from: a,
        to: b,
        mid: { x: turnX, y: top },
      };
    }
    return {
      path: roundedPolyline([a, { x: turnX, y: a.y }, { x: turnX, y: b.y }, b]),
      from: a,
      to: b,
      mid: { x: turnX, y: (a.y + b.y) / 2 },
    };
  }

  // Curved: a cubic whose tangents leave each side horizontally.
  let c1: Point;
  let c2: Point;
  if (sameSide) {
    const outX = Math.max(a.x, b.x) + 48 + Math.min(80, Math.abs(a.y - b.y) * 0.15);
    c1 = { x: outX, y: a.y - spread };
    c2 = { x: outX, y: b.y + spread };
  } else {
    const k = Math.min(160, Math.max(40, Math.abs(b.x - a.x) / 2));
    c1 = { x: a.x + a.dir * k, y: a.y };
    c2 = { x: b.x + b.dir * k, y: b.y };
  }
  return {
    path: `M ${pt(a)} C ${pt(c1)} ${pt(c2)} ${pt(b)}`,
    from: a,
    to: b,
    mid: {
      x: (a.x + 3 * c1.x + 3 * c2.x + b.x) / 8,
      y: (a.y + 3 * c1.y + 3 * c2.y + b.y) / 8,
    },
  };
}

/** Crow's foot ("many") at the child end: three prongs spreading onto the card's side. */
export function crowsFootPath(end: LineEnd): string {
  const { x, y, dir } = end;
  const tip = x + dir * 14;
  return `M ${r1(tip)} ${r1(y)} L ${r1(x)} ${r1(y - 6)} M ${r1(tip)} ${r1(y)} L ${r1(x)} ${r1(y + 6)} M ${r1(tip)} ${r1(y)} L ${r1(x)} ${r1(y)} M ${r1(x + dir * 18)} ${r1(y - 6)} L ${r1(x + dir * 18)} ${r1(y + 6)}`;
}

/**
 * The parent ("one") end: two bars when every child row needs a parent (the key's columns are
 * NOT NULL), one bar and a circle ("zero or one") when the key is nullable.
 */
export function oneEndMarks(
  end: LineEnd,
  optional: boolean,
): { bars: string; circle: Point | null } {
  const { x, y, dir } = end;
  const bar = (dx: number) =>
    `M ${r1(x + dir * dx)} ${r1(y - 6)} L ${r1(x + dir * dx)} ${r1(y + 6)}`;
  return optional
    ? { bars: bar(8), circle: { x: x + dir * 16, y } }
    : { bars: `${bar(8)} ${bar(13)}`, circle: null };
}

/** `text` cut to `max` characters with an ellipsis. */
export function fitText(text: string, max: number): string {
  if (max <= 0) return "";
  if (text.length <= max) return text;
  if (max === 1) return "…";
  return `${text.slice(0, max - 1)}…`;
}
