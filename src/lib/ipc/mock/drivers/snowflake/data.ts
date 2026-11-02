import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ForeignKeyMeta } from "../../../bindings/ForeignKeyMeta";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { TableKind } from "../../../bindings/TableKind";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The mock's Snowflake account: the `TABLER_DEMO` database (a shop in `PUBLIC`, regions in
 * `SALES`, and `INFORMATION_SCHEMA`), an `ANALYTICS` database, the warehouses and roles the form
 * lists, and the user's queries Running Queries shows. Names are stored upper case, as unquoted
 * names fold; values follow `drivers/snowflake/decode.rs` (NUMBER(p,0) as `int` within 2^53 and
 * `bigInt` beyond, NUMBER(p,s) as `decimal` text, timestamps as text, VARIANT as `json`).
 */

export const NULL: Value = { t: "null" };

/** `CURRENT_VERSION()`. */
export const VERSION = "9.38.1";

export const ACCOUNT = "myorg-tabler";

/** A column as `SHOW COLUMNS` describes it. */
export interface Column {
  name: string;
  /** As Snowflake SQL spells it (`NUMBER(38,0)`, `TIMESTAMP_TZ(3)`). */
  type: string;
  kind: ValueKind;
  nullable: boolean;
  default?: string;
  autoincrement?: string;
  comment?: string;
}

export interface Table {
  database: string;
  schema: string;
  name: string;
  kind: TableKind;
  /** `TRANSIENT`, `TEMPORARY` or `TABLE` (SHOW TABLES' kind). */
  showKind: string;
  cols: Column[];
  /** The declared primary key (informational: Snowflake does not enforce it). */
  pk: string[];
  unique: { name: string; columns: string[] }[];
  foreignKeys: ForeignKeyMeta[];
  rows: Value[][];
  comment: string | null;
  /** A view's text (SHOW VIEWS' `text`). */
  text?: string;
  /** The rows of a view, computed from its tables. */
  computed?: (account: Account) => Value[][];
}

export interface Routine extends RoutineSummary {
  database: string;
  definition: string;
}

/** A query of `QUERY_HISTORY_BY_USER` still running. */
export interface RunningQuery {
  id: string;
  session: string;
  user: string;
  warehouse: string | null;
  status: string;
  start: string;
  text: string;
}

export interface Account {
  databases: { name: string; owner: string; comment: string | null }[];
  schemas: { database: string; name: string }[];
  tables: Table[];
  routines: Routine[];
  warehouses: string[];
  roles: string[];
  queries: RunningQuery[];
}

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const dec = (v: string): Value => ({ t: "decimal", v });

const COUNTRIES = ["ID", "SG", "US", "DE", "JP", "AU", "BR", "NL"];
const FIRST = ["Ayu", "Budi", "Citra", "Dewi", "Eko", "Farah", "Gilang", "Hana", "Indra", "Joko"];
const LAST = ["Santoso", "Wijaya", "Lim", "Müller", "Tanaka", "Smith", "Silva", "de Vries"];
const STATUSES = ["PENDING", "PAID", "SHIPPED", "DELIVERED", "CANCELLED"];

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

function customers(): Value[][] {
  return Array.from({ length: 60 }, (_, i) => {
    const id = i + 1;
    const first = FIRST[i % FIRST.length]!;
    const last = LAST[(i * 3) % LAST.length]!;
    const country = COUNTRIES[(i * 5) % COUNTRIES.length]!;
    return [
      int(id),
      text(`${first} ${last}`),
      i % 11 === 7 ? NULL : text(`${first.toLowerCase()}.${id}@example.com`),
      text(country),
      {
        t: "timestamp",
        v: `2026-${pad((i % 9) + 1)}-${pad((i % 27) + 1)} 0${i % 10}:15:00.000000000`,
      },
      i % 4 === 0
        ? NULL
        : {
            t: "json",
            v: JSON.stringify({
              tier: ["gold", "silver", "bronze"][i % 3],
              tags: [country.toLowerCase()],
            }),
          },
    ];
  });
}

function orders(): Value[][] {
  return Array.from({ length: 140 }, (_, i) => {
    const id = 1001 + i;
    const cents = ((i * 7919) % 50000) + 499;
    return [
      int(id),
      int((i % 60) + 1),
      dec(`${Math.floor(cents / 100)}.${pad(cents % 100)}`),
      text(STATUSES[(i * 3) % STATUSES.length]!),
      { t: "timestampTz", v: `2026-10-${pad((i % 30) + 1)} 1${i % 10}:0${i % 6}:00.000 +07:00` },
      // A tracking id past 2^53 travels as text.
      { t: "bigInt", v: String(9007199254740993n + BigInt(i) * 1000003n) },
    ];
  });
}

function events(): Value[][] {
  return Array.from({ length: 25 }, (_, i) => [
    text(`evt_${pad(i + 1, 4)}`),
    {
      t: "json",
      v: JSON.stringify({ type: ["page_view", "add_to_cart", "checkout"][i % 3], n: i }),
    },
    { t: "timestampTz", v: `2026-11-01 0${i % 10}:${pad(i * 2)}:00.000 +00:00` },
  ]);
}

const regions = (): Value[][] =>
  [
    ["APAC", "Asia Pacific", "Dewi Lim"],
    ["EMEA", "Europe, Middle East and Africa", "Anna Müller"],
    ["AMER", "Americas", "Sam Silva"],
  ].map((r) => r.map(text));

function dailyRevenue(): Value[][] {
  return Array.from({ length: 31 }, (_, i) => [
    { t: "date", v: `2026-10-${pad(i + 1)}` },
    dec(`${(12000 + ((i * 3571) % 9000)).toString()}.${pad((i * 37) % 100)}`),
    int(30 + ((i * 13) % 40)),
  ]);
}

const col = (
  name: string,
  type: string,
  kind: ValueKind,
  nullable = true,
  extra: Partial<Column> = {},
): Column => ({ name, type, kind, nullable, ...extra });

/** The account as seeded. */
export function seedAccount(): Account {
  const tables: Table[] = [
    {
      database: "TABLER_DEMO",
      schema: "PUBLIC",
      name: "CUSTOMERS",
      kind: "table",
      showKind: "TABLE",
      cols: [
        col("ID", "NUMBER(38,0)", "int", false, { autoincrement: "IDENTITY START 1 INCREMENT 1" }),
        col("NAME", "VARCHAR(200)", "text", false),
        col("EMAIL", "VARCHAR(320)", "text"),
        col("COUNTRY", "VARCHAR(2)", "text", true, { default: "'ID'" }),
        col("SIGNED_UP", "TIMESTAMP_NTZ(9)", "timestamp", true, {
          default: "CURRENT_TIMESTAMP()",
        }),
        col("PROFILE", "VARIANT", "json"),
      ],
      pk: ["ID"],
      unique: [{ name: "UQ_CUSTOMERS_EMAIL", columns: ["EMAIL"] }],
      foreignKeys: [],
      rows: customers(),
      comment: "Shop customers",
    },
    {
      database: "TABLER_DEMO",
      schema: "PUBLIC",
      name: "ORDERS",
      kind: "table",
      showKind: "TABLE",
      cols: [
        col("ORDER_ID", "NUMBER(38,0)", "int", false),
        col("CUSTOMER_ID", "NUMBER(38,0)", "int", false),
        col("AMOUNT", "NUMBER(12,2)", "decimal", false),
        col("STATUS", "VARCHAR(16)", "text", false, { default: "'PENDING'" }),
        col("ORDERED_AT", "TIMESTAMP_TZ(3)", "timestampTz"),
        col("TRACKING_ID", "NUMBER(38,0)", "int"),
      ],
      pk: ["ORDER_ID"],
      unique: [],
      foreignKeys: [
        {
          name: "FK_ORDERS_CUSTOMER",
          columns: ["CUSTOMER_ID"],
          refTable: { schema: "PUBLIC", name: "CUSTOMERS" },
          refColumns: ["ID"],
          onUpdate: "NO ACTION",
          onDelete: "NO ACTION",
          definition: null,
        },
      ],
      rows: orders(),
      comment: null,
    },
    {
      database: "TABLER_DEMO",
      schema: "PUBLIC",
      name: "EVENTS_RAW",
      kind: "table",
      showKind: "TRANSIENT",
      cols: [
        col("EVENT_ID", "VARCHAR(16777216)", "text"),
        col("PAYLOAD", "VARIANT", "json"),
        col("RECEIVED_AT", "TIMESTAMP_LTZ(9)", "timestampTz"),
      ],
      pk: [],
      unique: [],
      foreignKeys: [],
      rows: events(),
      comment: "Loaded by Snowpipe; no key, so read-only in the grid",
    },
    {
      database: "TABLER_DEMO",
      schema: "PUBLIC",
      name: "CUSTOMER_TOTALS",
      kind: "view",
      showKind: "VIEW",
      cols: [
        col("CUSTOMER_ID", "NUMBER(38,0)", "int"),
        col("ORDERS", "NUMBER(18,0)", "int"),
        col("TOTAL", "NUMBER(24,2)", "decimal"),
      ],
      pk: [],
      unique: [],
      foreignKeys: [],
      rows: [],
      comment: null,
      text: "create or replace view CUSTOMER_TOTALS as\nselect CUSTOMER_ID, count(*) as ORDERS, sum(AMOUNT) as TOTAL\nfrom ORDERS group by CUSTOMER_ID;",
      computed: (a) => {
        const o = a.tables.find((t) => t.name === "ORDERS" && t.schema === "PUBLIC");
        const by = new Map<number, { n: number; cents: number }>();
        for (const r of o?.rows ?? []) {
          const id = r[1]?.t === "int" ? r[1].v : 0;
          const amount = r[2]?.t === "decimal" ? Math.round(Number(r[2].v) * 100) : 0;
          const e = by.get(id) ?? { n: 0, cents: 0 };
          by.set(id, { n: e.n + 1, cents: e.cents + amount });
        }
        return [...by.entries()]
          .sort(([x], [y]) => x - y)
          .map(([id, e]) => [
            int(id),
            int(e.n),
            dec(`${Math.floor(e.cents / 100)}.${pad(e.cents % 100)}`),
          ]);
      },
    },
    {
      database: "TABLER_DEMO",
      schema: "SALES",
      name: "REGIONS",
      kind: "table",
      showKind: "TABLE",
      cols: [
        col("REGION_CODE", "VARCHAR(4)", "text", false),
        col("NAME", "VARCHAR(100)", "text", false),
        col("MANAGER", "VARCHAR(100)", "text"),
      ],
      pk: ["REGION_CODE"],
      unique: [],
      foreignKeys: [],
      rows: regions(),
      comment: null,
    },
    {
      database: "ANALYTICS",
      schema: "PUBLIC",
      name: "DAILY_REVENUE",
      kind: "table",
      showKind: "TABLE",
      cols: [
        col("DAY", "DATE", "date", false),
        col("REVENUE", "NUMBER(14,2)", "decimal"),
        col("ORDERS", "NUMBER(38,0)", "int"),
      ],
      pk: ["DAY"],
      unique: [],
      foreignKeys: [],
      rows: dailyRevenue(),
      comment: null,
    },
  ];
  return {
    databases: [
      { name: "ANALYTICS", owner: "SYSADMIN", comment: null },
      { name: "SNOWFLAKE", owner: "", comment: "Account usage (shared by Snowflake)" },
      { name: "TABLER_DEMO", owner: "SYSADMIN", comment: "Tabler's demo shop" },
    ],
    schemas: [
      { database: "ANALYTICS", name: "INFORMATION_SCHEMA" },
      { database: "ANALYTICS", name: "PUBLIC" },
      { database: "TABLER_DEMO", name: "INFORMATION_SCHEMA" },
      { database: "TABLER_DEMO", name: "PUBLIC" },
      { database: "TABLER_DEMO", name: "SALES" },
    ],
    tables,
    routines: [
      {
        database: "TABLER_DEMO",
        schema: "PUBLIC",
        name: "ORDER_TOTAL",
        kind: "function",
        arguments: "CUSTOMER NUMBER",
        identityArguments: "CUSTOMER NUMBER",
        returnType: "NUMBER(24,2)",
        language: "SQL",
        comment: null,
        definition:
          'CREATE OR REPLACE FUNCTION "ORDER_TOTAL"("CUSTOMER" NUMBER(38,0))\nRETURNS NUMBER(24,2)\nLANGUAGE SQL\nAS \'\n  select sum(AMOUNT) from ORDERS where CUSTOMER_ID = CUSTOMER\n\';',
      },
      {
        database: "TABLER_DEMO",
        schema: "PUBLIC",
        name: "ARCHIVE_ORDERS",
        kind: "procedure",
        arguments: "BEFORE DATE",
        identityArguments: "BEFORE DATE",
        returnType: "VARCHAR",
        language: "SQL",
        comment: "Moves old orders out",
        definition:
          "CREATE OR REPLACE PROCEDURE \"ARCHIVE_ORDERS\"(\"BEFORE\" DATE)\nRETURNS VARCHAR\nLANGUAGE SQL\nEXECUTE AS OWNER\nAS '\nBEGIN\n  DELETE FROM ORDERS WHERE ORDERED_AT < :BEFORE;\n  RETURN ''done'';\nEND\n';",
      },
    ],
    warehouses: ["COMPUTE_WH", "ETL_WH", "REPORTING_WH"],
    roles: ["ACCOUNTADMIN", "ANALYST", "PUBLIC", "SYSADMIN", "TABLER_ROLE"],
    queries: [
      {
        id: "01b8c3a2-0000-4a1b-0000-0001a2b3c4d5",
        session: "18436727381",
        user: "TABLER_USER",
        warehouse: "ETL_WH",
        status: "RUNNING",
        start: "2026-11-02 08:58:11.204 +0700",
        text: "INSERT INTO ANALYTICS.PUBLIC.DAILY_REVENUE SELECT DATE_TRUNC('day', ORDERED_AT), SUM(AMOUNT), COUNT(*) FROM TABLER_DEMO.PUBLIC.ORDERS GROUP BY 1",
      },
      {
        id: "01b8c3a2-0000-4a1b-0000-0001a2b3c9f0",
        session: "18436727512",
        user: "TABLER_USER",
        warehouse: "REPORTING_WH",
        status: "RESUMING_WAREHOUSE",
        start: "2026-11-02 09:01:40.877 +0700",
        text: "SELECT COUNTRY, COUNT(*) FROM TABLER_DEMO.PUBLIC.CUSTOMERS GROUP BY COUNTRY",
      },
    ],
  };
}

/** A table's columns as `catalog::columns` describes them. */
export function columnsOf(t: Table): ColumnMeta[] {
  return t.cols.map((c, i) => {
    const fixed = /^NUMBER\((\d+),(\d+)\)$/.exec(c.type);
    const sized = /^(?:VARCHAR|BINARY)\((\d+)\)$/.exec(c.type);
    const pkAt = t.pk.indexOf(c.name);
    return {
      name: c.name,
      ordinal: i + 1,
      dataType: c.type,
      kind: c.kind,
      nullable: c.nullable,
      defaultExpr: c.default ?? null,
      isPk: pkAt >= 0,
      pkOrdinal: pkAt >= 0 ? pkAt + 1 : null,
      isAutoIncrement: c.autoincrement !== undefined,
      isGenerated: false,
      isUnsigned: false,
      comment: c.comment ?? null,
      enumValues: null,
      charMaxLen: sized ? Number(sized[1]) : null,
      numericPrecision: fixed ? Number(fixed[1]) : null,
      numericScale: fixed ? Number(fixed[2]) : null,
      collation: null,
      arrayElemType: null,
      extra: c.autoincrement ?? null,
    };
  });
}

/** A table's rows: its own, or a view's computed ones. */
export const rowsOf = (a: Account, t: Table): Value[][] => (t.computed ? t.computed(a) : t.rows);

/** `GET_DDL('TABLE', …)` as Snowflake writes it. */
export function getDdl(t: Table): string {
  if (t.kind === "view") return t.text ?? "";
  const lines = t.cols.map((c) => {
    const parts = [`\t${c.name} ${c.type}`];
    if (c.autoincrement) parts.push("autoincrement start 1 increment 1 noorder");
    if (!c.nullable) parts.push("NOT NULL");
    if (c.default) parts.push(`DEFAULT ${c.default}`);
    if (c.comment) parts.push(`COMMENT '${c.comment.replaceAll("'", "\\'")}'`);
    return parts.join(" ");
  });
  if (t.pk.length > 0) lines.push(`\tprimary key (${t.pk.join(", ")})`);
  for (const u of t.unique) lines.push(`\tconstraint ${u.name} unique (${u.columns.join(", ")})`);
  for (const f of t.foreignKeys)
    lines.push(
      `\tconstraint ${f.name} foreign key (${f.columns.join(", ")}) references ${t.database}.${f.refTable.schema}.${f.refTable.name}(${f.refColumns.join(", ")})`,
    );
  const transient = t.showKind === "TRANSIENT" ? "TRANSIENT " : "";
  const comment = t.comment ? `\nCOMMENT='${t.comment.replaceAll("'", "\\'")}'` : "";
  return `create or replace ${transient}TABLE ${t.name} (\n${lines.join(",\n")}\n)${comment};`;
}

/** The bytes the metadata reports for a table (a guess from its rows). */
export const bytesOf = (t: Table): number => t.rows.length * t.cols.length * 24 + 1024;
