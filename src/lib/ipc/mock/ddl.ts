// Mock handlers for preview_ddl, apply_ddl, get_table_ddl (the M5 structure editor).
//
// The plans mirror the shape of `src-tauri/src/sqlgen/ddl.rs`: PostgreSQL plans are transactional,
// MySQL plans run statement by statement (DDL auto-commits), SQLite rebuilds the table for type /
// nullability / default / key changes (own BEGIN…COMMIT, with a warning). `apply_ddl` updates the
// in-memory catalog (`TABLE_COLUMNS` plus per-table overlays for indexes, keys and table lists),
// so a refetch shows the change; a type named `nosuchtype` fails like the server would, and so
// does creating or renaming a table to a name the schema already uses.
//
// Limitation: `TABLE_COLUMNS` (read by every mock) is keyed by bare table name, so column changes
// show up wherever a table of that name exists (other schemas and drivers too).
//
// `get_table` and `list_tables` are re-registered here (same behaviour as in handlers.ts) so they
// can include the overlays.
import type { ColumnDef } from "../bindings/ColumnDef";
import type { ColumnMeta } from "../bindings/ColumnMeta";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { DdlOp } from "../bindings/DdlOp";
import type { DdlPlan } from "../bindings/DdlPlan";
import type { DdlResult } from "../bindings/DdlResult";
import type { Driver } from "../bindings/Driver";
import type { ErrorPayload } from "../bindings/ErrorPayload";
import type { ForeignKeyMeta } from "../bindings/ForeignKeyMeta";
import type { IndexDef } from "../bindings/IndexDef";
import type { IndexMeta } from "../bindings/IndexMeta";
import type { TableMeta } from "../bindings/TableMeta";
import type { TableRef } from "../bindings/TableRef";
import type { TableSummary } from "../bindings/TableSummary";
import type { Value } from "../bindings/Value";
import type { ValueKind } from "../bindings/ValueKind";
// handlers.ts registers the base get_table / list_tables; it must run before the overrides below.
import "./handlers";
import { TABLE_COLUMNS, tableMetaFor, tablesFor } from "./fixtures";
import { rowsFor } from "./rows";
import { mockError, mockInvoke, registerMock } from "./transport";

type ColSpec = NonNullable<(typeof TABLE_COLUMNS)[string]>[number];

const ORIGINAL_COLUMNS: string = JSON.stringify(TABLE_COLUMNS);

interface Overlay {
  indexes: IndexMeta[];
  droppedIndexes: string[];
  fks: ForeignKeyMeta[];
  droppedFks: string[];
  comment?: string | null;
}

/** `${driver}:${schema}` → the table list once a DDL changed it. */
const lists = new Map<string, TableSummary[]>();
/** `${driver}:${schema}.${name}` → index / key / comment changes. */
const overlays = new Map<string, Overlay>();

/** Test helper: restores the fixture catalog (cached mock rows are not reset). */
export function resetMockDdl(): void {
  const original = JSON.parse(ORIGINAL_COLUMNS) as typeof TABLE_COLUMNS;
  for (const k of Object.keys(TABLE_COLUMNS)) delete TABLE_COLUMNS[k];
  Object.assign(TABLE_COLUMNS, original);
  lists.clear();
  overlays.clear();
}

async function connectionOf(connectionId: unknown): Promise<ConnectionInfo> {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  return (
    all.find((c) => c.connectionId === connectionId) ?? mockError("NOT_CONNECTED", "not connected")
  );
}

const schemaOf = (t: TableRef) => t.schema ?? "public";
const tableId = (d: Driver, t: TableRef) => `${d}:${schemaOf(t)}.${t.name}`;
const listId = (d: Driver, schema: string) => `${d}:${schema}`;

function listOf(d: Driver, schema: string): TableSummary[] {
  return lists.get(listId(d, schema)) ?? tablesFor(d, schema);
}

function editableList(d: Driver, schema: string): TableSummary[] {
  const key = listId(d, schema);
  let list = lists.get(key);
  if (!list) {
    list = tablesFor(d, schema);
    lists.set(key, list);
  }
  return list;
}

function overlayOf(d: Driver, t: TableRef): Overlay {
  const key = tableId(d, t);
  let o = overlays.get(key);
  if (!o) {
    o = { indexes: [], droppedIndexes: [], fks: [], droppedFks: [] };
    overlays.set(key, o);
  }
  return o;
}

/** Catalog metadata of a table including the DDL overlays; null when it does not exist. */
function metaOf(d: Driver, t: TableRef): TableMeta | null {
  const schema = schemaOf(t);
  if (lists.has(listId(d, schema)) && !listOf(d, schema).some((x) => x.name === t.name))
    return null;
  const meta = tableMetaFor(d, schema, t.name);
  if (!meta) return null;
  const summary = listOf(d, schema).find((x) => x.name === t.name);
  if (summary && meta.rowEstimate === null) meta.rowEstimate = summary.rowEstimate;
  const o = overlays.get(tableId(d, t));
  if (!o) return meta;
  return {
    ...meta,
    indexes: [...meta.indexes.filter((i) => !o.droppedIndexes.includes(i.name)), ...o.indexes],
    foreignKeys: [...meta.foreignKeys.filter((f) => !o.droppedFks.includes(f.name)), ...o.fks],
    comment: o.comment !== undefined ? o.comment : meta.comment,
  };
}

registerMock("list_tables", async ({ connectionId, schema }) => {
  const c = await connectionOf(connectionId);
  if (schema === "temp") return [];
  return listOf(c.driver, schema as string).map((t) => ({ ...t }));
});

registerMock("get_table", async ({ connectionId, table }) => {
  const c = await connectionOf(connectionId);
  const t = table as TableRef;
  return metaOf(c.driver, t) ?? mockError("NOT_FOUND", `table ${t.name}`);
});

// --- SQL text ----------------------------------------------------------------------------------

const q = (d: Driver, ident: string) =>
  d === "mysql" ? `\`${ident.replace(/`/g, "``")}\`` : `"${ident.replace(/"/g, '""')}"`;
const qt = (d: Driver, t: TableRef) =>
  t.schema ? `${q(d, t.schema)}.${q(d, t.name)}` : q(d, t.name);
const lit = (d: Driver, s: string) =>
  d === "mysql"
    ? `'${s.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`
    : `'${s.replace(/'/g, "''")}'`;
const cols = (d: Driver, names: readonly string[]) => names.map((n) => q(d, n)).join(", ");

const SQLITE_NO_COMMENTS = "SQLite does not store table or column comments; ignored";
const POSITION_MYSQL_ONLY = "column position (AFTER/FIRST) is MySQL-only; ignored";

/** Column name of an index entry (`email`, `name(10)`, `created_at DESC`). */
function indexColumn(entry: string): string {
  return /^([\w$]+)/.exec(entry.trim())?.[1] ?? entry.trim();
}

function indexEntry(d: Driver, entry: string): string {
  const m = /^([\w$]+)(\s*\(\d+\))?(\s+(?:ASC|DESC))?$/i.exec(entry.trim());
  return m ? `${q(d, m[1]!)}${m[2] ?? ""}${m[3] ?? ""}` : entry;
}

/** Column definition text (for CREATE TABLE / ADD COLUMN / MODIFY). */
function columnSql(
  d: Driver,
  c: {
    name: string;
    dataType: string;
    nullable: boolean;
    defaultExpr: string | null;
    autoIncrement: boolean;
    comment: string | null;
    unsigned?: boolean;
    identity?: "ALWAYS" | "BY DEFAULT";
  },
  inlinePk = false,
): string {
  let sql = `${q(d, c.name)} ${c.dataType}`;
  if (d === "mysql" && c.unsigned && !/unsigned/i.test(c.dataType)) sql += " UNSIGNED";
  if (d === "postgres" && c.autoIncrement && !/nextval\(/i.test(c.defaultExpr ?? ""))
    sql += ` GENERATED ${c.identity ?? "BY DEFAULT"} AS IDENTITY`;
  if (d === "mysql") sql += c.nullable ? " NULL" : " NOT NULL";
  else if (!c.nullable) sql += " NOT NULL";
  if (inlinePk) sql += " PRIMARY KEY" + (c.autoIncrement ? " AUTOINCREMENT" : "");
  if (c.defaultExpr !== null) sql += ` DEFAULT ${c.defaultExpr}`;
  if (d === "mysql" && c.autoIncrement) sql += " AUTO_INCREMENT";
  if (d === "mysql" && c.comment) sql += ` COMMENT ${lit(d, c.comment)}`;
  return sql;
}

function fkSql(
  d: Driver,
  fk: Pick<ForeignKeyMeta, "columns" | "refColumns" | "refTable"> & {
    name: string | null;
    onUpdate: string | null;
    onDelete: string | null;
  },
): string {
  let sql = fk.name ? `CONSTRAINT ${q(d, fk.name)} ` : "";
  sql += `FOREIGN KEY (${cols(d, fk.columns)}) REFERENCES ${qt(d, fk.refTable)} (${cols(d, fk.refColumns)})`;
  if (fk.onUpdate && fk.onUpdate !== "NO ACTION") sql += ` ON UPDATE ${fk.onUpdate}`;
  if (fk.onDelete && fk.onDelete !== "NO ACTION") sql += ` ON DELETE ${fk.onDelete}`;
  return sql;
}

function indexSql(d: Driver, t: TableRef, index: IndexDef, warn: (w: string) => void): string {
  const entries = index.columns.map((e) => indexEntry(d, e)).join(", ");
  if (d === "mysql") {
    const kind = index.method?.toUpperCase();
    const special = kind === "FULLTEXT" || kind === "SPATIAL" ? `${kind} ` : "";
    if (index.predicate) warn(`partial indexes (WHERE) are not supported by MySQL; ignored`);
    const using = kind === "BTREE" || kind === "HASH" ? ` USING ${kind}` : "";
    return `CREATE ${index.unique ? "UNIQUE " : special}INDEX ${q(d, index.name)} ON ${qt(d, t)} (${entries})${using}`;
  }
  if (d === "sqlite") {
    if (index.method) warn(`index method "${index.method}" is not supported by SQLite; ignored`);
    const name = t.schema ? `${q(d, t.schema)}.${q(d, index.name)}` : q(d, index.name);
    return `CREATE ${index.unique ? "UNIQUE " : ""}INDEX ${name} ON ${q(d, t.name)} (${entries})${index.predicate ? ` WHERE ${index.predicate}` : ""}`;
  }
  return `CREATE ${index.unique ? "UNIQUE " : ""}INDEX ${q(d, index.name)} ON ${qt(d, t)}${index.method ? ` USING ${index.method}` : ""} (${entries})${index.predicate ? ` WHERE ${index.predicate}` : ""}`;
}

// --- planning ----------------------------------------------------------------------------------

const invalid = (msg: string): never => mockError("INVALID_INPUT", msg);

function requireName(value: string, what: string) {
  if (!value || !value.trim()) invalid(`${what} must not be empty`);
}

/** Working copy of a table while planning: columns with the name they were copied from. */
interface Work {
  meta: TableMeta;
  sources: Array<string | null>;
}

function findCol(w: Work, name: string): ColumnMeta {
  return (
    w.meta.columns.find((c) => c.name === name) ??
    invalid(`column "${name}" does not exist in "${w.meta.table.name}"`)
  );
}

function kindOf(type: string): ValueKind {
  const t = type.toLowerCase();
  if (/^(tiny|small|medium)?int(eger)?\b|^int[24]$|^serial$/.test(t))
    return t.startsWith("tinyint(1)") ? "bool" : "int";
  if (/^bigint|^int8|^bigserial/.test(t)) return "bigInt";
  if (/^(numeric|decimal)/.test(t)) return "decimal";
  if (/^(real|float|double)/.test(t)) return "float";
  if (/^bool/.test(t)) return "bool";
  if (/^(timestamptz|timestamp with time zone)/.test(t)) return "timestampTz";
  if (/^(timestamp|datetime)/.test(t)) return "timestamp";
  if (/^date/.test(t)) return "date";
  if (/^time/.test(t)) return "time";
  if (/^interval/.test(t)) return "interval";
  if (/^uuid/.test(t)) return "uuid";
  if (/^jsonb?/.test(t)) return "json";
  if (/^(bytea|blob|binary|varbinary)/.test(t)) return "bytes";
  if (/\[\]$/.test(t)) return "array";
  return "text";
}

function columnFromDef(def: ColumnDef, ordinal: number, pk: boolean): ColumnMeta {
  return {
    name: def.name,
    ordinal,
    dataType: def.dataType,
    kind: kindOf(def.dataType),
    nullable: pk ? false : def.nullable,
    defaultExpr: def.defaultExpr,
    isPk: pk,
    pkOrdinal: null,
    isAutoIncrement: def.autoIncrement,
    isGenerated: false,
    isUnsigned: def.unsigned,
    comment: def.comment,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: def.collation,
    arrayElemType: null,
    extra: null,
  };
}

function validateDef(def: ColumnDef) {
  requireName(def.name, "column name");
  requireName(def.dataType, `type of column "${def.name}"`);
}

const SQLITE_REBUILD_OPS = new Set<DdlOp["op"]>([
  "alterColumnType",
  "alterColumnNullable",
  "alterColumnDefault",
  "addForeignKey",
  "dropForeignKey",
  "addPrimaryKey",
  "dropPrimaryKey",
]);

/** Ops that act on a whole table and are never folded into a SQLite rebuild. */
const TABLE_LEVEL = new Set<DdlOp["op"]>([
  "createTable",
  "dropTable",
  "truncateTable",
  "renameTable",
]);

interface Planned extends DdlPlan {
  /** For each op: [first, end) statement range. */
  ranges: Array<[number, number]>;
  rebuild: boolean;
}

function mysqlModify(d: Driver, t: TableRef, c: ColumnMeta): string {
  return `ALTER TABLE ${qt(d, t)} MODIFY COLUMN ${columnSql(d, {
    name: c.name,
    dataType: c.dataType,
    nullable: c.nullable,
    defaultExpr: c.defaultExpr,
    autoIncrement: c.isAutoIncrement,
    comment: c.comment,
  })}`;
}

function plan(d: Driver, table: TableRef | null, ops: DdlOp[]): Planned {
  const statements: string[] = [];
  const warnings: string[] = [];
  const ranges: Array<[number, number]> = [];
  const warn = (w: string) => {
    if (!warnings.includes(w)) warnings.push(w);
  };
  let transactional = d !== "mysql";
  const base = table ? metaOf(d, table) : null;
  const work: Work | null = base
    ? {
        meta: { ...base, columns: base.columns.map((c) => ({ ...c })), indexes: [...base.indexes] },
        sources: base.columns.map((c) => c.name),
      }
    : null;
  const onPlanned = (op: DdlOp) =>
    op.op !== "rawSql" &&
    table !== null &&
    op.table.name === table.name &&
    (op.table.schema ?? "") === (table.schema ?? "");
  // SQLite folds every column / key change of the planned table into one rebuild.
  const rebuild =
    d === "sqlite" &&
    work !== null &&
    ops.some((op) => SQLITE_REBUILD_OPS.has(op.op) && onPlanned(op));

  const needWork = (t: TableRef): Work => {
    if (work && table && t.name === table.name && (t.schema ?? "") === (table.schema ?? ""))
      return work;
    return mockError("NOT_FOUND", `table "${t.name}" not found`);
  };

  for (const op of ops) {
    const start = statements.length;
    const folded = rebuild && onPlanned(op) && !TABLE_LEVEL.has(op.op);
    const push = (sql: string) => {
      if (!folded) statements.push(sql);
    };
    switch (op.op) {
      case "rawSql": {
        requireName(op.sql, "SQL");
        statements.push(op.sql);
        if (/\bconcurrently\b|^\s*vacuum\b/i.test(op.sql)) transactional = false;
        break;
      }
      case "createTable": {
        requireName(op.table.name, "table name");
        if (op.columns.length === 0) invalid("a table needs at least one column");
        const seen = new Set<string>();
        for (const c of op.columns) {
          validateDef(c);
          if (seen.has(c.name.toLowerCase())) invalid(`column "${c.name}" is listed twice`);
          seen.add(c.name.toLowerCase());
        }
        const inline =
          d === "sqlite" &&
          op.primaryKey.length === 1 &&
          op.columns.some((c) => c.name === op.primaryKey[0] && c.autoIncrement);
        const lines = op.columns.map(
          (c) => `  ${columnSql(d, c, inline && c.name === op.primaryKey[0])}`,
        );
        if (op.primaryKey.length > 0 && !inline)
          lines.push(`  PRIMARY KEY (${cols(d, op.primaryKey)})`);
        statements.push(
          `CREATE TABLE ${op.ifNotExists ? "IF NOT EXISTS " : ""}${qt(d, op.table)} (\n${lines.join(",\n")}\n)`,
        );
        if (d === "postgres")
          for (const c of op.columns)
            if (c.comment)
              statements.push(
                `COMMENT ON COLUMN ${qt(d, op.table)}.${q(d, c.name)} IS ${lit(d, c.comment)}`,
              );
        if (d === "sqlite" && op.columns.some((c) => c.comment)) warn(SQLITE_NO_COMMENTS);
        break;
      }
      case "dropTable":
        if (d !== "postgres" && op.cascade)
          warn("CASCADE is PostgreSQL-only; ignored for DROP TABLE");
        statements.push(
          `DROP TABLE ${op.ifExists ? "IF EXISTS " : ""}${qt(d, op.table)}${d === "postgres" && op.cascade ? " CASCADE" : ""}`,
        );
        break;
      case "truncateTable":
        if (d === "postgres")
          statements.push(
            `TRUNCATE TABLE ${qt(d, op.table)}${op.restartIdentity ? " RESTART IDENTITY" : ""}${op.cascade ? " CASCADE" : ""}`,
          );
        else if (d === "mysql") {
          if (!op.restartIdentity) warn("MySQL TRUNCATE always resets AUTO_INCREMENT");
          statements.push(`TRUNCATE TABLE ${qt(d, op.table)}`);
        } else {
          statements.push(`DELETE FROM ${qt(d, op.table)}`);
          if (op.restartIdentity)
            statements.push(
              `DELETE FROM ${op.table.schema ? `${q(d, op.table.schema)}.` : ""}"sqlite_sequence" WHERE name = ${lit(d, op.table.name)}`,
            );
        }
        break;
      case "renameTable":
        requireName(op.newName, "new table name");
        statements.push(
          d === "mysql"
            ? `RENAME TABLE ${qt(d, op.table)} TO ${qt(d, { schema: op.table.schema, name: op.newName })}`
            : `ALTER TABLE ${qt(d, op.table)} RENAME TO ${q(d, op.newName)}`,
        );
        break;
      case "addColumn": {
        const w = needWork(op.table);
        validateDef(op.column);
        if (w.meta.columns.some((c) => c.name.toLowerCase() === op.column.name.toLowerCase()))
          invalid(`column "${op.column.name}" already exists in "${op.table.name}"`);
        if (op.after !== null && d !== "mysql") warn(POSITION_MYSQL_ONLY);
        if (op.after) findCol(w, op.after);
        if (d === "sqlite" && op.column.comment) warn(SQLITE_NO_COMMENTS);
        if (d !== "mysql" && op.column.unsigned) warn("UNSIGNED is MySQL-only; ignored");
        let sql = `ALTER TABLE ${qt(d, op.table)} ADD COLUMN ${columnSql(d, op.column)}`;
        if (d === "mysql" && op.after !== null)
          sql += op.after === "" ? " FIRST" : ` AFTER ${q(d, op.after)}`;
        push(sql);
        if (d === "postgres" && op.column.comment)
          push(
            `COMMENT ON COLUMN ${qt(d, op.table)}.${q(d, op.column.name)} IS ${lit(d, op.column.comment)}`,
          );
        const col = columnFromDef(op.column, w.meta.columns.length + 1, false);
        const at =
          d === "mysql" && op.after !== null
            ? op.after === ""
              ? 0
              : w.meta.columns.findIndex((c) => c.name === op.after) + 1
            : w.meta.columns.length;
        w.meta.columns.splice(at, 0, col);
        w.sources.splice(at, 0, null);
        break;
      }
      case "dropColumn": {
        const w = needWork(op.table);
        const c = findCol(w, op.name);
        if (w.meta.columns.length === 1) invalid(`"${op.table.name}" would have no columns left`);
        push(`ALTER TABLE ${qt(d, op.table)} DROP COLUMN ${q(d, op.name)}`);
        const i = w.meta.columns.indexOf(c);
        w.meta.columns.splice(i, 1);
        w.sources.splice(i, 1);
        w.meta.indexes = w.meta.indexes.filter((x) => !x.columns.includes(op.name));
        break;
      }
      case "renameColumn": {
        const w = needWork(op.table);
        requireName(op.newName, "new column name");
        const c = findCol(w, op.name);
        if (
          w.meta.columns.some((x) => x !== c && x.name.toLowerCase() === op.newName.toLowerCase())
        )
          invalid(`column "${op.newName}" already exists in "${op.table.name}"`);
        push(
          `ALTER TABLE ${qt(d, op.table)} RENAME COLUMN ${q(d, op.name)} TO ${q(d, op.newName)}`,
        );
        c.name = op.newName;
        w.meta.indexes = w.meta.indexes.map((x) => ({
          ...x,
          columns: x.columns.map((n) => (n === op.name ? op.newName : n)),
        }));
        break;
      }
      case "alterColumnType": {
        const w = needWork(op.table);
        requireName(op.newType, "column type");
        const c = findCol(w, op.name);
        c.dataType = op.newType;
        if (d === "postgres")
          push(
            `ALTER TABLE ${qt(d, op.table)} ALTER COLUMN ${q(d, op.name)} TYPE ${op.newType}${op.using ? ` USING ${op.using}` : ""}`,
          );
        else {
          if (op.using) warn("USING is PostgreSQL-only; ignored");
          if (d === "mysql") push(mysqlModify(d, op.table, c));
        }
        break;
      }
      case "alterColumnNullable": {
        const w = needWork(op.table);
        const c = findCol(w, op.name);
        c.nullable = op.nullable;
        if (d === "postgres")
          push(
            `ALTER TABLE ${qt(d, op.table)} ALTER COLUMN ${q(d, op.name)} ${op.nullable ? "DROP NOT NULL" : "SET NOT NULL"}`,
          );
        else if (d === "mysql") push(mysqlModify(d, op.table, c));
        break;
      }
      case "alterColumnDefault": {
        const w = needWork(op.table);
        const c = findCol(w, op.name);
        c.defaultExpr = op.defaultExpr;
        if (d !== "sqlite")
          push(
            `ALTER TABLE ${qt(d, op.table)} ALTER COLUMN ${q(d, op.name)} ${op.defaultExpr === null ? "DROP DEFAULT" : `SET DEFAULT ${op.defaultExpr}`}`,
          );
        break;
      }
      case "setColumnComment": {
        const w = needWork(op.table);
        const c = findCol(w, op.name);
        if (d === "sqlite") {
          warn(SQLITE_NO_COMMENTS);
          break;
        }
        c.comment = op.comment;
        push(
          d === "postgres"
            ? `COMMENT ON COLUMN ${qt(d, op.table)}.${q(d, op.name)} IS ${op.comment === null ? "NULL" : lit(d, op.comment)}`
            : mysqlModify(d, op.table, c),
        );
        break;
      }
      case "setTableComment": {
        if (d === "sqlite") {
          warn(SQLITE_NO_COMMENTS);
          break;
        }
        const w = needWork(op.table);
        w.meta.comment = op.comment;
        push(
          d === "postgres"
            ? `COMMENT ON TABLE ${qt(d, op.table)} IS ${op.comment === null ? "NULL" : lit(d, op.comment)}`
            : `ALTER TABLE ${qt(d, op.table)} COMMENT = ${lit(d, op.comment ?? "")}`,
        );
        break;
      }
      case "addIndex": {
        const w = needWork(op.table);
        requireName(op.index.name, "index name");
        if (op.index.columns.length === 0) invalid("an index needs at least one column");
        if (w.meta.indexes.some((x) => x.name === op.index.name))
          invalid(`index "${op.index.name}" already exists on "${op.table.name}"`);
        const sql = indexSql(d, op.table, op.index, warn);
        push(sql);
        w.meta.indexes.push({
          name: op.index.name,
          columns: op.index.columns,
          unique: op.index.unique,
          primary: false,
          method: op.index.method,
          predicate: op.index.predicate,
          definition: sql,
        });
        break;
      }
      case "dropIndex": {
        const w = needWork(op.table);
        const idx =
          w.meta.indexes.find((x) => x.name === op.name) ??
          invalid(`index "${op.name}" does not exist on "${op.table.name}"`);
        if (idx.primary)
          push(
            d === "mysql"
              ? `ALTER TABLE ${qt(d, op.table)} DROP PRIMARY KEY`
              : `ALTER TABLE ${qt(d, op.table)} DROP CONSTRAINT ${q(d, op.name)}`,
          );
        else
          push(
            d === "mysql"
              ? `DROP INDEX ${q(d, op.name)} ON ${qt(d, op.table)}`
              : `DROP INDEX ${op.table.schema ? `${q(d, op.table.schema)}.` : ""}${q(d, op.name)}`,
          );
        w.meta.indexes = w.meta.indexes.filter((x) => x !== idx);
        break;
      }
      case "addForeignKey": {
        const w = needWork(op.table);
        if (op.fk.columns.length === 0) invalid("a foreign key needs at least one column");
        if (op.fk.columns.length !== op.fk.refColumns.length)
          invalid("a foreign key needs as many referenced columns as columns");
        for (const c of op.fk.columns) findCol(w, c);
        push(`ALTER TABLE ${qt(d, op.table)} ADD ${fkSql(d, op.fk)}`);
        w.meta.foreignKeys = [
          ...w.meta.foreignKeys,
          {
            name: op.fk.name ?? `${op.table.name}_${op.fk.columns.join("_")}_fkey`,
            columns: op.fk.columns,
            refTable: op.fk.refTable,
            refColumns: op.fk.refColumns,
            onUpdate: op.fk.onUpdate ?? "NO ACTION",
            onDelete: op.fk.onDelete ?? "NO ACTION",
            definition: null,
          },
        ];
        break;
      }
      case "dropForeignKey": {
        const w = needWork(op.table);
        if (!w.meta.foreignKeys.some((f) => f.name === op.name))
          invalid(`foreign key "${op.name}" does not exist on "${op.table.name}"`);
        push(
          d === "mysql"
            ? `ALTER TABLE ${qt(d, op.table)} DROP FOREIGN KEY ${q(d, op.name)}`
            : `ALTER TABLE ${qt(d, op.table)} DROP CONSTRAINT ${q(d, op.name)}`,
        );
        w.meta.foreignKeys = w.meta.foreignKeys.filter((f) => f.name !== op.name);
        break;
      }
      case "addPrimaryKey": {
        const w = needWork(op.table);
        for (const c of op.columns) findCol(w, c).isPk = true;
        push(`ALTER TABLE ${qt(d, op.table)} ADD PRIMARY KEY (${cols(d, op.columns)})`);
        w.meta.primaryKey = op.columns;
        break;
      }
      case "dropPrimaryKey": {
        const w = needWork(op.table);
        for (const c of w.meta.columns) c.isPk = false;
        push(
          d === "mysql"
            ? `ALTER TABLE ${qt(d, op.table)} DROP PRIMARY KEY`
            : `ALTER TABLE ${qt(d, op.table)} DROP CONSTRAINT ${q(d, `${op.table.name}_pkey`)}`,
        );
        w.meta.primaryKey = [];
        break;
      }
    }
    ranges.push([start, statements.length]);
  }

  if (rebuild && work && table) {
    const start = statements.length;
    statements.push(...rebuildStatements(d, work));
    warn(
      `"${table.name}" is rebuilt from introspected metadata (new table, copy, drop, rename): CHECK constraints, ON CONFLICT clauses, DEFERRABLE foreign-key options and any COLLATE or generated-column expression that cannot be recovered from the original definition may be lost; review the statements before applying`,
    );
    ops.forEach((op, i) => {
      if (onPlanned(op) && !TABLE_LEVEL.has(op.op)) ranges[i] = [start, statements.length];
    });
    transactional = false;
  }
  return { statements, warnings, transactional, ranges, rebuild };
}

function rebuildStatements(d: Driver, w: Work): string[] {
  const t = w.meta.table;
  const tmpName = `${t.name}__tabler_rebuild`;
  const tmp = qt(d, { schema: t.schema, name: tmpName });
  const pk = w.meta.columns.filter((c) => c.isPk).map((c) => c.name);
  const inline = pk.length === 1 && w.meta.columns.some((c) => c.isPk && c.isAutoIncrement);
  const lines = w.meta.columns.map(
    (c) =>
      `  ${columnSql(
        d,
        {
          name: c.name,
          dataType: c.dataType,
          nullable: c.nullable,
          defaultExpr: c.defaultExpr,
          autoIncrement: c.isAutoIncrement && inline,
          comment: null,
        },
        inline && c.isPk,
      )}`,
  );
  if (pk.length > 0 && !inline) lines.push(`  PRIMARY KEY (${cols(d, pk)})`);
  for (const f of w.meta.foreignKeys) lines.push(`  ${fkSql(d, { ...f, name: null })}`);
  const copied = w.meta.columns
    .map((c, i) => [c.name, w.sources[i]] as const)
    .filter(([, src]) => src !== null && src !== undefined);
  const out = [
    "PRAGMA foreign_keys = OFF",
    "BEGIN",
    `CREATE TABLE ${tmp} (\n${lines.join(",\n")}\n)`,
  ];
  if (copied.length > 0)
    out.push(
      `INSERT INTO ${tmp} (${cols(
        d,
        copied.map(([n]) => n),
      )}) SELECT ${cols(
        d,
        copied.map(([, s]) => s!),
      )} FROM ${qt(d, t)}`,
    );
  out.push(
    "PRAGMA legacy_alter_table = ON",
    `DROP TABLE ${qt(d, t)}`,
    `ALTER TABLE ${tmp} RENAME TO ${q(d, t.name)}`,
    "PRAGMA legacy_alter_table = OFF",
  );
  for (const idx of w.meta.indexes)
    if (!idx.primary)
      out.push(
        indexSql(
          d,
          t,
          {
            name: idx.name,
            columns: idx.columns,
            unique: idx.unique,
            method: null,
            predicate: idx.predicate,
          },
          () => {},
        ),
      );
  out.push(`PRAGMA foreign_key_check(${q(d, t.name)})`, "COMMIT", "PRAGMA foreign_keys = ON");
  return out;
}

registerMock("preview_ddl", async ({ connectionId, table, ops }) => {
  const c = await connectionOf(connectionId);
  const { statements, warnings, transactional } = plan(
    c.driver,
    (table as TableRef | null) ?? null,
    ops as DdlOp[],
  );
  return { statements, warnings, transactional } satisfies DdlPlan;
});

// --- apply -------------------------------------------------------------------------------------

function dbError(index: number, position: ErrorPayload["position"] = null): ErrorPayload {
  return {
    code: "DB_ERROR",
    message: "",
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position,
    statementIndex: index,
    data: null,
  };
}

function typeError(d: Driver, statement: string, index: number, table: string): ErrorPayload {
  const m = /\bnosuchtype\w*/i.exec(statement)!;
  const word = m[0];
  const offset = m.index;
  const base = dbError(index, { offset, line: 1, column: offset + 1 });
  if (d === "postgres")
    return { ...base, message: `type "${word}" does not exist`, sqlstate: "42704" };
  if (d === "mysql")
    return {
      ...base,
      message: `You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '${statement.slice(offset, offset + 40)}' at line 1`,
      sqlstate: "42000",
      vendorCode: 1064,
    };
  return { ...base, message: `unknown datatype for ${table}: "${word}"`, vendorCode: 1 };
}

/** "already exists" as each database words it. */
function existsError(d: Driver, name: string, index: number): ErrorPayload {
  const base = dbError(index);
  if (d === "postgres")
    return { ...base, message: `relation "${name}" already exists`, sqlstate: "42P07" };
  if (d === "mysql")
    return {
      ...base,
      message: `Table '${name}' already exists`,
      sqlstate: "42S01",
      vendorCode: 1050,
    };
  return { ...base, message: `table "${name}" already exists`, vendorCode: 1 };
}

/** Name a `createTable` / `renameTable` op would give a table that its schema already has. */
function takenName(d: Driver, op: DdlOp): string | null {
  if (op.op === "createTable" && !op.ifNotExists) {
    const name = op.table.name;
    return listOf(d, schemaOf(op.table)).some((x) => x.name === name) ? name : null;
  }
  if (op.op === "renameTable") {
    const name = op.newName;
    return listOf(d, schemaOf(op.table)).some((x) => x.name === name) ? name : null;
  }
  return null;
}

function defaultValue(expr: string | null): Value {
  if (expr === null) return { t: "null" };
  const e = expr.trim();
  if (/^-?\d+$/.test(e)) return { t: "int", v: Number(e) };
  if (/^(true|false)$/i.test(e)) return { t: "bool", v: e.toLowerCase() === "true" };
  const s = /^'(.*)'(::\w+)?$/.exec(e);
  return s ? { t: "text", v: s[1]!.replace(/''/g, "'") } : { t: "null" };
}

function specOf(c: ColumnMeta): ColSpec {
  return [
    c.name,
    c.dataType,
    c.kind,
    {
      nullable: c.nullable,
      defaultExpr: c.defaultExpr,
      isPk: c.isPk,
      pkOrdinal: c.pkOrdinal,
      isAutoIncrement: c.isAutoIncrement,
      comment: c.comment,
      enumValues: c.enumValues,
      arrayElemType: c.arrayElemType,
      extra: c.extra,
    },
  ];
}

const IDENT = String.raw`(?:"((?:[^"]|"")+)"|\`((?:[^\`]|\`\`)+)\`)`;
const DROP_VIEW = new RegExp(
  String.raw`^\s*drop\s+(?:materialized\s+)?view\s+(?:if\s+exists\s+)?(?:${IDENT}\.)?${IDENT}`,
  "i",
);

/** `{schema, name}` of a raw `DROP [MATERIALIZED] VIEW` statement, else null. */
function droppedView(sql: string): TableRef | null {
  const m = DROP_VIEW.exec(sql);
  if (!m) return null;
  const unquote = (dq?: string, bt?: string) =>
    dq !== undefined ? dq.replace(/""/g, '"') : bt !== undefined ? bt.replace(/``/g, "`") : null;
  const name = unquote(m[3], m[4]);
  return name === null ? null : { schema: unquote(m[1], m[2]), name };
}

/**
 * The overlay of `t` with the fixture foreign keys copied into it (and hidden in their fixture
 * form), so they can be rewritten when columns or tables are renamed or dropped.
 */
function ownFks(d: Driver, t: TableRef): Overlay {
  const o = overlayOf(d, t);
  for (const f of tableMetaFor(d, schemaOf(t), t.name)?.foreignKeys ?? []) {
    if (o.droppedFks.includes(f.name)) continue;
    o.droppedFks.push(f.name);
    o.fks.push({ ...f, columns: [...f.columns], refColumns: [...f.refColumns] });
  }
  return o;
}

/** True when foreign key `f` (of a table in `schema`) references table `t`. */
function refersTo(f: ForeignKeyMeta, schema: string, t: TableRef): boolean {
  return f.refTable.name === t.name && (f.refTable.schema ?? schema) === schemaOf(t);
}

/** Rewrites the foreign keys of every table of `t`'s schema that reference `t`. */
function rewriteReferences(d: Driver, t: TableRef, edit: (f: ForeignKeyMeta) => void) {
  const schema = schemaOf(t);
  for (const other of listOf(d, schema)) {
    const ref: TableRef = { schema: t.schema, name: other.name };
    if (!metaOf(d, ref)?.foreignKeys.some((f) => refersTo(f, schema, t))) continue;
    for (const f of ownFks(d, ref).fks) if (refersTo(f, schema, t)) edit(f);
  }
}

/** Applies successful ops to the mock catalog (and keeps cached mock rows in shape). */
function applyToCatalog(d: Driver, table: TableRef | null, ops: DdlOp[]) {
  for (const op of ops) {
    if (op.op === "rawSql") {
      const view = droppedView(op.sql);
      if (view) {
        const list = editableList(d, view.schema ?? (table ? schemaOf(table) : "public"));
        const i = list.findIndex((x) => x.name === view.name);
        if (i >= 0) list.splice(i, 1);
      }
      continue;
    }
    const t = op.table;
    const schema = schemaOf(t);
    const rowsKey = tableId(d, t);
    const specs = TABLE_COLUMNS[t.name];
    const metaNow = () => tableMetaFor(d, schema, t.name);
    const cachedRows = () => {
      const m = metaNow();
      return m ? rowsFor(rowsKey, m.columns, m.rowEstimate ?? 25) : [];
    };
    switch (op.op) {
      case "createTable": {
        if (op.ifNotExists && listOf(d, schema).some((x) => x.name === t.name)) break;
        TABLE_COLUMNS[t.name] = op.columns.map((def, i) => {
          const pkAt = op.primaryKey.indexOf(def.name);
          const c = columnFromDef(def, i + 1, pkAt >= 0);
          c.pkOrdinal = pkAt >= 0 ? pkAt + 1 : null;
          return specOf(c);
        });
        const list = editableList(d, schema);
        if (!list.some((x) => x.name === t.name))
          list.push({
            name: t.name,
            schema,
            kind: "table",
            rowEstimate: 0,
            sizeBytes: 0,
            comment: null,
          });
        overlays.delete(rowsKey);
        rowsFor(rowsKey, metaNow()!.columns, 0);
        break;
      }
      case "dropTable": {
        const list = editableList(d, schema);
        const i = list.findIndex((x) => x.name === t.name);
        if (i >= 0) list.splice(i, 1);
        overlays.delete(rowsKey);
        break;
      }
      case "truncateTable":
        cachedRows().splice(0);
        break;
      case "renameTable": {
        if (!specs) break;
        const rows = cachedRows().slice();
        // Fixture keys are tied to the table's name: carry them over in the overlay.
        ownFks(d, t);
        const renamed = { schema: t.schema, name: op.newName };
        rewriteReferences(d, t, (f) => {
          f.refTable = { ...f.refTable, name: op.newName };
        });
        TABLE_COLUMNS[op.newName] = specs;
        const list = editableList(d, schema);
        const entry = list.find((x) => x.name === t.name);
        if (entry) entry.name = op.newName;
        const o = overlays.get(rowsKey);
        if (o) {
          overlays.delete(rowsKey);
          overlays.set(tableId(d, renamed), o);
        }
        const target = rowsFor(tableId(d, renamed), metaNow()?.columns ?? [], 0);
        target.splice(0, target.length, ...rows);
        break;
      }
      case "addColumn": {
        if (!specs) break;
        const rows = cachedRows();
        const at =
          d === "mysql" && op.after !== null
            ? op.after === ""
              ? 0
              : specs.findIndex((s) => s[0] === op.after) + 1
            : specs.length;
        specs.splice(at, 0, specOf(columnFromDef(op.column, at + 1, false)));
        const v = defaultValue(op.column.defaultExpr);
        for (const r of rows) r.splice(at, 0, v);
        break;
      }
      case "dropColumn": {
        if (!specs) break;
        const rows = cachedRows();
        const i = specs.findIndex((s) => s[0] === op.name);
        if (i < 0) break;
        specs.splice(i, 1);
        for (const r of rows) r.splice(i, 1);
        // Indexes and foreign keys on the column go with it.
        const o = ownFks(d, t);
        o.fks = o.fks.filter((f) => !f.columns.includes(op.name));
        o.indexes = o.indexes.filter((x) => !x.columns.some((e) => indexColumn(e) === op.name));
        break;
      }
      case "renameColumn":
      case "alterColumnType":
      case "alterColumnNullable":
      case "alterColumnDefault":
      case "setColumnComment": {
        if (!specs) break;
        const i = specs.findIndex((s) => s[0] === op.name);
        const s = specs[i];
        if (!s) break;
        const [name, type, kind, opts = {}] = s;
        if (op.op === "renameColumn") {
          specs[i] = [op.newName, type, kind, opts];
          const rename = (n: string) => (n === name ? op.newName : n);
          const o = ownFks(d, t);
          for (const x of o.indexes)
            x.columns = x.columns.map((e) =>
              indexColumn(e) === name ? op.newName + e.trim().slice(name.length) : e,
            );
          for (const f of o.fks) f.columns = f.columns.map(rename);
          rewriteReferences(d, t, (f) => {
            f.refColumns = f.refColumns.map(rename);
          });
        } else if (op.op === "alterColumnType")
          specs[i] = [name, op.newType, kindOf(op.newType), opts];
        else if (op.op === "alterColumnNullable")
          specs[i] = [name, type, kind, { ...opts, nullable: op.nullable }];
        else if (op.op === "alterColumnDefault")
          specs[i] = [name, type, kind, { ...opts, defaultExpr: op.defaultExpr }];
        else if (d !== "sqlite") specs[i] = [name, type, kind, { ...opts, comment: op.comment }];
        break;
      }
      case "setTableComment":
        if (d !== "sqlite") overlayOf(d, t).comment = op.comment;
        break;
      case "addIndex":
        overlayOf(d, t).indexes.push({
          name: op.index.name,
          columns: op.index.columns,
          unique: op.index.unique,
          primary: false,
          method: op.index.method ?? (d === "postgres" ? "btree" : null),
          predicate: op.index.predicate,
          definition: indexSql(d, t, op.index, () => {}),
        });
        break;
      case "dropIndex": {
        const o = overlayOf(d, t);
        const i = o.indexes.findIndex((x) => x.name === op.name);
        if (i >= 0) o.indexes.splice(i, 1);
        else o.droppedIndexes.push(op.name);
        break;
      }
      case "addForeignKey":
        overlayOf(d, t).fks.push({
          name: op.fk.name ?? `${t.name}_${op.fk.columns.join("_")}_fkey`,
          columns: op.fk.columns,
          refTable: op.fk.refTable,
          refColumns: op.fk.refColumns,
          onUpdate: op.fk.onUpdate ?? "NO ACTION",
          onDelete: op.fk.onDelete ?? "NO ACTION",
          definition: null,
        });
        break;
      case "dropForeignKey": {
        const o = overlayOf(d, t);
        const i = o.fks.findIndex((x) => x.name === op.name);
        if (i >= 0) o.fks.splice(i, 1);
        else o.droppedFks.push(op.name);
        break;
      }
      case "addPrimaryKey":
      case "dropPrimaryKey": {
        if (!specs) break;
        specs.forEach((s, i) => {
          const at = op.op === "addPrimaryKey" ? op.columns.indexOf(s[0]) : -1;
          specs[i] = [
            s[0],
            s[1],
            s[2],
            { ...(s[3] ?? {}), isPk: at >= 0, pkOrdinal: at >= 0 ? at + 1 : null },
          ];
        });
        break;
      }
    }
  }
}

registerMock("apply_ddl", async ({ connectionId, table, ops }) => {
  const c = await connectionOf(connectionId);
  if (c.readOnly) mockError("UNSUPPORTED", "this connection is read-only");
  const started = performance.now();
  const t = (table as TableRef | null) ?? null;
  const list = ops as DdlOp[];
  const p = plan(c.driver, t, list);
  const elapsed = () => Math.max(2, Math.round(performance.now() - started)) + 9;
  // The first statement that fails: an unknown type, or a table name that is already taken.
  const failures: Array<{ at: number; error: ErrorPayload }> = [];
  const typo = p.statements.findIndex((s) => /\bnosuchtype\w*/i.test(s));
  if (typo >= 0)
    failures.push({
      at: typo,
      error: typeError(c.driver, p.statements[typo]!, typo, t?.name ?? "table"),
    });
  list.forEach((op, i) => {
    const name = takenName(c.driver, op);
    const at = p.ranges[i]?.[0];
    if (name !== null && at !== undefined)
      failures.push({ at, error: existsError(c.driver, name, at) });
  });
  const failure = failures.sort((a, b) => a.at - b.at)[0];
  if (failure) {
    const { at: bad, error } = failure;
    let executed: string[] = [];
    if (p.rebuild) {
      // The BEGIN block rolled back; the foreign-key pragmas around it ran.
      executed = [p.statements[0]!, p.statements[p.statements.length - 1]!];
    } else if (!p.transactional) {
      executed = p.statements.slice(0, bad);
      applyToCatalog(
        c.driver,
        t,
        list.filter((_, i) => (p.ranges[i]?.[1] ?? Infinity) <= bad),
      );
    }
    return { executed, failedIndex: bad, error, durationMs: elapsed() } satisfies DdlResult;
  }
  applyToCatalog(c.driver, t, list);
  return {
    executed: p.statements,
    failedIndex: null,
    error: null,
    durationMs: elapsed(),
  } satisfies DdlResult;
});

// --- get_table_ddl -----------------------------------------------------------------------------

function createTableText(d: Driver, meta: TableMeta): string {
  const t = meta.table;
  const pk = meta.primaryKey;
  const inline =
    d === "sqlite" && pk.length === 1 && meta.columns.some((c) => c.isPk && c.isAutoIncrement);
  const lines = meta.columns.map(
    (c) =>
      `  ${columnSql(
        d,
        {
          name: c.name,
          dataType: c.dataType,
          nullable: c.nullable,
          defaultExpr: c.defaultExpr,
          autoIncrement: c.isAutoIncrement,
          comment: c.comment,
          unsigned: c.isUnsigned,
          identity: c.extra === "ALWAYS" ? "ALWAYS" : "BY DEFAULT",
        },
        inline && c.isPk,
      )}`,
  );
  const pkIndex = meta.indexes.find((i) => i.primary);
  if (pk.length > 0 && !inline)
    lines.push(
      `  ${d === "postgres" && pkIndex ? `CONSTRAINT ${q(d, pkIndex.name)} ` : ""}PRIMARY KEY (${cols(d, pk)})`,
    );
  for (const f of meta.foreignKeys) lines.push(`  ${fkSql(d, f)}`);
  let sql = `CREATE TABLE ${qt(d, t)} (\n${lines.join(",\n")}\n)`;
  if (d === "mysql") {
    sql += " ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci";
    if (meta.comment) sql += ` COMMENT=${lit(d, meta.comment)}`;
  }
  const out = [sql];
  for (const idx of meta.indexes)
    if (!idx.primary)
      out.push(
        idx.definition ??
          indexSql(
            d,
            t,
            {
              name: idx.name,
              columns: idx.columns,
              unique: idx.unique,
              method: idx.method,
              predicate: idx.predicate,
            },
            () => {},
          ),
      );
  if (d === "postgres") {
    if (meta.comment) out.push(`COMMENT ON TABLE ${qt(d, t)} IS ${lit(d, meta.comment)}`);
    for (const c of meta.columns)
      if (c.comment)
        out.push(`COMMENT ON COLUMN ${qt(d, t)}.${q(d, c.name)} IS ${lit(d, c.comment)}`);
  }
  for (const tr of meta.triggers) out.push(tr.definition);
  return out.map((s) => `${s};`).join("\n\n");
}

registerMock("get_table_ddl", async ({ connectionId, table }) => {
  const c = await connectionOf(connectionId);
  const t = table as TableRef;
  const meta = metaOf(c.driver, t) ?? mockError("NOT_FOUND", `table ${t.name}`);
  if (meta.kind === "view" || meta.kind === "materializedView") {
    const what =
      meta.kind === "materializedView" && c.driver === "postgres" ? "MATERIALIZED VIEW" : "VIEW";
    return `CREATE ${what} ${qt(c.driver, meta.table)} AS\nSELECT ${cols(
      c.driver,
      meta.columns.map((x) => x.name),
    )}\nFROM ${qt(c.driver, { schema: meta.table.schema, name: "users" })}\nWHERE is_active;`;
  }
  return createTableText(c.driver, meta);
});
