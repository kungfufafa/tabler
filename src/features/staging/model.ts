import type { Value } from "@/lib/ipc";
import { valuesEqual } from "@/lib/values";
import {
  MAX_UNDO,
  type CellEdit,
  type RowChange,
  type StagingState,
  type UndoEntry,
} from "./types";

export function emptyStaging(): StagingState {
  return { changes: new Map(), insertOrder: [], undo: [], redo: [], version: 0 };
}

/**
 * Commits a batch of row changes as ONE undo step (`next` maps row key → new change or removal). A
 * new edit forks the history: whatever was undone before it can no longer be redone.
 */
function withChanges(
  s: StagingState,
  next: ReadonlyMap<string, RowChange | undefined>,
  insertOrder = s.insertOrder,
): StagingState {
  if (next.size === 0 && insertOrder === s.insertOrder) return s;
  const changes = new Map(s.changes);
  const rows: UndoEntry["rows"] = [];
  for (const [rowKey, change] of next) {
    rows.push({ rowKey, before: s.changes.get(rowKey) });
    if (change) changes.set(rowKey, change);
    else changes.delete(rowKey);
  }
  const entry: UndoEntry = {
    rows,
    insertOrderBefore: insertOrder === s.insertOrder ? undefined : s.insertOrder,
  };
  return {
    changes,
    insertOrder,
    undo: [...s.undo.slice(-(MAX_UNDO - 1)), entry],
    redo: [],
    version: s.version + 1,
  };
}

function withChange(
  s: StagingState,
  rowKey: string,
  change: RowChange | undefined,
  insertOrder = s.insertOrder,
): StagingState {
  return withChanges(s, new Map([[rowKey, change]]), insertOrder);
}

/**
 * The change a cell edit produces from `cur` (`undefined` = row back to its original; `null` = the
 * edit does not apply, e.g. the row is marked deleted or has no original snapshot).
 */
function editedRow(
  cur: RowChange | undefined,
  col: number,
  value: Value,
  original: Value[] | undefined,
): RowChange | undefined | null {
  if (cur?.status === "deleted") return null;
  if (cur?.status === "inserted") {
    const patch = new Map(cur.patch);
    patch.set(col, value);
    return { ...cur, patch, error: undefined };
  }
  const orig = cur?.original ?? original;
  if (!orig) return null;
  const patch = new Map(cur?.patch ?? []);
  if (valuesEqual(value, orig[col] ?? { t: "null" })) patch.delete(col);
  else patch.set(col, value);
  if (patch.size === 0) return undefined;
  return { status: "updated", original: orig, patch, error: undefined };
}

/** Stages a cell edit. Reverting to the original value removes the patch entry. */
export function setCell(
  s: StagingState,
  rowKey: string,
  col: number,
  value: Value,
  original: Value[] | undefined,
): StagingState {
  const next = editedRow(s.changes.get(rowKey), col, value, original);
  if (next === null) return s;
  return withChange(s, rowKey, next);
}

/**
 * Stages many cell edits (paste, multi-cell Set NULL/DEFAULT) with `setCell` semantics, as ONE undo
 * step. Returns `s` unchanged when no edit applies.
 */
export function setCells(s: StagingState, edits: readonly CellEdit[]): StagingState {
  const next = new Map<string, RowChange | undefined>();
  for (const { rowKey, col, value, original } of edits) {
    const cur = next.has(rowKey) ? next.get(rowKey) : s.changes.get(rowKey);
    const edited = editedRow(cur, col, value, original);
    if (edited === null) continue;
    next.set(rowKey, edited);
  }
  // Rows that ended up exactly as they were do not need an undo slot.
  for (const [rowKey, change] of next) {
    const before = s.changes.get(rowKey);
    if (change === undefined && before === undefined) next.delete(rowKey);
  }
  if (next.size === 0) return s;
  return withChanges(s, next);
}

/** Adds a new row whose cells start as `defaults` (DEFAULT/NULL/empty per column). */
export function insertRow(s: StagingState, rowKey: string, defaults: Value[]): StagingState {
  const patch = new Map<number, Value>();
  defaults.forEach((v, i) => patch.set(i, v));
  return withChange(s, rowKey, { status: "inserted", patch }, [...s.insertOrder, rowKey]);
}

/** Adds several new rows (paste, duplicate) as ONE undo step, in the given order. */
export function insertRows(
  s: StagingState,
  rows: ReadonlyArray<{ rowKey: string; values: Value[] }>,
): StagingState {
  if (rows.length === 0) return s;
  const next = new Map<string, RowChange | undefined>();
  for (const { rowKey, values } of rows) {
    const patch = new Map<number, Value>();
    values.forEach((v, i) => patch.set(i, v));
    next.set(rowKey, { status: "inserted", patch });
  }
  return withChanges(s, next, [...s.insertOrder, ...rows.map((r) => r.rowKey)]);
}

/** Marks rows for deletion (one undo step); inserted rows are simply dropped. */
export function deleteRows(
  s: StagingState,
  rows: Array<{ rowKey: string; original: Value[] | undefined }>,
): StagingState {
  const next = new Map<string, RowChange | undefined>();
  let insertOrder = s.insertOrder;
  for (const { rowKey, original } of rows) {
    if (next.has(rowKey)) continue;
    const cur = s.changes.get(rowKey);
    if (cur?.status === "inserted") {
      next.set(rowKey, undefined);
      insertOrder = insertOrder.filter((k) => k !== rowKey);
    } else if (cur?.status !== "deleted") {
      const orig = cur?.original ?? original;
      if (!orig) continue;
      next.set(rowKey, { status: "deleted", original: orig, patch: cur?.patch ?? new Map() });
    }
  }
  return withChanges(s, next, insertOrder);
}

export function revertRow(s: StagingState, rowKey: string): StagingState {
  const cur = s.changes.get(rowKey);
  if (!cur) return s;
  return withChange(
    s,
    rowKey,
    undefined,
    cur.status === "inserted" ? s.insertOrder.filter((k) => k !== rowKey) : s.insertOrder,
  );
}

export function revertCell(s: StagingState, rowKey: string, col: number): StagingState {
  const cur = s.changes.get(rowKey);
  if (!cur || cur.status !== "updated") return s;
  const patch = new Map(cur.patch);
  patch.delete(col);
  return patch.size === 0
    ? withChange(s, rowKey, undefined)
    : withChange(s, rowKey, { ...cur, patch });
}

export function setRowError(
  s: StagingState,
  rowKey: string,
  error: string | undefined,
): StagingState {
  const cur = s.changes.get(rowKey);
  if (!cur) return s;
  const changes = new Map(s.changes);
  changes.set(rowKey, { ...cur, error });
  return { ...s, changes, version: s.version + 1 };
}

export function discardAll(): StagingState {
  return emptyStaging();
}

/**
 * After a commit that stopped partway (critique D2): the changes the server applied leave the
 * staging, the others stay with their errors. The undo history goes too, as after any commit:
 * undoing an edit of an applied row would stage its change again, and a retry would send it
 * twice.
 */
export function dropApplied(
  s: StagingState,
  applied: ReadonlySet<string>,
  errors: ReadonlyMap<string, string>,
): StagingState {
  const changes = new Map<string, RowChange>();
  for (const [rowKey, change] of s.changes) {
    if (applied.has(rowKey)) continue;
    const error = errors.get(rowKey);
    changes.set(rowKey, error === undefined ? change : { ...change, error });
  }
  return {
    changes,
    insertOrder: s.insertOrder.filter((k) => !applied.has(k)),
    undo: [],
    redo: [],
    version: s.version + 1,
  };
}

/**
 * Puts back every row `entry` touched; returns the new changes / insert order plus the inverse entry
 * (the rows as they are now), which undoes this restore.
 */
function restore(
  s: StagingState,
  entry: UndoEntry,
): { changes: Map<string, RowChange>; insertOrder: string[]; inverse: UndoEntry } {
  const changes = new Map(s.changes);
  const rows: UndoEntry["rows"] = [];
  for (const { rowKey, before } of entry.rows) {
    rows.push({ rowKey, before: s.changes.get(rowKey) });
    if (before) changes.set(rowKey, before);
    else changes.delete(rowKey);
  }
  return {
    changes,
    insertOrder: entry.insertOrderBefore ?? s.insertOrder,
    inverse: {
      rows,
      insertOrderBefore: entry.insertOrderBefore === undefined ? undefined : s.insertOrder,
    },
  };
}

/** Reverts the last undo step (every row it touched); it can be redone until the next edit. */
export function undo(s: StagingState): StagingState {
  const entry = s.undo[s.undo.length - 1];
  if (!entry) return s;
  const { changes, insertOrder, inverse } = restore(s, entry);
  return {
    changes,
    insertOrder,
    undo: s.undo.slice(0, -1),
    redo: [...s.redo.slice(-(MAX_UNDO - 1)), inverse],
    version: s.version + 1,
  };
}

/** Re-applies the last undone step. */
export function redo(s: StagingState): StagingState {
  const entry = s.redo[s.redo.length - 1];
  if (!entry) return s;
  const { changes, insertOrder, inverse } = restore(s, entry);
  return {
    changes,
    insertOrder,
    undo: [...s.undo.slice(-(MAX_UNDO - 1)), inverse],
    redo: s.redo.slice(0, -1),
    version: s.version + 1,
  };
}

export function canRedo(s: StagingState): boolean {
  return s.redo.length > 0;
}

export function pendingCount(s: StagingState): number {
  return s.changes.size;
}

export function cellValue(s: StagingState, rowKey: string, col: number): Value | undefined {
  return s.changes.get(rowKey)?.patch.get(col);
}
