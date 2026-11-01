import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The keyspaces of the mock's cluster: `shop` (users by id with a set and a map, orders by
 * customer clustered newest first with a `bigint` past 2^53, page views as counters, and a
 * materialized view of the orders by status), `iot` (readings by a two-column partition key, with
 * blobs) and the system keyspaces the sidebar hides. They live as long as the page, so edits stay
 * across connections as on a cluster. Values are what the backend decodes (`values.rs`): integers
 * past 2^53 as `bigInt`, decimals as text, timestamps as UTC text with milliseconds, collections
 * as JSON.
 */

/** A column's place in the key (`catalog::KeyRole`). */
export type Role = "partition" | "clustering" | "static" | "regular";

export interface Col {
  name: string;
  /** The server's type text (`frozen<list<text>>`). */
  type: string;
  role: Role;
  /** Its place in the partition key or among the clustering columns. */
  position: number;
  desc?: boolean;
}

export interface Table {
  keyspace: string;
  name: string;
  kind: "table" | "materializedView";
  /** In key order (`catalog::column_metas`): partition, clustering, static, regular. */
  cols: Col[];
  /** Rows in the table's order (token, then clustering), cells by `cols`. */
  data: Value[][];
  comment: string | null;
  indexes: { name: string; column: string; method: string }[];
  /** A view's base table and filter. */
  view?: { base: string; where: string };
  /** `system.size_estimates`: partitions, which stand in for the count. */
  estimate: number | null;
}

export interface Keyspace {
  name: string;
  replication: string;
  tables: Table[];
}

/** The keyspaces the sidebar hides (`catalog::is_system_keyspace`). */
export const SYSTEM_KEYSPACES = [
  "system",
  "system_auth",
  "system_distributed",
  "system_schema",
  "system_traces",
  "system_views",
  "system_virtual_schema",
];

/** `types::native_kind`, and JSON for collections, tuples, UDTs and vectors. */
export function kindOf(type: string): ValueKind {
  const t = type
    .trim()
    .toLowerCase()
    .replace(/^frozen<(.*)>$/, "$1");
  if (/^(list|set|map|tuple|vector)</.test(t) || t.startsWith('"')) return "json";
  switch (t) {
    case "ascii":
    case "text":
    case "varchar":
      return "text";
    case "int":
    case "smallint":
    case "tinyint":
      return "int";
    case "bigint":
    case "counter":
    case "varint":
      return "bigInt";
    case "decimal":
      return "decimal";
    case "float":
    case "double":
      return "float";
    case "boolean":
      return "bool";
    case "blob":
      return "bytes";
    case "uuid":
    case "timeuuid":
      return "uuid";
    case "timestamp":
      return "timestampTz";
    case "date":
      return "date";
    case "time":
      return "time";
    case "duration":
      return "interval";
    default:
      return "other";
  }
}

const isKey = (c: Col) => c.role === "partition" || c.role === "clustering";

/** The element type of a list, set or vector (`ColumnMeta.arrayElemType`). */
function elemType(type: string): string | null {
  const m = /^(?:frozen<)?(?:list|set|vector)<(.+?)(?:,\s*\d+)?>>?$/.exec(type);
  return m ? m[1]! : null;
}

/** `catalog::column_metas`. */
export function columnsOf(t: Table): ColumnMeta[] {
  let pk = 0;
  return t.cols.map((c, i) => {
    const key = isKey(c);
    if (key) pk++;
    return {
      name: c.name,
      ordinal: i + 1,
      dataType: c.type,
      kind: kindOf(c.type),
      nullable: !key,
      defaultExpr: null,
      isPk: key,
      pkOrdinal: key ? pk : null,
      isAutoIncrement: false,
      isGenerated: false,
      isUnsigned: false,
      comment: null,
      enumValues: null,
      charMaxLen: null,
      numericPrecision: null,
      numericScale: null,
      collation: null,
      arrayElemType: elemType(c.type),
      extra:
        c.role === "partition"
          ? "partition key"
          : c.role === "clustering"
            ? `clustering ${c.desc ? "DESC" : "ASC"}`
            : c.role === "static"
              ? "static"
              : null,
      baseType: null,
    };
  });
}

export const primaryKeyOf = (t: Table): string[] => t.cols.filter(isKey).map((c) => c.name);
export const partitionKeyOf = (t: Table): string[] =>
  t.cols.filter((c) => c.role === "partition").map((c) => c.name);

/** `"name"` with `"` doubled (`cql.rs` `double_quoted`). */
export const quoted = (name: string) => `"${name.replaceAll('"', '""')}"`;
export const qualified = (t: { keyspace: string; name: string }) =>
  `${quoted(t.keyspace)}.${quoted(t.name)}`;

/** `catalog::parse_indexes`: each with the statement that creates it. */
export function indexesOf(t: Table): IndexMeta[] {
  return t.indexes
    .map((i) => ({
      name: i.name,
      columns: [i.column],
      unique: false,
      primary: false,
      method: i.method,
      predicate: null,
      definition:
        i.method === "sai"
          ? `CREATE CUSTOM INDEX ${quoted(i.name)} ON ${qualified(t)} (${quoted(i.column)}) USING 'StorageAttachedIndex';`
          : `CREATE INDEX ${quoted(i.name)} ON ${qualified(t)} (${quoted(i.column)});`,
      comment: null,
      include: [],
    }))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
}

/** The table's DDL as the server's `DESCRIBE` gives it (Cassandra 4.0+). */
export function ddlOf(t: Table): string {
  const pk = t.cols.filter((c) => c.role === "partition").map((c) => quoted(c.name));
  const ck = t.cols.filter((c) => c.role === "clustering");
  const key = `PRIMARY KEY (${pk.length > 1 ? `(${pk.join(", ")})` : pk[0]}${ck.map((c) => `, ${quoted(c.name)}`).join("")})`;
  const order = ck.length
    ? `CLUSTERING ORDER BY (${ck.map((c) => `${quoted(c.name)} ${c.desc ? "DESC" : "ASC"}`).join(", ")})\n    AND `
    : "";
  const options = `${order}comment = '${(t.comment ?? "").replaceAll("'", "''")}'\n    AND gc_grace_seconds = 864000`;
  if (t.view) {
    const cols = t.cols.map((c) => quoted(c.name)).join(", ");
    const notNull = primaryKeyOf(t)
      .map((k) => `${quoted(k)} IS NOT NULL`)
      .join(" AND ");
    return `CREATE MATERIALIZED VIEW ${qualified(t)} AS\n    SELECT ${cols}\n    FROM ${quoted(t.keyspace)}.${quoted(t.view.base)}\n    WHERE ${notNull}\n    ${key}\n    WITH ${options};`;
  }
  const cols = t.cols.map(
    (c) => `    ${quoted(c.name)} ${c.type}${c.role === "static" ? " static" : ""}`,
  );
  const out = [
    `CREATE TABLE ${qualified(t)} (\n${[...cols, `    ${key}`].join(",\n")}\n) WITH ${options};`,
  ];
  for (const i of indexesOf(t)) out.push(i.definition!);
  return out.join("\n\n");
}

/** `catalog::table_meta`. */
export function metaOf(t: Table): TableMeta {
  return {
    table: { schema: t.keyspace, name: t.name },
    kind: t.kind,
    columns: columnsOf(t),
    primaryKey: primaryKeyOf(t),
    indexes: indexesOf(t),
    foreignKeys: [],
    triggers: [],
    comment: t.comment,
    rowEstimate: t.estimate,
    ddl: ddlOf(t),
    checks: [],
    options: null,
  };
}

// --- the seed ------------------------------------------------------------------------------

/** A deterministic uuid from a number (version 4 shaped). */
function uuidOf(n: number, salt: number): string {
  const hex = (v: number, len: number) => (v >>> 0).toString(16).padStart(len, "0").slice(-len);
  const a = Math.imul(n + 1, 2654435761) ^ salt;
  const b = Math.imul(n + 7, 40503) ^ (salt >>> 3);
  return `${hex(a, 8)}-${hex(b, 4)}-4${hex(n, 3)}-a${hex(salt, 3)}-${hex(Math.imul(a, 31), 8)}${hex(b, 4)}`;
}

const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });
const NULL: Value = { t: "null" };

/** UTC text with milliseconds, as `values::timestamp_text`. */
export const tsText = (ms: number) =>
  new Date(ms).toISOString().replace("T", " ").replace("Z", "+0000");

const DAY = 86_400_000;
const BASE = Date.UTC(2026, 9, 1, 8, 0, 0);

const NAMES = [
  "Ada",
  "Grace",
  "Linus",
  "Margaret",
  "Ken",
  "Barbara",
  "Dennis",
  "Frances",
  "Edsger",
  "Radia",
  "Tim",
  "Hedy",
];

function shop(): Keyspace {
  const users: Table = {
    keyspace: "shop",
    name: "users",
    kind: "table",
    cols: [
      { name: "user_id", type: "uuid", role: "partition", position: 0 },
      { name: "created_at", type: "timestamp", role: "regular", position: -1 },
      { name: "email", type: "text", role: "regular", position: -1 },
      { name: "name", type: "text", role: "regular", position: -1 },
      { name: "prefs", type: "map<text, text>", role: "regular", position: -1 },
      { name: "tags", type: "set<text>", role: "regular", position: -1 },
    ],
    data: NAMES.map((name, i) => [
      { t: "uuid", v: uuidOf(i, 0x5a1) },
      { t: "timestampTz", v: tsText(BASE - (40 - i) * DAY + i * 3_600_123) },
      text(`${name.toLowerCase()}@example.com`),
      text(name),
      i % 3 === 0
        ? NULL
        : { t: "json", v: JSON.stringify({ lang: i % 2 ? "en" : "id", theme: "dark" }) },
      { t: "json", v: JSON.stringify(i % 2 ? ["vip"] : ["new", "newsletter"]) },
    ]),
    comment: "Customers by id",
    indexes: [{ name: "users_email_idx", column: "email", method: "secondary" }],
    estimate: NAMES.length,
  };
  const statuses = ["paid", "shipped", "delivered", "cancelled"];
  const orders: Value[][] = [];
  for (let c = 0; c < 5; c++)
    for (let k = 0; k < 9; k++) {
      const n = c * 9 + k;
      orders.push([
        users.data[c]![0]!,
        // Newest first within a partition (`CLUSTERING ORDER BY (order_time DESC)`).
        { t: "timestampTz", v: tsText(BASE - k * DAY - c * 61_000 + 117) },
        { t: "uuid", v: uuidOf(n, 0x0d0) },
        { t: "json", v: JSON.stringify([`sku-${100 + (n % 7)}`, `sku-${200 + (n % 3)}`]) },
        // A loyalty ledger id past 2^53 on every fifth order.
        n % 5 === 0 ? { t: "bigInt", v: String(9_007_199_254_740_993n + BigInt(n)) } : int(n),
        text(statuses[n % 4]!),
        { t: "decimal", v: (12.5 + n * 3.75).toFixed(2) },
      ]);
    }
  const ordersTable: Table = {
    keyspace: "shop",
    name: "orders_by_customer",
    kind: "table",
    cols: [
      { name: "customer_id", type: "uuid", role: "partition", position: 0 },
      { name: "order_time", type: "timestamp", role: "clustering", position: 0, desc: true },
      { name: "order_id", type: "timeuuid", role: "clustering", position: 1 },
      { name: "items", type: "list<text>", role: "regular", position: -1 },
      { name: "ledger_id", type: "bigint", role: "regular", position: -1 },
      { name: "status", type: "text", role: "regular", position: -1 },
      { name: "total", type: "decimal", role: "regular", position: -1 },
    ],
    data: orders,
    comment: "Orders of each customer, newest first",
    indexes: [{ name: "orders_status_sai", column: "status", method: "sai" }],
    estimate: 5,
  };
  const byStatus: Table = {
    keyspace: "shop",
    name: "orders_by_status",
    kind: "materializedView",
    cols: [
      { name: "status", type: "text", role: "partition", position: 0 },
      { name: "customer_id", type: "uuid", role: "clustering", position: 0 },
      { name: "order_time", type: "timestamp", role: "clustering", position: 1, desc: true },
      { name: "order_id", type: "timeuuid", role: "clustering", position: 2 },
      { name: "total", type: "decimal", role: "regular", position: -1 },
    ],
    data: [],
    comment: null,
    indexes: [],
    view: { base: "orders_by_customer", where: "status IS NOT NULL" },
    estimate: null,
  };
  const pageViews: Table = {
    keyspace: "shop",
    name: "page_views",
    kind: "table",
    cols: [
      { name: "page", type: "text", role: "partition", position: 0 },
      { name: "day", type: "date", role: "clustering", position: 0 },
      { name: "views", type: "counter", role: "regular", position: -1 },
    ],
    data: ["/", "/cart", "/checkout"].flatMap((page, p) =>
      [0, 1, 2].map((d) => [
        text(page),
        { t: "date", v: tsText(BASE - d * DAY).slice(0, 10) } as Value,
        int(1000 - p * 300 + d * 17),
      ]),
    ),
    comment: "Views per page and day (counters)",
    indexes: [],
    estimate: 3,
  };
  return {
    name: "shop",
    replication: "{'class': 'SimpleStrategy', 'replication_factor': '1'}",
    tables: [users, ordersTable, byStatus, pageViews],
  };
}

function iot(): Keyspace {
  const sensors = ["boiler-1", "boiler-2", "roof-west"];
  const data: Value[][] = [];
  for (const [s, sensor] of sensors.entries())
    for (let d = 0; d < 2; d++)
      for (let k = 0; k < 6; k++) {
        const ms = BASE - d * DAY + (5 - k) * 600_000;
        const bytes = new Uint8Array([s, d, k, 0xca, 0xfe]);
        data.push([
          text(sensor),
          { t: "date", v: tsText(BASE - d * DAY).slice(0, 10) },
          { t: "timestampTz", v: tsText(ms) },
          { t: "float", v: 0.5 + k / 20 },
          {
            t: "bytes",
            v: { base64: btoa(String.fromCharCode(...bytes)), len: 5, truncated: false },
          },
          { t: "float", v: Math.round((40 + 20 * s - k * 0.4 + d) * 100) / 100 },
        ]);
      }
  return {
    name: "iot",
    replication: "{'class': 'NetworkTopologyStrategy', 'dc1': '3'}",
    tables: [
      {
        keyspace: "iot",
        name: "readings",
        kind: "table",
        cols: [
          { name: "sensor_id", type: "text", role: "partition", position: 0 },
          { name: "day", type: "date", role: "partition", position: 1 },
          { name: "ts", type: "timestamp", role: "clustering", position: 0, desc: true },
          { name: "humidity", type: "float", role: "regular", position: -1 },
          { name: "raw", type: "blob", role: "regular", position: -1 },
          { name: "temperature", type: "double", role: "regular", position: -1 },
        ],
        data,
        comment: "Sensor readings, one partition per sensor and day",
        indexes: [],
        estimate: sensors.length * 2,
      },
    ],
  };
}

let cluster: Map<string, Keyspace> | null = null;

/** The cluster's keyspaces, seeded once per page (`resetCluster` seeds them again). */
export function clusterKeyspaces(): Map<string, Keyspace> {
  if (!cluster) {
    cluster = new Map([shop(), iot()].map((k) => [k.name, k]));
    for (const name of SYSTEM_KEYSPACES)
      cluster.set(name, { name, replication: "{'class': 'LocalStrategy'}", tables: [] });
  }
  return cluster;
}

/** Tests: a fresh cluster. */
export function resetCluster(): void {
  cluster = null;
}

/** The rows of a table, a view's computed from its base. */
export function rowsOf(ks: Keyspace, t: Table): Value[][] {
  if (!t.view) return t.data;
  const base = ks.tables.find((x) => x.name === t.view!.base);
  if (!base) return [];
  const at = (name: string) => base.cols.findIndex((c) => c.name === name);
  const picks = t.cols.map((c) => at(c.name));
  const rows = base.data.map((r) => picks.map((i) => r[i] ?? NULL));
  // Clustered by status, then by the base table's key.
  return rows.sort((a, b) => cmpText(a[0]!, b[0]!));
}

/** The text of a scalar value, for display and comparisons. */
export function display(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "";
    case "bytes":
      return `0x${[...atob(v.v.base64)].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("")}`;
    case "array":
      return JSON.stringify(v.v.map(display));
    case "other":
      return v.v.text;
    default:
      return String(v.v);
  }
}

function cmpText(a: Value, b: Value): number {
  const x = display(a);
  const y = display(b);
  return x < y ? -1 : x > y ? 1 : 0;
}

/** Orders two cells as CQL compares them: numbers by value, anything else by its text. */
export function compare(a: Value, b: Value): number {
  const num = (v: Value) =>
    v.t === "int" || v.t === "float"
      ? v.v
      : v.t === "bigInt" || v.t === "decimal"
        ? Number(v.v)
        : null;
  const x = num(a);
  const y = num(b);
  if (x !== null && y !== null) return x - y;
  return cmpText(a, b);
}

export const same = (a: Value, b: Value) => a.t !== "null" && b.t !== "null" && compare(a, b) === 0;
