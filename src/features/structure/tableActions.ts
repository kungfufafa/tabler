import type { DdlOp, Dialect, FkDef, IndexDef, TableKind, TableRef, TableSummary } from "@/lib/ipc";
import { quoteIdent } from "@/lib/sql/quote";

/** Kinds whose structure can be altered (and that can be renamed / truncated). */
export const ALTERABLE_KINDS: readonly TableKind[] = ["table", "partitioned"];

export function isAlterable(kind: TableKind): boolean {
  return ALTERABLE_KINDS.includes(kind);
}

export function isViewKind(kind: TableKind): boolean {
  return kind === "view" || kind === "materializedView";
}

export const KIND_LABEL: Record<TableKind, string> = {
  table: "Table",
  partitioned: "Partitioned table",
  foreignTable: "Foreign table",
  view: "View",
  systemView: "System view",
  materializedView: "Materialized view",
};

export function refOf(t: Pick<TableSummary, "schema" | "name">): TableRef {
  return { schema: t.schema, name: t.name };
}

export function renameTableOp(table: TableRef, newName: string): DdlOp {
  return { op: "renameTable", table, newName: newName.trim() };
}

export function truncateTableOp(
  table: TableRef,
  dialect: Dialect,
  restartIdentity: boolean,
): DdlOp {
  return {
    op: "truncateTable",
    table,
    restartIdentity: dialect === "postgres" && restartIdentity,
    cascade: false,
  };
}

export function dropTableOp(table: TableRef, dialect: Dialect, cascade: boolean): DdlOp {
  return { op: "dropTable", table, ifExists: false, cascade: dialect === "postgres" && cascade };
}

/**
 * `DROP VIEW` / `DROP MATERIALIZED VIEW` as raw SQL (the DDL ops have no view variant). Names are
 * quoted with `quoteIdent`; CASCADE is PostgreSQL-only.
 */
export function dropViewOp(
  table: TableRef,
  kind: TableKind,
  dialect: Dialect,
  cascade: boolean,
): DdlOp {
  const name = table.schema
    ? `${quoteIdent(dialect, table.schema)}.${quoteIdent(dialect, table.name)}`
    : quoteIdent(dialect, table.name);
  const what = kind === "materializedView" && dialect === "postgres" ? "MATERIALIZED VIEW" : "VIEW";
  const tail = dialect === "postgres" && cascade ? " CASCADE" : "";
  return { op: "rawSql", sql: `DROP ${what} ${name}${tail}` };
}

/** Error for a new table name, else null. */
export function tableNameError(name: string, current?: string): string | null {
  const n = name.trim();
  if (!n) return "Name is required";
  if (current !== undefined && n === current) return "Enter a different name";
  return null;
}

// --- indexes / foreign keys ------------------------------------------------------------------

const MAX_IDENT = 63;

/** Conventional index name: `<table>_<cols>_idx` (`_key` for unique), at most 63 chars. */
export function suggestIndexName(
  table: string,
  columns: readonly string[],
  unique: boolean,
): string {
  const cols = columns.map((c) => c.replace(/[^\w]+/g, "_").toLowerCase()).join("_");
  const base = `${table.replace(/[^\w]+/g, "_").toLowerCase()}_${cols || "col"}_${unique ? "key" : "idx"}`;
  return base.slice(0, MAX_IDENT);
}

export function indexMethods(dialect: Dialect): readonly string[] {
  if (dialect === "postgres") return ["btree", "hash", "gin", "gist", "brin", "spgist"];
  if (dialect === "mysql") return ["BTREE", "HASH", "FULLTEXT", "SPATIAL"];
  return [];
}

export function supportsPartialIndex(dialect: Dialect): boolean {
  return dialect !== "mysql";
}

export interface IndexDraft {
  name: string;
  columns: string[];
  unique: boolean;
  method: string;
  predicate: string;
}

export function validateIndex(
  draft: IndexDraft,
  dialect: Dialect,
  existingNames: readonly string[],
): { ok: true; index: IndexDef } | { ok: false; errors: string[] } {
  const errors: string[] = [];
  const name = draft.name.trim();
  if (!name) errors.push("Index name is required");
  else if (existingNames.some((n) => n.toLowerCase() === name.toLowerCase()))
    errors.push(`An index named “${name}” already exists`);
  if (draft.columns.length === 0) errors.push("Pick at least one column");
  if (errors.length > 0) return { ok: false, errors };
  const method = indexMethods(dialect).includes(draft.method) ? draft.method : null;
  const predicate = supportsPartialIndex(dialect) ? draft.predicate.trim() || null : null;
  return {
    ok: true,
    index: { name, columns: [...draft.columns], unique: draft.unique, method, predicate },
  };
}

export const FK_ACTIONS = ["NO ACTION", "RESTRICT", "CASCADE", "SET NULL", "SET DEFAULT"] as const;

export interface FkDraft {
  name: string;
  columns: string[];
  refTable: TableRef | null;
  refColumns: string[];
  /** "" = database default. */
  onUpdate: string;
  onDelete: string;
}

export function validateForeignKey(
  draft: FkDraft,
): { ok: true; fk: FkDef } | { ok: false; errors: string[] } {
  const errors: string[] = [];
  if (draft.columns.length === 0) errors.push("Pick at least one column");
  if (!draft.refTable) errors.push("Pick the referenced table");
  else if (draft.refColumns.length === 0) errors.push("Pick the referenced columns");
  else if (draft.refColumns.length !== draft.columns.length)
    errors.push("Pick as many referenced columns as columns");
  if (errors.length > 0 || !draft.refTable) return { ok: false, errors };
  return {
    ok: true,
    fk: {
      name: draft.name.trim() || null,
      columns: [...draft.columns],
      refTable: draft.refTable,
      refColumns: [...draft.refColumns],
      onUpdate: draft.onUpdate || null,
      onDelete: draft.onDelete || null,
    },
  };
}

/** Toggles `item` in an ordered selection (appends when missing). */
export function toggleOrdered(list: readonly string[], item: string): string[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}
