import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ForeignKeyMeta } from "../../../bindings/ForeignKeyMeta";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { TableKind } from "../../../bindings/TableKind";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The mock's Vertica cluster: one database, `VMart` (Vertica's sample, cut down), with the user
 * schemas `public`, `store` and `online_sales` and the system schemas `v_catalog`, `v_monitor`,
 * `v_internal`, `v_func` and `v_txtindex` (which the sidebar hides through the descriptor). Tables
 * have projections, shown as the Structure tab's indexes as the backend's `parse_projections`
 * shows them; `store.store_sales_fact` has no primary key, so its rows cannot be edited, and is
 * big enough to page through.
 */

export const NULL: Value = { t: "null" };
const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const dec = (v: string): Value => ({ t: "decimal", v });
const date = (v: string): Value => ({ t: "date", v });
const bool = (v: boolean): Value => ({ t: "bool", v });

export interface Col {
  name: string;
  /** As `v_catalog.columns.data_type` spells it. */
  type: string;
  kind: ValueKind;
  nullable: boolean;
  default?: string;
  identity?: boolean;
  comment?: string;
}

export interface Check {
  name: string;
  expression: string;
  columns: string[];
  /** A disabled constraint is kept but not enforced. */
  enabled: boolean;
}

export interface Table {
  schema: string;
  name: string;
  kind: TableKind;
  cols: Col[];
  pk: string[];
  rows: Value[][];
  comment: string | null;
  foreignKeys: ForeignKeyMeta[];
  checks: Check[];
  projections: IndexMeta[];
  viewSql?: string;
}

export interface Routine extends RoutineSummary {
  /** A SQL function's `RETURN` text or a procedure's EXPORT_OBJECTS text; null for a UDx. */
  definition: string | null;
}

export interface Cluster {
  database: string;
  owner: string;
  schemas: string[];
  tables: Table[];
  routines: Routine[];
}

/** `Vertica Analytic Database v24.1.0-0`, as `SELECT version()` answers. */
export const VERSION_TEXT = "Vertica Analytic Database v24.1.0-0";

/** The system schemas of the descriptor (`system_schemas`). */
export const SYSTEM_SCHEMAS = ["v_catalog", "v_func", "v_internal", "v_monitor", "v_txtindex"];

/** A column as the structure view and the grid get it (`catalog::parse_columns`). */
export function columnMeta(t: Table, c: Col, i: number): ColumnMeta {
  const pkOrdinal = t.pk.indexOf(c.name);
  const size = /\((\d+)(?:,(\d+))?\)$/.exec(c.type);
  return {
    name: c.name,
    ordinal: i + 1,
    dataType: c.type,
    kind: c.kind,
    nullable: c.nullable,
    defaultExpr: c.default ?? null,
    isPk: pkOrdinal >= 0,
    pkOrdinal: pkOrdinal >= 0 ? pkOrdinal + 1 : null,
    isAutoIncrement: c.identity === true,
    isGenerated: false,
    isUnsigned: false,
    comment: c.comment ?? null,
    enumValues: null,
    charMaxLen: c.kind === "text" && size ? Number(size[1]) : null,
    numericPrecision: c.kind === "decimal" && size ? Number(size[1]) : null,
    numericScale: c.kind === "decimal" && size?.[2] ? Number(size[2]) : null,
    collation: null,
    arrayElemType: null,
    extra: c.identity ? "identity" : null,
  };
}

export const columnsOf = (t: Table): ColumnMeta[] => t.cols.map((c, i) => columnMeta(t, c, i));

const projection = (name: string, columns: string[], method: string): IndexMeta => ({
  name,
  columns,
  unique: false,
  primary: false,
  method,
  predicate: null,
  definition: null,
  comment: null,
  include: [],
});

const fk = (
  name: string,
  columns: string[],
  schema: string,
  table: string,
  refColumns: string[],
): ForeignKeyMeta => ({
  name,
  columns,
  refTable: { schema, name: table },
  refColumns,
  // Vertica's foreign keys are informational: no actions.
  onUpdate: "NO ACTION",
  onDelete: "NO ACTION",
  definition: null,
});

const CITIES: [string, string][] = [
  ["Boston", "MA"],
  ["Denver", "CO"],
  ["Austin", "TX"],
  ["Seattle", "WA"],
  ["Portland", "OR"],
  ["Chicago", "IL"],
  ["Atlanta", "GA"],
  ["Phoenix", "AZ"],
];
const FIRST = ["Ada", "Grace", "Linus", "Ken", "Barbara", "Edsger", "Frances", "Donald", "Radia"];
const LAST = ["Lovelace", "Hopper", "Torvalds", "Thompson", "Liskov", "Dijkstra", "Allen", "Knuth"];
const PRODUCTS: [string, string, string][] = [
  ["Brand #1 butter", "Food", "Dairy"],
  ["Brand #2 bagels", "Food", "Bakery"],
  ["Brand #3 lamb", "Food", "Meat"],
  ["Brand #4 detergent", "Non-food", "Cleaning supplies"],
  ["Brand #5 vitamins", "Medical", "Pharmacy"],
  ["Brand #6 frozen pizza", "Food", "Frozen goods"],
  ["Brand #7 soap", "Non-food", "Health and beauty"],
  ["Brand #8 apples", "Food", "Produce"],
];
const TENDERS = ["Cash", "Credit", "Debit", "Check"];

function customers(): Value[][] {
  return Array.from({ length: 60 }, (_, i) => {
    const [city, state] = CITIES[i % CITIES.length]!;
    const company = i % 4 === 3;
    const name = company
      ? `${LAST[i % LAST.length]!} Systems`
      : `${FIRST[i % FIRST.length]!} ${LAST[(i * 3) % LAST.length]!}`;
    return [
      int(i + 1),
      text(company ? "Company" : "Individual"),
      text(name),
      company ? NULL : text(i % 2 === 0 ? "Female" : "Male"),
      text(city),
      text(state),
      int(20_000 + ((i * 7919) % 180_000)),
      date(
        `20${String(10 + (i % 15)).padStart(2, "0")}-${String((i % 12) + 1).padStart(2, "0")}-15`,
      ),
      i === 7
        ? text(
            "Long-standing customer. ".repeat(40) +
              "Prefers e-mail; asked to be called after 5 pm.",
          )
        : NULL,
    ];
  });
}

function products(): Value[][] {
  return PRODUCTS.flatMap(([description, department, category], i) =>
    [1, 2].map((version) => [
      int(i + 1),
      int(version),
      text(version === 1 ? description : `${description} (new pack)`),
      text(category),
      text(department),
      text(`SKU-${String(i * 10 + version).padStart(6, "0")}`),
      dec(((i + 1) * 2.25 + version * 0.5).toFixed(2)),
      bool(version === 1 && i % 3 === 0),
    ]),
  );
}

function dates(): Value[][] {
  const days = ["Wednesday", "Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday"];
  return Array.from({ length: 31 }, (_, i) => [
    int(i + 1),
    date(`2025-01-${String(i + 1).padStart(2, "0")}`),
    text(days[i % 7]!),
    text("January"),
    bool(i === 0 || i === 19),
  ]);
}

function stores(): Value[][] {
  return CITIES.map(([city, state], i) => [
    int(i + 1),
    text(`Store${i + 1}`),
    text(city),
    text(state),
    date(`201${i}-0${(i % 9) + 1}-01`),
  ]);
}

function sales(): Value[][] {
  return Array.from({ length: 250 }, (_, i) => [
    int((i % 31) + 1),
    int((i % PRODUCTS.length) + 1),
    int((i % 2) + 1),
    int((i % CITIES.length) + 1),
    int(((i * 13) % 60) + 1),
    int(100_000 + i),
    int((i % 9) + 1),
    int(((i * 37) % 400) + 5),
    {
      t: "time",
      v: `${String(8 + (i % 12)).padStart(2, "0")}:${String((i * 7) % 60).padStart(2, "0")}:00`,
    },
    text(TENDERS[i % TENDERS.length]!),
  ]);
}

const col = (
  name: string,
  type: string,
  kind: ValueKind,
  nullable = true,
  extra: Partial<Col> = {},
): Col => ({ name, type, kind, nullable, ...extra });

/** A new cluster, as `?mockDrivers=vertica` first shows it. */
export function seedCluster(): Cluster {
  const customer: Table = {
    schema: "public",
    name: "customer_dimension",
    kind: "table",
    cols: [
      col("customer_key", "int", "int", false, { comment: "Surrogate key" }),
      col("customer_type", "varchar(16)", "text", false),
      col("customer_name", "varchar(256)", "text", false),
      col("customer_gender", "varchar(8)", "text"),
      col("customer_city", "varchar(64)", "text"),
      col("customer_state", "char(2)", "text"),
      col("annual_income", "int", "int", true, { default: "0" }),
      col("customer_since", "date", "date"),
      col("notes", "long varchar(1048576)", "text"),
    ],
    pk: ["customer_key"],
    rows: customers(),
    comment: "One row per customer",
    foreignKeys: [],
    checks: [
      {
        name: "C_INCOME_CHECK",
        expression: "(customer_dimension.annual_income >= 0)",
        columns: ["annual_income"],
        enabled: true,
      },
    ],
    projections: [
      projection(
        "customer_dimension_super",
        ["customer_key", "customer_name", "customer_state"],
        "super projection, segmented by hash(customer_dimension.customer_key)",
      ),
    ],
  };
  const product: Table = {
    schema: "public",
    name: "product_dimension",
    kind: "table",
    cols: [
      col("product_key", "int", "int", false),
      col("product_version", "int", "int", false),
      col("product_description", "varchar(128)", "text"),
      col("category_description", "char(32)", "text"),
      col("department_description", "char(32)", "text"),
      col("sku_number", "char(32)", "text"),
      col("product_price", "numeric(10,2)", "decimal"),
      col("discontinued_flag", "boolean", "bool", true, { default: "false" }),
    ],
    pk: ["product_key", "product_version"],
    rows: products(),
    comment: null,
    foreignKeys: [],
    checks: [],
    projections: [
      projection(
        "product_dimension_super",
        ["product_key", "product_version"],
        "super projection, unsegmented",
      ),
    ],
  };
  const dateDim: Table = {
    schema: "public",
    name: "date_dimension",
    kind: "table",
    cols: [
      col("date_key", "int", "int", false),
      col("date", "date", "date"),
      col("day_of_week", "varchar(9)", "text"),
      col("calendar_month_name", "varchar(9)", "text"),
      col("holiday_indicator", "boolean", "bool"),
    ],
    pk: ["date_key"],
    rows: dates(),
    comment: null,
    foreignKeys: [],
    checks: [],
    projections: [
      projection("date_dimension_super", ["date_key"], "super projection, unsegmented"),
    ],
  };
  const summary: Table = {
    schema: "public",
    name: "customer_summary",
    kind: "view",
    cols: [
      col("customer_state", "char(2)", "text"),
      col("customers", "int", "int"),
      col("average_income", "float", "float"),
    ],
    pk: [],
    rows: [],
    comment: "Customers per state",
    foreignKeys: [],
    checks: [],
    projections: [],
    viewSql:
      "SELECT customer_dimension.customer_state, count(*) AS customers, avg(customer_dimension.annual_income) AS average_income FROM public.customer_dimension GROUP BY customer_dimension.customer_state",
  };
  const store: Table = {
    schema: "store",
    name: "store_dimension",
    kind: "table",
    cols: [
      col("store_key", "int", "int", false),
      col("store_name", "varchar(64)", "text"),
      col("store_city", "varchar(64)", "text"),
      col("store_state", "char(2)", "text"),
      col("opened", "date", "date"),
    ],
    pk: ["store_key"],
    rows: stores(),
    comment: null,
    foreignKeys: [],
    checks: [],
    projections: [
      projection("store_dimension_super", ["store_key"], "super projection, unsegmented"),
    ],
  };
  const fact: Table = {
    schema: "store",
    name: "store_sales_fact",
    kind: "table",
    cols: [
      col("date_key", "int", "int", false),
      col("product_key", "int", "int", false),
      col("product_version", "int", "int", false),
      col("store_key", "int", "int", false),
      col("customer_key", "int", "int", false),
      col("pos_transaction_number", "int", "int", false),
      col("sales_quantity", "int", "int"),
      col("sales_dollar_amount", "int", "int"),
      col("transaction_time", "time", "time"),
      col("tender_type", "varchar(8)", "text"),
    ],
    pk: [],
    rows: sales(),
    comment: "Point-of-sale transactions (no primary key: read only in the grid)",
    foreignKeys: [
      fk("fk_store_sales_date", ["date_key"], "public", "date_dimension", ["date_key"]),
      fk(
        "fk_store_sales_product",
        ["product_key", "product_version"],
        "public",
        "product_dimension",
        ["product_key", "product_version"],
      ),
      fk("fk_store_sales_store", ["store_key"], "store", "store_dimension", ["store_key"]),
      fk("fk_store_sales_customer", ["customer_key"], "public", "customer_dimension", [
        "customer_key",
      ]),
    ],
    checks: [],
    projections: [
      projection(
        "store_sales_fact_super",
        ["date_key", "store_key", "product_key", "pos_transaction_number"],
        "super projection, segmented by hash(store_sales_fact.date_key, store_sales_fact.product_key)",
      ),
      projection(
        "store_sales_fact_by_customer",
        ["customer_key", "date_key"],
        "projection, segmented by hash(store_sales_fact.customer_key)",
      ),
    ],
  };
  const clicks: Table = {
    schema: "online_sales",
    name: "clickstream_ext",
    kind: "foreignTable",
    cols: [
      col("event_time", "timestamptz", "timestampTz"),
      col("page", "varchar(256)", "text"),
      col("session_id", "uuid", "uuid"),
    ],
    pk: [],
    rows: [
      [
        { t: "timestampTz", v: "2025-01-03 09:14:02.118+00" },
        text("/products/42"),
        { t: "uuid", v: "8f14e45f-ceea-467a-9b36-5e8c6f6e4b1a" },
      ],
      [
        { t: "timestampTz", v: "2025-01-03 09:14:09.507+00" },
        text("/cart"),
        { t: "uuid", v: "8f14e45f-ceea-467a-9b36-5e8c6f6e4b1a" },
      ],
      [
        { t: "timestampTz", v: "2025-01-03 10:02:44.001+00" },
        text("/"),
        { t: "uuid", v: "c9f0f895-fb98-4b91-8f5f-1f2c7e1d5a3b" },
      ],
    ],
    comment: "External table over Parquet files in S3",
    foreignKeys: [],
    checks: [],
    projections: [],
  };
  return {
    database: "VMart",
    owner: "dbadmin",
    schemas: ["online_sales", "public", "store", ...SYSTEM_SCHEMAS].sort(),
    tables: [customer, product, dateDim, summary, store, fact, clicks],
    routines: [
      {
        schema: "public",
        name: "fiscal_quarter",
        kind: "function",
        arguments: "d Date",
        identityArguments: "d Date",
        returnType: "Integer",
        language: "SQL",
        comment: "The fiscal quarter of a date (the year starts in February)",
        definition: "RETURN (((date_part('month', d) + 10) % 12) // 3 + 1)::int",
      },
      {
        schema: "public",
        name: "geo_distance",
        kind: "function",
        arguments: "Float, Float, Float, Float",
        identityArguments: "Float, Float, Float, Float",
        returnType: "Float",
        language: "C++",
        comment: null,
        definition: null,
      },
      {
        schema: "store",
        name: "refresh_daily_totals",
        kind: "procedure",
        arguments: "day Date",
        identityArguments: "day Date",
        returnType: null,
        language: "PL/vSQL",
        comment: null,
        definition:
          "CREATE PROCEDURE store.refresh_daily_totals(day Date)\nLANGUAGE 'PL/vSQL'\nSECURITY INVOKER\nAS '\nBEGIN\n  DELETE FROM store.daily_totals WHERE sales_date = day;\n  INSERT INTO store.daily_totals SELECT day, sum(sales_dollar_amount) FROM store.store_sales_fact f JOIN public.date_dimension d USING (date_key) WHERE d.date = day;\nEND\n';",
      },
    ],
  };
}

/** Bytes a table's projections use, as `v_monitor.projection_storage` sums them. */
export const sizeOf = (t: Table): number =>
  t.projections.length * (65_536 + t.rows.length * 24 * t.cols.length);

/** `EXPORT_OBJECTS('', 'schema.table', false)` of a table or view. */
export function exportObjects(t: Table): string {
  const name = `${t.schema}.${t.name}`;
  if (t.kind === "view") return `CREATE VIEW ${name} AS\n ${t.viewSql ?? ""};`;
  const cols = t.cols.map((c) => {
    const parts = [`    ${c.name} ${c.type}`];
    if (c.default) parts.push(`DEFAULT ${c.default}`);
    if (!c.nullable) parts.push("NOT NULL");
    return parts.join(" ");
  });
  const constraints = [
    ...(t.pk.length ? [`    CONSTRAINT C_PRIMARY PRIMARY KEY (${t.pk.join(", ")}) DISABLED`] : []),
    ...t.checks.map(
      (c) => `    CONSTRAINT ${c.name} CHECK ${c.expression} ${c.enabled ? "ENABLED" : "DISABLED"}`,
    ),
  ];
  const out = [
    `CREATE ${t.kind === "foreignTable" ? "EXTERNAL " : ""}TABLE ${name}\n(\n${[...cols, ...constraints].join(",\n")}\n)${t.kind === "foreignTable" ? " AS COPY FROM 's3://vmart-clicks/*.parquet' PARQUET" : ""};`,
  ];
  for (const f of t.foreignKeys)
    out.push(
      `ALTER TABLE ${name} ADD CONSTRAINT ${f.name} FOREIGN KEY (${f.columns.join(", ")}) references ${f.refTable.schema}.${f.refTable.name} (${f.refColumns.join(", ")});`,
    );
  for (const p of t.projections)
    out.push(
      `CREATE PROJECTION ${t.schema}.${p.name}\n(\n${p.columns.map((c) => ` ${c}`).join(",\n")}\n)\nAS\n SELECT ${p.columns.join(", ")}\n FROM ${name}\n ORDER BY ${p.columns.join(", ")}\n${/segmented by (.*)$/.exec(p.method ?? "")?.[1] ? `SEGMENTED BY ${/segmented by (.*)$/.exec(p.method ?? "")![1]} ALL NODES` : "UNSEGMENTED ALL NODES"};`,
    );
  return out.join("\n\n");
}
