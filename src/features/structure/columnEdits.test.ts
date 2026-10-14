import { describe, expect, it } from "vitest";
import {
  columnEditError,
  columnFields,
  gridColumns,
  onUpdateOf,
  columnOp,
  fractionalSeconds,
  onUpdateFor,
  requiresIndexPrefix,
  supportsAddAutoIncrement,
  supportsIndexPrefix,
  defaultColumnType,
  isIntegerType,
  parseColumnInput,
  typeSuggestions,
  withField,
  newColumnDef,
} from "./columnEdits";

const T = { schema: "main", name: "t" };

describe("parseColumnInput", () => {
  it("trims names and types and requires them", () => {
    expect(parseColumnInput("postgres", "name", "  nick ")).toEqual({ ok: true, value: "nick" });
    expect(parseColumnInput("postgres", "name", "   ")).toEqual({
      ok: false,
      error: "Column name is required",
    });
    expect(parseColumnInput("mysql", "dataType", " varchar(  255 )  ")).toEqual({
      ok: true,
      value: "varchar( 255 )",
    });
    expect(parseColumnInput("mysql", "dataType", "")).toMatchObject({ ok: false });
  });

  it("maps an empty default or comment to null", () => {
    expect(parseColumnInput("postgres", "defaultExpr", "   ")).toEqual({ ok: true, value: null });
    expect(parseColumnInput("postgres", "defaultExpr", " now() ")).toEqual({
      ok: true,
      value: "now()",
    });
    expect(parseColumnInput("mysql", "comment", "")).toEqual({ ok: true, value: null });
  });

  it("refuses comments on SQLite", () => {
    expect(parseColumnInput("sqlite", "comment", "x")).toMatchObject({ ok: false });
  });
});

describe("columnOp", () => {
  it("maps each field to its DDL op", () => {
    expect(columnOp("postgres", T, "a", "name", "b")).toEqual({
      op: "renameColumn",
      table: T,
      name: "a",
      newName: "b",
    });
    expect(columnOp("mysql", T, "a", "dataType", "bigint")).toEqual({
      op: "alterColumnType",
      table: T,
      name: "a",
      newType: "bigint",
      using: null,
    });
    expect(columnOp("sqlite", T, "a", "nullable", false)).toEqual({
      op: "alterColumnNullable",
      table: T,
      name: "a",
      nullable: false,
    });
  });

  it("an empty default drops it (alterColumnDefault null)", () => {
    for (const d of ["postgres", "mysql", "sqlite"] as const) {
      expect(columnOp(d, T, "a", "defaultExpr", null)).toEqual({
        op: "alterColumnDefault",
        table: T,
        name: "a",
        defaultExpr: null,
      });
      expect(columnOp(d, T, "a", "defaultExpr", "")).toMatchObject({ defaultExpr: null });
    }
  });

  it("comments: setColumnComment on PG / MySQL, nothing on SQLite", () => {
    expect(columnOp("postgres", T, "a", "comment", "hi")).toMatchObject({
      op: "setColumnComment",
      comment: "hi",
    });
    expect(columnOp("mysql", T, "a", "comment", null)).toMatchObject({ comment: null });
    expect(columnOp("sqlite", T, "a", "comment", "hi")).toBeNull();
  });

  it("auto increment, ON UPDATE and collation of existing columns", () => {
    expect(columnOp("postgres", T, "a", "autoIncrement", true)).toEqual({
      op: "alterColumnAutoIncrement",
      table: T,
      name: "a",
      autoIncrement: true,
    });
    expect(columnOp("mysql", T, "a", "onUpdate", "CURRENT_TIMESTAMP")).toEqual({
      op: "alterColumnOnUpdate",
      table: T,
      name: "a",
      onUpdate: "CURRENT_TIMESTAMP",
    });
    expect(columnOp("mysql", T, "a", "onUpdate", "")).toMatchObject({ onUpdate: null });
    expect(columnOp("postgres", T, "a", "onUpdate", "now()")).toBeNull();
    expect(columnOp("mysql", T, "a", "collation", "utf8mb4_bin")).toEqual({
      op: "alterColumnCollation",
      table: T,
      name: "a",
      collation: "utf8mb4_bin",
    });
    expect(columnOp("mysql", T, "a", "collation", " ")).toMatchObject({ collation: null });
  });
});

describe("dialect helpers", () => {
  it("hides the comment column on SQLite", () => {
    expect(columnFields("sqlite")).not.toContain("comment");
    expect(columnFields("postgres")).toContain("comment");
    expect(columnFields("mysql")).toContain("comment");
  });

  it("suggests dialect types and a sensible default", () => {
    expect(typeSuggestions("postgres")).toContain("jsonb");
    expect(typeSuggestions("postgres")).toContain("timestamptz");
    expect(typeSuggestions("mysql")).toContain("varchar(255)");
    expect(typeSuggestions("sqlite")).toContain("INTEGER");
    expect(defaultColumnType("mysql")).toBe("varchar(255)");
    expect(defaultColumnType("sqlite")).toBe("TEXT");
  });

  it("recognises integer types", () => {
    for (const t of ["int", "INTEGER", "bigint", "smallint", "int(11) unsigned", "int8", "serial"])
      expect(isIntegerType(t)).toBe(true);
    for (const t of ["text", "numeric(10,2)", "interval", "point"])
      expect(isIntegerType(t)).toBe(false);
  });

  it("withField ignores what the dialect cannot store", () => {
    const def = newColumnDef("sqlite", "c");
    expect(withField("sqlite", def, "comment", "x")).toBe(def);
    expect(withField("sqlite", def, "autoIncrement", true)).toBe(def);
    expect(withField("postgres", def, "autoIncrement", true).autoIncrement).toBe(true);
    expect(withField("postgres", def, "defaultExpr", "").defaultExpr).toBeNull();
  });

  it("MySQL new columns cannot be auto increment (ADD COLUMN cannot also make them a key)", () => {
    expect(supportsAddAutoIncrement("postgres")).toBe(true);
    expect(supportsAddAutoIncrement("mysql")).toBe(false);
    expect(supportsAddAutoIncrement("sqlite")).toBe(false);
    const def = newColumnDef("mysql", "c");
    expect(withField("mysql", def, "autoIncrement", true)).toBe(def);
  });

  it("existing PostgreSQL columns are not offered the serial pseudo-types", () => {
    expect(typeSuggestions("postgres")).toContain("serial");
    expect(typeSuggestions("postgres", true)).not.toContain("serial");
    expect(typeSuggestions("postgres", true)).not.toContain("bigserial");
    expect(typeSuggestions("postgres", true)).toContain("bigint");
    expect(typeSuggestions("mysql", true)).toBe(typeSuggestions("mysql"));
  });
});

describe("columnEditError", () => {
  const added = { added: true, name: "c", dataType: "text", autoIncrement: false };
  const existing = { added: false, name: "c", dataType: "integer", autoIncrement: false };

  it("auto increment on a new column needs an integer type", () => {
    expect(columnEditError("postgres", added, "autoIncrement", true)).toMatch(/integer type/);
    expect(columnEditError("postgres", added, "autoIncrement", false)).toBeNull();
    const intCol = { ...added, dataType: "bigint" };
    expect(columnEditError("postgres", intCol, "autoIncrement", true)).toBeNull();
    const aiCol = { ...intCol, autoIncrement: true };
    expect(columnEditError("postgres", aiCol, "dataType", "text")).toMatch(/integer type/);
    expect(columnEditError("postgres", aiCol, "dataType", "integer")).toBeNull();
    expect(columnEditError("postgres", aiCol, "name", "x")).toBeNull();
  });

  it("PostgreSQL existing columns cannot become serial", () => {
    expect(columnEditError("postgres", existing, "dataType", "bigserial")).toMatch(/use bigint/);
    expect(columnEditError("postgres", existing, "dataType", "serial")).toMatch(/use integer/);
    expect(columnEditError("postgres", existing, "dataType", "bigint")).toBeNull();
    expect(columnEditError("postgres", added, "dataType", "serial")).toBeNull();
    expect(columnEditError("mysql", existing, "dataType", "serial")).toBeNull();
  });
});

describe("TablePlus grid columns", () => {
  it("lists the columns of each dialect in TablePlus order", () => {
    expect(gridColumns("postgres")).toEqual([
      "name",
      "dataType",
      "nullable",
      "check",
      "defaultExpr",
      "foreignKey",
      "comment",
    ]);
    expect(gridColumns("mysql")).toEqual([
      "name",
      "dataType",
      "nullable",
      "defaultExpr",
      "extra",
      "foreignKey",
      "comment",
    ]);
    expect(gridColumns("sqlite")).toEqual([
      "name",
      "dataType",
      "nullable",
      "defaultExpr",
      "foreignKey",
    ]);
  });

  it("reads ON UPDATE out of MySQL's EXTRA", () => {
    expect(onUpdateOf("DEFAULT_GENERATED on update CURRENT_TIMESTAMP(3)")).toBe(
      "CURRENT_TIMESTAMP(3)",
    );
    expect(onUpdateOf("auto_increment")).toBeNull();
    expect(onUpdateOf(null)).toBeNull();
  });

  it("MySQL AUTO_INCREMENT on an existing column needs a key and an integer", () => {
    const col = { added: false, name: "n", dataType: "int", autoIncrement: false };
    expect(columnEditError("mysql", { ...col, isKey: false }, "autoIncrement", true)).toMatch(
      /needs a key/,
    );
    expect(columnEditError("mysql", { ...col, isKey: true }, "autoIncrement", true)).toBeNull();
    expect(
      columnEditError("mysql", { ...col, dataType: "text", isKey: true }, "autoIncrement", true),
    ).toMatch(/integer type/);
    expect(columnEditError("mysql", { ...col, isKey: false }, "autoIncrement", false)).toBeNull();
  });
});

describe("ON UPDATE of a MySQL column", () => {
  it("is CURRENT_TIMESTAMP with the column's fractional seconds", () => {
    // MySQL refuses one whose precision differs from the column's (1294)
    expect(onUpdateFor("datetime(3)")).toBe("CURRENT_TIMESTAMP(3)");
    expect(onUpdateFor("timestamp(6)")).toBe("CURRENT_TIMESTAMP(6)");
    expect(onUpdateFor("DATETIME( 2 ) NOT NULL")).toBe("CURRENT_TIMESTAMP(2)");
    // no precision, or a precision of 0: the bare form
    expect(onUpdateFor("datetime")).toBe("CURRENT_TIMESTAMP");
    expect(onUpdateFor("timestamp")).toBe("CURRENT_TIMESTAMP");
    expect(onUpdateFor("datetime(0)")).toBe("CURRENT_TIMESTAMP");
    expect(onUpdateFor("timestamp without time zone")).toBe("CURRENT_TIMESTAMP");
    expect(onUpdateFor("varchar(30)")).toBe("CURRENT_TIMESTAMP");
    expect(fractionalSeconds("time(3)")).toBe(0);
  });
});

describe("index prefix lengths (MySQL)", () => {
  it("char, text, blob and binary columns take a prefix; only text and blob need one", () => {
    for (const type of [
      "char(10)",
      "varchar(500)",
      "VARCHAR(20)",
      "national varchar(9)",
      "text",
      "mediumtext",
      "longtext",
      "tinyblob",
      "blob",
      "longblob",
      "binary(16)",
      "varbinary(255)",
    ])
      expect(supportsIndexPrefix(type), type).toBe(true);
    for (const type of [
      "int",
      "bigint unsigned",
      "datetime(3)",
      "json",
      "decimal(10,2)",
      "enum('a')",
    ])
      expect(supportsIndexPrefix(type), type).toBe(false);
    for (const type of [
      "text",
      "TINYTEXT",
      "mediumtext",
      "longtext",
      "blob",
      "text(100)",
      "longblob",
    ])
      expect(requiresIndexPrefix(type), type).toBe(true);
    for (const type of ["varchar(500)", "char(10)", "binary(16)", "varbinary(9)", "int"])
      expect(requiresIndexPrefix(type), type).toBe(false);
  });
});
