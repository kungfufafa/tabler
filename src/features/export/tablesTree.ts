import type { TableRef, TableSummary } from "@/lib/ipc";

/**
 * The multi-table export's choice of tables across the databases of a connection: the keys of
 * the checked tables, grouped back into one request per database.
 */

/** A table of one database, as the dialog keys it. */
export interface TreeTable {
  key: string;
  database: string | null;
  table: TableRef;
  summary: TableSummary;
}

const SEP = "\u0000";

/** `database` null: the connection has no database level (SQLite). */
export function tableKey(database: string | null, table: TableRef): string {
  return [database ?? "", table.schema ?? "", table.name].join(SEP);
}

export function treeTables(database: string | null, list: readonly TableSummary[]): TreeTable[] {
  return list.map((summary) => {
    const table: TableRef = { schema: summary.schema, name: summary.name };
    return { key: tableKey(database, table), database, table, summary };
  });
}

/** The label of a table in its database: `schema.name` when the database has several schemas. */
export function tableLabel(t: TreeTable, multiSchema: boolean): string {
  return multiSchema && t.table.schema ? `${t.table.schema}.${t.table.name}` : t.table.name;
}

export function hasSeveralSchemas(list: readonly TreeTable[]): boolean {
  return new Set(list.map((t) => t.table.schema ?? "")).size > 1;
}

/** Tables whose name (or `schema.name`) or database matches `search`, case-insensitively. */
export function matchTables(list: readonly TreeTable[], search: string): TreeTable[] {
  const q = search.trim().toLowerCase();
  if (q === "") return [...list];
  return list.filter((t) => {
    const full = `${t.table.schema ?? ""}.${t.table.name}`.toLowerCase();
    return full.includes(q) || (t.database ?? "").toLowerCase().includes(q);
  });
}

/** One request's worth: the checked tables of one database, in list order. */
export interface TablesGroup {
  database: string | null;
  tables: TableRef[];
}

/**
 * The checked tables grouped by database, the current database first, then in the order the
 * databases are listed.
 */
export function groupChosen(
  databases: readonly (string | null)[],
  loaded: ReadonlyMap<string | null, readonly TreeTable[]>,
  checked: ReadonlySet<string>,
  current: string | null,
): TablesGroup[] {
  const order = [...databases].sort((a, b) => Number(b === current) - Number(a === current));
  const out: TablesGroup[] = [];
  for (const db of order) {
    const tables = (loaded.get(db) ?? []).filter((t) => checked.has(t.key)).map((t) => t.table);
    if (tables.length > 0) out.push({ database: db, tables });
  }
  return out;
}

/**
 * The tri-state of a database's checkbox: every table checked, some, or none (`false` too while
 * its tables are not loaded, unless it is waiting to be checked in full).
 */
export function databaseState(
  tables: readonly TreeTable[] | undefined,
  checked: ReadonlySet<string>,
  pendingAll: boolean,
): boolean | "indeterminate" {
  if (!tables) return pendingAll;
  const n = tables.filter((t) => checked.has(t.key)).length;
  if (tables.length > 0 && n === tables.length) return true;
  return n > 0 ? "indeterminate" : false;
}

/** A file-system friendly name for a database's file or folder of a multi-database export. */
export function databaseFileBase(database: string): string {
  const cleaned = database
    .normalize("NFC")
    .replace(/[^\p{L}\p{M}\p{N}_.-]+/gu, "_")
    .replace(/^[_.]+|_+$/g, "");
  return cleaned || "database";
}

/**
 * `databaseFileBase` of every database, made distinct: two databases whose names reduce to the
 * same base (`my db` / `my_db`, `$$` / `@@`) or differ only in case (`Sales` / `sales`, one file on
 * a case-insensitive disk) would write the same file or folder, the second replacing the first.
 * Each later duplicate gets `_2`, `_3`… (the first free one, free of every other base too).
 */
export function uniqueDatabaseBases(databases: readonly string[]): string[] {
  const bases = databases.map(databaseFileBase);
  const key = (name: string) => name.toLowerCase();
  const firstAt = new Map<string, number>();
  bases.forEach((base, i) => {
    if (!firstAt.has(key(base))) firstAt.set(key(base), i);
  });
  // Names in use: those every database keeps (its first occurrence), then those handed out.
  const taken = new Set(firstAt.keys());
  return bases.map((base, i) => {
    if (firstAt.get(key(base)) === i) return base;
    let n = 2;
    while (taken.has(key(`${base}_${n}`))) n++;
    const unique = `${base}_${n}`;
    taken.add(key(unique));
    return unique;
  });
}
