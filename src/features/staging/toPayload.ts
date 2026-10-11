import type { BrowsePage, RowKey, StagedChange, Value } from "@/lib/ipc";
import type { StagingState } from "./types";

/** Backend row key for a fetched row (by PK, SQLite rowid, or every original column). */
export function rowKeyFor(page: BrowsePage, index: number): RowKey {
  const row = page.rows[index] ?? [];
  const colIndex = (name: string) => page.columns.findIndex((c) => c.name === name);
  if (page.identity === "rowid" && page.rowIds)
    return { kind: "rowid", value: page.rowIds[index] ?? 0 };
  if (page.identity === "pk" && page.pkColumns.length > 0) {
    return {
      kind: "pk",
      value: Object.fromEntries(page.pkColumns.map((c) => [c, row[colIndex(c)] ?? { t: "null" }])),
    };
  }
  return {
    kind: "allColumns",
    value: Object.fromEntries(
      page.columns
        .filter((c) => c.kind !== "bytes")
        .map((c, i) => [c.name, row[i] ?? { t: "null" }]),
    ),
  };
}

/** Converts staged edits to the backend payload. Keys use ORIGINAL values, so edited PKs still match. */
export function toStagedChanges(
  s: StagingState,
  columns: BrowsePage["columns"],
  keyOf: (rowKey: string) => RowKey | null,
): StagedChange[] {
  const out: StagedChange[] = [];
  const name = (i: number) => columns[i]?.name ?? "";
  for (const [rowKey, change] of s.changes) {
    if (change.status === "inserted") {
      const values: Record<string, Value> = {};
      columns.forEach((c, i) => {
        values[c.name] = change.patch.get(i) ?? { t: "default" };
      });
      out.push({ op: "insert", rowId: rowKey, values });
      continue;
    }
    const key = keyOf(rowKey);
    if (!key) continue;
    if (change.status === "deleted") {
      out.push({ op: "delete", rowId: rowKey, key });
    } else if (change.patch.size > 0) {
      const changes: Record<string, Value> = {};
      for (const [i, v] of change.patch) changes[name(i)] = v;
      out.push({ op: "update", rowId: rowKey, key, changes });
    }
  }
  return out;
}
