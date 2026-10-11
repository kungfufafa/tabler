import type { ColumnDef, DdlOp, Dialect, TableRef } from "@/lib/ipc";
import { defaultColumnType, idColumnType, isIntegerType, supportsComments } from "./columnEdits";

/** One editable row of the Create Table dialog. */
export interface ColumnDraft {
  id: string;
  name: string;
  dataType: string;
  nullable: boolean;
  defaultExpr: string;
  primaryKey: boolean;
  autoIncrement: boolean;
  comment: string;
}

let seq = 0;
const nextId = () => `draft-${++seq}`;

export function newColumnDraft(
  dialect: Dialect,
  overrides: Partial<ColumnDraft> = {},
): ColumnDraft {
  return {
    id: nextId(),
    name: "",
    dataType: defaultColumnType(dialect),
    nullable: true,
    defaultExpr: "",
    primaryKey: false,
    autoIncrement: false,
    comment: "",
    ...overrides,
  };
}

/** Starting rows: an auto-incrementing `id` primary key. */
export function initialColumns(dialect: Dialect): ColumnDraft[] {
  return [
    newColumnDraft(dialect, {
      name: "id",
      dataType: idColumnType(dialect),
      nullable: false,
      primaryKey: true,
      autoIncrement: true,
    }),
  ];
}

export type CreateTableResult =
  { ok: true; op: Extract<DdlOp, { op: "createTable" }> } | { ok: false; errors: string[] };

function toDef(dialect: Dialect, d: ColumnDraft): ColumnDef {
  const comment = d.comment.trim();
  const defaultExpr = d.defaultExpr.trim();
  return {
    name: d.name.trim(),
    dataType: d.dataType.trim().replace(/\s+/g, " "),
    nullable: d.primaryKey ? false : d.nullable,
    defaultExpr: defaultExpr === "" ? null : defaultExpr,
    autoIncrement: d.autoIncrement,
    comment: supportsComments(dialect) && comment !== "" ? comment : null,
    collation: null,
    unsigned: false,
  };
}

/**
 * Validates the dialog and builds the `createTable` op. Errors: missing table name, no columns,
 * unnamed / untyped columns, duplicate names (case-insensitive: MySQL and SQLite fold case) and
 * auto-increment rules of each dialect.
 */
export function buildCreateTable(
  dialect: Dialect,
  schema: string | null,
  name: string,
  drafts: readonly ColumnDraft[],
): CreateTableResult {
  const errors: string[] = [];
  const tableName = name.trim();
  if (!tableName) errors.push("Table name is required");
  if (drafts.length === 0) errors.push("Add at least one column");

  const seen = new Set<string>();
  const dupes = new Set<string>();
  drafts.forEach((d, i) => {
    const n = d.name.trim();
    if (!n) errors.push(`Column ${i + 1} needs a name`);
    else {
      const k = n.toLowerCase();
      if (seen.has(k)) dupes.add(n);
      seen.add(k);
    }
    if (!d.dataType.trim()) errors.push(`Column “${n || i + 1}” needs a type`);
  });
  for (const n of dupes) errors.push(`Duplicate column name “${n}”`);

  const pk = drafts.filter((d) => d.primaryKey);
  const auto = drafts.filter((d) => d.autoIncrement);
  for (const d of auto) {
    const label = d.name.trim() || "?";
    if (!isIntegerType(d.dataType))
      errors.push(`Auto increment needs an integer type (column “${label}”)`);
    if (dialect === "sqlite" && !(pk.length === 1 && d.primaryKey))
      errors.push("SQLite AUTOINCREMENT needs a single INTEGER PRIMARY KEY column");
    if (dialect === "mysql" && !d.primaryKey)
      errors.push(`MySQL AUTO_INCREMENT column “${label}” must be part of the primary key`);
  }
  if (auto.length > 1 && dialect !== "postgres")
    errors.push("Only one auto-increment column is allowed");

  if (errors.length > 0) return { ok: false, errors: [...new Set(errors)] };
  const table: TableRef = { schema, name: tableName };
  return {
    ok: true,
    op: {
      op: "createTable",
      table,
      columns: drafts.map((d) => toDef(dialect, d)),
      primaryKey: pk.map((d) => d.name.trim()),
      ifNotExists: false,
    },
  };
}

/** Moves the draft at `from` by `delta` rows (clamped). */
export function moveDraft<T>(list: readonly T[], from: number, delta: number): T[] {
  const to = Math.max(0, Math.min(list.length - 1, from + delta));
  if (to === from || from < 0 || from >= list.length) return list.slice();
  const next = list.slice();
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item!);
  return next;
}
