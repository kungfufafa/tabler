import { describe, expect, it } from "vitest";
import {
  columnEditError,
  columnFields,
  columnOp,
  supportsAddAutoIncrement,
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
    expect(columnOp("postgres", T, "a", "autoIncrement", true)).toBeNull();
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
