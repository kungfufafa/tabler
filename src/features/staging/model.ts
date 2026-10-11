import type { Value } from "@/lib/ipc";
import { valuesEqual } from "@/lib/values";
import { MAX_UNDO, type RowChange, type StagingState, type UndoEntry } from "./types";

export function emptyStaging(): StagingState {
  return { changes: new Map(), insertOrder: [], undo: [], version: 0 };
}

function withChange(
  s: StagingState,
  rowKey: string,
  change: RowChange | undefined,
  insertOrder = s.insertOrder,
): StagingState {
  const changes = new Map(s.changes);
  if (change) changes.set(rowKey, change);
  else changes.delete(rowKey);
  const entry: UndoEntry = {
    rowKey,
    before: s.changes.get(rowKey),
    insertOrderBefore: insertOrder === s.insertOrder ? undefined : s.insertOrder,
  };
  return {
    changes,
    insertOrder,
    undo: [...s.undo.slice(-(MAX_UNDO - 1)), entry],
    version: s.version + 1,
  };
}

/** Stages a cell edit. Reverting to the original value removes the patch entry. */
export function setCell(
  s: StagingState,
  rowKey: string,
  col: number,
  value: Value,
  original: Value[] | undefined,
): StagingState {
  const cur = s.changes.get(rowKey);
  if (cur?.status === "deleted") return s;
  if (cur?.status === "inserted") {
    const patch = new Map(cur.patch);
    patch.set(col, value);
    return withChange(s, rowKey, { ...cur, patch, error: undefined });
  }
  const orig = cur?.original ?? original;
  if (!orig) return s;
  const patch = new Map(cur?.patch ?? []);
  if (valuesEqual(value, orig[col] ?? { t: "null" })) patch.delete(col);
  else patch.set(col, value);
  if (patch.size === 0) return withChange(s, rowKey, undefined);
  return withChange(s, rowKey, { status: "updated", original: orig, patch, error: undefined });
}

/** Adds a new row whose cells start as `defaults` (DEFAULT/NULL/empty per column). */
export function insertRow(s: StagingState, rowKey: string, defaults: Value[]): StagingState {
  const patch = new Map<number, Value>();
  defaults.forEach((v, i) => patch.set(i, v));
  return withChange(s, rowKey, { status: "inserted", patch }, [...s.insertOrder, rowKey]);
}

/** Marks rows for deletion; inserted rows are simply dropped. */
export function deleteRows(
  s: StagingState,
  rows: Array<{ rowKey: string; original: Value[] | undefined }>,
): StagingState {
  let next = s;
  for (const { rowKey, original } of rows) {
    const cur = next.changes.get(rowKey);
    if (cur?.status === "inserted") {
      next = withChange(
        next,
        rowKey,
        undefined,
        next.insertOrder.filter((k) => k !== rowKey),
      );
    } else if (cur?.status !== "deleted") {
      const orig = cur?.original ?? original;
      if (!orig) continue;
      next = withChange(next, rowKey, {
        status: "deleted",
        original: orig,
        patch: cur?.patch ?? new Map(),
      });
    }
  }
  return next;
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

export function undo(s: StagingState): StagingState {
  const entry = s.undo[s.undo.length - 1];
  if (!entry) return s;
  const changes = new Map(s.changes);
  if (entry.before) changes.set(entry.rowKey, entry.before);
  else changes.delete(entry.rowKey);
  return {
    changes,
    insertOrder: entry.insertOrderBefore ?? s.insertOrder,
    undo: s.undo.slice(0, -1),
    version: s.version + 1,
  };
}

export function pendingCount(s: StagingState): number {
  return s.changes.size;
}

export function cellValue(s: StagingState, rowKey: string, col: number): Value | undefined {
  return s.changes.get(rowKey)?.patch.get(col);
}
