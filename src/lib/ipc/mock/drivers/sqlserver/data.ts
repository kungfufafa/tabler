import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { RoutineSummary } from "../../../bindings/RoutineSummary";
import type { RowKey } from "../../../bindings/RowKey";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";

/**
 * The databases of the mock's SQL Server: `shop` (customers with an identity key and money,
 * orders with `bigint` counters past 2^53 and `datetimeoffset`, `sales.invoices` with no primary
 * key and PDFs as `varbinary(max)`, a view, a procedure and a function) and `reporting` (one
 * table and a sequence's view), next to `master`. They live as long as the page, so edits stay
 * across connections as on a server. Values are what the backend decodes (`decode.rs`): `bigint`
 * past 2^53 as `bigInt`, money and decimals as `decimal` text, temporal values as text, GUIDs as
 * `uuid`.
 */

export interface Col {
  name: string;
  /** As `introspect::full_type` spells it: `nvarchar(100)`, `decimal(12,2)`, `datetime2(7)`. */
  type: string;
  kind: ValueKind;
  nullable: boolean;
  identity?: boolean;
  /** The default as `sys.default_constraints` keeps it, without its outer parentheses. */
  default?: string;
  unique?: string;
  /** A CHECK on the column: its name, expression and test. */
  check?: { name: string; expression: string; ok: (v: Value) => boolean };
  /** For `n…char(n)` / `…char(n)`: the length a value may have. */
  maxLen?: number;
  collation?: string;
}

export interface Table {
  schema: string;
  name: string;
  kind: "table" | "view";
  cols: Col[];
  pk: string[];
  /** The rows of a table (a view's are computed by `viewRows`). */
  data: Value[][];
  nextId: number;
  comment: string | null;
  indexes: TableMeta["indexes"];
  foreignKeys: TableMeta["foreignKeys"];
  triggers: TableMeta["triggers"];
  viewRows?: (db: Database) => Value[][];
  viewSql?: string;
}

export interface Routine extends RoutineSummary {
  definition: string | null;
}

export interface Database {
  name: string;
  owner: string;
  schemas: string[];
  tables: Table[];
  routines: Routine[];
}

const int = (v: number): Value => ({ t: "int", v });
const text = (v: string): Value => ({ t: "text", v });
const dec = (v: string): Value => ({ t: "decimal", v });
const NULL: Value = { t: "null" };

/** A column of the catalog, as `introspect::column` builds it. */
export function columnMeta(t: Table, c: Col, i: number): ColumnMeta {
  const pkAt = t.pk.indexOf(c.name);
  const numeric = /^(decimal|numeric)\((\d+),(\d+)\)$/.exec(c.type);
  const chars = /char\((\d+|max)\)$/.exec(c.type);
  return {
    name: c.name,
    ordinal: i + 1,
    dataType: c.type,
    kind: c.kind,
    nullable: c.nullable,
    defaultExpr: c.default ?? null,
    isPk: pkAt >= 0,
    pkOrdinal: pkAt >= 0 ? pkAt + 1 : null,
    isAutoIncrement: c.identity === true,
    isGenerated: false,
    isUnsigned: c.type === "tinyint",
    comment: null,
    enumValues: null,
    charMaxLen: chars ? (chars[1] === "max" ? null : Number(chars[1])) : null,
    numericPrecision: numeric ? Number(numeric[1]) : null,
    numericScale: numeric ? Number(numeric[2]) : null,
    collation: c.collation ?? (chars ? "SQL_Latin1_General_CP1_CI_AS" : null),
    arrayElemType: null,
    extra: c.identity ? "IDENTITY(1,1)" : null,
    baseType: null,
  };
}

export const columnsOf = (t: Table): ColumnMeta[] => t.cols.map((c, i) => columnMeta(t, c, i));

/** The rows of a table or a view. */
export const rowsOf = (db: Database, t: Table): Value[][] => (t.viewRows ? t.viewRows(db) : t.data);

/** A server error as `errors::server_error` reports it: the number in `vendorCode`. */
export function serverError(
  code: number,
  message: string,
  line = 1,
  statementIndex: number | null = null,
): ErrorPayload {
  return {
    code: "DB_ERROR",
    message,
    detail: `Msg ${code}, Level ${code === 3609 ? 16 : code < 1000 ? 15 : 16}, State 1, Line ${line}`,
    hint: null,
    sqlstate: null,
    vendorCode: code,
    position: null,
    statementIndex,
    data: null,
  };
}

/** What a failed statement throws inside the mock: a server error of its own. */
export class ServerFailure extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
  }
}

/** The backend's affected-rows guard (`drivers::check_affected`'s words). */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

/** A constraint a row breaks (2627 unique, 515 NULL, 547 check or reference, 8152 too long). */
export function constraintError(
  db: Database,
  t: Table,
  row: Value[],
  at: number,
): ServerFailure | null {
  const where = `${db.name}.${t.schema}.${t.name}`;
  for (const [i, c] of t.cols.entries()) {
    const v = row[i] ?? NULL;
    if (v.t === "null") {
      if (!c.nullable)
        return new ServerFailure(
          515,
          `Cannot insert the value NULL into column '${c.name}', table '${where}'; column does not allow nulls. ${at < 0 ? "INSERT" : "UPDATE"} fails.`,
        );
      continue;
    }
    if (c.maxLen !== undefined && v.t === "text" && v.v.length > c.maxLen)
      return new ServerFailure(
        2628,
        `String or binary data would be truncated in table '${where}', column '${c.name}'. Truncated value: '${v.v.slice(0, c.maxLen)}'.`,
      );
    if (c.check && !c.check.ok(v))
      return new ServerFailure(
        547,
        `The ${at < 0 ? "INSERT" : "UPDATE"} statement conflicted with the CHECK constraint "${c.check.name}". The conflict occurred in database "${db.name}", table "${t.schema}.${t.name}", column '${c.name}'.`,
      );
    const unique = c.unique;
    if (unique || t.pk.includes(c.name)) {
      const keyCols = unique ? [i] : t.pk.map((k) => t.cols.findIndex((x) => x.name === k));
      const clash = t.data.findIndex(
        (other, j) => j !== at && keyCols.every((k) => sameValue(other[k] ?? NULL, row[k] ?? NULL)),
      );
      if (clash >= 0) {
        const name = unique ?? `PK_${t.name}`;
        const kind = unique ? "UNIQUE KEY" : "PRIMARY KEY";
        const shown = keyCols.map((k) => display(row[k] ?? NULL)).join(", ");
        return new ServerFailure(
          2627,
          `Violation of ${kind} constraint '${name}'. Cannot insert duplicate key in object '${t.schema}.${t.name}'. The duplicate key value is (${shown}).`,
        );
      }
    }
  }
  for (const fk of t.foreignKeys) {
    const col = t.cols.findIndex((c) => c.name === fk.columns[0]);
    const v = row[col] ?? NULL;
    if (v.t === "null") continue;
    const ref = db.tables.find(
      (x) => x.schema === fk.refTable.schema && x.name === fk.refTable.name,
    );
    const refCol = ref?.cols.findIndex((c) => c.name === fk.refColumns[0]) ?? -1;
    if (ref && !ref.data.some((r) => sameValue(r[refCol] ?? NULL, v)))
      return new ServerFailure(
        547,
        `The ${at < 0 ? "INSERT" : "UPDATE"} statement conflicted with the FOREIGN KEY constraint "${fk.name}". The conflict occurred in database "${db.name}", table "${fk.refTable.schema}.${fk.refTable.name}", column '${fk.refColumns[0]}'.`,
      );
  }
  return null;
}

/** A value as SQL Server shows it in a message. */
export function display(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "<NULL>";
    case "bool":
      return v.v ? "1" : "0";
    case "int":
    case "float":
      return String(v.v);
    case "bytes":
      return `0x${atob(v.v.base64)
        .split("")
        .map((c) => c.charCodeAt(0).toString(16).padStart(2, "0"))
        .join("")
        .toUpperCase()}`;
    case "other":
      return v.v.text;
    case "array":
      return JSON.stringify(v.v);
    default:
      return v.v;
  }
}

/** Whether two values are equal as SQL Server compares them (text without case). */
export function sameValue(a: Value, b: Value): boolean {
  if (a.t === "null" || b.t === "null") return false;
  const num = (v: Value) =>
    v.t === "int" || v.t === "float"
      ? v.v
      : v.t === "bigInt" || v.t === "decimal"
        ? Number(v.v)
        : null;
  const na = num(a);
  const nb = num(b);
  if (na !== null && nb !== null) {
    if (a.t === "bigInt" || b.t === "bigInt") return display(a) === display(b);
    return na === nb;
  }
  return display(a).toLowerCase() === display(b).toLowerCase();
}

/** The row a key addresses (-1: none). A table without a primary key has no key to edit by. */
export function findRow(t: Table, key: RowKey): number {
  if (key.kind !== "pk") return -1;
  return t.data.findIndex((row) =>
    Object.entries(key.value).every(([k, v]) => {
      const i = t.cols.findIndex((c) => c.name === k);
      return i >= 0 && sameValue(row[i] ?? NULL, v);
    }),
  );
}

/** A value of a column from a typed value: ints and decimals as the column's kind. */
export function coerce(c: Col, v: Value): Value {
  if (v.t === "null" || v.t === "default") return v;
  const raw = display(v);
  switch (c.kind) {
    case "int":
      return /^-?\d+$/.test(raw) ? int(Number(raw)) : v;
    case "bigInt": {
      if (!/^-?\d+$/.test(raw)) return v;
      const n = BigInt(raw);
      return n >= BigInt(Number.MIN_SAFE_INTEGER) && n <= BigInt(Number.MAX_SAFE_INTEGER)
        ? int(Number(n))
        : { t: "bigInt", v: n.toString() };
    }
    case "decimal": {
      const m = /^-?\d+(\.\d+)?$/.exec(raw);
      const scale = /\(\d+,(\d+)\)$/.exec(c.type)?.[1] ?? (c.type.includes("money") ? "4" : "0");
      return m ? dec(Number(raw).toFixed(Number(scale))) : v;
    }
    case "bool":
      return { t: "bool", v: raw === "1" || raw.toLowerCase() === "true" };
    case "uuid":
      return { t: "uuid", v: raw.toUpperCase() };
    case "text":
      return text(raw);
    case "date":
    case "time":
    case "timestamp":
    case "timestampTz":
      return v.t === "text" ? { t: c.kind, v: raw } : v;
    default:
      return v;
  }
}

// --- the seed ------------------------------------------------------------------------------

const FIRST = [
  "Ada",
  "Grace",
  "Linus",
  "Margaret",
  "Alan",
  "Barbara",
  "Edsger",
  "Frances",
  "Ken",
  "Radia",
];
const LAST = [
  "Lovelace",
  "Hopper",
  "Torvalds",
  "Hamilton",
  "Turing",
  "Liskov",
  "Dijkstra",
  "Allen",
  "Thompson",
  "Perlman",
];
const STATUSES = ["new", "paid", "shipped", "cancelled"];

const guid = (i: number) =>
  `6F9619FF-8B86-D011-B42D-${(0xc04fc964ff + i).toString(16).toUpperCase().padStart(12, "0")}`;

const pad = (n: number) => String(n).padStart(2, "0");

/** A `datetime2(7)` as the backend writes it. */
const stamp = (day: number, hour: number) =>
  `2026-${pad(1 + Math.floor(day / 28))}-${pad(1 + (day % 28))} ${pad(hour)}:${pad((day * 7) % 60)}:00.0000000`;

/** A PDF of `len` bytes, whole (the session previews it as the backend does). */
function pdf(len: number): Value {
  const head = "%PDF-1.7\n%âãÏÓ\n1 0 obj";
  return { t: "bytes", v: { base64: btoa(head.padEnd(len, " ")), len, truncated: false } };
}

function shop(): Database {
  const customers: Table = {
    schema: "dbo",
    name: "customers",
    kind: "table",
    pk: ["id"],
    comment: "People who ordered at least once",
    cols: [
      { name: "id", type: "int", kind: "int", nullable: false, identity: true },
      { name: "name", type: "nvarchar(100)", kind: "text", nullable: false, maxLen: 100 },
      {
        name: "email",
        type: "nvarchar(255)",
        kind: "text",
        nullable: true,
        unique: "UQ_customers_email",
        maxLen: 255,
      },
      { name: "balance", type: "money", kind: "decimal", nullable: false, default: "0" },
      {
        name: "external_id",
        type: "uniqueidentifier",
        kind: "uuid",
        nullable: false,
        default: "newid()",
      },
      {
        name: "created_at",
        type: "datetime2(7)",
        kind: "timestamp",
        nullable: false,
        default: "sysdatetime()",
      },
    ],
    data: Array.from({ length: 30 }, (_, i) => [
      int(i + 1),
      text(`${FIRST[i % 10]} ${LAST[(i * 3) % 10]}`),
      i % 7 === 6 ? NULL : text(`${FIRST[i % 10]!.toLowerCase()}${i + 1}@example.com`),
      dec(((i * 1234.5) % 9000).toFixed(4)),
      { t: "uuid", v: guid(i) },
      { t: "timestamp", v: stamp(i, 9) },
    ]),
    nextId: 31,
    indexes: [
      {
        name: "PK_customers",
        columns: ["id"],
        unique: true,
        primary: true,
        method: "CLUSTERED",
        predicate: null,
        definition: null,
        comment: null,
      },
      {
        name: "UQ_customers_email",
        columns: ["email"],
        unique: true,
        primary: false,
        method: "NONCLUSTERED",
        predicate: null,
        definition: null,
        comment: null,
      },
    ],
    foreignKeys: [],
    triggers: [],
  };
  const orders: Table = {
    schema: "dbo",
    name: "orders",
    kind: "table",
    pk: ["id"],
    comment: null,
    cols: [
      { name: "id", type: "bigint", kind: "bigInt", nullable: false },
      { name: "customer_id", type: "int", kind: "int", nullable: false },
      { name: "total", type: "decimal(12,2)", kind: "decimal", nullable: false },
      {
        name: "status",
        type: "nvarchar(20)",
        kind: "text",
        nullable: false,
        default: "N'new'",
        maxLen: 20,
        check: {
          name: "CK_orders_status",
          expression:
            "[status]=N'cancelled' OR [status]=N'shipped' OR [status]=N'paid' OR [status]=N'new'",
          ok: (v) => STATUSES.includes(display(v)),
        },
      },
      { name: "placed_at", type: "datetimeoffset(7)", kind: "timestampTz", nullable: false },
      { name: "views", type: "bigint", kind: "bigInt", nullable: true },
      { name: "notes", type: "nvarchar(max)", kind: "text", nullable: true },
    ],
    data: Array.from({ length: 120 }, (_, i) => [
      int(1000 + i),
      int((i % 30) + 1),
      dec((((i * 37.25) % 500) + 5).toFixed(2)),
      text(STATUSES[i % 4]!),
      {
        t: "timestampTz",
        v: `${stamp(i % 300, 8 + (i % 10)).slice(0, 19)}.0000000 ${i % 3 === 0 ? "+07:00" : "+00:00"}`,
      },
      // A counter past 2^53 every tenth row, as `bigInt` text.
      i % 10 === 0 ? { t: "bigInt", v: (9007199254740993n + BigInt(i)).toString() } : int(i * 17),
      i % 5 === 0 ? text(`Gift wrap, order ${1000 + i}`) : NULL,
    ]),
    nextId: 1120,
    indexes: [
      {
        name: "PK_orders",
        columns: ["id"],
        unique: true,
        primary: true,
        method: "CLUSTERED",
        predicate: null,
        definition: null,
        comment: null,
      },
      {
        name: "IX_orders_customer",
        columns: ["customer_id"],
        unique: false,
        primary: false,
        method: "NONCLUSTERED",
        predicate: "([status]<>N'cancelled')",
        definition: null,
        comment: null,
        include: ["total"],
      },
    ],
    foreignKeys: [
      {
        name: "FK_orders_customers",
        columns: ["customer_id"],
        refTable: { schema: "dbo", name: "customers" },
        refColumns: ["id"],
        onUpdate: "NO ACTION",
        onDelete: "NO ACTION",
        definition: null,
      },
    ],
    triggers: [
      {
        name: "TR_orders_audit",
        timing: "AFTER",
        events: ["UPDATE"],
        enabled: true,
        definition:
          "CREATE TRIGGER [dbo].[TR_orders_audit] ON [dbo].[orders] AFTER UPDATE AS\nBEGIN\n  SET NOCOUNT ON;\n  INSERT INTO [audit].[order_changes] ([order_id], [changed_at]) SELECT [id], SYSDATETIME() FROM inserted;\nEND",
      },
    ],
  };
  const invoices: Table = {
    schema: "sales",
    name: "invoices",
    kind: "table",
    pk: [],
    comment: "Imported from the old ERP: no primary key",
    cols: [
      { name: "invoice_no", type: "varchar(20)", kind: "text", nullable: false, maxLen: 20 },
      { name: "order_id", type: "bigint", kind: "bigInt", nullable: true },
      { name: "amount", type: "decimal(12,2)", kind: "decimal", nullable: false },
      { name: "issued_on", type: "date", kind: "date", nullable: false },
      { name: "pdf", type: "varbinary(max)", kind: "bytes", nullable: true },
    ],
    data: Array.from({ length: 12 }, (_, i) => [
      text(`INV-2026-${String(i + 1).padStart(4, "0")}`),
      int(1000 + i * 3),
      dec((i * 41.5 + 10).toFixed(2)),
      { t: "date", v: `2026-${pad(1 + (i % 12))}-15` },
      i % 4 === 3 ? NULL : pdf(i === 0 ? 70_000 : 2048 + i * 100),
    ]),
    nextId: 0,
    indexes: [],
    foreignKeys: [],
    triggers: [],
  };
  const totals: Table = {
    schema: "dbo",
    name: "v_customer_totals",
    kind: "view",
    pk: [],
    comment: null,
    cols: [
      { name: "customer_id", type: "int", kind: "int", nullable: false },
      { name: "name", type: "nvarchar(100)", kind: "text", nullable: false },
      { name: "orders", type: "int", kind: "int", nullable: true },
      { name: "spent", type: "decimal(38,2)", kind: "decimal", nullable: true },
    ],
    data: [],
    nextId: 0,
    indexes: [],
    foreignKeys: [],
    triggers: [],
    viewSql:
      "CREATE VIEW [dbo].[v_customer_totals] AS\nSELECT c.id AS customer_id, c.name, COUNT(o.id) AS orders, SUM(o.total) AS spent\nFROM dbo.customers AS c\nLEFT JOIN dbo.orders AS o ON o.customer_id = c.id\nGROUP BY c.id, c.name",
    viewRows: (db) => {
      const c = db.tables.find((t) => t.name === "customers")!;
      const o = db.tables.find((t) => t.name === "orders")!;
      return c.data.map((row) => {
        const mine = o.data.filter((r) => sameValue(r[1]!, row[0]!));
        const spent = mine.reduce((s, r) => s + Number(display(r[2]!)), 0);
        return [row[0]!, row[1]!, int(mine.length), mine.length ? dec(spent.toFixed(2)) : NULL];
      });
    },
  };
  return {
    name: "shop",
    owner: "sa",
    schemas: ["dbo", "sales"],
    tables: [customers, orders, totals, invoices],
    routines: [
      {
        schema: "dbo",
        name: "usp_place_order",
        kind: "procedure",
        arguments: "@customer_id int, @total decimal(12,2), @order_id bigint OUTPUT",
        identityArguments: "",
        returnType: null,
        language: "SQL",
        comment: null,
        definition:
          "CREATE PROCEDURE [dbo].[usp_place_order]\n  @customer_id int,\n  @total decimal(12,2),\n  @order_id bigint OUTPUT\nAS\nBEGIN\n  SET NOCOUNT ON;\n  SELECT @order_id = NEXT VALUE FOR dbo.order_ids;\n  INSERT INTO dbo.orders (id, customer_id, total, placed_at)\n  VALUES (@order_id, @customer_id, @total, SYSDATETIMEOFFSET());\nEND",
      },
      {
        schema: "dbo",
        name: "fn_order_total",
        kind: "function",
        arguments: "@customer_id int",
        identityArguments: "",
        returnType: "decimal(38,2)",
        language: "SQL",
        comment: null,
        definition:
          "CREATE FUNCTION [dbo].[fn_order_total] (@customer_id int)\nRETURNS decimal(38,2)\nAS\nBEGIN\n  RETURN (SELECT SUM(total) FROM dbo.orders WHERE customer_id = @customer_id);\nEND",
      },
      {
        schema: "dbo",
        name: "usp_legacy_export",
        kind: "procedure",
        arguments: "",
        identityArguments: "",
        returnType: null,
        language: "SQL",
        comment: null,
        // Created WITH ENCRYPTION: the server keeps no text.
        definition: null,
      },
    ],
  };
}

function reporting(): Database {
  const daily: Table = {
    schema: "dbo",
    name: "daily_sales",
    kind: "table",
    pk: ["day"],
    comment: null,
    cols: [
      { name: "day", type: "date", kind: "date", nullable: false },
      { name: "orders", type: "int", kind: "int", nullable: false },
      { name: "revenue", type: "money", kind: "decimal", nullable: false },
    ],
    data: Array.from({ length: 60 }, (_, i) => [
      { t: "date", v: `2026-${pad(8 + Math.floor(i / 30))}-${pad(1 + (i % 30))}` },
      int(20 + ((i * 13) % 40)),
      dec((1500 + ((i * 271.3) % 3000)).toFixed(4)),
    ]),
    nextId: 0,
    indexes: [
      {
        name: "PK_daily_sales",
        columns: ["day"],
        unique: true,
        primary: true,
        method: "CLUSTERED",
        predicate: null,
        definition: null,
        comment: null,
      },
    ],
    foreignKeys: [],
    triggers: [],
  };
  return { name: "reporting", owner: "sa", schemas: ["dbo"], tables: [daily], routines: [] };
}

function master(): Database {
  return { name: "master", owner: "sa", schemas: ["dbo"], tables: [], routines: [] };
}

let databases = new Map<string, Database>();

/** The server's databases, built on first use. */
export function serverDatabases(): Map<string, Database> {
  if (databases.size === 0)
    databases = new Map([master(), reporting(), shop()].map((d) => [d.name, d]));
  return databases;
}

/** Tests: every database as seeded again. */
export function resetSqlServer(): void {
  databases = new Map();
}

/** A table of a database by schema and name (names compare without case, as the server's). */
export function tableIn(db: Database, schema: string | null, name: string): Table | null {
  const s = (schema ?? "dbo").toLowerCase();
  return (
    db.tables.find(
      (t) => t.schema.toLowerCase() === s && t.name.toLowerCase() === name.toLowerCase(),
    ) ?? null
  );
}

export { NULL, int, text };
