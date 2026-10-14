import { describe, expect, it } from "vitest";
import type { ColumnMeta, ForeignKeyMeta, IndexMeta, TableMeta } from "@/lib/ipc";
import { baseType, laravelMigration, laravelMigrations, phpDefault, phpString } from "./laravel";

function col(name: string, dataType: string, patch: Partial<ColumnMeta> = {}): ColumnMeta {
  return {
    name,
    ordinal: 0,
    dataType,
    kind: "text",
    nullable: false,
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: null,
    ...patch,
  };
}

function meta(
  name: string,
  columns: ColumnMeta[],
  patch: Partial<TableMeta> = {},
  schema: string | null = "public",
): TableMeta {
  return {
    table: { schema, name },
    kind: "table",
    columns: columns.map((c, i) => ({ ...c, ordinal: i + 1 })),
    primaryKey: columns.filter((c) => c.isPk).map((c) => c.name),
    indexes: [],
    foreignKeys: [],
    triggers: [],
    comment: null,
    rowEstimate: null,
    ddl: null,
    checks: [],
    options: null,
    ...patch,
  };
}

const fk = (columns: string[], table: string, refColumns = ["id"], onDelete = "NO ACTION") =>
  ({
    name: `${columns.join("_")}_fkey`,
    columns,
    refTable: { schema: "public", name: table },
    refColumns,
    onUpdate: "NO ACTION",
    onDelete,
    definition: null,
  }) satisfies ForeignKeyMeta;

const index = (name: string, columns: string[], unique = false): IndexMeta => ({
  name,
  columns,
  unique,
  primary: false,
  method: null,
  predicate: null,
  definition: null,
  comment: null,
  include: [],
});

const PG_POSTS = meta(
  "posts",
  [
    col("id", "bigint", {
      kind: "bigInt",
      isPk: true,
      defaultExpr: "nextval('posts_id_seq'::regclass)",
    }),
    col("user_id", "bigint", { kind: "bigInt" }),
    col("editor_id", "bigint", { kind: "bigInt", nullable: true }),
    col("title", "character varying(200)", { charMaxLen: 200 }),
    col("slug", "character varying(255)", { charMaxLen: 255 }),
    col("status", "character varying(20)", {
      charMaxLen: 20,
      defaultExpr: "'draft'::character varying",
    }),
    col("body", "text", { nullable: true, comment: "Markdown, it's long" }),
    col("price", "numeric(10,2)", {
      kind: "decimal",
      numericPrecision: 10,
      numericScale: 2,
      defaultExpr: "0",
    }),
    col("is_public", "boolean", { kind: "bool", defaultExpr: "true" }),
    col("meta", "jsonb", { kind: "json", defaultExpr: "'{}'::jsonb" }),
    col("uid", "uuid", { kind: "uuid", defaultExpr: "gen_random_uuid()" }),
    col("role", "user_role", { kind: "enum", defaultExpr: "'viewer'::user_role" }),
    col("published_at", "timestamp with time zone", {
      kind: "timestampTz",
      defaultExpr: "now()",
    }),
    col("created_at", "timestamp(0) without time zone", { kind: "timestamp", nullable: true }),
    col("updated_at", "timestamp(0) without time zone", { kind: "timestamp", nullable: true }),
    col("deleted_at", "timestamp(0) without time zone", { kind: "timestamp", nullable: true }),
  ],
  {
    foreignKeys: [
      fk(["user_id"], "users", ["id"], "CASCADE"),
      fk(["editor_id"], "users", ["id"], "SET NULL"),
    ],
    indexes: [
      index("posts_pkey", ["id"]),
      index("posts_slug_unique", ["slug"], true),
      index("posts_status_published_at_index", ["status", "published_at"]),
      index("posts_title_idx", ["title"]),
    ],
    comment: "Blog posts",
  },
);
PG_POSTS.indexes[0]!.primary = true;

const MY_ORDERS = meta(
  "orders",
  [
    col("id", "int unsigned", { kind: "int", isPk: true, isAutoIncrement: true, isUnsigned: true }),
    col("customer_id", "bigint unsigned", { kind: "bigInt", isUnsigned: true }),
    col("code", "char(8)", { charMaxLen: 8 }),
    col("state", "enum('new','paid','shipped')", {
      kind: "enum",
      enumValues: ["new", "paid", "shipped"],
      defaultExpr: "new",
    }),
    col("total", "decimal(12,2)", {
      kind: "decimal",
      numericPrecision: 12,
      numericScale: 2,
      defaultExpr: "0.00",
    }),
    col("paid", "tinyint(1)", { kind: "bool", defaultExpr: "0" }),
    col("qty", "smallint", { kind: "int", defaultExpr: "1" }),
    col("note", "varchar(255)", { charMaxLen: 255, nullable: true, defaultExpr: "it's \\ fine" }),
    col("ref", "varchar(36)", {
      charMaxLen: 36,
      defaultExpr: "(uuid())",
      extra: "DEFAULT_GENERATED",
    }),
    col("geo", "point", { kind: "geometry", nullable: true }),
    col("blob", "mediumblob", { kind: "bytes", nullable: true }),
    col("ordered_at", "datetime", {
      kind: "timestamp",
      defaultExpr: "CURRENT_TIMESTAMP",
      extra: "DEFAULT_GENERATED",
    }),
    col("touched_at", "timestamp", {
      kind: "timestamp",
      nullable: true,
      defaultExpr: "CURRENT_TIMESTAMP",
      extra: "DEFAULT_GENERATED on update CURRENT_TIMESTAMP",
    }),
  ],
  {
    foreignKeys: [fk(["customer_id"], "customers", ["id"], "RESTRICT")],
    indexes: [
      index("PRIMARY", ["id"]),
      index("customer_id_fkey", ["customer_id"]),
      index("orders_code_unique", ["code"], true),
      index("orders_state_index", ["state"]),
    ],
  },
  "shop",
);
MY_ORDERS.indexes[0]!.primary = true;

const LITE_TAGS = meta(
  "post_tags",
  [
    col("post_id", "INTEGER", { kind: "int", isPk: true }),
    col("tag", "TEXT", { isPk: true }),
    col("weight", "REAL", { kind: "float", defaultExpr: "1.5" }),
    col("label", "VARCHAR(40)", { nullable: true, defaultExpr: "'x'" }),
    col("added", "DATETIME", { kind: "timestamp", defaultExpr: "CURRENT_TIMESTAMP" }),
    col("data", "BLOB", { kind: "bytes", nullable: true }),
    col("misc", "NUMERIC", { kind: "decimal", nullable: true }),
  ],
  { foreignKeys: [fk(["post_id"], "posts")] },
  "main",
);

describe("Laravel migrations", () => {
  it("reads types and defaults", () => {
    expect(baseType("timestamp(0) without time zone")).toBe("timestamp without time zone");
    expect(baseType("int(10) unsigned zerofill")).toBe("int");
    expect(phpString("it's \\ here")).toBe("'it\\'s \\\\ here'");
    expect(phpDefault(col("a", "text", { defaultExpr: "'o''k'::text" }), "postgres")).toEqual({
      kind: "value",
      php: "'o\\'k'",
    });
    expect(phpDefault(col("a", "integer", { defaultExpr: "(-1)" }), "postgres")).toEqual({
      kind: "value",
      php: "-1",
    });
    expect(
      phpDefault(col("a", "integer", { defaultExpr: "NULL::integer" }), "postgres"),
    ).toBeNull();
    expect(phpDefault(col("a", "text", { defaultExpr: "lower('X')" }), "postgres")).toEqual({
      kind: "raw",
      sql: "lower('X')",
    });
  });

  it("generates a PostgreSQL migration", () => {
    expect(laravelMigration(PG_POSTS, "postgres")).toMatchSnapshot();
  });

  it("generates a MySQL migration", () => {
    expect(laravelMigration(MY_ORDERS, "mysql")).toMatchSnapshot();
  });

  it("generates a SQLite migration", () => {
    expect(laravelMigration(LITE_TAGS, "sqlite")).toMatchSnapshot();
  });

  it("concatenates one file per table, each after a separator", () => {
    const out = laravelMigrations([LITE_TAGS, MY_ORDERS], "mysql");
    expect(out.match(/^\/\/ ----- .+ -----$/gm)).toEqual([
      "// ----- post_tags -----",
      "// ----- orders -----",
    ]);
    expect(out.match(/<\?php/g)).toHaveLength(2);
    // A PostgreSQL table outside public keeps its schema.
    expect(
      laravelMigration(
        meta(
          "audit_log",
          [col("id", "bigint", { isPk: true, isAutoIncrement: true })],
          {},
          "audit",
        ),
        "postgres",
      ),
    ).toContain("Schema::create('audit.audit_log'");
  });
});
