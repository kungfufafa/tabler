import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { TableKind } from "../../../bindings/TableKind";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The databases of the mock's ClickHouse server: `shop` (orders in a MergeTree keyed by id with
 * decimals, an enum, an array, a map and a MATERIALIZED column; customers in a ReplacingMergeTree;
 * a view, a materialized view, a dictionary and a keyless Log table), `analytics` (page views
 * keyed by `(site_id, ts, id)` with a UInt256 and an IPv4), an empty `default` and `system`. They live as long as the
 * page, so edits stay across connections as on a server. Values are what the backend decodes
 * (`decode.rs`): integers past 2^53 as `bigInt`, decimals and times as text, arrays of values,
 * maps and tuples as JSON, IPs as `other`.
 */

/** The four parts of a table's key a column can be used in (`dialect::clickhouse::KEY_ROLES`). */
export type KeyRole = "primary key" | "sorting key" | "partition key" | "sampling key";
export const KEY_ROLES: readonly KeyRole[] = [
  "primary key",
  "sorting key",
  "partition key",
  "sampling key",
];

export interface Col {
  name: string;
  /** The server's type text (`LowCardinality(String)`). */
  type: string;
  /** `system.columns.default_kind` and `default_expression`. */
  default?: { kind: "DEFAULT" | "MATERIALIZED" | "ALIAS"; expr: string };
  roles?: KeyRole[];
  codec?: string;
  comment?: string;
}

export interface Table {
  database: string;
  name: string;
  engine: string;
  /** `engine_full`: the engine with its clauses. */
  engineFull: string;
  partitionKey: string;
  sortingKey: string;
  primaryKey: string;
  samplingKey: string;
  cols: Col[];
  /** Rows in insertion order, cells by `cols`. */
  data: Value[][];
  comment: string;
  /** A view's SELECT (`as_select`). */
  asSelect?: string;
  skipping: { name: string; type: string; expr: string; granularity: number }[];
  projections: { name: string; query: string }[];
}

export interface Database {
  name: string;
  engine: string;
  tables: Table[];
}

/** Databases ClickHouse keeps for itself (`catalog::SYSTEM_DATABASES`). */
export const SYSTEM_DATABASES = ["system", "INFORMATION_SCHEMA", "information_schema"];

export const NULL: Value = { t: "null" };

// --- types -----------------------------------------------------------------------------------

/** `Nullable(T)` and `LowCardinality(T)` stripped, as `ChType::base`. */
export function baseType(type: string): string {
  let t = type.trim();
  for (;;) {
    const m = /^(?:Nullable|LowCardinality)\((.*)\)$/s.exec(t);
    if (!m) return t;
    t = m[1]!.trim();
  }
}

export const isNullable = (type: string): boolean =>
  /^(LowCardinality\()?Nullable\(/.test(type.trim());

/** `ChType::kind`. */
export function kindOf(type: string): ValueKind {
  const t = baseType(type);
  if (/^U?Int(8|16|32)$/.test(t)) return "int";
  if (/^U?Int(64|128|256)$/.test(t)) return "bigInt";
  if (/^(Float32|Float64|BFloat16)$/.test(t)) return "float";
  if (/^Decimal/.test(t)) return "decimal";
  if (t === "Bool") return "bool";
  if (t === "String" || t.startsWith("FixedString(")) return "text";
  if (t === "UUID") return "uuid";
  if (t === "Date" || t === "Date32") return "date";
  if (/^DateTime(64)?(\(|$)/.test(t)) return "timestamp";
  if (/^Time(64)?(\(|$)/.test(t)) return "time";
  if (/^Enum(8|16)\(/.test(t)) return "enum";
  if (t.startsWith("Array(")) return "array";
  if (/^(Tuple|Map|Nested|Variant)\(/.test(t) || t === "JSON" || t === "Dynamic") return "json";
  if (/^(Point|Ring|LineString|MultiLineString|Polygon|MultiPolygon)$/.test(t)) return "geometry";
  return "other";
}

/** The labels of `Enum8('a' = 1, …)`. */
export function enumLabels(type: string): string[] | null {
  const m = /^Enum(?:8|16)\((.*)\)$/s.exec(baseType(type));
  if (!m) return null;
  return [...m[1]!.matchAll(/'((?:[^'\\]|\\.)*)'\s*=/g)].map((x) => x[1]!.replace(/\\(.)/g, "$1"));
}

/** `(precision, scale)` of `Decimal(P, S)`, `DecimalN(S)`. */
export function decimalOf(type: string): [number, number] | null {
  const t = baseType(type);
  const full = /^Decimal\((\d+),\s*(\d+)\)$/.exec(t);
  if (full) return [Number(full[1]), Number(full[2])];
  const sized = /^Decimal(32|64|128|256)\((\d+)\)$/.exec(t);
  if (!sized) return null;
  const precision = { "32": 9, "64": 18, "128": 38, "256": 76 }[sized[1]!]!;
  return [precision, Number(sized[2])];
}

// --- values ----------------------------------------------------------------------------------

const SAFE = BigInt(Number.MAX_SAFE_INTEGER);

/** An integer as `decode.rs` gives one: `int` within 2^53, `bigInt` text past it. */
export function integer(n: bigint): Value {
  return n > SAFE || n < -SAFE ? { t: "bigInt", v: n.toString() } : { t: "int", v: Number(n) };
}

const ts = (s: string): Value => ({ t: "timestamp", v: s });
const text = (s: string): Value => ({ t: "text", v: s });
const dec = (s: string): Value => ({ t: "decimal", v: s });

/** A cell's text as the editor and filters see it. */
export function display(v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "float":
      return String(v.v);
    case "array":
      return `[${v.v.map(display).join(", ")}]`;
    case "bytes":
      return v.v.base64;
    case "other":
      return v.v.text;
    default:
      return v.v;
  }
}

// --- the server's data -------------------------------------------------------------------------

const STATUSES = ["new", "paid", "shipped", "delivered", "cancelled"] as const;
const CUSTOMERS = [
  ["Ada Lovelace", "ada@example.com", "GB"],
  ["Alan Turing", "alan@example.com", "GB"],
  ["Grace Hopper", "grace@example.com", "US"],
  ["Edsger Dijkstra", "edsger@example.com", "NL"],
  ["Barbara Liskov", "barbara@example.com", "US"],
  ["Linus Torvalds", "linus@example.com", "FI"],
  ["Margaret Hamilton", "margaret@example.com", "US"],
  ["Ken Thompson", "ken@example.com", "US"],
  ["Sophie Wilson", "sophie@example.com", "GB"],
  ["Tim Berners-Lee", "tim@example.com", "GB"],
  ["Hedy Lamarr", "hedy@example.com", "AT"],
  ["Niklaus Wirth", "niklaus@example.com", "CH"],
] as const;
const TAGS = ["gift", "express", "fragile", "bulk", "promo"];

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** A time `minutes` after 2026-10-01 00:00:00 (UTC), as ClickHouse prints a DateTime. */
function at(minutes: number): string {
  const d = new Date(Date.UTC(2026, 9, 1) + minutes * 60_000);
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}

const ORDER_COUNT = 240;

function orders(): Value[][] {
  return Array.from({ length: ORDER_COUNT }, (_, i) => {
    const id = i + 1;
    const cents = 1999 + ((id * 7919) % 48000);
    const tags = TAGS.filter((_, k) => (id + k) % 4 === 0);
    return [
      { t: "int", v: id },
      { t: "int", v: (id % CUSTOMERS.length) + 1 },
      { t: "enum", v: STATUSES[id % STATUSES.length]! },
      dec(`${Math.floor(cents / 100)}.${pad(cents % 100)}`),
      ts(at(id * 37)),
      { t: "array", v: tags.map(text) },
      { t: "json", v: JSON.stringify({ channel: id % 3 === 0 ? "app" : "web" }) },
      id % 5 === 0 ? text(`Leave at the door (${id})`) : NULL,
      { t: "date", v: at(id * 37).slice(0, 10) },
    ];
  });
}

function customers(): Value[][] {
  return CUSTOMERS.map(([name, email, country], i) => [
    { t: "int", v: i + 1 },
    text(name),
    text(email),
    text(country),
    { t: "date", v: `2025-${pad((i % 12) + 1)}-${pad((i % 27) + 1)}` },
    ts(at(-i * 600)),
  ]);
}

function pageViews(): Value[][] {
  return Array.from({ length: 60 }, (_, i) => [
    { t: "int", v: (i % 3) + 1 },
    { t: "timestamp", v: `${at(i * 11)}.${pad((i * 137) % 1000, 3)}` },
    integer(9_007_199_254_740_992n + BigInt(i)),
    text(["/", "/pricing", "/docs", "/blog/clickhouse"][i % 4]!),
    { t: "other", v: { ty: "IPv4", text: `10.0.${i % 4}.${(i * 7) % 250}` } },
    integer(2n ** 200n + BigInt(i)),
  ]);
}

const orderCols: Col[] = [
  { name: "id", type: "UInt64", roles: ["primary key", "sorting key"] },
  { name: "customer_id", type: "UInt32" },
  {
    name: "status",
    type: "Enum8('new' = 1, 'paid' = 2, 'shipped' = 3, 'delivered' = 4, 'cancelled' = 5)",
  },
  { name: "total", type: "Decimal(12, 2)", codec: "CODEC(ZSTD(1))" },
  {
    name: "created_at",
    type: "DateTime('UTC')",
    default: { kind: "DEFAULT", expr: "now()" },
    roles: ["partition key"],
  },
  { name: "tags", type: "Array(LowCardinality(String))" },
  { name: "attrs", type: "Map(String, String)" },
  { name: "note", type: "Nullable(String)", comment: "What the customer asked for" },
  {
    name: "created_day",
    type: "Date",
    default: { kind: "MATERIALIZED", expr: "toDate(created_at)" },
  },
];

const customerCols: Col[] = [
  { name: "id", type: "UInt32", roles: ["primary key", "sorting key"] },
  { name: "name", type: "String" },
  { name: "email", type: "String" },
  { name: "country", type: "FixedString(2)" },
  { name: "signed_up", type: "Date" },
  { name: "updated_at", type: "DateTime", default: { kind: "DEFAULT", expr: "now()" } },
];

const viewCols: Col[] = [
  { name: "customer_id", type: "UInt32" },
  { name: "orders", type: "UInt64" },
  { name: "revenue", type: "Decimal(38, 2)" },
];

/** What the materialized view keeps: orders and revenue per day. */
function dailySales(rows: Value[][]): Value[][] {
  const days = new Map<string, { orders: number; cents: number }>();
  for (const r of rows) {
    const day = display(r[8]!);
    const d = days.get(day) ?? { orders: 0, cents: 0 };
    d.orders++;
    d.cents += Math.round(Number(display(r[3]!)) * 100);
    days.set(day, d);
  }
  return [...days].map(([day, d]) => [
    { t: "date", v: day },
    { t: "int", v: d.orders },
    dec(`${Math.floor(d.cents / 100)}.${pad(d.cents % 100)}`),
  ]);
}

function shop(): Database {
  const orderRows = orders();
  const base = {
    database: "shop",
    partitionKey: "",
    sortingKey: "",
    primaryKey: "",
    samplingKey: "",
    comment: "",
    skipping: [],
    projections: [],
  };
  return {
    name: "shop",
    engine: "Atomic",
    tables: [
      {
        ...base,
        name: "orders",
        engine: "MergeTree",
        engineFull:
          "MergeTree PARTITION BY toYYYYMM(created_at) ORDER BY id SETTINGS index_granularity = 8192",
        partitionKey: "toYYYYMM(created_at)",
        sortingKey: "id",
        primaryKey: "id",
        cols: orderCols,
        data: orderRows,
        comment: "Orders of the web shop",
        skipping: [{ name: "idx_status", type: "set(5)", expr: "status", granularity: 4 }],
        projections: [
          { name: "by_customer", query: "SELECT customer_id, sum(total) GROUP BY customer_id" },
        ],
      },
      {
        ...base,
        name: "customers",
        engine: "ReplacingMergeTree",
        engineFull: "ReplacingMergeTree(updated_at) ORDER BY id SETTINGS index_granularity = 8192",
        sortingKey: "id",
        primaryKey: "id",
        cols: customerCols,
        data: customers(),
      },
      {
        ...base,
        name: "customer_revenue",
        engine: "View",
        engineFull: "",
        cols: viewCols,
        data: [],
        asSelect:
          "SELECT customer_id, count() AS orders, sum(total) AS revenue FROM shop.orders GROUP BY customer_id",
      },
      {
        ...base,
        name: "daily_sales_mv",
        engine: "MaterializedView",
        engineFull: "",
        cols: [
          { name: "day", type: "Date" },
          { name: "orders", type: "UInt64" },
          { name: "revenue", type: "Decimal(38, 2)" },
        ],
        data: dailySales(orderRows),
        asSelect:
          "SELECT toDate(created_at) AS day, count() AS orders, sum(total) AS revenue FROM shop.orders GROUP BY day",
      },
      {
        ...base,
        name: "countries",
        engine: "Dictionary",
        engineFull: "Dictionary(countries)",
        cols: [
          { name: "code", type: "String" },
          { name: "name", type: "String" },
        ],
        data: [
          ["AT", "Austria"],
          ["CH", "Switzerland"],
          ["FI", "Finland"],
          ["GB", "United Kingdom"],
          ["NL", "Netherlands"],
          ["US", "United States"],
        ].map((r) => r.map(text)),
        comment: "Country names from countries.csv",
      },
      {
        ...base,
        name: "import_log",
        engine: "Log",
        engineFull: "Log",
        cols: [
          { name: "at", type: "DateTime" },
          { name: "file", type: "String" },
          { name: "rows", type: "UInt64" },
        ],
        data: [
          [ts("2026-10-01 06:00:00"), text("orders-2026-09.csv"), { t: "int", v: 1830 }],
          [ts("2026-10-01 06:00:04"), text("customers.csv"), { t: "int", v: 12 }],
        ],
      },
    ],
  };
}

function analytics(): Database {
  return {
    name: "analytics",
    engine: "Atomic",
    tables: [
      {
        database: "analytics",
        name: "page_views",
        engine: "MergeTree",
        engineFull:
          "MergeTree PARTITION BY toDate(ts) ORDER BY (site_id, ts, id) SAMPLE BY id SETTINGS index_granularity = 8192",
        partitionKey: "toDate(ts)",
        sortingKey: "site_id, ts, id",
        primaryKey: "site_id, ts, id",
        samplingKey: "id",
        cols: [
          { name: "site_id", type: "UInt16", roles: ["primary key", "sorting key"] },
          {
            name: "ts",
            type: "DateTime64(3, 'UTC')",
            roles: ["primary key", "sorting key", "partition key"],
          },
          {
            name: "id",
            type: "UInt64",
            roles: ["primary key", "sorting key", "sampling key"],
          },
          { name: "path", type: "LowCardinality(String)" },
          { name: "ip", type: "IPv4" },
          { name: "visitor_hash", type: "UInt256" },
        ],
        data: pageViews(),
        comment: "",
        skipping: [],
        projections: [],
      },
    ],
  };
}

function system(): Database {
  const sys = (name: string, cols: Col[], data: Value[][]): Table => ({
    database: "system",
    name,
    engine: `System${name[0]!.toUpperCase()}${name.slice(1)}`,
    engineFull: "",
    partitionKey: "",
    sortingKey: "",
    primaryKey: "",
    samplingKey: "",
    cols,
    data,
    comment: "",
    skipping: [],
    projections: [],
  });
  return {
    name: "system",
    engine: "Memory",
    tables: [
      sys("one", [{ name: "dummy", type: "UInt8" }], [[{ t: "int", v: 0 }]]),
      sys(
        "settings",
        [
          { name: "name", type: "String" },
          { name: "value", type: "String" },
          { name: "changed", type: "UInt8" },
        ],
        [
          [text("max_threads"), text("'auto(8)'"), { t: "int", v: 0 }],
          [text("max_execution_time"), text("0"), { t: "int", v: 0 }],
          [text("readonly"), text("0"), { t: "int", v: 0 }],
        ],
      ),
    ],
  };
}

/** The server's databases, as seeded; edits change them in place. */
let SERVER: Map<string, Database> | null = null;

export function serverDatabases(): Map<string, Database> {
  // `default` is every server's, empty here.
  const fresh = (): Database[] => [
    { name: "default", engine: "Atomic", tables: [] },
    shop(),
    analytics(),
    system(),
  ];
  if (!SERVER) SERVER = new Map(fresh().map((d) => [d.name, d]));
  return SERVER;
}

/** Tests: the server as seeded again. */
export function resetServer(): void {
  SERVER = null;
}

// --- structure (`catalog.rs`) ------------------------------------------------------------------

/** `catalog::table_kind`. */
export function tableKind(t: Table): TableKind {
  if (["View", "LiveView", "WindowView"].includes(t.engine)) return "view";
  if (t.engine === "MaterializedView") return "materializedView";
  if (t.engine.startsWith("System") || SYSTEM_DATABASES.includes(t.database)) return "systemView";
  if (["Dictionary", "MySQL", "PostgreSQL", "S3", "URL", "File", "Kafka"].includes(t.engine))
    return "foreignTable";
  return "table";
}

/** `catalog::parse_columns`, with the key's order set as `parse_table` sets it. */
export function columnsOf(t: Table): ColumnMeta[] {
  const pk = primaryKeyOf(t);
  return t.cols.map((c, i) => {
    const extra: string[] = [...(c.roles ?? [])];
    const kind = c.default?.kind;
    if (kind === "MATERIALIZED" || kind === "ALIAS") extra.push(kind);
    if (c.codec) extra.push(c.codec);
    const decimal = decimalOf(c.type);
    const fixed = /^FixedString\((\d+)\)$/.exec(baseType(c.type));
    const ordinal = pk.indexOf(c.name);
    return {
      name: c.name,
      ordinal: i + 1,
      dataType: c.type,
      kind: kindOf(c.type),
      nullable: isNullable(c.type),
      defaultExpr: c.default?.expr ?? null,
      isPk: ordinal >= 0,
      pkOrdinal: ordinal >= 0 ? ordinal + 1 : null,
      isAutoIncrement: false,
      isGenerated: kind === "MATERIALIZED" || kind === "ALIAS",
      isUnsigned: /^UInt/.test(baseType(c.type)),
      comment: c.comment ?? null,
      enumValues: enumLabels(c.type),
      charMaxLen: fixed ? Number(fixed[1]) : null,
      numericPrecision: decimal?.[0] ?? null,
      numericScale: decimal?.[1] ?? null,
      collation: null,
      arrayElemType: /^Array\((.*)\)$/.exec(c.type.trim())?.[1]?.trim() ?? null,
      extra: extra.length > 0 ? extra.join(", ") : null,
      baseType: null,
    };
  });
}

/** The primary key's plain columns in key order (`parse_table`). */
export function primaryKeyOf(t: Table): string[] {
  return t.primaryKey
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "" && t.cols.some((c) => c.name === p));
}

/** `catalog::parse_table`. */
export function metaOf(t: Table): TableMeta {
  const indexes: IndexMeta[] = [];
  const keyIndex = (name: string, expr: string, clause: string, primary: boolean) => {
    if (expr === "") return;
    indexes.push({
      name,
      columns: expr
        .split(",")
        .map((p) => p.trim())
        .filter((p) => /^[\w.]+$/.test(p)),
      unique: false,
      primary,
      method: clause.toLowerCase(),
      predicate: null,
      definition: `${clause} (${expr})`,
      comment: null,
      include: [],
    });
  };
  keyIndex("PRIMARY", t.primaryKey, "PRIMARY KEY", true);
  if (t.sortingKey !== t.primaryKey) keyIndex("ORDER BY", t.sortingKey, "ORDER BY", false);
  keyIndex("PARTITION BY", t.partitionKey, "PARTITION BY", false);
  keyIndex("SAMPLE BY", t.samplingKey, "SAMPLE BY", false);
  for (const s of t.skipping)
    indexes.push({
      name: s.name,
      columns: [s.expr],
      unique: false,
      primary: false,
      method: s.type,
      predicate: null,
      definition: `INDEX \`${s.name}\` ${s.expr} TYPE ${s.type} GRANULARITY ${s.granularity}`,
      comment: null,
      include: [],
    });
  for (const p of t.projections)
    indexes.push({
      name: p.name,
      columns: [],
      unique: false,
      primary: false,
      method: "projection",
      predicate: null,
      definition: `PROJECTION \`${p.name}\` (${p.query})`,
      comment: null,
      include: [],
    });
  return {
    table: { schema: t.database, name: t.name },
    kind: tableKind(t),
    columns: columnsOf(t),
    primaryKey: primaryKeyOf(t),
    indexes,
    foreignKeys: [],
    triggers: [],
    comment: t.comment === "" ? null : t.comment,
    rowEstimate: hasRows(t) ? t.data.length : null,
    ddl: createQuery(t),
    checks: [],
    options: {
      engine: t.engineFull || t.engine || null,
      charset: null,
      collation: null,
      autoIncrement: null,
      rowFormat: null,
      // In IPC only when set, as the backend sends it.
      ...(t.sortingKey === "" ? {} : { sortKey: t.sortingKey }),
    },
  };
}

/** Whether `system.tables.total_rows` has a number for the table (stored rows only). */
export const hasRows = (t: Table): boolean =>
  !["View", "MaterializedView", "Dictionary"].includes(t.engine) && !t.engine.startsWith("System");

/** `system.tables.create_table_query`, as the server keeps it. */
export function createQuery(t: Table): string {
  const name = `${t.database}.${t.name}`;
  if (t.engine === "View") return `CREATE VIEW ${name}\n(\n${colList(t)}\n)\nAS ${t.asSelect}`;
  if (t.engine === "MaterializedView")
    return `CREATE MATERIALIZED VIEW ${name} TO shop.daily_sales\n(\n${colList(t)}\n)\nAS ${t.asSelect}`;
  if (t.engine === "Dictionary")
    return `CREATE DICTIONARY ${name}\n(\n${colList(t)}\n)\nPRIMARY KEY code\nSOURCE(FILE(PATH '/var/lib/clickhouse/user_files/countries.csv' FORMAT 'CSVWithNames'))\nLIFETIME(MIN 0 MAX 3600)\nLAYOUT(COMPLEX_KEY_HASHED())`;
  const extra = [
    ...t.skipping.map(
      (s) => `    INDEX ${s.name} ${s.expr} TYPE ${s.type} GRANULARITY ${s.granularity}`,
    ),
    ...t.projections.map((p) => `    PROJECTION ${p.name}\n    (\n        ${p.query}\n    )`),
  ];
  const comment = t.comment === "" ? "" : `\nCOMMENT '${t.comment.replaceAll("'", "\\'")}'`;
  return `CREATE TABLE ${name}\n(\n${[colList(t), ...extra].join(",\n")}\n)\nENGINE = ${t.engineFull}${comment}`;
}

function colList(t: Table): string {
  return t.cols
    .map((c) => {
      const parts = [`    \`${c.name}\` ${c.type}`];
      if (c.default) parts.push(`${c.default.kind} ${c.default.expr}`);
      if (c.comment) parts.push(`COMMENT '${c.comment.replaceAll("'", "\\'")}'`);
      if (c.codec) parts.push(c.codec);
      return parts.join(" ");
    })
    .join(",\n");
}
