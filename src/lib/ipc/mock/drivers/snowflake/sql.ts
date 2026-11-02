import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import { sortRows } from "../../rows";
import { ACCOUNT, type Account, columnsOf, NULL, rowsOf, type Table, VERSION } from "./data";

/**
 * The mock's Snowflake SQL: enough of it for the editor to feel real. It runs `SELECT` over one
 * table (columns or `*`, `COUNT(*)`, one `WHERE column = literal`, `ORDER BY`, `LIMIT`), `SELECT`
 * of literals and of the context functions (`CURRENT_VERSION()`, `CURRENT_DATABASE()`…),
 * `SHOW DATABASES / SCHEMAS / TABLES / WAREHOUSES / ROLES`, `EXPLAIN` (a plan as `EXPLAIN USING
 * TEXT` prints it), `INSERT … VALUES`, `UPDATE … SET … [WHERE column = literal]` and `DELETE …
 * [WHERE column = literal]` on one table, which report their count (the `number of rows …`
 * row, which the backend turns into rows affected), `USE` and `ALTER SESSION`, and `BEGIN` /
 * `COMMIT` / `ROLLBACK`. A missing object fails as Snowflake does (002003, 42S02), anything else
 * it does not know as a syntax error (001003, 42000).
 */

/** A Snowflake error: its SQLSTATE and its own code. */
export class ServerFailure extends Error {
  constructor(
    readonly sqlstate: string,
    readonly vendor: number,
    message: string,
  ) {
    super(message);
  }
}

/** The error the backend reports for a Snowflake failure of statement `index` (`errors.rs`). */
export const serverError = (f: ServerFailure, index: number | null): ErrorPayload => ({
  code: "DB_ERROR",
  message: f.message,
  detail: null,
  hint: null,
  sqlstate: f.sqlstate,
  vendorCode: f.vendor,
  position: null,
  statementIndex: index,
  data: null,
});

export type Outcome =
  | { kind: "rows"; columns: ResultColumn[]; rows: Value[][] }
  | { kind: "affected"; affected: number }
  | { kind: "empty" };

/** Where a statement runs: the account and the session's state, which `USE` changes. */
export interface SqlContext {
  account: Account;
  user: string;
  database: string | null;
  schema: string | null;
  warehouse: string | null;
  role: string;
}

const NAME = String.raw`(?:"((?:[^"]|"")+)"|([A-Za-z_][\w$]*))`;
const QUALIFIED = String.raw`${NAME}(?:\s*\.\s*${NAME})?(?:\s*\.\s*${NAME})?`;

/** A name of `NAME`'s two groups: quoted as written, bare in upper case (Snowflake folds). */
const nameOf = (quoted: string | undefined, bare: string | undefined): string | null =>
  quoted !== undefined
    ? quoted.replaceAll('""', '"')
    : bare !== undefined
      ? bare.toUpperCase()
      : null;

const notFound = (what: string) =>
  new ServerFailure(
    "42S02",
    2003,
    `SQL compilation error:\nObject '${what}' does not exist or not authorized.`,
  );

/** A table by its qualified name's six groups at `at`. */
function tableOf(ctx: SqlContext, g: (string | undefined)[], at: number): Table {
  const parts = [0, 2, 4]
    .map((k) => nameOf(g[at + k], g[at + k + 1]))
    .filter((p): p is string => p !== null);
  const name = parts.at(-1)!;
  const schema = parts.length >= 2 ? parts.at(-2)! : ctx.schema;
  const database = parts.length === 3 ? parts[0]! : ctx.database;
  const t = ctx.account.tables.find(
    (x) => x.database === database && x.schema === schema && x.name === name,
  );
  if (!t) throw notFound(parts.join("."));
  return t;
}

/** A literal: a number, a string (backslash escapes and `''`), TRUE / FALSE or NULL. */
function literal(raw: string): Value {
  const t = raw.trim();
  if (/^null$/i.test(t)) return NULL;
  if (/^(true|false)$/i.test(t)) return { t: "bool", v: t.toLowerCase() === "true" };
  if (/^-?\d+$/.test(t)) return { t: "int", v: Number(t) };
  if (/^-?\d+\.\d+$/.test(t)) return { t: "decimal", v: t };
  const s = /^'((?:[^'\\]|''|\\.)*)'$/s.exec(t);
  if (s)
    return {
      t: "text",
      v: s[1]!
        .replaceAll("''", "'")
        .replace(/\\(.)/gs, (_, c: string) => (c === "n" ? "\n" : c === "t" ? "\t" : c)),
    };
  throw syntax(t);
}

const syntax = (near: string) =>
  new ServerFailure(
    "42000",
    1003,
    `SQL compilation error:\nsyntax error line 1 at position 0 unexpected '${near.split(/\s/)[0] ?? ""}'.`,
  );

/** The values of a comma-separated list, commas inside strings kept. */
function literals(list: string): Value[] {
  const parts = list.match(/'(?:[^'\\]|''|\\.)*'|[^,]+/gs) ?? [];
  return parts.map((p) => literal(p));
}

/** A cell's text as a comparison sees it. */
const cellText = (v: Value): string | null =>
  v.t === "null" || v.t === "default"
    ? null
    : typeof v.v === "object"
      ? JSON.stringify(v.v)
      : String(v.v);

/** The row indexes `WHERE col = literal` keeps (all of them without a condition). */
function where(t: Table, rows: Value[][], condition: string | undefined): number[] {
  const all = rows.map((_, i) => i);
  if (!condition) return all;
  const m = new RegExp(String.raw`^${NAME}\s*=\s*(.+)$`, "s").exec(condition.trim());
  if (!m) throw new ServerFailure("0A000", 2, "The mock compares one column with a literal");
  const column = nameOf(m[1], m[2])!;
  const at = t.cols.findIndex((c) => c.name === column);
  if (at < 0)
    throw new ServerFailure(
      "42000",
      904,
      `SQL compilation error: error line 1 at position 0\ninvalid identifier '${column}'`,
    );
  const want = cellText(literal(m[3]!));
  return all.filter((i) => cellText(rows[i]![at] ?? NULL) === want);
}

/** A value for a column: numbers from their text, NOT NULL checked by the caller. */
export function coerce(t: Table, column: string, v: Value): Value {
  const c = t.cols.find((x) => x.name === column);
  if (!c || v.t === "null") return v;
  const raw = cellText(v) ?? "";
  if (c.kind === "int" && /^-?\d+$/.test(raw))
    return Number.isSafeInteger(Number(raw))
      ? { t: "int", v: Number(raw) }
      : { t: "bigInt", v: raw };
  if (c.kind === "decimal" && /^-?\d+(\.\d+)?$/.test(raw)) {
    const scale = Number(/,(\d+)\)$/.exec(c.type)?.[1] ?? 0);
    return { t: "decimal", v: Number(raw).toFixed(scale) };
  }
  if (c.kind === "int" || c.kind === "decimal")
    throw new ServerFailure("22018", 100038, `Numeric value '${raw}' is not recognized`);
  if (c.kind === "bool") return { t: "bool", v: /^(true|t|1|yes)$/i.test(raw) };
  if (c.kind === "json") return { t: "json", v: raw };
  if (c.kind === "timestamp" || c.kind === "timestampTz" || c.kind === "date")
    return { t: c.kind, v: raw };
  return { t: "text", v: raw };
}

/** Why a row breaks the table's NOT NULL columns (the only constraint Snowflake enforces). */
export function rowProblem(t: Table, row: Value[]): ServerFailure | null {
  for (const [i, c] of t.cols.entries())
    if (!c.nullable && (row[i] ?? NULL).t === "null")
      return new ServerFailure("22000", 100072, `NULL result in a non-nullable column '${c.name}'`);
  return null;
}

const resultCol = (name: string, dataType: string, kind: ValueKind, table: string | null = null) =>
  ({ name, dataType, kind, nullable: true, table }) satisfies ResultColumn;

const textCols = (...names: string[]) => names.map((n) => resultCol(n, "VARCHAR", "text"));

/** One `number of rows …` row, as DML answers. */
const counted = (n: number): Outcome => ({ kind: "affected", affected: n });

/** The context functions of a `SELECT` without `FROM`. */
function contextValue(ctx: SqlContext, fn: string): Value | undefined {
  const s = (v: string | null): Value => (v === null ? NULL : { t: "text", v });
  switch (fn) {
    case "CURRENT_VERSION":
      return s(VERSION);
    case "CURRENT_ACCOUNT":
      return s(ACCOUNT.toUpperCase());
    case "CURRENT_REGION":
      return s("AWS_AP_SOUTHEAST_3");
    case "CURRENT_USER":
      return s(ctx.user);
    case "CURRENT_ROLE":
      return s(ctx.role);
    case "CURRENT_WAREHOUSE":
      return s(ctx.warehouse);
    case "CURRENT_DATABASE":
      return s(ctx.database);
    case "CURRENT_SCHEMA":
      return s(ctx.schema);
    default:
      return undefined;
  }
}

/** `SELECT <expressions>` without a table. */
function selectExpressions(ctx: SqlContext, list: string): Outcome {
  const parts = list.match(/'(?:[^'\\]|''|\\.)*'|[^,]+/gs) ?? [];
  const columns: ResultColumn[] = [];
  const row: Value[] = [];
  for (const raw of parts) {
    const p = raw.trim();
    const fn = /^([A-Za-z_]+)\s*\(\s*\)$/.exec(p)?.[1]?.toUpperCase();
    const v = fn ? contextValue(ctx, fn) : literal(p);
    if (v === undefined) throw notFound(fn ?? p);
    columns.push(
      resultCol(
        p.toUpperCase(),
        v.t === "int" ? "NUMBER(38,0)" : "VARCHAR",
        v.t === "int" ? "int" : "text",
      ),
    );
    row.push(v);
  }
  return { kind: "rows", columns, rows: [row] };
}

/** The `EXPLAIN USING TEXT` of a statement over one table. */
function explain(ctx: SqlContext, sql: string): Outcome {
  const m = new RegExp(String.raw`\bFROM\s+${QUALIFIED}`, "is").exec(sql);
  const t = m ? tableOf(ctx, [...m], 1) : null;
  const plan = t
    ? [
        "GlobalStats:",
        "    partitionsTotal=1",
        "    partitionsAssigned=1",
        `    bytesAssigned=${t.rows.length * 64}`,
        "Operations:",
        "1:0     ->Result  ",
        `1:1          ->TableScan  ${t.database}.${t.schema}.${t.name}  ${t.cols.map((c) => c.name).join(", ")}  {partitionsTotal=1, partitionsAssigned=1, bytesAssigned=${t.rows.length * 64}}`,
      ].join("\n")
    : "GlobalStats:\n    partitionsTotal=0\n    partitionsAssigned=0\n    bytesAssigned=0\nOperations:\n1:0     ->Result  ";
  return {
    kind: "rows",
    columns: [resultCol("content", "VARCHAR", "text")],
    rows: [[{ t: "text", v: plan }]],
  };
}

/** `SHOW …` as the session's catalog reads it (a few columns of each). */
function show(ctx: SqlContext, what: string): Outcome {
  const a = ctx.account;
  const rows = (cols: string[], values: (string | null)[][]): Outcome => ({
    kind: "rows",
    columns: textCols(...cols),
    rows: values.map((r) => r.map((v) => (v === null ? NULL : { t: "text", v }))),
  });
  switch (what) {
    case "DATABASES":
      return rows(
        ["name", "owner", "is_current", "comment"],
        a.databases.map((d) => [d.name, d.owner, d.name === ctx.database ? "Y" : "N", d.comment]),
      );
    case "SCHEMAS":
      return rows(
        ["name", "database_name", "is_current"],
        a.schemas
          .filter((s) => s.database === ctx.database)
          .map((s) => [s.name, s.database, s.name === ctx.schema ? "Y" : "N"]),
      );
    case "TABLES":
      return rows(
        ["name", "database_name", "schema_name", "kind", "rows", "comment"],
        a.tables
          .filter(
            (t) => t.database === ctx.database && t.schema === ctx.schema && t.kind === "table",
          )
          .map((t) => [t.name, t.database, t.schema, t.showKind, String(t.rows.length), t.comment]),
      );
    case "WAREHOUSES":
      return rows(
        ["name", "state", "size"],
        a.warehouses.map((w) => [w, w === ctx.warehouse ? "STARTED" : "SUSPENDED", "X-Small"]),
      );
    case "ROLES":
      return rows(
        ["name", "is_current"],
        a.roles.map((r) => [r, r === ctx.role ? "Y" : "N"]),
      );
    default:
      throw syntax(what);
  }
}

/** Runs one statement of the editor. */
export function runSql(ctx: SqlContext, sql: string): Outcome {
  const text = sql
    .trim()
    .replace(/;\s*$/, "")
    .replace(/^(--|\/\/)[^\n]*\n/gm, "")
    .trim();
  let m: RegExpExecArray | null;

  if (/^explain\b/i.test(text)) return explain(ctx, text);
  if ((m = /^show\s+(\w+)/i.exec(text))) return show(ctx, m[1]!.toUpperCase());

  if (
    (m = new RegExp(
      String.raw`^select\s+(.+?)\s+from\s+${QUALIFIED}(?:\s+where\s+(.+?))?(?:\s+order\s+by\s+${NAME}(?:\s+(asc|desc))?)?(?:\s+limit\s+(\d+))?$`,
      "is",
    ).exec(text))
  ) {
    const t = tableOf(ctx, [...m], 2);
    const all = rowsOf(ctx.account, t);
    const columns = columnsOf(t);
    const kept = where(t, all, m[8]).map((i) => all[i]!);
    if (/^count\s*\(\s*\*\s*\)$/i.test(m[1]!.trim()))
      return {
        kind: "rows",
        columns: [resultCol("COUNT(*)", "NUMBER(18,0)", "int")],
        rows: [[{ t: "int", v: kept.length }]],
      };
    const order = nameOf(m[9], m[10]);
    const sorted = order
      ? sortRows(
          kept,
          [{ column: order, dir: m[11]?.toLowerCase() === "desc" ? "desc" : "asc", nulls: null }],
          columns,
        )
      : kept;
    const limited = m[12] ? sorted.slice(0, Number(m[12])) : sorted;
    const list = m[1]!.trim();
    const picked =
      list === "*"
        ? t.cols.map((_, i) => i)
        : list.split(",").map((raw) => {
            const n = new RegExp(`^${NAME}$`).exec(raw.trim());
            const name = n ? nameOf(n[1], n[2]) : null;
            const at = t.cols.findIndex((c) => c.name === name);
            if (at < 0)
              throw new ServerFailure(
                "42000",
                904,
                `SQL compilation error: error line 1 at position 7\ninvalid identifier '${raw.trim()}'`,
              );
            return at;
          });
    return {
      kind: "rows",
      columns: picked.map((i) => {
        const c = t.cols[i]!;
        return { ...resultCol(c.name, c.type, c.kind, t.name), nullable: c.nullable };
      }),
      rows: limited.map((r) => picked.map((i) => r[i] ?? NULL)),
    };
  }
  if ((m = /^select\s+(.+)$/is.exec(text))) return selectExpressions(ctx, m[1]!);

  if (
    (m = new RegExp(
      String.raw`^insert\s+into\s+${QUALIFIED}\s*\(([^)]*)\)\s*values\s*(.+)$`,
      "is",
    ).exec(text))
  ) {
    const t = tableOf(ctx, [...m], 1);
    if (t.kind !== "table") throw notFound(t.name);
    const names = m[7]!.split(",").map((n) => {
      const x = new RegExp(`^${NAME}$`).exec(n.trim());
      return x ? nameOf(x[1], x[2])! : n.trim();
    });
    const tuples = m[8]!.match(/\((?:'(?:[^'\\]|''|\\.)*'|[^()'])*\)/gs) ?? [];
    const added = tuples.map((tuple) => {
      const values = literals(tuple.slice(1, -1));
      const row = t.cols.map((c) => {
        const at = names.indexOf(c.name);
        if (at >= 0) return coerce(t, c.name, values[at] ?? NULL);
        if (c.autoincrement) return { t: "int", v: nextId(t) } satisfies Value;
        return c.default ? defaultOf(t, c.name) : NULL;
      });
      const problem = rowProblem(t, row);
      if (problem) throw problem;
      return row;
    });
    t.rows.push(...added);
    return counted(added.length);
  }

  if (
    (m = new RegExp(
      String.raw`^update\s+${QUALIFIED}\s+set\s+(.+?)(?:\s+where\s+(.+))?$`,
      "is",
    ).exec(text))
  ) {
    const t = tableOf(ctx, [...m], 1);
    if (t.kind !== "table") throw notFound(t.name);
    const sets = (m[7]!.match(/(?:[^,']|'(?:[^'\\]|''|\\.)*')+/gs) ?? []).map((pair) => {
      const x = new RegExp(String.raw`^\s*${NAME}\s*=\s*(.+)$`, "s").exec(pair);
      if (!x) throw syntax(pair.trim());
      return [nameOf(x[1], x[2])!, literal(x[3]!)] as const;
    });
    const hit = where(t, t.rows, m[8]);
    for (const i of hit) {
      const row = t.rows[i]!.slice();
      for (const [name, v] of sets) {
        const at = t.cols.findIndex((c) => c.name === name);
        if (at < 0)
          throw new ServerFailure(
            "42000",
            904,
            `SQL compilation error:\ninvalid identifier '${name}'`,
          );
        row[at] = coerce(t, name, v);
      }
      const problem = rowProblem(t, row);
      if (problem) throw problem;
      t.rows[i] = row;
    }
    return counted(hit.length);
  }

  if (
    (m = new RegExp(String.raw`^delete\s+from\s+${QUALIFIED}(?:\s+where\s+(.+))?$`, "is").exec(
      text,
    ))
  ) {
    const t = tableOf(ctx, [...m], 1);
    if (t.kind !== "table") throw notFound(t.name);
    const hit = new Set(where(t, t.rows, m[7]));
    t.rows = t.rows.filter((_, i) => !hit.has(i));
    return counted(hit.size);
  }

  if (
    (m = new RegExp(
      String.raw`^use\s+(database|schema|warehouse|role)?\s*${QUALIFIED}$`,
      "is",
    ).exec(text))
  ) {
    const what = (m[1] ?? "database").toUpperCase();
    const parts = [2, 4, 6]
      .map((k) => nameOf(m![k], m![k + 1]))
      .filter((p): p is string => p !== null);
    if (what === "WAREHOUSE") {
      if (!ctx.account.warehouses.includes(parts[0]!)) throw notFound(parts[0]!);
      ctx.warehouse = parts[0]!;
    } else if (what === "ROLE") {
      if (!ctx.account.roles.includes(parts[0]!)) throw notFound(parts[0]!);
      ctx.role = parts[0]!;
    } else if (what === "SCHEMA") {
      const database = parts.length > 1 ? parts[0]! : ctx.database;
      const schema = parts.at(-1)!;
      if (!ctx.account.schemas.some((s) => s.database === database && s.name === schema))
        throw notFound(parts.join("."));
      ctx.database = database;
      ctx.schema = schema;
    } else {
      if (!ctx.account.databases.some((d) => d.name === parts[0])) throw notFound(parts[0]!);
      ctx.database = parts[0]!;
      ctx.schema = "PUBLIC";
    }
    return statusRow("Statement executed successfully.");
  }
  if (/^alter\s+session\s+(set|unset)\b/i.test(text))
    return statusRow("Statement executed successfully.");
  if (/^(begin|start)\b/i.test(text)) return statusRow("Statement executed successfully.");
  if (/^(commit|rollback)\b/i.test(text)) return statusRow("Statement executed successfully.");
  throw syntax(text);
}

const statusRow = (status: string): Outcome => ({
  kind: "rows",
  columns: textCols("status"),
  rows: [[{ t: "text", v: status }]],
});

function nextId(t: Table): number {
  const at = t.cols.findIndex((c) => c.autoincrement !== undefined);
  return t.rows.reduce((max, r) => Math.max(max, r[at]?.t === "int" ? r[at].v : 0), 0) + 1;
}

/** A column's default, as Snowflake fills it for an insert that leaves it out. */
export function defaultOf(t: Table, column: string): Value {
  const c = t.cols.find((x) => x.name === column)!;
  if (c.autoincrement) return { t: "int", v: nextId(t) };
  if (c.default === "CURRENT_TIMESTAMP()")
    return { t: "timestamp", v: "2026-11-02 09:00:00.000000000" };
  const quoted = /^'(.*)'$/.exec(c.default ?? "");
  return quoted ? { t: "text", v: quoted[1]! } : NULL;
}
