import type { Dialect, TableRef, Value } from "@/lib/ipc";
import { literal, qualify, quoteIdent } from "@/lib/sql/quote";
import type { BuiltinPlugin, PluginRowData } from "../types";

function condition(dialect: Dialect, column: string, value: Value): string {
  const name = quoteIdent(dialect, column);
  return value.t === "null" ? `${name} IS NULL` : `${name} = ${literal(dialect, value)}`;
}

/**
 * Copy as UPDATE Statement: one `UPDATE … SET … WHERE …;` per row. The primary key identifies the
 * row (and is left out of SET); without one, every column's current value does. Staged rows that
 * are not saved yet (DEFAULT values in their key) are skipped.
 */
export function updateStatements(
  dialect: Dialect,
  table: TableRef,
  rows: readonly PluginRowData[],
): string[] {
  const target = qualify(dialect, table.schema, table.name);
  const out: string[] = [];
  for (const row of rows) {
    const pk = row.columns.flatMap((c, i) => (c.isPk ? [i] : []));
    const keyCols = pk.length > 0 ? pk : row.columns.map((_, i) => i);
    const setCols =
      pk.length > 0 ? row.columns.map((_, i) => i).filter((i) => !pk.includes(i)) : keyCols;
    const value = (i: number): Value => row.cells[i] ?? { t: "null" };
    if (setCols.length === 0 || keyCols.some((i) => value(i).t === "default")) continue;
    const set = setCols
      .map((i) => `${quoteIdent(dialect, row.columns[i]!.name)} = ${literal(dialect, value(i))}`)
      .join(", ");
    const where = keyCols
      .map((i) => condition(dialect, row.columns[i]!.name, value(i)))
      .join(" AND ");
    out.push(`UPDATE ${target} SET ${set} WHERE ${where};`);
  }
  return out;
}

export const COPY_AS_UPDATE: BuiltinPlugin = {
  identifier: "tabler.copy-as-update",
  name: "Copy as UPDATE Statement",
  version: "1.0",
  author: "Tabler",
  description: "Copies the selected rows as UPDATE statements, matched on the primary key.",
  commands: [
    {
      name: "Copy as UPDATE Statement",
      contexts: ["grid"],
      run: async (inv, host) => {
        const rows =
          inv.selectedRows.length > 0 ? inv.selectedRows : inv.clickedRow ? [inv.clickedRow] : [];
        if (!inv.item || !inv.driver || rows.length === 0) {
          host.notify("Copy as UPDATE Statement", "Select rows of a table first.");
          return;
        }
        const sql = updateStatements(inv.driver, inv.item, rows);
        if (sql.length === 0) {
          host.notify("Copy as UPDATE Statement", "The selected rows are not saved yet.");
          return;
        }
        await host.insertToClipboard(sql.join("\n"));
        host.notify(
          sql.length === 1 ? "Copied 1 UPDATE statement" : `Copied ${sql.length} UPDATE statements`,
          "",
        );
      },
    },
  ],
};
