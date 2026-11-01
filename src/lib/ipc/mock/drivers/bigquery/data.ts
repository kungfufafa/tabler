import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ForeignKeyMeta } from "../../../bindings/ForeignKeyMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { TableKind } from "../../../bindings/TableKind";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The mock's BigQuery project: `shop` (customers, orders with a repeated RECORD, a view) and
 * `analytics` (3 GiB of events with JSON, GEOGRAPHY, BYTES and INT64 beyond 2^53, a materialized
 * view, an external table), and two routines, with the types, kinds and values `decode.rs` and
 * `catalog.rs` make of BigQuery's replies. Every project the mock opens holds the same datasets.
 */

/** A column as `tables.get` describes it. */
export interface Field {
  name: string;
  /** GoogleSQL's type as `FieldSchema::data_type` writes it (`ARRAY<…>` when repeated). */
  type: string;
  kind: ValueKind;
  required?: boolean;
  description?: string;
}

export interface Table {
  dataset: string;
  name: string;
  /** `tables.get`'s `type`. */
  type: "TABLE" | "VIEW" | "MATERIALIZED_VIEW" | "EXTERNAL";
  fields: Field[];
  primaryKey: string[];
  foreignKeys: ForeignKeyMeta[];
  /** The stand-in indexes `catalog::stand_in_indexes` makes of partitioning and clustering. */
  indexes: IndexMeta[];
  description: string | null;
  /** What a query reading every column scans (and a dry run reports). */
  bytes: number;
  /** A view's or materialized view's query. */
  definition: string | null;
  rows: Value[][];
}

export interface Dataset {
  name: string;
  location: string;
  tables: Map<string, Table>;
  routines: RoutineSummary[];
}

const int = (v: number): Value =>
  Number.isSafeInteger(v) ? { t: "int", v } : { t: "bigInt", v: String(v) };
const text = (v: string): Value => ({ t: "text", v });
const decimal = (v: string): Value => ({ t: "decimal", v });
const NULL: Value = { t: "null" };

/** `2024-01-02 03:04:05.123456+00`, as `decode.rs` writes a TIMESTAMP. */
export function timestampText(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number, w = 2) => String(n).padStart(w, "0");
  return (
    `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ` +
    `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}` +
    `${d.getUTCMilliseconds() ? `.${pad(d.getUTCMilliseconds(), 3)}000` : ""}+00`
  );
}

const dateText = (ms: number) => timestampText(ms).slice(0, 10);

/** A column of the structure, as `decode::column_meta` makes it. */
export function columnOf(f: Field, ordinal: number, primaryKey: readonly string[]): ColumnMeta {
  const repeated = f.type.startsWith("ARRAY<");
  const numeric = /^(BIG)?NUMERIC\((\d+), (\d+)\)$/.exec(f.type);
  const sized = /^(STRING|BYTES)\((\d+)\)$/.exec(f.type);
  const pk = primaryKey.indexOf(f.name);
  return {
    name: f.name,
    ordinal,
    dataType: f.type,
    kind: f.kind,
    nullable: !f.required && !repeated,
    defaultExpr: null,
    isPk: pk >= 0,
    pkOrdinal: pk >= 0 ? pk + 1 : null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: f.description ?? null,
    enumValues: null,
    charMaxLen: sized ? Number(sized[2]) : null,
    numericPrecision: numeric ? Number(numeric[2]) : null,
    numericScale: numeric ? Number(numeric[3]) : null,
    collation: null,
    arrayElemType: repeated ? f.type.slice(6, -1) : null,
    extra: null,
    baseType: null,
  };
}

const backticked = (name: string): string =>
  `\`${name.replaceAll("\\", "\\\\").replaceAll("`", "\\`")}\``;

const index = (
  name: string,
  columns: string[],
  method: string | null,
  definition: string,
  primary = false,
): IndexMeta => ({
  name,
  columns,
  unique: false,
  primary,
  method,
  predicate: null,
  definition,
  comment: null,
  include: [],
});

const DAY = 86_400_000;
const START = Date.UTC(2024, 0, 1);
const COUNTRIES = ["ID", "SG", "US", "DE", "JP"];
const STATUSES = ["new", "paid", "shipped", "cancelled"];
const EVENTS = ["page_view", "add_to_cart", "checkout", "purchase"];
const SKUS = ["SKU-100", "SKU-200", "SKU-300"];

function customers(): Table {
  const names = ["Ayu", "Budi", "Citra", "Dewi", "Eko", "Fajar", "Gita", "Hadi"];
  return {
    dataset: "shop",
    name: "customers",
    type: "TABLE",
    fields: [
      { name: "customer_id", type: "INT64", kind: "bigInt", required: true },
      { name: "name", type: "STRING", kind: "text" },
      { name: "email", type: "STRING(320)", kind: "text" },
      { name: "country", type: "STRING", kind: "text" },
      { name: "signed_up", type: "DATE", kind: "date" },
      { name: "vip", type: "BOOL", kind: "bool" },
    ],
    primaryKey: ["customer_id"],
    foreignKeys: [],
    indexes: [
      index("PRIMARY KEY", ["customer_id"], null, "PRIMARY KEY (`customer_id`) NOT ENFORCED", true),
    ],
    description: "One row per customer",
    bytes: 4_200,
    definition: null,
    rows: Array.from({ length: 40 }, (_, i) => {
      const name = `${names[i % names.length]} ${String.fromCharCode(65 + (i % 26))}.`;
      return [
        int(i + 1),
        text(name),
        i % 9 === 8 ? NULL : text(`customer${i + 1}@example.com`),
        text(COUNTRIES[i % COUNTRIES.length]!),
        { t: "date", v: dateText(START + i * 3 * DAY) },
        { t: "bool", v: i % 7 === 0 },
      ];
    }),
  };
}

function orders(): Table {
  return {
    dataset: "shop",
    name: "orders",
    type: "TABLE",
    fields: [
      { name: "order_id", type: "INT64", kind: "bigInt", required: true },
      { name: "customer_id", type: "INT64", kind: "bigInt" },
      { name: "status", type: "STRING", kind: "text" },
      { name: "total", type: "NUMERIC(10, 2)", kind: "decimal" },
      { name: "created_at", type: "TIMESTAMP", kind: "timestampTz" },
      {
        name: "items",
        type: "ARRAY<STRUCT<sku STRING, qty INT64>>",
        kind: "json",
        description: "The order's lines",
      },
    ],
    primaryKey: ["order_id"],
    foreignKeys: [
      {
        name: "fk_orders_customer",
        columns: ["customer_id"],
        refTable: { schema: "shop", name: "customers" },
        refColumns: ["customer_id"],
        onUpdate: "NO ACTION",
        onDelete: "NO ACTION",
        definition: "NOT ENFORCED",
      },
    ],
    indexes: [
      index("PRIMARY KEY", ["order_id"], null, "PRIMARY KEY (`order_id`) NOT ENFORCED", true),
      index(
        "partitioning",
        ["created_at"],
        "DAY",
        "PARTITION BY TIMESTAMP_TRUNC(`created_at`, DAY)",
      ),
      index("clustering", ["status"], "CLUSTER", "CLUSTER BY `status`"),
    ],
    description: null,
    bytes: 64_000,
    definition: null,
    rows: Array.from({ length: 250 }, (_, i) => {
      const cents = 1_000 + ((i * 7_919) % 90_000);
      const lines = Array.from({ length: (i % 3) + 1 }, (_, j) => ({
        sku: SKUS[(i + j) % SKUS.length],
        qty: ((i + j) % 4) + 1,
      }));
      return [
        int(i + 1),
        int((i % 40) + 1),
        text(STATUSES[i % STATUSES.length]!),
        decimal((cents / 100).toFixed(2)),
        { t: "timestampTz", v: timestampText(START + i * 4 * 3_600_000 + (i % 1000)) },
        { t: "json", v: JSON.stringify(lines) },
      ];
    }),
  };
}

function bigSpenders(orderRows: Value[][]): Table {
  const spent = new Map<number, number>();
  for (const r of orderRows) {
    const id = r[1]?.t === "int" ? r[1].v : 0;
    const total = r[3]?.t === "decimal" ? Number(r[3].v) : 0;
    spent.set(id, (spent.get(id) ?? 0) + total);
  }
  return {
    dataset: "shop",
    name: "v_big_spenders",
    type: "VIEW",
    fields: [
      { name: "customer_id", type: "INT64", kind: "bigInt" },
      { name: "spent", type: "NUMERIC", kind: "decimal" },
    ],
    primaryKey: [],
    foreignKeys: [],
    indexes: [],
    description: "Customers who spent more than 3,000",
    bytes: 6_000,
    definition:
      "SELECT customer_id, SUM(total) AS spent\nFROM `demo-proj.shop.orders`\nGROUP BY customer_id\nHAVING spent > 3000",
    rows: [...spent]
      .filter(([, s]) => s > 3000)
      .sort(([a], [b]) => a - b)
      .map(([id, s]) => [int(id), decimal(s.toFixed(2))]),
  };
}

function events(): Table {
  return {
    dataset: "analytics",
    name: "events",
    type: "TABLE",
    fields: [
      { name: "event_id", type: "STRING", kind: "text", required: true },
      { name: "user_id", type: "INT64", kind: "bigInt" },
      { name: "event_name", type: "STRING", kind: "text" },
      { name: "event_ts", type: "TIMESTAMP", kind: "timestampTz" },
      { name: "props", type: "JSON", kind: "json" },
      { name: "geo", type: "GEOGRAPHY", kind: "geometry" },
      { name: "payload", type: "BYTES", kind: "bytes" },
      {
        name: "sequence",
        type: "INT64",
        kind: "bigInt",
        description: "Above 2^53: sent as text",
      },
    ],
    primaryKey: [],
    foreignKeys: [],
    indexes: [
      index("partitioning", ["event_ts"], "DAY", "PARTITION BY TIMESTAMP_TRUNC(`event_ts`, DAY)"),
      index(
        "clustering",
        ["event_name", "user_id"],
        "CLUSTER",
        "CLUSTER BY `event_name`, `user_id`",
      ),
    ],
    description: "Clickstream, one row per event",
    // 3.2 GiB: a full scan is more than a profile's 1 GiB cost guard allows.
    bytes: 3_435_973_837,
    definition: null,
    rows: Array.from({ length: 1_200 }, (_, i) => {
      const bytes = new TextEncoder().encode(`evt-${i}`);
      return [
        text(`e-${(i + 1).toString(16).padStart(6, "0")}`),
        i % 11 === 10 ? NULL : int((i % 40) + 1),
        text(EVENTS[i % EVENTS.length]!),
        { t: "timestampTz", v: timestampText(START + i * 61_000) },
        { t: "json", v: JSON.stringify({ path: `/p/${i % 12}`, mobile: i % 2 === 0 }) },
        {
          t: "other",
          v: { ty: "geometry", text: `POINT(${(106.8 + (i % 10) / 100).toFixed(2)} -6.2)` },
        },
        {
          t: "bytes",
          v: { base64: btoa(String.fromCharCode(...bytes)), len: bytes.length, truncated: false },
        },
        { t: "bigInt", v: (9_007_199_254_740_993n + BigInt(i)).toString() },
      ];
    }),
  };
}

function dailyRevenue(orderRows: Value[][]): Table {
  const byDay = new Map<string, number>();
  for (const r of orderRows) {
    const day = r[4]?.t === "timestampTz" ? r[4].v.slice(0, 10) : "";
    const total = r[3]?.t === "decimal" ? Number(r[3].v) : 0;
    byDay.set(day, (byDay.get(day) ?? 0) + total);
  }
  return {
    dataset: "analytics",
    name: "daily_revenue",
    type: "MATERIALIZED_VIEW",
    fields: [
      { name: "day", type: "DATE", kind: "date" },
      { name: "revenue", type: "NUMERIC", kind: "decimal" },
    ],
    primaryKey: [],
    foreignKeys: [],
    indexes: [],
    description: null,
    bytes: 2_048,
    definition:
      "SELECT DATE(created_at) AS day, SUM(total) AS revenue\nFROM `demo-proj.shop.orders`\nGROUP BY day",
    rows: [...byDay]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([day, revenue]) => [{ t: "date", v: day }, decimal(revenue.toFixed(2))]),
  };
}

function gcsExports(): Table {
  return {
    dataset: "analytics",
    name: "gcs_exports",
    type: "EXTERNAL",
    fields: [
      { name: "file", type: "STRING", kind: "text" },
      { name: "size_bytes", type: "INT64", kind: "bigInt" },
    ],
    primaryKey: [],
    foreignKeys: [],
    indexes: [],
    description: "CSV files in gs://demo-proj-exports",
    // External data: a dry run cannot say what it would read.
    bytes: 0,
    definition: null,
    rows: [
      [text("gs://demo-proj-exports/2024-01.csv"), int(48_213)],
      [text("gs://demo-proj-exports/2024-02.csv"), int(51_007)],
      [text("gs://demo-proj-exports/2024-03.csv"), int(49_870)],
    ],
  };
}

const ROUTINES: RoutineSummary[] = [
  {
    schema: "shop",
    name: "archive_orders",
    kind: "procedure",
    arguments: "before DATE",
    identityArguments: "",
    returnType: null,
    language: "SQL",
    comment: "Moves old orders to shop.orders_archive",
  },
  {
    schema: "shop",
    name: "net_total",
    kind: "function",
    arguments: "total NUMERIC, tax_rate FLOAT64",
    identityArguments: "",
    returnType: "NUMERIC",
    language: "SQL",
    comment: null,
  },
];

/** The CREATE statements INFORMATION_SCHEMA keeps (`ddl`), by routine. */
export const ROUTINE_DDL: Record<string, string> = {
  archive_orders:
    "CREATE PROCEDURE `demo-proj.shop.archive_orders`(before DATE)\nBEGIN\n  INSERT INTO `demo-proj.shop.orders_archive` SELECT * FROM `demo-proj.shop.orders` WHERE DATE(created_at) < before;\n  DELETE FROM `demo-proj.shop.orders` WHERE DATE(created_at) < before;\nEND;",
  net_total:
    "CREATE FUNCTION `demo-proj.shop.net_total`(total NUMERIC, tax_rate FLOAT64) RETURNS NUMERIC AS (\n  total / (1 + CAST(tax_rate AS NUMERIC))\n);",
};

/** `INFORMATION_SCHEMA.TABLES.ddl` of a table: what BigQuery keeps, in its words. */
export function tableDdl(project: string, t: Table): string {
  const path = `\`${project}.${t.dataset}.${t.name}\``;
  if (t.type === "VIEW") return `CREATE VIEW ${path}\nAS ${t.definition ?? ""};`;
  if (t.type === "MATERIALIZED_VIEW")
    return `CREATE MATERIALIZED VIEW ${path}\nAS ${t.definition ?? ""};`;
  const cols = t.fields
    .map(
      (f) =>
        `  ${backticked(f.name).replace(/^`([a-z_][a-z0-9_]*)`$/i, "$1")} ${f.type}${f.required ? " NOT NULL" : ""}`,
    )
    .join(",\n");
  if (t.type === "EXTERNAL")
    return `CREATE EXTERNAL TABLE ${path}\n(\n${cols}\n)\nOPTIONS(\n  format="CSV",\n  uris=["gs://demo-proj-exports/*.csv"]\n);`;
  const pk = t.primaryKey.length
    ? `,\n  PRIMARY KEY (${t.primaryKey.join(", ")}) NOT ENFORCED`
    : "";
  const extra = t.indexes
    .filter((i) => !i.primary)
    .map((i) => (i.definition ?? "").replace(/`/g, "").replace("", ""))
    .join("\n");
  return `CREATE TABLE ${path}\n(\n${cols}${pk}\n)${extra ? `\n${extra}` : ""};`;
}

/** The projects `projects.list` shows the mock's credentials. */
export const LISTED_PROJECTS: readonly { id: string; name: string | null }[] = [
  { id: "demo-proj", name: "Demo Project" },
  { id: "acme-analytics", name: "Acme Analytics" },
];

/** A fresh copy of the project's datasets (edits in the editor change it). */
export function seedDatasets(): Map<string, Dataset> {
  const o = orders();
  const table = (list: Table[]) => new Map(list.map((t) => [t.name, t]));
  return new Map([
    [
      "analytics",
      {
        name: "analytics",
        location: "US",
        tables: table([dailyRevenue(o.rows), events(), gcsExports()]),
        routines: [],
      },
    ],
    [
      "shop",
      {
        name: "shop",
        location: "US",
        tables: table([customers(), o, bigSpenders(o.rows)]),
        routines: ROUTINES,
      },
    ],
  ]);
}

/** The sidebar's kind of a `tables.get` type (`catalog::table_kind`). */
export function tableKind(t: Table): TableKind {
  return t.type === "VIEW"
    ? "view"
    : t.type === "MATERIALIZED_VIEW"
      ? "materializedView"
      : t.type === "EXTERNAL"
        ? "foreignTable"
        : "table";
}
