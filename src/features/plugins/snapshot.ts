import type { Driver, Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";
import type { PlainValue, PluginSnapshot, SnapshotRow } from "./sandbox/protocol";
import type { PluginInvocation, PluginRowData } from "./types";

/** TablePlus's driver names (`context.driver()`). */
export const DRIVER_NAMES: Record<Driver, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  sqlite: "SQLite",
};

/**
 * A cell value as plain data for a plugin: NULL → null, booleans and safe integers / floats as
 * JS values, everything else (big integers, decimals, dates, JSON, UUIDs…) as the database's text;
 * bytes as base64; arrays element by element. DEFAULT (a staged new row) reads as null.
 */
export function plainValue(v: Value): PlainValue {
  switch (v.t) {
    case "null":
    case "default":
      return null;
    case "bool":
    case "int":
    case "float":
      return v.v;
    case "array":
      return v.v.map(plainValue);
    default:
      return valueText(v);
  }
}

export function snapshotRow(row: PluginRowData): SnapshotRow {
  return {
    columns: row.columns.map((c) => c.name),
    values: row.columns.map((_, i) => plainValue(row.cells[i] ?? { t: "null" })),
  };
}

/** What a sandboxed plugin can read about `inv` (plain, structured-cloneable data). */
export function toSnapshot(
  inv: PluginInvocation,
  connection: { name: string; readOnly: boolean } | null,
): PluginSnapshot {
  const clicked =
    inv.clickedRow && inv.clickedColumn !== null ? inv.clickedRow.columns[inv.clickedColumn] : null;
  return {
    source: inv.source,
    driver: inv.driver ? DRIVER_NAMES[inv.driver] : null,
    connection,
    item: inv.item ? { name: inv.item.name, schema: inv.item.schema, type: inv.item.type } : null,
    clickedRow: inv.clickedRow ? snapshotRow(inv.clickedRow) : null,
    clickedColumn: clicked ? { name: clicked.name, type: clicked.dataType } : null,
    selectedRows: inv.selectedRows.map(snapshotRow),
    sql: inv.sql ? { ...inv.sql } : null,
  };
}
