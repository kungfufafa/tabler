import type { ViewRow } from "@/features/datagrid/types";
import type { BrowsePage, Value } from "@/lib/ipc";
import type { StagingState } from "./types";

/** Combines the fetched page with staged edits into what the grid renders. */
export function mergeRows(page: BrowsePage, keys: string[], staging: StagingState): ViewRow[] {
  const rows: ViewRow[] = page.rows.map((cells, i) => {
    const key = keys[i] ?? `row:${i}`;
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
  });
  for (const key of staging.insertOrder) {
    const change = staging.changes.get(key);
    if (!change) continue;
    const cells: Value[] = page.columns.map((_, i) => change.patch.get(i) ?? { t: "default" });
    rows.push({
      key,
      cells,
      status: "inserted",
      changed: new Set(change.patch.keys()),
      error: change.error,
    });
  }
  return rows;
}

/** Starting cells for a new row: DEFAULT for auto/default columns, NULL when nullable, else empty text. */
export function defaultsFor(columns: BrowsePage["columns"]): Value[] {
  return columns.map((c) => {
    if (c.isAutoIncrement || c.defaultExpr !== null) return { t: "default" };
    if (c.nullable) return { t: "null" };
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
