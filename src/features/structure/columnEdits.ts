import type { ColumnDef, DdlOp, Dialect, TableRef } from "@/lib/ipc";

/** Column attributes the structure grid can edit. */
export type ColumnField =
  | "name"
  | "dataType"
  | "nullable"
  | "defaultExpr"
  | "comment"
  | "autoIncrement"
  /** MySQL `ON UPDATE …` (the "extra" cell). */
  | "onUpdate"
  | "collation";

export type ColumnValue = string | boolean | null;

/** SQLite stores neither table nor column comments. */
export function supportsComments(dialect: Dialect): boolean {
  return dialect !== "sqlite";
}

/** A column of the structure grid (TablePlus names them after `information_schema`). */
export type GridColumn =
  "name" | "dataType" | "nullable" | "check" | "defaultExpr" | "extra" | "foreignKey" | "comment";

export const GRID_LABEL: Record<GridColumn, string> = {
  name: "column_name",
  dataType: "data_type",
  nullable: "is_nullable",
  check: "check",
  defaultExpr: "column_default",
  extra: "extra",
  foreignKey: "foreign_key",
  comment: "comment",
};

/**
 * The columns of the structure grid per dialect, in TablePlus's order:
 * PG `column_name, data_type, is_nullable, check, column_default, foreign_key, comment`;
 * MySQL `column_name, data_type, is_nullable, column_default, extra, foreign_key, comment`;
 * SQLite `column_name, data_type, is_nullable, column_default, foreign_key`.
 */
export function gridColumns(dialect: Dialect): GridColumn[] {
  if (dialect === "postgres")
    return ["name", "dataType", "nullable", "check", "defaultExpr", "foreignKey", "comment"];
  if (dialect === "mysql")
    return ["name", "dataType", "nullable", "defaultExpr", "extra", "foreignKey", "comment"];
  return ["name", "dataType", "nullable", "defaultExpr", "foreignKey"];
}

/** Fields shown (and editable) in the columns grid for a dialect, in display order. */
export function columnFields(dialect: Dialect): ColumnField[] {
  const fields: ColumnField[] = ["name", "dataType", "nullable", "defaultExpr"];
  if (dialect === "mysql") fields.push("autoIncrement", "onUpdate");
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

/** Per-column collations are editable on MySQL text columns (details popover). */
export function supportsColumnCollation(dialect: Dialect, dataType: string): boolean {
  return dialect === "mysql" && isTextualType(dataType);
}

const TEXTUAL = /^(national\s+)?(var)?char|^(tiny|medium|long)?text\b|^enum\b|^set\b|^n(var)?char/i;

/** MySQL character types (the ones with a character set and collation). */
export function isTextualType(type: string): boolean {
  return TEXTUAL.test(type.trim());
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
    "int unsigned",
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

/**
 * Type of the default `id` column of a new table (TablePlus): PostgreSQL `serial` (an `integer`
 * with a sequence default), MySQL `int` + AUTO_INCREMENT, SQLite `INTEGER PRIMARY KEY`.
 */
export function idColumnType(dialect: Dialect): string {
  return dialect === "sqlite" ? "INTEGER" : dialect === "mysql" ? "int" : "serial";
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
  /** Whether the column is (staged to be) part of the primary key or leads an index. */
  isKey?: boolean;
}

/**
 * Error for setting `field` of `target` to the parsed `value`, else null. Catches edits that
 * would stage a statement bound to fail: auto increment on a non-integer column, MySQL
 * AUTO_INCREMENT on a column that is no key (error 1075), and PostgreSQL's serial pseudo-types
 * on an existing column (`ALTER COLUMN … TYPE serial` fails).
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
    if (field === "autoIncrement" && value === true) {
      if (!isIntegerType(target.dataType))
        return `Auto increment needs an integer type such as bigint (column “${target.name}”)`;
      if (dialect === "mysql" && target.isKey === false)
        return `MySQL AUTO_INCREMENT needs a key: add “${target.name}” to the primary key or an index first`;
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
    case "defaultExpr":
    case "onUpdate":
    case "collation": {
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

const textOrNull = (value: ColumnValue) =>
  typeof value === "string" && value.trim() !== "" ? value : null;

/**
 * The single DDL op expressing "set `field` of column `name` to `value`" on an existing column,
 * or null when the dialect cannot change that attribute in place (comments on SQLite, ON UPDATE
 * outside MySQL).
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
      return { op: "alterColumnDefault", table, name, defaultExpr: textOrNull(value) };
    case "comment":
      if (!supportsComments(dialect)) return null;
      return {
        op: "setColumnComment",
        table,
        name,
        comment: typeof value === "string" && value !== "" ? value : null,
      };
    case "autoIncrement":
      return { op: "alterColumnAutoIncrement", table, name, autoIncrement: value === true };
    case "onUpdate":
      if (dialect !== "mysql") return null;
      return { op: "alterColumnOnUpdate", table, name, onUpdate: textOrNull(value) };
    case "collation":
      return { op: "alterColumnCollation", table, name, collation: textOrNull(value) };
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
    onUpdate: null,
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
    case "onUpdate":
      return dialect === "mysql" ? { ...def, onUpdate: textOrNull(value) } : def;
    case "collation":
      return { ...def, collation: textOrNull(value) };
  }
}

/** The `ON UPDATE …` expression of a MySQL `EXTRA` text (`on update CURRENT_TIMESTAMP(3)`). */
export function onUpdateOf(extra: string | null): string | null {
  const m = /\bon update ([a-z_]+(?:\(\d*\))?)/i.exec(extra ?? "");
  return m ? m[1]!.toUpperCase() : null;
}
