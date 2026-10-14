import type { CellPos } from "./types";

/**
 * An inclusive run of row (or column) indexes. ⇧ extends the last run from its `anchor`; `focus` is
 * the end that moves.
 */
export interface Span {
  anchor: number;
  focus: number;
}

/**
 * The grid selects ROWS (TablePlus): a click selects the row and focuses the clicked cell, ⇧ / ⌘ /
 * a drag extend or toggle rows. ⌥-clicking a header selects whole columns instead. A drag across
 * columns, or ⇧← / ⇧→, selects a rectangle of cells (TablePlus's "Copy Selected Cells" range):
 * `anchor` is its fixed corner and `focus` the moving one, and `rows` holds its rows.
 */
export interface SelectionState {
  /** Selected rows, as runs in the order they were made (⌘-click adds or removes a row). */
  rows: Span[];
  /** Selected whole (visual) columns; only set while `rows` is empty. */
  cols: Span[];
  /** The focused cell (VISUAL position): what ↩ edits and the row details show. */
  focus: CellPos | null;
  /** The fixed corner of a cell range; null outside a range. */
  anchor?: CellPos | null;
  /** The moving corner of the cell range (where the focus went when it last grew). */
  end?: CellPos | null;
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

export const EMPTY_SELECTION: SelectionState = { rows: [], cols: [], focus: null, anchor: null };

export type SelectionAction =
  /** A press on a cell: selects its row (⇧ extends the last run, ⌘ toggles the row) and focuses it. */
  | { type: "click"; pos: CellPos; shift: boolean; meta: boolean }
  /**
   * A drag from a press: extends the last run to the row under the pointer, or, once it moves
   * into another column, selects the cell range from the press (`rowsOnly`: a drag down the row
   * numbers, which stays a row selection).
   */
  | { type: "dragTo"; pos: CellPos; rowsOnly?: boolean }
  /** ↑↓ move (⇧ extend) the row selection; ←→ move the focused cell within its row. */
  | { type: "move"; dr: number; dc: number; shift: boolean; jump: boolean }
  | { type: "selectAll" }
  /** A press on a row number: like a click on the row's cell in the focused column. */
  | { type: "selectRow"; r: number; shift: boolean; meta: boolean }
  | { type: "selectColumn"; c: number; shift: boolean; meta: boolean }
  /** Selects rows `r0`…`r1` (e.g. rows just pasted) and focuses `focus`. */
  | { type: "selectRows"; r0: number; r1: number; focus: CellPos }
  /** Selects only `pos`'s row and focuses `pos`. */
  | { type: "set"; pos: CellPos }
  /** Moves the focused cell without changing which rows are selected (a right-click inside them). */
  | { type: "focus"; pos: CellPos }
  | { type: "collapse" }
  | { type: "clear" }
  | { type: "clamp" };

const lo = (s: Span) => Math.min(s.anchor, s.focus);
const hi = (s: Span) => Math.max(s.anchor, s.focus);

function clampIndex(i: number, n: number): number {
  return Math.max(0, Math.min(i, Math.max(0, n - 1)));
}

function clampPos(pos: CellPos, b: Bounds): CellPos {
  return { r: clampIndex(pos.r, b.rows), c: clampIndex(pos.c, b.cols) };
}

function single(pos: CellPos): SelectionState {
  return { rows: [{ anchor: pos.r, focus: pos.r }], cols: [], focus: pos, anchor: null };
}

/** A cell range from `anchor` to `focus`: its rows are the selected rows. */
function range(anchor: CellPos, focus: CellPos): SelectionState {
  return { rows: [{ anchor: anchor.r, focus: focus.r }], cols: [], focus, anchor, end: focus };
}

/** The selected cell range, normalized (VISUAL indexes); null without one. */
export function selectionRect(s: SelectionState): Rect | null {
  const a = s.anchor;
  const f = s.end ?? s.focus;
  if (!a || !f) return null;
  return {
    r0: Math.min(a.r, f.r),
    r1: Math.max(a.r, f.r),
    c0: Math.min(a.c, f.c),
    c1: Math.max(a.c, f.c),
  };
}

const inSpans = (spans: readonly Span[], i: number) => spans.some((s) => i >= lo(s) && i <= hi(s));

/** `spans` with index `i` taken out (runs that contain it are split around it). */
function withoutIndex(spans: readonly Span[], i: number): Span[] {
  return spans.flatMap((s) => {
    const a = lo(s);
    const b = hi(s);
    if (i < a || i > b) return [s];
    const out: Span[] = [];
    if (a <= i - 1) out.push({ anchor: a, focus: i - 1 });
    if (i + 1 <= b) out.push({ anchor: i + 1, focus: b });
    return out;
  });
}

/** Runs within `0…n-1`: runs past the end are dropped, the rest are cut at the end. */
function clampSpans(spans: readonly Span[], n: number): Span[] {
  const out: Span[] = [];
  for (const s of spans) {
    if (lo(s) >= n) continue;
    out.push({ anchor: clampIndex(s.anchor, n), focus: clampIndex(s.focus, n) });
  }
  return out;
}

const sameSpans = (a: readonly Span[], b: readonly Span[]) =>
  a.length === b.length && a.every((s, i) => s.anchor === b[i]!.anchor && s.focus === b[i]!.focus);

/** Sorted, non-overlapping `[first, last]` runs covering `spans`. */
export function mergeSpans(spans: readonly Span[]): Array<[number, number]> {
  const sorted = spans.map((s): [number, number] => [lo(s), hi(s)]).sort((x, y) => x[0] - y[0]);
  const out: Array<[number, number]> = [];
  for (const [a, b] of sorted) {
    const last = out[out.length - 1];
    if (last && a <= last[1] + 1) last[1] = Math.max(last[1], b);
    else out.push([a, b]);
  }
  return out;
}

export function isRowSelected(s: SelectionState, r: number): boolean {
  return inSpans(s.rows, r);
}

export function isColumnSelected(s: SelectionState, c: number): boolean {
  return inSpans(s.cols, c);
}

/**
 * Whether the cell is part of the selection: inside the cell range, else its row (or its column
 * while columns are selected).
 */
export function isCellSelected(s: SelectionState, r: number, c: number): boolean {
  const rect = selectionRect(s);
  if (rect) return r >= rect.r0 && r <= rect.r1 && c >= rect.c0 && c <= rect.c1;
  return s.cols.length > 0 ? isColumnSelected(s, c) : isRowSelected(s, r);
}

function expand(runs: Array<[number, number]>): number[] {
  const out: number[] = [];
  for (const [a, b] of runs) for (let i = a; i <= b; i++) out.push(i);
  return out;
}

/** Sorted unique indexes of the selected rows (none while columns are selected). */
export function selectedRows(s: SelectionState): number[] {
  return expand(mergeSpans(s.rows));
}

/** How many rows are selected, without listing them (⌘A on a large result). */
export function selectedRowCount(s: SelectionState): number {
  return mergeSpans(s.rows).reduce((n, [a, b]) => n + b - a + 1, 0);
}

/** Sorted unique (visual) indexes of the selected columns. */
export function selectedColumns(s: SelectionState): number[] {
  return expand(mergeSpans(s.cols));
}

export function selectionReducer(s: SelectionState, a: SelectionAction, b: Bounds): SelectionState {
  if (b.rows === 0 || b.cols === 0) return EMPTY_SELECTION;
  switch (a.type) {
    case "set":
      return single(clampPos(a.pos, b));
    case "focus":
      return { ...s, focus: clampPos(a.pos, b) };
    case "selectRow":
      return selectionReducer(
        s,
        {
          type: "click",
          pos: { r: a.r, c: s.focus?.c ?? 0 },
          shift: a.shift,
          meta: a.meta,
        },
        b,
      );
    case "click": {
      const pos = clampPos(a.pos, b);
      // ⇧ inside a cell range extends the range.
      if (a.shift && s.anchor) return range(s.anchor, pos);
      if (a.shift && s.focus) {
        // Extends the last run (earlier ⌘-clicked rows stay selected, as in Finder).
        const anchor = s.rows[s.rows.length - 1]?.anchor ?? s.focus.r;
        return {
          rows: [...s.rows.slice(0, -1), { anchor, focus: pos.r }],
          cols: [],
          focus: pos,
          anchor: null,
        };
      }
      if (a.meta) {
        if (s.cols.length === 0 && !s.anchor && isRowSelected(s, pos.r))
          return { rows: withoutIndex(s.rows, pos.r), cols: [], focus: pos, anchor: null };
        const rows = s.cols.length === 0 ? s.rows : [];
        return {
          rows: [...rows, { anchor: pos.r, focus: pos.r }],
          cols: [],
          focus: pos,
          anchor: null,
        };
      }
      return single(pos);
    }
    case "dragTo": {
      const pos = clampPos(a.pos, b);
      const last = s.rows[s.rows.length - 1];
      if (!last) return single(pos);
      if (s.focus?.r === pos.r && s.focus.c === pos.c) return s;
      // Into another column: the drag becomes a cell range from where it started.
      if (s.anchor || (s.focus && pos.c !== s.focus.c && !a.rowsOnly))
        return range(s.anchor ?? { r: last.anchor, c: s.focus!.c }, pos);
      return {
        rows: [...s.rows.slice(0, -1), { anchor: last.anchor, focus: pos.r }],
        cols: [],
        focus: pos,
        anchor: null,
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
      // ⇧← / ⇧→ start (or grow) a cell range from the selected rows' first row; ⇧↑ / ⇧↓ grow one.
      if (a.shift && s.focus && (s.anchor || (a.dr === 0 && s.cols.length === 0))) {
        const anchor = s.anchor ?? { r: s.rows[s.rows.length - 1]?.anchor ?? from.r, c: from.c };
        if (s.focus.r === pos.r && s.focus.c === pos.c) return s;
        return range(anchor, pos);
      }
      // ←→ / ⇥ move the focused cell within its row; the selected rows stay.
      if (a.dr === 0 && s.rows.length > 0 && !s.anchor) {
        if (s.focus && s.focus.r === pos.r && s.focus.c === pos.c) return s;
        return { ...s, focus: pos };
      }
      if (a.shift && s.focus) {
        const anchor = s.rows[s.rows.length - 1]?.anchor ?? from.r;
        return {
          rows: [...s.rows.slice(0, -1), { anchor, focus: pos.r }],
          cols: [],
          focus: pos,
          anchor: null,
        };
      }
      return single(pos);
    }
    case "selectAll": {
      const focus = s.focus ? clampPos(s.focus, b) : { r: 0, c: 0 };
      return { rows: [{ anchor: 0, focus: b.rows - 1 }], cols: [], focus, anchor: null };
    }
    case "selectRows": {
      const r0 = clampIndex(a.r0, b.rows);
      const r1 = clampIndex(a.r1, b.rows);
      return {
        rows: [{ anchor: r0, focus: r1 }],
        cols: [],
        focus: clampPos(a.focus, b),
        anchor: null,
      };
    }
    case "selectColumn": {
      const c = clampIndex(a.c, b.cols);
      const r = s.focus ? clampIndex(s.focus.r, b.rows) : 0;
      if (a.shift && s.focus) {
        const anchor = s.cols[s.cols.length - 1]?.anchor ?? s.focus.c;
        return {
          rows: [],
          cols: [...s.cols.slice(0, -1), { anchor, focus: c }],
          focus: { r, c },
          anchor: null,
        };
      }
      if (a.meta) {
        if (isColumnSelected(s, c))
          return { rows: [], cols: withoutIndex(s.cols, c), focus: { r, c }, anchor: null };
        return {
          rows: [],
          cols: [...s.cols, { anchor: c, focus: c }],
          focus: { r, c },
          anchor: null,
        };
      }
      return { rows: [], cols: [{ anchor: c, focus: c }], focus: { r, c }, anchor: null };
    }
    case "collapse":
      return s.focus ? single(s.focus) : EMPTY_SELECTION;
    case "clear":
      return EMPTY_SELECTION;
    case "clamp": {
      if (!s.focus && s.rows.length === 0 && s.cols.length === 0) return s;
      let rows = clampSpans(s.rows, b.rows);
      const cols = clampSpans(s.cols, b.cols);
      const focus = s.focus ? clampPos(s.focus, b) : null;
      const anchor = s.anchor && focus ? clampPos(s.anchor, b) : null;
      const end = anchor ? clampPos(s.end ?? focus!, b) : null;
      // Every selected row is gone (a shorter page): the focused row is the selection.
      if (s.rows.length > 0 && rows.length === 0 && focus)
        rows = [{ anchor: focus.r, focus: focus.r }];
      if (anchor && end) rows = [{ anchor: anchor.r, focus: end.r }];
      if (
        sameSpans(rows, s.rows) &&
        sameSpans(cols, s.cols) &&
        focus?.r === s.focus?.r &&
        focus?.c === s.focus?.c &&
        anchor?.r === s.anchor?.r &&
        anchor?.c === s.anchor?.c &&
        (!anchor || (end?.r === s.end?.r && end?.c === s.end?.c))
      )
        return s;
      return { rows, cols, focus, anchor, end };
    }
  }
}
