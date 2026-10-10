import type { CellPos } from "./types";

export interface Range {
  anchor: CellPos;
  focus: CellPos;
}

export interface SelectionState {
  ranges: Range[];
  /** The keyboard focus cell (always inside the last range when ranges exist). */
  focus: CellPos | null;
}

export interface Bounds {
  rows: number;
  cols: number;
}

export interface Rect {
  r0: number;
  c0: number;
  r1: number;
  c1: number;
}

export const EMPTY_SELECTION: SelectionState = { ranges: [], focus: null };

export type SelectionAction =
  | { type: "click"; pos: CellPos; shift: boolean; meta: boolean }
  | { type: "dragTo"; pos: CellPos }
  | { type: "move"; dr: number; dc: number; shift: boolean; jump: boolean }
  | { type: "selectAll" }
  | { type: "selectRow"; r: number; shift: boolean; meta: boolean }
  | { type: "selectColumn"; c: number; shift: boolean; meta: boolean }
  | { type: "set"; pos: CellPos }
  | { type: "collapse" }
  | { type: "clear" }
  | { type: "clamp" };

export function normalize(range: Range): Rect {
  return {
    r0: Math.min(range.anchor.r, range.focus.r),
    r1: Math.max(range.anchor.r, range.focus.r),
    c0: Math.min(range.anchor.c, range.focus.c),
    c1: Math.max(range.anchor.c, range.focus.c),
  };
}

function clampPos(pos: CellPos, b: Bounds): CellPos {
  return {
    r: Math.max(0, Math.min(pos.r, Math.max(0, b.rows - 1))),
    c: Math.max(0, Math.min(pos.c, Math.max(0, b.cols - 1))),
  };
}

function single(pos: CellPos): SelectionState {
  return { ranges: [{ anchor: pos, focus: pos }], focus: pos };
}

export function isCellSelected(s: SelectionState, r: number, c: number): boolean {
  return s.ranges.some((range) => {
    const n = normalize(range);
    return r >= n.r0 && r <= n.r1 && c >= n.c0 && c <= n.c1;
  });
}

/** Bounding rectangle of all ranges. */
export function selectionBounds(s: SelectionState): Rect | null {
  if (s.ranges.length === 0) return null;
  const rects = s.ranges.map(normalize);
  return {
    r0: Math.min(...rects.map((x) => x.r0)),
    c0: Math.min(...rects.map((x) => x.c0)),
    r1: Math.max(...rects.map((x) => x.r1)),
    c1: Math.max(...rects.map((x) => x.c1)),
  };
}

/** Sorted unique row indexes touched by the selection. */
export function selectedRows(s: SelectionState): number[] {
  const set = new Set<number>();
  for (const range of s.ranges) {
    const n = normalize(range);
    for (let r = n.r0; r <= n.r1; r++) set.add(r);
  }
  return [...set].sort((a, b) => a - b);
}

export function selectedCellCount(s: SelectionState): number {
  const seen = new Set<string>();
  for (const range of s.ranges) {
    const n = normalize(range);
    for (let r = n.r0; r <= n.r1; r++) for (let c = n.c0; c <= n.c1; c++) seen.add(`${r}:${c}`);
  }
  return seen.size;
}

export function selectionReducer(s: SelectionState, a: SelectionAction, b: Bounds): SelectionState {
  if (b.rows === 0 || b.cols === 0) return EMPTY_SELECTION;
  switch (a.type) {
    case "set":
      return single(clampPos(a.pos, b));
    case "click": {
      const pos = clampPos(a.pos, b);
      if (a.shift && s.focus) {
        const last = s.ranges[s.ranges.length - 1];
        const anchor = last?.anchor ?? s.focus;
        const rest = a.meta ? s.ranges.slice(0, -1) : [];
        return { ranges: [...rest, { anchor, focus: pos }], focus: pos };
      }
      if (a.meta) return { ranges: [...s.ranges, { anchor: pos, focus: pos }], focus: pos };
      return single(pos);
    }
    case "dragTo": {
      const pos = clampPos(a.pos, b);
      const last = s.ranges[s.ranges.length - 1];
      if (!last) return single(pos);
      if (last.focus.r === pos.r && last.focus.c === pos.c) return s;
      return {
        ranges: [...s.ranges.slice(0, -1), { anchor: last.anchor, focus: pos }],
        focus: pos,
      };
    }
    case "move": {
      const from = s.focus ?? { r: 0, c: 0 };
      const target = a.jump
        ? {
            r: a.dr === 0 ? from.r : a.dr < 0 ? 0 : b.rows - 1,
            c: a.dc === 0 ? from.c : a.dc < 0 ? 0 : b.cols - 1,
          }
        : { r: from.r + a.dr, c: from.c + a.dc };
      const pos = clampPos(target, b);
      if (a.shift) {
        const last = s.ranges[s.ranges.length - 1];
        const anchor = last?.anchor ?? from;
        return { ranges: [...s.ranges.slice(0, -1), { anchor, focus: pos }], focus: pos };
      }
      return single(pos);
    }
    case "selectAll":
      return {
        ranges: [{ anchor: { r: 0, c: 0 }, focus: { r: b.rows - 1, c: b.cols - 1 } }],
        focus: s.focus ?? { r: 0, c: 0 },
      };
    case "selectRow": {
      const r = Math.max(0, Math.min(a.r, b.rows - 1));
      const row: Range = { anchor: { r, c: 0 }, focus: { r, c: b.cols - 1 } };
      if (a.shift && s.focus) {
        const anchorR = s.ranges[s.ranges.length - 1]?.anchor.r ?? s.focus.r;
        const rest = a.meta ? s.ranges.slice(0, -1) : [];
        return {
          ranges: [...rest, { anchor: { r: anchorR, c: 0 }, focus: { r, c: b.cols - 1 } }],
          focus: { r, c: s.focus.c },
        };
      }
      const focus = { r, c: s.focus?.c ?? 0 };
      return a.meta ? { ranges: [...s.ranges, row], focus } : { ranges: [row], focus };
    }
    case "selectColumn": {
      const c = Math.max(0, Math.min(a.c, b.cols - 1));
      const col: Range = { anchor: { r: 0, c }, focus: { r: b.rows - 1, c } };
      if (a.shift && s.focus) {
        const anchorC = s.ranges[s.ranges.length - 1]?.anchor.c ?? s.focus.c;
        const rest = a.meta ? s.ranges.slice(0, -1) : [];
        return {
          ranges: [...rest, { anchor: { r: 0, c: anchorC }, focus: { r: b.rows - 1, c } }],
          focus: { r: s.focus.r, c },
        };
      }
      const focus = { r: s.focus?.r ?? 0, c };
      return a.meta ? { ranges: [...s.ranges, col], focus } : { ranges: [col], focus };
    }
    case "collapse":
      return s.focus ? single(s.focus) : EMPTY_SELECTION;
    case "clear":
      return EMPTY_SELECTION;
    case "clamp": {
      if (!s.focus) return s;
      const ranges = s.ranges.map((r) => ({
        anchor: clampPos(r.anchor, b),
        focus: clampPos(r.focus, b),
      }));
      return { ranges, focus: clampPos(s.focus, b) };
    }
  }
}
