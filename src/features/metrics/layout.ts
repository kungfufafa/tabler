/**
 * The board canvas's grid: 12 columns that share the canvas width, rows of a fixed height, an
 * 8px gutter. Objects are placed in whole cells; dragging and resizing snap to them, and an
 * object dropped onto others pushes them down.
 */

export const GRID_COLUMNS = 12;
/** Row height in px. */
export const ROW_H = 44;
/** Gutter between cells in px. */
export const GAP = 8;

/** A place on the grid, in cells (x: column, y: row). */
export interface Layout {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Width of one column for a canvas `width` px wide (gutters between the columns). */
export function columnWidth(width: number): number {
  return Math.max(8, (width - GAP * (GRID_COLUMNS - 1)) / GRID_COLUMNS);
}

/** Where a layout sits on the canvas, in px. */
export function toRect(l: Layout, colW: number): Rect {
  return {
    left: l.x * (colW + GAP),
    top: l.y * (ROW_H + GAP),
    width: l.w * colW + (l.w - 1) * GAP,
    height: l.h * ROW_H + (l.h - 1) * GAP,
  };
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** `start` moved by a pointer delta (px), snapped to the nearest cell and kept on the grid. */
export function moveBy(start: Layout, dx: number, dy: number, colW: number): Layout {
  return {
    ...start,
    x: clamp(Math.round(start.x + dx / (colW + GAP)), 0, GRID_COLUMNS - start.w),
    y: Math.max(0, Math.round(start.y + dy / (ROW_H + GAP))),
  };
}

/**
 * `start` resized from its bottom-right corner by a pointer delta (px): whole cells, at least
 * `min`, never past the grid's right edge.
 */
export function resizeBy(
  start: Layout,
  dx: number,
  dy: number,
  colW: number,
  min: { w: number; h: number },
): Layout {
  return {
    ...start,
    w: clamp(Math.round(start.w + dx / (colW + GAP)), min.w, GRID_COLUMNS - start.x),
    h: Math.max(min.h, Math.round(start.h + dy / (ROW_H + GAP))),
  };
}

export function overlaps(a: Layout, b: Layout): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/**
 * The layouts after `movedId` took its place: every object it now overlaps (and every object
 * those land on) moves down just below what is in its way; the others keep their places.
 */
export function resolveCollisions<T extends { id: string; layout: Layout }>(
  items: readonly T[],
  movedId: string,
): T[] {
  const moved = items.find((i) => i.id === movedId);
  if (!moved) return [...items];
  const placed: T[] = [moved];
  const others = items
    .filter((i) => i.id !== movedId)
    .sort((a, b) => a.layout.y - b.layout.y || a.layout.x - b.layout.x);
  for (const item of others) {
    let layout = item.layout;
    for (let hit = placed.find((p) => overlaps(p.layout, layout)); hit;) {
      layout = { ...layout, y: hit.layout.y + hit.layout.h };
      hit = placed.find((p) => overlaps(p.layout, layout));
    }
    placed.push(layout === item.layout ? item : { ...item, layout });
  }
  const byId = new Map(placed.map((p) => [p.id, p]));
  return items.map((i) => byId.get(i.id) ?? i);
}

/** The first free spot (top to bottom, left to right) for a `w` × `h` object. */
export function findFreeSpot(
  layouts: readonly Layout[],
  w: number,
  h: number,
): { x: number; y: number } {
  const width = Math.min(w, GRID_COLUMNS);
  for (let y = 0; ; y++)
    for (let x = 0; x + width <= GRID_COLUMNS; x++) {
      const spot = { x, y, w: width, h };
      if (!layouts.some((l) => overlaps(l, spot))) return { x, y };
    }
}

/** Rows the objects use (the canvas is at least this tall). */
export function usedRows(layouts: readonly Layout[]): number {
  return layouts.reduce((n, l) => Math.max(n, l.y + l.h), 0);
}

/** Canvas height in px for `rows` rows. */
export function rowsHeight(rows: number): number {
  return rows <= 0 ? 0 : rows * ROW_H + (rows - 1) * GAP;
}
