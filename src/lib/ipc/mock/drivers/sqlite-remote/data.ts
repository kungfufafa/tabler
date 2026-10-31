import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { RowKey } from "../../../bindings/RowKey";
import type { ServerExpr } from "../../../bindings/ServerExpr";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import { mockError } from "../../transport";

/**
 * The databases the mock's LibSQL and D1 servers hold: a shop (customers, orders, an `events`
 * table without a primary key, a view), with D1's `d1_migrations` table on D1. Each database lives
 * as long as the page, so edits stay across connections like on a server. Values are SQLite's
 * storage classes as the backend decodes them: integers beyond 2^53 as `bigInt`.
 */

export type Engine = "libsql" | "d1";

interface Col {
  name: string;
  type: "INTEGER" | "TEXT" | "REAL" | "JSON";
  pk?: boolean;
  notNull?: boolean;
  unique?: boolean;
  default?: string;
}

export interface Table {
  name: string;
  kind: "table" | "view";
  cols: Col[];
  /** The rows of a table, or the computed rows of a view. */
  rows: () => Value[][];
  /** The rowid of each stored row (parallel to `rows`). */
  rowids: number[];
  data: Value[][];
  nextRowid: number;
  ddl: string;
  indexes: TableMeta["indexes"];
  foreignKeys: TableMeta["foreignKeys"];
  triggers: TableMeta["triggers"];
  viewSql?: string;
}

const KIND: Record<Col["type"], ValueKind> = {
  INTEGER: "int",
  TEXT: "text",
  REAL: "float",
  JSON: "json",
};

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const NULL: Value = { t: "null" };

/** SQLite's own error, as both engines report it (LibSQL with the extended code). */
export function sqliteError(
  engine: Engine,
  message: string,
  extended: number,
  statementIndex: number | null = null,
): ErrorPayload {
  return {
    code: "DB_ERROR",
    message,
    detail: null,
    hint: null,
    sqlstate: null,
    // D1 names only the primary code (`: SQLITE_CONSTRAINT`).
    vendorCode: engine === "d1" ? extended & 0xff : extended,
    position: null,
    statementIndex,
    data: null,
  };
}

/** What the backend's affected-rows guard reports (`check_affected`'s words). */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

export const SQLITE_CONSTRAINT_NOTNULL = 1299;
export const SQLITE_CONSTRAINT_UNIQUE = 2067;

function columnMeta(c: Col, ordinal: number, pkOrdinal: number | null): ColumnMeta {
  return {
    name: c.name,
    ordinal,
    dataType: c.type,
    kind: KIND[c.type],
    nullable: !c.notNull && !c.pk,
    defaultExpr: c.default ?? null,
    isPk: c.pk === true,
    pkOrdinal,
    isAutoIncrement: c.pk === true && c.type === "INTEGER",
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
  };
}

export const columnsOf = (t: Table): ColumnMeta[] =>
  t.cols.map((c, i) => columnMeta(c, i + 1, c.pk ? 1 : null));

export const pkOf = (t: Table): string[] => t.cols.filter((c) => c.pk).map((c) => c.name);

const NAMES = ["Ada", "Grace", "Linus", "Barbara", "Ken", "Margaret", "Dennis", "Frances"];
const PLANS = ["free", "pro", "team"];
const STATUSES = ["new", "paid", "shipped", "refunded"];

const day = (i: number) =>
  `2026-${String((i % 9) + 1).padStart(2, "0")}-${String((i % 28) + 1).padStart(2, "0")}`;

function table(
  name: string,
  cols: Col[],
  data: Value[][],
  extra: Partial<Pick<Table, "indexes" | "foreignKeys" | "triggers">> = {},
): Table {
  const ddl = `CREATE TABLE "${name}" (\n${cols
    .map(
      (c) =>
        `  "${c.name}" ${c.type}${c.pk ? " PRIMARY KEY" : ""}${c.notNull ? " NOT NULL" : ""}${
          c.unique ? " UNIQUE" : ""
        }${c.default ? ` DEFAULT ${c.default}` : ""}`,
    )
    .join(",\n")}\n)`;
  const t: Table = {
    name,
    kind: "table",
    cols,
    data,
    rowids: data.map((_, i) => i + 1),
    nextRowid: data.length + 1,
    rows: () => t.data,
    ddl,
    indexes: extra.indexes ?? [],
    foreignKeys: extra.foreignKeys ?? [],
    triggers: extra.triggers ?? [],
  };
  return t;
}

function shop(engine: Engine): Map<string, Table> {
  const customers = table(
    "customers",
    [
      { name: "id", type: "INTEGER", pk: true },
      { name: "name", type: "TEXT", notNull: true },
      { name: "email", type: "TEXT", unique: true },
      { name: "plan", type: "TEXT", default: "'free'" },
      { name: "created_at", type: "TEXT", default: "CURRENT_TIMESTAMP" },
    ],
    Array.from({ length: 120 }, (_, i) => {
      const name = NAMES[i % NAMES.length]!;
      return [
        int(i + 1),
        text(`${name} ${String.fromCharCode(65 + (i % 26))}.`),
        text(`${name.toLowerCase()}${i + 1}@example.com`),
        text(PLANS[i % PLANS.length]!),
        text(`${day(i)} 10:${String(i % 60).padStart(2, "0")}:00`),
      ];
    }),
    {
      indexes: [
        {
          name: "sqlite_autoindex_customers_1",
          columns: ["email"],
          unique: true,
          primary: false,
          method: null,
          predicate: null,
          definition: null,
          comment: null,
        },
      ],
    },
  );
  const orders = table(
    "orders",
    [
      { name: "id", type: "INTEGER", pk: true },
      { name: "customer_id", type: "INTEGER", notNull: true },
      { name: "total", type: "REAL", notNull: true },
      { name: "status", type: "TEXT", default: "'new'" },
      { name: "ref", type: "INTEGER" },
      { name: "placed_at", type: "TEXT" },
      { name: "note", type: "TEXT" },
    ],
    Array.from({ length: 300 }, (_, i) => [
      int(i + 1),
      int(((i * 7) % 120) + 1),
      { t: "float", v: Math.round((((i * 37.5) % 480) + 9.99) * 100) / 100 },
      text(STATUSES[i % STATUSES.length]!),
      // Beyond 2^53 an integer travels as text, so it is never rounded.
      i % 50 === 0 ? { t: "bigInt", v: String(9007199254740993n + BigInt(i)) } : int(100000 + i),
      text(`${day(i)} 1${i % 10}:00:00`),
      i % 25 === 3 ? text("gift wrap") : NULL,
    ]),
    {
      indexes: [
        {
          name: "orders_customer",
          columns: ["customer_id"],
          unique: false,
          primary: false,
          method: null,
          predicate: null,
          definition: 'CREATE INDEX "orders_customer" ON "orders" ("customer_id")',
          comment: null,
        },
      ],
      foreignKeys: [
        {
          name: "fk_orders_0",
          columns: ["customer_id"],
          refTable: { schema: "main", name: "customers" },
          refColumns: ["id"],
          onUpdate: "NO ACTION",
          onDelete: "CASCADE",
          definition: null,
        },
      ],
      triggers: [
        {
          name: "orders_paid_note",
          timing: "AFTER",
          events: ["UPDATE"],
          enabled: true,
          definition:
            "CREATE TRIGGER orders_paid_note AFTER UPDATE OF status ON orders WHEN NEW.status = 'refunded' BEGIN UPDATE orders SET note = 'refunded' WHERE id = NEW.id; END",
        },
      ],
    },
  );
  const events = table(
    "events",
    [
      { name: "kind", type: "TEXT", notNull: true },
      { name: "payload", type: "JSON" },
      { name: "at", type: "TEXT" },
    ],
    Array.from({ length: 40 }, (_, i) => [
      text(i % 3 === 0 ? "signup" : i % 3 === 1 ? "login" : "checkout"),
      { t: "json", v: JSON.stringify({ customer: (i % 120) + 1, ok: i % 7 !== 0 }) },
      text(`${day(i)} 08:${String(i % 60).padStart(2, "0")}:00`),
    ]),
  );
  const bigSpenders: Table = {
    name: "big_spenders",
    kind: "view",
    cols: [
      { name: "customer_id", type: "INTEGER" },
      { name: "spent", type: "REAL" },
    ],
    data: [],
    rowids: [],
    nextRowid: 1,
    rows: () => {
      const spent = new Map<number, number>();
      for (const row of orders.data) {
        const c = row[1];
        const total = row[2];
        if (c?.t === "int" && total?.t === "float") spent.set(c.v, (spent.get(c.v) ?? 0) + total.v);
      }
      return [...spent]
        .filter(([, s]) => s > 1000)
        .sort(([a], [b]) => a - b)
        .map(([c, s]): Value[] => [int(c), { t: "float", v: Math.round(s * 100) / 100 }]);
    },
    viewSql:
      "SELECT customer_id, SUM(total) AS spent FROM orders GROUP BY customer_id HAVING spent > 1000",
    ddl: "CREATE VIEW big_spenders AS SELECT customer_id, SUM(total) AS spent FROM orders GROUP BY customer_id HAVING spent > 1000",
    indexes: [],
    foreignKeys: [],
    triggers: [],
  };
  const all = [customers, orders, events, bigSpenders];
  if (engine === "d1")
    all.push(
      table(
        "d1_migrations",
        [
          { name: "id", type: "INTEGER", pk: true },
          { name: "name", type: "TEXT", unique: true },
          { name: "applied_at", type: "TEXT", notNull: true, default: "CURRENT_TIMESTAMP" },
        ],
        ["0001_init.sql", "0002_orders.sql", "0003_events.sql"].map((name, i) => [
          int(i + 1),
          text(name),
          text(`2026-0${i + 1}-01 09:00:00`),
        ]),
      ),
    );
  return new Map(all.map((t) => [t.name, t]));
}

function analytics(): Map<string, Table> {
  const views = table(
    "page_views",
    [
      { name: "id", type: "INTEGER", pk: true },
      { name: "path", type: "TEXT", notNull: true },
      { name: "hits", type: "INTEGER", notNull: true, default: "0" },
    ],
    ["/", "/pricing", "/docs", "/blog"].map((path, i) => [
      int(i + 1),
      text(path),
      int(1000 - i * 90),
    ]),
  );
  return new Map([[views.name, views]]);
}

/** A D1 database of the mock's account. */
export interface D1Database {
  uuid: string;
  name: string;
  createdAt: string;
  fileSize: number;
}

/** The databases of every account the mock's D1 knows. */
export const D1_DATABASES: readonly D1Database[] = [
  {
    uuid: "5a0c1f9e-2b7d-4c51-9a3e-0d6f2e8b7c41",
    name: "shop",
    createdAt: "2026-03-02T10:12:00.000Z",
    fileSize: 1_232_896,
  },
  {
    uuid: "c3e2d4a1-8f60-4b1e-b7d9-52a0f6e1c9d8",
    name: "analytics",
    createdAt: "2026-05-18T07:40:00.000Z",
    fileSize: 98_304,
  },
];

const stores = new Map<string, Map<string, Table>>();

/** The tables of a database of the mock's servers, made on first use and kept for the page. */
export function database(engine: Engine, name: string): Map<string, Table> {
  const key = `${engine}:${name}`;
  let tables = stores.get(key);
  if (!tables) {
    tables = name === "analytics" ? analytics() : shop(engine);
    stores.set(key, tables);
  }
  return tables;
}

/** Tests only: every database back to its seed. */
export function resetDatabases(): void {
  stores.clear();
}

/** The row a key names: its index in the table's rows, or -1. */
export function findRow(t: Table, key: RowKey): number {
  if (key.kind === "rowid") return t.rowids.indexOf(key.value);
  const names = t.cols.map((c) => c.name);
  return t.data.findIndex((row) =>
    Object.entries(key.value).every(([name, v]) => {
      const at = names.indexOf(name);
      return at >= 0 && sameValue(row[at] ?? NULL, v);
    }),
  );
}

/** SQLite's comparison of two stored values (integers and reals compare by number). */
export function sameValue(a: Value, b: Value): boolean {
  const num = (v: Value): number | null =>
    v.t === "int" || v.t === "float" ? v.v : v.t === "bigInt" ? Number(v.v) : null;
  const x = num(a);
  const y = num(b);
  if (x !== null && y !== null) {
    if (a.t === "bigInt" || b.t === "bigInt") return BigInt(stringOf(a)) === BigInt(stringOf(b));
    return x === y;
  }
  return JSON.stringify(a) === JSON.stringify(b);
}

const stringOf = (v: Value): string =>
  v.t === "int" || v.t === "float" ? String(Math.trunc(v.v)) : "v" in v ? String(v.v) : "0";

/** A server expression as SQLite's clock gives it. */
export function serverExprValue(expr: ServerExpr): Value {
  const now = new Date().toISOString().replace("T", " ").slice(0, 19);
  if (expr.startsWith("currentDate")) return text(now.slice(0, 10));
  if (expr.startsWith("currentTime")) return text(now.slice(11));
  if (expr === "default") return NULL;
  return text(now);
}

/**
 * Checks a row against the table's constraints (NOT NULL, UNIQUE, the integer primary key);
 * null when it holds, else SQLite's message and extended code. `skip`: the row's own index.
 */
export function constraintError(
  t: Table,
  row: Value[],
  skip: number,
): { message: string; code: number } | null {
  for (const [i, c] of t.cols.entries()) {
    const v = row[i] ?? NULL;
    if (c.notNull && v.t === "null")
      return {
        message: `NOT NULL constraint failed: ${t.name}.${c.name}`,
        code: SQLITE_CONSTRAINT_NOTNULL,
      };
    if ((c.unique || c.pk) && v.t !== "null")
      for (const [j, other] of t.data.entries())
        if (j !== skip && sameValue(other[i] ?? NULL, v))
          return {
            message: `UNIQUE constraint failed: ${t.name}.${c.name}`,
            code: c.pk ? 1555 : SQLITE_CONSTRAINT_UNIQUE,
          };
  }
  return null;
}

/** A new row of `values` (by column), with the integer key and the defaults SQLite gives. */
export function newRow(t: Table, values: Record<string, Value>): Value[] {
  return t.cols.map((c) => {
    const v = values[c.name];
    if (v !== undefined && v.t !== "default") return v;
    if (c.pk && c.type === "INTEGER") {
      const max = t.data.reduce((m, r) => {
        const id = r[0];
        return id?.t === "int" ? Math.max(m, id.v) : m;
      }, 0);
      return int(max + 1);
    }
    if (c.default === "CURRENT_TIMESTAMP") return serverExprValue("currentTimestamp");
    if (c.default?.startsWith("'")) return text(c.default.slice(1, -1));
    if (c.default !== undefined && /^\d+$/.test(c.default)) return int(Number(c.default));
    return NULL;
  });
}

/** The table `name` of `tables`, or the error SQLite gives for a missing one. */
export function tableOf(tables: Map<string, Table>, name: string): Table {
  return tables.get(name) ?? mockError("DB_ERROR", `no such table: ${name}`);
}
