import type { GridColumn } from "@/features/datagrid/types";
import type { SqlDialect } from "@/features/editor/scanner";
import type { ColumnMeta, ResultColumn, TableMeta, TableRef } from "@/lib/ipc";
import { isNumericKind } from "@/lib/values";
import {
  aliasedAs,
  isCompound,
  names,
  readsCteTwice,
  references,
  sqlTokens,
  type SqlToken,
} from "./statementRefs";

/**
 * Whether a SQL editor result can be edited like a table (TablePlus b210 / b600): its columns
 * report the base-table column they read (`ResultColumn.origin`), and one of those tables has all
 * of its primary-key columns in the result. That table is the result's *edit target*: its columns
 * become editable, everything else (other tables of a join, expressions, a repeated column) stays
 * read-only, and new / duplicated / deleted rows are rows of that table.
 *
 * An edit must reach the row the result shows, so the statement text is checked too: a result of
 * a set operation (UNION…) is read-only, and so is a table the statement does not name (read
 * through a view or a procedure). Columns only edit the table *reference* their key came from:
 * MySQL names it (`origin.instance`, the alias); without that, a table the statement reads more
 * than once (a self-join, a subquery or CTE over the same table) is not editable at all.
 */

/** What an edit attempt on a read-only result says (TablePlus: "Unable to detect table"). */
export const READ_ONLY_RESULT = "Read-only result: unable to detect the source table";

/** Why staged edits are refused (new ones, or the commit) after the edit target moved. */
export const TARGET_CHANGED = "The result's source table changed; discard the edits and re-run";

export interface ResultEditTarget {
  table: TableRef;
  meta: TableMeta;
  /** Per result column: its column in `table`, or null when it is not one (read-only). */
  columns: (ColumnMeta | null)[];
  /** Result column index of each primary-key column, in key order (`meta.primaryKey`). */
  keyIndexes: number[];
}

export type ResultEditability =
  | { kind: "editable"; target: ResultEditTarget }
  | { kind: "readOnly"; reason: string }
  /** Waiting for the table metadata. */
  | { kind: "pending" };

export const sameTable = (a: TableRef, b: TableRef): boolean =>
  a.name === b.name && (a.schema ?? null) === (b.schema ?? null);

/** `schema.table` (or just `table`) for messages. */
export const tableLabel = (t: TableRef): string => (t.schema ? `${t.schema}.${t.name}` : t.name);

/**
 * Whether two edit targets write the same columns of the same table under the same key: staged
 * edits keep the target they were made against, and a commit refuses when it moved.
 */
export function sameTarget(a: ResultEditTarget, b: ResultEditTarget): boolean {
  const same = <T>(x: readonly T[], y: readonly T[]) =>
    x.length === y.length && x.every((v, i) => v === y[i]);
  const names = (t: ResultEditTarget) => t.columns.map((c) => c?.name ?? null);
  return (
    sameTable(a.table, b.table) &&
    same(a.meta.primaryKey, b.meta.primaryKey) &&
    same(a.keyIndexes, b.keyIndexes) &&
    same(names(a), names(b))
  );
}

/** The distinct base tables the result's columns come from, in column order. */
export function originTables(columns: readonly ResultColumn[]): TableRef[] {
  const out: TableRef[] = [];
  for (const c of columns) {
    const t = c.origin?.table;
    if (t && !out.some((o) => sameTable(o, t))) out.push(t);
  }
  return out;
}

/** One reference to a base table: the table, and the alias it is read through when known. */
export interface OriginSource {
  table: TableRef;
  /** `ColumnOrigin.instance` (MySQL); null when the server does not tell. */
  instance: string | null;
}

const instanceOf = (c: ResultColumn): string | null => c.origin?.instance ?? null;

/** The distinct table references the result's columns come from, in column order. */
export function originSources(columns: readonly ResultColumn[]): OriginSource[] {
  const out: OriginSource[] = [];
  for (const c of columns) {
    const t = c.origin?.table;
    const instance = instanceOf(c);
    if (t && !out.some((o) => sameTable(o.table, t) && o.instance === instance))
      out.push({ table: t, instance });
  }
  return out;
}

/** The statement a result came from. */
export interface ResultStatement {
  /** Its text; null when it is not known (the result then stays read-only). */
  sql: string | null;
  /** Null: SQL of another engine, which the core lexers cannot read (read-only). */
  dialect: SqlDialect | null;
}

export interface EditabilityOptions extends ResultStatement {
  readOnlyConnection?: boolean;
  /** The session offers editing rows (`Feature::EditRows`; default yes). */
  canEditRows?: boolean;
}

const readOnly = (reason: string): ResultEditability => ({ kind: "readOnly", reason });

/** Why the statement text rules out editing `source`, or null when it does not. */
function referenceProblem(source: OriginSource, tokens: readonly SqlToken[]): string | null {
  const { table, instance } = source;
  const label = tableLabel(table);
  if (!names(tokens, table.name))
    return `The query does not name ${label} (its rows come through a view, a function or a procedure).`;
  if (instance !== null) {
    // MySQL tells the references apart; an alias must be one the query defines on this table
    // (a merged derived table or CTE reports the inner alias as the table).
    if (
      instance.toLowerCase() === table.name.toLowerCase() ||
      aliasedAs(tokens, table.name, instance)
    )
      return null;
    return `${label} is read through a derived table or CTE (${instance}).`;
  }
  if (references(tokens, table.name) > 1 || readsCteTwice(tokens))
    return `The table ${label} appears more than once in the query.`;
  return null;
}

/**
 * The result's edit target, given the metadata of its origin tables (`metaOf` returns undefined
 * while one is still loading, null when it could not be loaded). The first table reference (in
 * column order) whose whole primary key is in the result wins. A table whose metadata failed to
 * load makes the result read-only: it never lets a later table become the target instead.
 */
export function resultEditability(
  columns: readonly ResultColumn[],
  metaOf: (table: TableRef) => TableMeta | null | undefined,
  { sql, dialect, readOnlyConnection = false, canEditRows = true }: EditabilityOptions,
): ResultEditability {
  const sources = originSources(columns);
  if (sources.length === 0) return readOnly("Its columns do not come from a table.");
  if (readOnlyConnection) return readOnly("The connection is read-only.");
  if (!canEditRows) return readOnly("The connection cannot edit rows.");
  if (sql === null) return readOnly("The query that produced it is not known.");
  // The statement must be read to find its table; only the core dialects are. Edits always go
  // by the table's primary key, never by all the row's columns.
  if (dialect === null) return readOnly("The query is not SQL this editor reads.");
  const tokens = sqlTokens(sql, dialect);
  if (isCompound(tokens))
    return readOnly("It combines the rows of several queries (UNION, EXCEPT or INTERSECT).");
  const reasons: string[] = [];
  let pending = false;
  for (const source of sources) {
    const { table } = source;
    const problem = referenceProblem(source, tokens);
    if (problem) {
      reasons.push(problem);
      continue;
    }
    const meta = metaOf(table);
    if (meta === undefined) {
      pending = true;
      continue;
    }
    if (meta === null)
      return readOnly(`The structure of ${tableLabel(table)} could not be loaded.`);
    // Earlier tables win: until they are known, a later one must not be picked.
    if (pending) continue;
    if (meta.kind !== "table") {
      reasons.push(`${tableLabel(table)} is a ${meta.kind}, not a table.`);
      continue;
    }
    if (meta.primaryKey.length === 0) {
      reasons.push(`${tableLabel(table)} has no primary key.`);
      continue;
    }
    const target = targetFor(columns, table, meta, source.instance);
    if (target) return { kind: "editable", target };
    const missing = meta.primaryKey.filter((pk) => !hasOrigin(columns, source, pk));
    reasons.push(
      `The primary key of ${tableLabel(table)} (${missing.join(", ")}) is not in the result.`,
    );
  }
  if (pending) return { kind: "pending" };
  return readOnly(reasons[0] ?? "");
}

const fromSource = (c: ResultColumn, table: TableRef, instance: string | null): boolean =>
  !!c.origin && sameTable(c.origin.table, table) && instanceOf(c) === instance;

function hasOrigin(
  columns: readonly ResultColumn[],
  source: OriginSource,
  column: string,
): boolean {
  return columns.some(
    (c) => fromSource(c, source.table, source.instance) && c.origin!.column === column,
  );
}

/**
 * Maps the result columns read through one reference to `table` (`instance`, see
 * `OriginSource`) onto it; null when one of its key columns is missing.
 */
export function targetFor(
  columns: readonly ResultColumn[],
  table: TableRef,
  meta: TableMeta,
  instance: string | null = null,
): ResultEditTarget | null {
  const seen = new Set<string>();
  const mapped = columns.map((c) => {
    const o = c.origin;
    if (!o || !fromSource(c, table, instance) || seen.has(o.column)) return null;
    const col = meta.columns.find((m) => m.name === o.column) ?? null;
    // `select id, id from t`: only the first one edits the column.
    if (col) seen.add(o.column);
    return col;
  });
  const keyIndexes = meta.primaryKey.map((pk) => mapped.findIndex((m) => m?.name === pk));
  if (keyIndexes.some((i) => i < 0)) return null;
  return { table, meta, columns: mapped, keyIndexes };
}

/**
 * Grid columns of a result. Columns of the edit target take their editing traits (NOT NULL,
 * defaults, enum values, a foreign key) from the table; the others are read-only. The grid id is
 * the column position (result columns can repeat a name).
 */
export function resultGridColumns(
  columns: readonly ResultColumn[],
  target: ResultEditTarget | null = null,
): GridColumn[] {
  return columns.map((c, i) => {
    const meta = target?.columns[i] ?? null;
    const fk = meta
      ? target!.meta.foreignKeys.find((f) => f.columns.length === 1 && f.columns[0] === meta.name)
      : undefined;
    return {
      id: String(i),
      name: c.name,
      kind: c.kind,
      dataType: c.dataType,
      nullable: meta ? meta.nullable : (c.nullable ?? true),
      isPk: meta?.isPk ?? false,
      hasDefault: meta ? meta.defaultExpr !== null || meta.isAutoIncrement : false,
      isAutoInc: meta?.isAutoIncrement ?? false,
      editable: meta !== null && !meta.isGenerated,
      align: isNumericKind(c.kind) ? "right" : "left",
      fk: fk
        ? { schema: fk.refTable.schema, table: fk.refTable.name, column: fk.refColumns[0] ?? "" }
        : undefined,
      enumValues: meta?.enumValues ?? null,
    } satisfies GridColumn;
  });
}
