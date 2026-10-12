import type { ViewRow } from "@/features/datagrid/types";
import type { RowKey, RowResult, StagedChange, Value } from "@/lib/ipc";
import type { StagingState } from "@/features/staging/types";
import type { ResultEditTarget } from "./editability";

/**
 * Staged edits of a SQL editor result. The rows live in the non-reactive `resultData` buffer (and
 * may be sorted on the client), so rows are keyed by their position in that buffer, which does not
 * change while edits are staged; new rows get `new:<uuid>` keys and show after the fetched rows.
 */

const NULL: Value = { t: "null" };

/** Staging key of the buffer row at `index`. */
export const bufferKey = (index: number): string => `r${index}`;

/** The buffer index a staging key names (null for new rows). */
export function bufferIndex(key: string): number | null {
  if (!key.startsWith("r")) return null;
  const n = Number(key.slice(1));
  return Number.isInteger(n) && n >= 0 ? n : null;
}

/** New rows still staged, in display order. */
export function insertedKeys(staging: StagingState): string[] {
  return staging.insertOrder.filter((k) => staging.changes.get(k)?.status === "inserted");
}

/** Rows the grid shows: the buffer's rows plus the staged new rows. */
export function resultRowCount(bufferRows: number, staging: StagingState): number {
  return bufferRows + insertedKeys(staging).length;
}

/**
 * Grid row `i`: buffer rows in `order` (client-side sort, null = buffer order) with their staged
 * edits, then the new rows (their unset cells DEFAULT).
 */
export function resultRow(
  rows: readonly Value[][],
  order: readonly number[] | null,
  staging: StagingState,
  i: number,
  inserted: readonly string[] = insertedKeys(staging),
  width = 0,
): ViewRow | undefined {
  if (i < rows.length) {
    const idx = order ? order[i] : i;
    if (idx === undefined) return undefined;
    const cells = rows[idx];
    if (!cells) return undefined;
    const key = bufferKey(idx);
    const change = staging.changes.get(key);
    if (!change) return { key, cells };
    const merged = cells.slice();
    for (const [c, v] of change.patch) merged[c] = v;
    return {
      key,
      cells: merged,
      status: change.status,
      changed: new Set(change.patch.keys()),
      error: change.error,
    };
  }
  const key = inserted[i - rows.length];
  const change = key ? staging.changes.get(key) : undefined;
  if (!key || !change) return undefined;
  let n = Math.max(width, rows[0]?.length ?? 0);
  for (const c of change.patch.keys()) n = Math.max(n, c + 1);
  const cells: Value[] = Array.from(
    { length: n },
    (_, c) => change.patch.get(c) ?? { t: "default" },
  );
  return {
    key,
    cells,
    status: "inserted",
    changed: new Set(change.patch.keys()),
    error: change.error,
  };
}

/** Starting cells of a new result row: the target table's defaults, NULL in other columns. */
export function newRowCells(target: ResultEditTarget): Value[] {
  return target.columns.map((c): Value => {
    if (!c) return NULL;
    if (c.isAutoIncrement || c.defaultExpr !== null) return { t: "default" };
    if (c.nullable) return NULL;
    switch (c.kind) {
      case "int":
      case "bigInt":
      case "decimal":
      case "float":
        return { t: "int", v: 0 };
      case "bool":
        return { t: "bool", v: false };
      default:
        return { t: "text", v: "" };
    }
  });
}

/** A copy of `cells` for Duplicate: auto-increment columns get a fresh value, others are kept. */
export function duplicateCells(target: ResultEditTarget, cells: readonly Value[]): Value[] {
  return target.columns.map((c, i) =>
    c ? (c.isAutoIncrement ? { t: "default" } : (cells[i] ?? NULL)) : NULL,
  );
}

/** The primary key of a fetched row (its ORIGINAL values, so an edited key still matches). */
export function keyOf(target: ResultEditTarget, original: readonly Value[]): RowKey {
  const value: Record<string, Value> = {};
  target.meta.primaryKey.forEach((pk, j) => {
    value[pk] = original[target.keyIndexes[j] ?? -1] ?? NULL;
  });
  return { kind: "pk", value };
}

/** The staged edits as `ApplyRequest` changes on the target table. */
export function toResultChanges(staging: StagingState, target: ResultEditTarget): StagedChange[] {
  const out: StagedChange[] = [];
  for (const [rowId, change] of staging.changes) {
    if (change.status === "inserted") {
      const values: Record<string, Value> = {};
      target.columns.forEach((c, i) => {
        if (c) values[c.name] = change.patch.get(i) ?? { t: "default" };
      });
      out.push({ op: "insert", rowId, values });
      continue;
    }
    if (!change.original) continue;
    const key = keyOf(target, change.original);
    if (change.status === "deleted") {
      out.push({ op: "delete", rowId, key });
      continue;
    }
    const changes: Record<string, Value> = {};
    for (const [i, v] of change.patch) {
      const c = target.columns[i];
      if (c) changes[c.name] = v;
    }
    if (Object.keys(changes).length > 0) out.push({ op: "update", rowId, key, changes });
  }
  return out;
}

/**
 * The buffer rows after a successful commit: edited rows updated in place (with the stored row
 * the server returned, when it did: defaults, triggers), deleted rows removed, new rows appended.
 * Cells of other tables in a new row are NULL; target cells the server did not return stay as
 * staged (DEFAULT for a server-assigned value).
 */
export function applyCommitted(
  rows: readonly Value[][],
  staging: StagingState,
  target: ResultEditTarget,
  results: readonly RowResult[],
): Value[][] {
  const returned = new Map(results.map((r) => [r.rowId, r.returned]));
  const fill = (cells: Value[], rowId: string) => {
    const stored = returned.get(rowId);
    if (!stored) return cells;
    target.columns.forEach((c, i) => {
      const v = c ? stored[c.name] : undefined;
      if (v !== undefined) cells[i] = v;
    });
    return cells;
  };
  const removed = new Set<number>();
  const next = rows.slice();
  for (const [key, change] of staging.changes) {
    const idx = bufferIndex(key);
    if (idx === null || !next[idx]) continue;
    if (change.status === "deleted") {
      removed.add(idx);
    } else if (change.status === "updated") {
      const cells = next[idx].slice();
      for (const [c, v] of change.patch) cells[c] = v;
      next[idx] = fill(cells, key);
    }
  }
  const out = removed.size > 0 ? next.filter((_, i) => !removed.has(i)) : next;
  const width = rows[0]?.length ?? target.columns.length;
  for (const key of insertedKeys(staging)) {
    const change = staging.changes.get(key)!;
    const cells = Array.from({ length: width }, (_, i): Value => {
      if (!target.columns[i]) return NULL;
      return change.patch.get(i) ?? { t: "default" };
    });
    out.push(fill(cells, key));
  }
  return out;
}
