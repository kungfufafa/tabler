import { describe, expect, it } from "vitest";
import { buildCreateTable, initialColumns, moveDraft, newColumnDraft } from "./createTable";
import {
  dropTableOp,
  dropViewOp,
  suggestIndexName,
  truncateTableOp,
  validateForeignKey,
  validateIndex,
} from "./tableActions";

const col = (name: string, extra: Parameters<typeof newColumnDraft>[1] = {}) =>
  newColumnDraft("postgres", { name, ...extra });

describe("buildCreateTable", () => {
  it("requires a table name and at least one column", () => {
    const r = buildCreateTable("postgres", "public", "  ", []);
    expect(r).toEqual({
      ok: false,
      errors: ["Table name is required", "Add at least one column"],
    });
  });

  it("requires column names and types and rejects duplicates (case-insensitive)", () => {
    const r = buildCreateTable("postgres", "public", "t", [
      col("id"),
      col(""),
      col("ID"),
      col("x", { dataType: " " }),
    ]);
    expect(r.ok).toBe(false);
    if (r.ok) return;
    expect(r.errors).toContain("Column 2 needs a name");
    expect(r.errors).toContain("Duplicate column name “ID”");
    expect(r.errors).toContain("Column “x” needs a type");
  });

  it("builds the createTable op with the primary key in column order", () => {
    const r = buildCreateTable("postgres", "public", " posts ", [
      ...initialColumns("postgres"),
      col("title", { nullable: false, defaultExpr: " 'untitled' ", comment: " Title " }),
      col("slug", { primaryKey: true, nullable: true }),
    ]);
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.op.table).toEqual({ schema: "public", name: "posts" });
    expect(r.op.primaryKey).toEqual(["id", "slug"]);
    expect(r.op.columns[0]).toMatchObject({
      name: "id",
      // TablePlus: a serial (an integer with a sequence default) on PostgreSQL
      dataType: "serial",
      nullable: false,
      autoIncrement: true,
    });
    expect(r.op.columns[1]).toMatchObject({ defaultExpr: "'untitled'", comment: "Title" });
    // Primary key columns are never nullable.
    expect(r.op.columns[2]?.nullable).toBe(false);
  });

  it("checks auto increment rules per dialect", () => {
    const bad = buildCreateTable("postgres", null, "t", [col("id", { autoIncrement: true })]);
    expect(bad.ok ? [] : bad.errors).toContain(
      "Auto increment needs an integer type (column “id”)",
    );
    const sqlite = buildCreateTable("sqlite", "main", "t", [
      newColumnDraft("sqlite", { name: "id", dataType: "INTEGER", autoIncrement: true }),
    ]);
    expect(sqlite.ok ? [] : sqlite.errors).toContain(
      "SQLite AUTOINCREMENT needs a single INTEGER PRIMARY KEY column",
    );
    const mysql = buildCreateTable("mysql", "db", "t", [
      newColumnDraft("mysql", { name: "id", dataType: "int", autoIncrement: true }),
    ]);
    expect(mysql.ok ? [] : mysql.errors[0]).toMatch(/must be part of the primary key/);
    expect(buildCreateTable("sqlite", "main", "t", initialColumns("sqlite")).ok).toBe(true);
    expect(buildCreateTable("mysql", "db", "t", initialColumns("mysql")).ok).toBe(true);
  });

  it("drops comments on SQLite", () => {
    const r = buildCreateTable("sqlite", "main", "t", [
      newColumnDraft("sqlite", { name: "a", comment: "x" }),
    ]);
    expect(r.ok && r.op.columns[0]?.comment).toBeNull();
  });

  it("moves drafts within bounds", () => {
    expect(moveDraft(["a", "b", "c"], 0, 1)).toEqual(["b", "a", "c"]);
    expect(moveDraft(["a", "b", "c"], 0, -1)).toEqual(["a", "b", "c"]);
    expect(moveDraft(["a", "b", "c"], 2, 1)).toEqual(["a", "b", "c"]);
  });
});

describe("table actions", () => {
  const t = { schema: "public", name: 'we"ird' };

  it("quotes view names for DROP VIEW and only cascades on PG", () => {
    expect(dropViewOp(t, "view", "postgres", true)).toEqual({
      op: "rawSql",
      sql: 'DROP VIEW "public"."we""ird" CASCADE',
    });
    expect(dropViewOp({ schema: "db", name: "v`1" }, "view", "mysql", true)).toEqual({
      op: "rawSql",
      sql: "DROP VIEW `db`.`v``1`",
    });
    expect(dropViewOp(t, "materializedView", "postgres", false)).toMatchObject({
      sql: 'DROP MATERIALIZED VIEW "public"."we""ird"',
    });
  });

  it("only PostgreSQL gets RESTART IDENTITY / CASCADE", () => {
    expect(truncateTableOp(t, "postgres", true)).toMatchObject({ restartIdentity: true });
    expect(truncateTableOp(t, "mysql", true)).toMatchObject({ restartIdentity: false });
    expect(dropTableOp(t, "sqlite", true)).toMatchObject({ cascade: false, ifExists: false });
    expect(dropTableOp(t, "postgres", true)).toMatchObject({ cascade: true });
  });

  it("validates indexes and suggests names", () => {
    expect(suggestIndexName("Order Items", ["order_id", "item"], true)).toBe(
      "order_items_order_id_item_key",
    );
    const ok = validateIndex(
      { name: "ix", columns: ["a"], unique: false, method: "gin", predicate: " a > 1 " },
      "postgres",
      [],
    );
    expect(ok).toEqual({
      ok: true,
      index: {
        name: "ix",
        columns: ["a"],
        unique: false,
        method: "gin",
        predicate: "a > 1",
        comment: null,
      },
    });
    const mysql = validateIndex(
      { name: "ix", columns: ["a"], unique: false, method: "gin", predicate: "a > 1" },
      "mysql",
      [],
    );
    expect(mysql.ok && mysql.index).toMatchObject({ method: null, predicate: null });
    expect(
      validateIndex(
        { name: "IX", columns: [], unique: false, method: "", predicate: "" },
        "postgres",
        ["ix"],
      ),
    ).toEqual({
      ok: false,
      errors: ["An index named “IX” already exists", "Pick at least one column"],
    });
  });

  it("validates foreign keys", () => {
    expect(
      validateForeignKey({
        name: "",
        columns: ["a", "b"],
        refTable: { schema: "public", name: "r" },
        refColumns: ["id"],
        onUpdate: "",
        onDelete: "CASCADE",
      }),
    ).toEqual({ ok: false, errors: ["Pick as many referenced columns as columns"] });
    expect(
      validateForeignKey({
        name: " fk ",
        columns: ["a"],
        refTable: { schema: "public", name: "r" },
        refColumns: ["id"],
        onUpdate: "",
        onDelete: "CASCADE",
      }),
    ).toEqual({
      ok: true,
      fk: {
        name: "fk",
        columns: ["a"],
        refTable: { schema: "public", name: "r" },
        refColumns: ["id"],
        onUpdate: null,
        onDelete: "CASCADE",
      },
    });
  });
});
