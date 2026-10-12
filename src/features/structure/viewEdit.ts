import type { DdlOp, Dialect, TableKind, TableRef } from "@/lib/ipc";
import { dropViewOp } from "./tableActions";

/**
 * The statement the view editor starts from: PostgreSQL and MySQL views are replaced in place
 * (`CREATE OR REPLACE VIEW`); PostgreSQL materialized views and SQLite views have no OR REPLACE,
 * so their text stays as the server reports it and applying drops and creates them.
 */
export function editableViewSql(dialect: Dialect, kind: TableKind, ddl: string): string {
  const text = ddl.trim();
  if (kind === "materializedView" || dialect === "sqlite") return text;
  if (/^\s*create\s+or\s+replace\b/i.test(text)) return text;
  return text.replace(/^\s*create\s+/i, "CREATE OR REPLACE ");
}

/** The staged ops for an edited view statement (none while it is unchanged). */
export function viewOps(
  dialect: Dialect,
  kind: TableKind,
  table: TableRef,
  original: string,
  draft: string,
): DdlOp[] {
  const sql = draft.trim();
  if (!sql || sql === original.trim()) return [];
  const replaceable = kind !== "materializedView" && dialect !== "sqlite";
  if (replaceable) return [{ op: "rawSql", sql }];
  return [dropViewOp(table, kind, dialect, false), { op: "rawSql", sql }];
}
