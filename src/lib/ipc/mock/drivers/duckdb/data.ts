import type { CheckMeta } from "../../../bindings/CheckMeta";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ForeignKeyMeta } from "../../../bindings/ForeignKeyMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { TableKind } from "../../../bindings/TableKind";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The mock's DuckDB databases: the lane's sample (`src-tauri/tests/fixtures/duckdb/sample.sql`:
 * schemas `main` and `crm`, keys, a CHECK, indexes, a sequence, an ENUM, a view, a macro and a
 * table macro, comments, a row of every value type) as the backend decodes it, plus `page_views`,
 * big enough to page through. `events` has no primary key, so its rows cannot be edited (no
 * `EditWithoutKey`). An in-memory database starts empty, its catalog named `memory`.
 */

export const NULL: Value = { t: "null" };
const int = (v: number): Value => ({ t: "int", v });
const big = (v: string): Value => ({ t: "bigInt", v });
const text = (v: string): Value => ({ t: "text", v });
const dec = (v: string): Value => ({ t: "decimal", v });
const json = (v: string): Value => ({ t: "json", v });
const tstz = (v: string): Value => ({ t: "timestampTz", v });
const ts = (v: string): Value => ({ t: "timestamp", v });

export interface Col {
  name: string;
  /** As `duckdb_columns().data_type` spells it. */
  type: string;
  kind: ValueKind;
  nullable: boolean;
  default?: string;
  comment?: string;
  enumValues?: string[];
}

export interface Table {
  schema: string;
  name: string;
  kind: TableKind;
  cols: Col[];
  pk: string[];
  rows: Value[][];
  comment: string | null;
  /** The primary key and UNIQUE constraints, then the CREATE INDEXes. */
  indexes: IndexMeta[];
  foreignKeys: ForeignKeyMeta[];
  checks: CheckMeta[];
  /** The `CREATE TABLE` / `CREATE VIEW` text DuckDB keeps. */
  sql: string;
}

export interface Macro {
  schema: string;
  name: string;
  params: string[];
  table: boolean;
  body: string;
  comment: string | null;
}

export interface Database {
  /** The catalog's name: the file's stem, or `memory`. */
  name: string;
  /** The file; null in memory. */
  path: string | null;
  schemas: string[];
  tables: Table[];
  macros: Macro[];
  /** Sequences by `schema.name`: the next value. */
  sequences: Map<string, number>;
}

/** `DuckDB v1.5.6 (069cc9f9b5)`: `DuckdbServerInfo::label` of the bundled crate. */
export const VERSION = "v1.5.6";
export const SOURCE_ID = "069cc9f9b5";
export const VERSION_TEXT = `DuckDB ${VERSION} (${SOURCE_ID})`;

const MOOD = ["sad", "ok", "happy"];

/** `catalog::auto_increments`: a `nextval(…)` default. */
const autoIncrements = (c: Col) => /^nextval\(/i.test(c.default ?? "");

/** A column as the structure view and the grid get it (`catalog::parse_columns`). */
export function columnMeta(t: Table, c: Col, i: number): ColumnMeta {
  const pkOrdinal = t.pk.indexOf(c.name);
  const size = /^DECIMAL\((\d+),(\d+)\)$/.exec(c.type);
  return {
    name: c.name,
    ordinal: i + 1,
    dataType: c.type,
    kind: c.kind,
    nullable: c.nullable,
    defaultExpr: c.default ?? null,
    isPk: pkOrdinal >= 0,
    pkOrdinal: pkOrdinal >= 0 ? pkOrdinal + 1 : null,
    isAutoIncrement: autoIncrements(c),
    isGenerated: false,
    isUnsigned: c.type.startsWith("U") && (c.kind === "int" || c.kind === "bigInt"),
    comment: c.comment ?? null,
    enumValues: c.enumValues ?? null,
    charMaxLen: null,
    numericPrecision: size ? Number(size[1]) : null,
    numericScale: size ? Number(size[2]) : null,
    collation: null,
    arrayElemType: c.type.endsWith("[]") ? c.type.slice(0, -2) : null,
    extra: null,
  };
}

export const columnsOf = (t: Table): ColumnMeta[] => t.cols.map((c, i) => columnMeta(t, c, i));

const col = (name: string, type: string, kind: ValueKind, more: Partial<Col> = {}): Col => ({
  name,
  type,
  kind,
  nullable: true,
  ...more,
});

const key = (
  table: string,
  columns: string[],
  primary: boolean,
  definition: string,
): IndexMeta => ({
  name: `${table}_${columns.join("_")}_${primary ? "pkey" : "key"}`,
  columns,
  unique: true,
  primary,
  method: null,
  predicate: null,
  definition,
  comment: null,
  include: [],
});

const index = (name: string, columns: string[], unique: boolean, sql: string): IndexMeta => ({
  name,
  columns,
  unique,
  primary: false,
  method: null,
  predicate: null,
  definition: sql,
  comment: null,
  include: [],
});

const fk = (
  table: string,
  columns: string[],
  schema: string,
  ref: string,
  refColumns: string[],
): ForeignKeyMeta => ({
  name: `${table}_${columns.join("_")}_fkey`,
  columns,
  refTable: { schema, name: ref },
  refColumns,
  // DuckDB enforces only NO ACTION (RESTRICT).
  onUpdate: "NO ACTION",
  onDelete: "NO ACTION",
  definition: `FOREIGN KEY (${columns.join(", ")}) REFERENCES ${ref}(${refColumns.join(", ")})`,
});

/** `CREATE TABLE` as DuckDB keeps it: its columns with their types and constraints. */
function createTable(t: Omit<Table, "sql">): string {
  const lines = t.cols.map((c) => {
    const parts = [c.name, c.type];
    if (!c.nullable) parts.push("NOT NULL");
    if (c.default !== undefined) parts.push(`DEFAULT(${c.default})`);
    return parts.join(" ");
  });
  for (const ch of t.checks) lines.push(`CHECK(${ch.expression})`);
  if (t.pk.length > 0) lines.push(`PRIMARY KEY(${t.pk.join(", ")})`);
  for (const i of t.indexes.filter(
    (x) => x.unique && !x.primary && x.definition?.startsWith("UNIQUE"),
  ))
    lines.push(`UNIQUE(${i.columns.join(", ")})`);
  for (const f of t.foreignKeys) lines.push(f.definition ?? "");
  const schema = t.schema === "main" ? "" : `${t.schema}.`;
  return `CREATE TABLE ${schema}${t.name}(${lines.join(", ")});`;
}

const table = (t: Omit<Table, "sql" | "kind"> & { kind?: TableKind }): Table => {
  const full = { kind: "table" as TableKind, ...t };
  return { ...full, sql: createTable(full) };
};

/** `page_views`: 1,240 rows, enough to page through. */
function pageViews(): Value[][] {
  const paths = ["/", "/pricing", "/docs", "/docs/sql", "/blog", "/signup"];
  const agents = ["Firefox", "Safari", "Chrome", "Edge"];
  return Array.from({ length: 1240 }, (_, i) => {
    const day = 1 + (i % 28);
    const hour = i % 24;
    return [
      int(i + 1),
      text(paths[(i * 7) % paths.length]!),
      tstz(
        `2024-02-${String(day).padStart(2, "0")} ${String(hour).padStart(2, "0")}:${String((i * 13) % 60).padStart(2, "0")}:00+00`,
      ),
      i % 9 === 0 ? NULL : text(agents[i % agents.length]!),
      dec((((i * 37) % 5000) / 100).toFixed(2)),
    ];
  });
}

/** The sample database (`sample.sql`), its catalog named after the file. */
export function seedSample(name: string, path: string): Database {
  const crmCustomers = table({
    schema: "crm",
    name: "customers",
    cols: [
      col("id", "INTEGER", "int", { nullable: false }),
      col("name", "VARCHAR", "text", { nullable: false }),
      col("email", "VARCHAR", "text", { comment: "Login" }),
      col("feeling", "mood", "enum", { enumValues: MOOD }),
      col("tags", "VARCHAR[]", "json"),
      col("created_at", "TIMESTAMP", "timestamp", { default: "CURRENT_TIMESTAMP" }),
    ],
    pk: ["id"],
    rows: [
      [
        int(1),
        text("Ada"),
        text("ada@example.com"),
        { t: "enum", v: "happy" },
        json('["vip", "early"]'),
        ts("2024-05-01 09:30:00.125"),
      ],
      [
        int(2),
        text("Linus"),
        NULL,
        { t: "enum", v: "ok" },
        json("[]"),
        ts("2024-05-01 09:30:00.125"),
      ],
    ],
    comment: "People who buy",
    indexes: [
      key("customers", ["id"], true, "PRIMARY KEY(id)"),
      key("customers", ["email"], false, "UNIQUE(email)"),
    ],
    foreignKeys: [],
    checks: [],
  });
  const customers = table({
    schema: "main",
    name: "customers",
    cols: [
      col("id", "INTEGER", "int", { nullable: false }),
      col("name", "VARCHAR", "text", { nullable: false }),
    ],
    pk: ["id"],
    rows: [
      [int(1), text("Ada")],
      [int(2), text("Linus")],
      [int(3), text("Grace")],
    ],
    comment: null,
    indexes: [key("customers", ["id"], true, "PRIMARY KEY(id)")],
    foreignKeys: [],
    checks: [],
  });
  const orders = table({
    schema: "main",
    name: "orders",
    cols: [
      col("id", "BIGINT", "bigInt", { nullable: false, default: "nextval('order_seq')" }),
      col("customer_id", "INTEGER", "int", { nullable: false }),
      col("total", "DECIMAL(12,2)", "decimal", { nullable: false }),
      col("status", "VARCHAR", "text", { default: "'new'" }),
      col("placed_at", "TIMESTAMP WITH TIME ZONE", "timestampTz"),
      col("meta", "JSON", "json"),
    ],
    pk: ["id"],
    rows: [
      [
        int(1000),
        int(1),
        dec("19.99"),
        text("paid"),
        tstz("2024-03-01 10:00:00+00"),
        json('{"coupon": "SPRING"}'),
      ],
      [int(1001), int(1), dec("5.00"), text("new"), tstz("2023-12-24 18:30:00+00"), NULL],
      [
        int(1002),
        int(2),
        dec("120.50"),
        text("shipped"),
        tstz("2024-06-15 08:15:30+00"),
        json('{"gift": true}'),
      ],
    ],
    comment: null,
    indexes: [
      key("orders", ["id"], true, "PRIMARY KEY(id)"),
      index(
        "orders_placed",
        ["placed_at"],
        false,
        "CREATE INDEX orders_placed ON orders(placed_at);",
      ),
      index(
        "orders_customer_status",
        ["customer_id", "lower(status)"],
        true,
        "CREATE UNIQUE INDEX orders_customer_status ON orders(customer_id, lower(status));",
      ),
    ],
    foreignKeys: [fk("orders", ["customer_id"], "main", "customers", ["id"])],
    checks: [{ name: "orders_total_check", expression: "(total >= 0)", columns: ["total"] }],
  });
  const orderItems = table({
    schema: "main",
    name: "order_items",
    cols: [
      col("order_id", "BIGINT", "bigInt", { nullable: false }),
      col("line", "INTEGER", "int", { nullable: false }),
      col("sku", "VARCHAR", "text", { nullable: false }),
      col("qty", "INTEGER", "int", { default: "1" }),
    ],
    pk: ["order_id", "line"],
    rows: [
      [int(1000), int(1), text("A-1"), int(2)],
      [int(1000), int(2), text("B-7"), int(1)],
      [int(1002), int(1), text("C-3"), int(5)],
    ],
    comment: null,
    indexes: [key("order_items", ["order_id", "line"], true, "PRIMARY KEY(order_id, line)")],
    foreignKeys: [fk("order_items", ["order_id"], "main", "orders", ["id"])],
    checks: [],
  });
  const events = table({
    schema: "main",
    name: "events",
    cols: [
      col("happened_at", "TIMESTAMP", "timestamp"),
      col("kind", "VARCHAR", "text"),
      col("payload", 'STRUCT("source" VARCHAR, attempts INTEGER)', "json"),
    ],
    pk: [],
    rows: [0, 1].map(() => [
      ts("2024-01-01 00:00:00"),
      text("login"),
      json('{"source": "web", "attempts": 1}'),
    ]),
    comment: null,
    indexes: [],
    foreignKeys: [],
    checks: [],
  });
  const allTypes = table({
    schema: "main",
    name: "all_types",
    cols: [
      col("id", "INTEGER", "int", { nullable: false }),
      col("b", "BOOLEAN", "bool"),
      col("ti", "TINYINT", "int"),
      col("si", "SMALLINT", "int"),
      col("i", "INTEGER", "int"),
      col("bi", "BIGINT", "bigInt"),
      col("hi", "HUGEINT", "bigInt"),
      col("uti", "UTINYINT", "int"),
      col("usi", "USMALLINT", "int"),
      col("ui", "UINTEGER", "int"),
      col("ubi", "UBIGINT", "bigInt"),
      col("uhi", "UHUGEINT", "bigInt"),
      col("f", "FLOAT", "float"),
      col("d", "DOUBLE", "float"),
      col("num", "DECIMAL(18,3)", "decimal"),
      col("v", "VARCHAR", "text"),
      col("bl", "BLOB", "bytes"),
      col("bt", "BIT", "bit"),
      col("dt", "DATE", "date"),
      col("tm", "TIME", "time"),
      col("ttz", "TIME WITH TIME ZONE", "time"),
      col("ts", "TIMESTAMP", "timestamp"),
      col("ts_s", "TIMESTAMP_S", "timestamp"),
      col("ts_ms", "TIMESTAMP_MS", "timestamp"),
      col("ts_ns", "TIMESTAMP_NS", "timestamp"),
      col("tstz", "TIMESTAMP WITH TIME ZONE", "timestampTz"),
      col("iv", "INTERVAL", "interval"),
      col("u", "UUID", "uuid"),
      col("j", "JSON", "json"),
      col("e", "mood", "enum", { enumValues: MOOD }),
      col("l", "INTEGER[]", "json"),
      col("a", "VARCHAR[2]", "json"),
      col("s", "STRUCT(a INTEGER, b VARCHAR)", "json"),
      col("m", "MAP(VARCHAR, INTEGER)", "json"),
      col("un", "UNION(num INTEGER, str VARCHAR)", "json"),
      col("bn", "BIGNUM", "bigInt"),
    ],
    pk: ["id"],
    rows: [
      [
        int(1),
        { t: "bool", v: true },
        int(-8),
        int(-16),
        int(-32),
        big("9007199254740993"),
        big("-170141183460469231731687303715884105728"),
        int(255),
        int(65535),
        int(4294967295),
        big("18446744073709551615"),
        big("340282366920938463463374607431768211455"),
        { t: "float", v: 0.5 },
        { t: "float", v: 0.1 },
        dec("12345.678"),
        text("héllo"),
        { t: "bytes", v: { base64: "qgBhYg==", len: 4, truncated: false } },
        { t: "other", v: { ty: "bit", text: "10101" } },
        { t: "date", v: "2024-01-31" },
        { t: "time", v: "12:00:00.5" },
        { t: "time", v: "12:00:00+02" },
        ts("2024-01-31 12:00:00.5"),
        ts("2024-01-31 12:00:00"),
        ts("2024-01-31 12:00:00.123"),
        ts("2024-01-31 12:00:00.123456789"),
        tstz("2024-01-31 10:00:00+00"),
        { t: "interval", v: "1 year 2 months 3 days 04:05:06.789" },
        { t: "uuid", v: "6bea4f37-ddba-4e31-95b0-15bb71efdb48" },
        json('{"a": [1, 2]}'),
        { t: "enum", v: "happy" },
        json("[1,2,null]"),
        json('["x","y"]'),
        json('{"a":1,"b":"it\'s"}'),
        json('{"k":1,"j":2}'),
        json('"u"'),
        big("-42"),
      ],
      [int(2), ...Array.from({ length: 35 }, () => NULL)],
    ],
    comment: null,
    indexes: [key("all_types", ["id"], true, "PRIMARY KEY(id)")],
    foreignKeys: [],
    checks: [],
  });
  const views = table({
    schema: "main",
    name: "page_views",
    cols: [
      col("id", "INTEGER", "int", { nullable: false }),
      col("path", "VARCHAR", "text", { nullable: false }),
      col("viewed_at", "TIMESTAMP WITH TIME ZONE", "timestampTz", { nullable: false }),
      col("agent", "VARCHAR", "text"),
      col("load_ms", "DECIMAL(8,2)", "decimal"),
    ],
    pk: ["id"],
    rows: pageViews(),
    comment: "A month of traffic",
    indexes: [key("page_views", ["id"], true, "PRIMARY KEY(id)")],
    foreignKeys: [],
    checks: [],
  });
  const recent: Table = {
    schema: "main",
    name: "recent_orders",
    kind: "view",
    cols: orders.cols,
    pk: [],
    rows: [],
    comment: "Orders of 2024 on",
    indexes: [],
    foreignKeys: [],
    checks: [],
    sql: "CREATE VIEW recent_orders AS SELECT * FROM orders WHERE (placed_at > CAST('2024-01-01 00:00:00+00' AS TIMESTAMP WITH TIME ZONE));",
  };
  return {
    name,
    path,
    schemas: ["crm", "main"],
    tables: [crmCustomers, customers, orders, orderItems, events, allTypes, views, recent],
    macros: [
      {
        schema: "main",
        name: "add_tax",
        params: ["x", "rate"],
        table: false,
        body: "(x * (1 + rate))",
        comment: null,
      },
      {
        schema: "main",
        name: "top_orders",
        params: ["n"],
        table: true,
        body: "SELECT * FROM orders ORDER BY total DESC LIMIT n",
        comment: null,
      },
    ],
    sequences: new Map([["main.order_seq", 1003]]),
  };
}

/** An empty database: a new file's, or `memory`. */
export const emptyDatabase = (name: string, path: string | null): Database => ({
  name,
  path,
  schemas: ["main"],
  tables: [],
  macros: [],
  sequences: new Map(),
});

/** The rows a view shows, as DuckDB would compute them (the mock's two views). */
export function viewRows(db: Database, t: Table): Value[][] {
  if (t.name !== "recent_orders") return [];
  const orders = db.tables.find((x) => x.schema === t.schema && x.name === "orders");
  return (orders?.rows ?? []).filter((r) => {
    const at = r[4];
    return at?.t === "timestampTz" && at.v > "2024-01-01 00:00:00+00";
  });
}
