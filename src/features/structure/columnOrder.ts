import type { StructureColumnOrder } from "@/stores/prefs";
import type { ColumnRow } from "./model";

/**
 * The columns grid in the order the bottom bar's "Order columns by" says (TablePlus b514): the
 * table's own ordinal position, or by name (case-insensitive, stable for equal names). A staged
 * new column sits where it was added (ordinal) or among the others by its current name.
 */
export function orderColumns(
  columns: readonly ColumnRow[],
  order: StructureColumnOrder,
): ColumnRow[] {
  if (order === "ordinal") return [...columns];
  const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
  return columns
    .map((row, i) => ({ row, i }))
    .sort((a, b) => collator.compare(a.row.name, b.row.name) || a.i - b.i)
    .map(({ row }) => row);
}

/** Row key → its number in the `#` column: the position in the table's own order. */
export function ordinalNumbers(columns: readonly ColumnRow[]): ReadonlyMap<string, number> {
  return new Map(columns.map((c, i) => [c.key, i + 1]));
}
