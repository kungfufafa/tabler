import type { ColumnDef, DdlOp, Dialect, TableRef } from "@/lib/ipc";

/** Column attributes the structure grid can edit. */
export type ColumnField =
  "name" | "dataType" | "nullable" | "defaultExpr" | "comment" | "autoIncrement";

export type ColumnValue = string | boolean | null;

/** SQLite stores neither table nor column comments. */
export function supportsComments(dialect: Dialect): boolean {
  return dialect !== "sqlite";
}

/** Fields shown (and editable) in the columns grid for a dialect, in display order. */
export function columnFields(dialect: Dialect): ColumnField[] {
  const fields: ColumnField[] = ["name", "dataType", "nullable", "defaultExpr", "autoIncrement"];
  if (supportsComments(dialect)) fields.push("comment");
  return fields;
}

/** Column position (`AFTER x` / `FIRST`) is MySQL-only. */
export function supportsColumnPosition(dialect: Dialect): boolean {
  return dialect === "mysql";
}

/**
 * Auto increment can be set when adding a column on PostgreSQL only (an identity column). MySQL
 * needs the AUTO_INCREMENT column to be a key in the same statement, which a lone `ADD COLUMN`
 * cannot express (error 1075), and SQLite would need a table rebuild.
 */
export function supportsAddAutoIncrement(dialect: Dialect): boolean {
  return dialect === "postgres";
}

const TYPES: Record<Dialect, string[]> = {
  postgres: [
    "integer",
    "bigint",
    "smallint",
    "serial",
    "bigserial",
    "numeric(10,2)",
    "real",
    "double precision",
    "boolean",
    "text",
    "varchar(255)",
    "char(1)",
    "uuid",
    "date",
    "time",
    "timestamp",
    "timestamptz",
    "interval",
    "json",
    "jsonb",
    "bytea",
    "inet",
    "text[]",
    "integer[]",
  ],
  mysql: [
    "int",
    "bigint",
    "bigint unsigned",
    "tinyint(1)",
    "smallint",
    "decimal(10,2)",
    "float",
    "double",
    "varchar(255)",
    "char(36)",
    "text",
    "mediumtext",
    "longtext",
    "date",
    "datetime",
    "timestamp",
    "time",
    "year",
    "json",
    "blob",
    "binary(16)",
    "enum('a','b')",
  ],
  sqlite: [
    "INTEGER",
    "TEXT",
    "REAL",
    "NUMERIC",
    "BLOB",
    "BOOLEAN",
    "DATE",
    "DATETIME",
    "VARCHAR(255)",
    "JSON",
  ],
};

/** PostgreSQL pseudo-types that only exist in `CREATE TABLE` / `ADD COLUMN`. */
const PG_SERIAL = /^(small|big)?serial$/i;
const PG_ALTER_TYPES = TYPES.postgres.filter((t) => !PG_SERIAL.test(t));

/**
 * Type names offered as suggestions in the type cell (free text is always allowed). For an
 * existing column (`ALTER COLUMN … TYPE`) PostgreSQL's serial pseudo-types are left out.
 */
export function typeSuggestions(dialect: Dialect, existingColumn = false): readonly string[] {
  return existingColumn && dialect === "postgres" ? PG_ALTER_TYPES : TYPES[dialect];
}

/** Type given to a freshly added column. */
export function defaultColumnType(dialect: Dialect): string {
  return dialect === "mysql" ? "varchar(255)" : dialect === "sqlite" ? "TEXT" : "text";
}

/** Type of the default `id` column of a new table. */
export function idColumnType(dialect: Dialect): string {
  return dialect === "sqlite" ? "INTEGER" : "bigint";
}

const INTEGER_TYPE =
  /^(tiny|small|medium|big)?int(eger)?\s*(\(\s*\d+\s*\))?(\s+unsigned)?$|^int[248]$|^(small|big)?serial$/i;

/** True for integer column types (auto increment / identity needs one). */
export function isIntegerType(type: string): boolean {
  return INTEGER_TYPE.test(type.trim());
}

export type ParseResult = { ok: true; value: ColumnValue } | { ok: false; error: string };

/** The column a grid edit applies to, as far as `columnEditError` needs it. */
export interface EditTarget {
  /** True for a staged new column (its edits fold into the `addColumn` definition). */
  added: boolean;
  name: string;
  dataType: string;
  autoIncrement: boolean;
}

/**
 * Error for setting `field` of `target` to the parsed `value`, else null. Catches edits that
 * would stage a statement bound to fail: auto increment on a non-integer new column, and
 * PostgreSQL's serial pseudo-types on an existing column (`ALTER COLUMN … TYPE serial` fails).
 */
export function columnEditError(
  dialect: Dialect,
  target: EditTarget,
  field: ColumnField,
  value: ColumnValue,
): string | null {
  if (!target.added) {
    const type = field === "dataType" ? String(value).trim() : "";
    if (dialect === "postgres" && PG_SERIAL.test(type)) {
      const base = /^big/i.test(type) ? "bigint" : /^small/i.test(type) ? "smallint" : "integer";
      return `${type} only exists for new columns; use ${base} with a sequence default instead`;
    }
    return null;
  }
  if (field !== "autoIncrement" && field !== "dataType") return null;
  const autoIncrement = field === "autoIncrement" ? value === true : target.autoIncrement;
  const type = field === "dataType" ? String(value) : target.dataType;
  if (autoIncrement && !isIntegerType(type))
    return `Auto increment needs an integer type such as bigint (column “${target.name}”)`;
  return null;
}

/**
 * Turns what the user typed into the typed value of a field: names and types are trimmed and
 * required, an empty default means "no default" (null), an empty comment means "no comment".
 */
export function parseColumnInput(
  dialect: Dialect,
  field: ColumnField,
  raw: string | boolean,
): ParseResult {
  if (field === "nullable" || field === "autoIncrement") return { ok: true, value: raw === true };
  const text = String(raw);
  switch (field) {
    case "name": {
      const name = text.trim();
      return name ? { ok: true, value: name } : { ok: false, error: "Column name is required" };
    }
    case "dataType": {
      const type = text.trim().replace(/\s+/g, " ");
      return type ? { ok: true, value: type } : { ok: false, error: "Column type is required" };
    }
    case "defaultExpr": {
      const expr = text.trim();
      return { ok: true, value: expr === "" ? null : expr };
    }
    case "comment": {
      if (!supportsComments(dialect))
        return { ok: false, error: "SQLite does not store column comments" };
      return { ok: true, value: text.trim() === "" ? null : text };
    }
  }
}

/**
 * The single DDL op expressing "set `field` of column `name` to `value`" on an existing column,
 * or null when the dialect cannot change that attribute in place (comments on SQLite, auto
 * increment anywhere).
 */
export function columnOp(
  dialect: Dialect,
  table: TableRef,
  name: string,
  field: ColumnField,
  value: ColumnValue,
): DdlOp | null {
  switch (field) {
    case "name":
      return { op: "renameColumn", table, name, newName: String(value) };
    case "dataType":
      return { op: "alterColumnType", table, name, newType: String(value), using: null };
    case "nullable":
      return { op: "alterColumnNullable", table, name, nullable: value === true };
    case "defaultExpr":
      return {
        op: "alterColumnDefault",
        table,
        name,
        defaultExpr: typeof value === "string" && value.trim() !== "" ? value : null,
      };
    case "comment":
      if (!supportsComments(dialect)) return null;
      return {
        op: "setColumnComment",
        table,
        name,
        comment: typeof value === "string" && value !== "" ? value : null,
      };
    case "autoIncrement":
      return null;
  }
}

/** A blank column definition for the "add column" flow. */
export function newColumnDef(dialect: Dialect, name: string): ColumnDef {
  return {
    name,
    dataType: defaultColumnType(dialect),
    nullable: true,
    defaultExpr: null,
    autoIncrement: false,
    comment: null,
    collation: null,
    unsigned: false,
  };
}

/** Applies one field edit to a column definition (used for staged new columns). */
export function withField(
  dialect: Dialect,
  def: ColumnDef,
  field: ColumnField,
  value: ColumnValue,
): ColumnDef {
  switch (field) {
    case "name":
      return { ...def, name: String(value) };
    case "dataType":
      return { ...def, dataType: String(value) };
    case "nullable":
      return { ...def, nullable: value === true };
    case "defaultExpr":
      return { ...def, defaultExpr: typeof value === "string" && value !== "" ? value : null };
    case "comment":
      return supportsComments(dialect)
        ? { ...def, comment: typeof value === "string" && value !== "" ? value : null }
        : def;
    case "autoIncrement":
      return supportsAddAutoIncrement(dialect) ? { ...def, autoIncrement: value === true } : def;
  }
}
