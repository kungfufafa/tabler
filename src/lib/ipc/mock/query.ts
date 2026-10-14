// Mock handlers for run_sql, cancel_query, explain, list_history, delete_history, clear_history,
// read_text_file and write_text_file. They run against the same fixture data as browse_table.
import { utf16ToChar } from "@/features/editor/offsets";
import { splitStatements } from "@/features/editor/scanner";
import type { ColumnMeta } from "../bindings/ColumnMeta";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { Driver } from "../bindings/Driver";
import type { ErrorPayload } from "../bindings/ErrorPayload";
import type { ExplainMode } from "../bindings/ExplainMode";
import type { ExplainResult } from "../bindings/ExplainResult";
import type { HistoryEntry } from "../bindings/HistoryEntry";
import type { HistoryFilter } from "../bindings/HistoryFilter";
import type { QueryEvent } from "../bindings/QueryEvent";
import type { ResultColumn } from "../bindings/ResultColumn";
import type { RunOptions } from "../bindings/RunOptions";
import type { StatementResult } from "../bindings/StatementResult";
import type { TableMeta } from "../bindings/TableMeta";
import type { Value } from "../bindings/Value";
import type { ValueKind } from "../bindings/ValueKind";
import { tableMetaFor, tablesFor } from "./fixtures";
import { rowsFor } from "./rows";
import { mockError, mockInvoke, registerMock } from "./transport";

const CHUNK = 500;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// --- shared state ----------------------------------------------------------------

interface Running {
  cancelled: boolean;
}
/** queryId → cancel flag of the statement batch / explain in flight. */
const running = new Map<string, Running>();

const SEED_FILES: Array<[string, string]> = [
  [
    "/Users/me/queries/sample.sql",
    "-- Recent posts with their authors\nSELECT p.id, p.title, u.name, p.published_at\nFROM posts p\nJOIN users u ON u.id = p.user_id\nORDER BY p.published_at DESC\nLIMIT 20;\n",
  ],
  [
    "/Users/me/queries/nightly.sql.gz",
    "-- A gzipped script (opened decompressed)\nSELECT count(*) FROM events_big;\n",
  ],
];
const files = new Map<string, string>(SEED_FILES);

let history: HistoryEntry[] = [];
let nextHistoryId = 1;

function seedHistory(): void {
  const day = 86_400_000;
  const at = (msAgo: number) => new Date(Date.now() - msAgo).toISOString();
  const base = {
    profileId: "p-local-pg",
    connectionName: "Local Postgres",
    database: "tabler_dev",
    affected: null,
    error: null,
    success: true,
    source: "editor",
  };
  history = [
    {
      ...base,
      id: 1,
      sql: "select * from users limit 50",
      startedAt: at(3 * day),
      durationMs: 4,
      rowCount: 50,
    },
    {
      ...base,
      id: 2,
      sql: "selec * from posts",
      startedAt: at(day + 3_600_000),
      durationMs: 1,
      rowCount: null,
      success: false,
      error: 'syntax error at or near "selec"',
    },
    {
      ...base,
      id: 3,
      sql: "update posts set views = views + 1 where id = 7",
      startedAt: at(day),
      durationMs: 3,
      rowCount: null,
      affected: 1,
    },
    {
      ...base,
      id: 4,
      sql: "select count(*) from events_big",
      startedAt: at(600_000),
      durationMs: 41,
      rowCount: 1,
    },
  ];
  nextHistoryId = history.length + 1;
}
seedHistory();

/** Test helper: back to the seeded history and files, nothing running. */
export function resetMockQueryState(opts: { emptyHistory?: boolean } = {}): void {
  running.clear();
  files.clear();
  for (const [k, v] of SEED_FILES) files.set(k, v);
  seedHistory();
  if (opts.emptyHistory) {
    history = [];
    nextHistoryId = 1;
  }
}

function record(
  info: ConnectionInfo,
  sql: string,
  source: "editor" | "explain",
  r: {
    durationMs: number;
    rowCount?: number | null;
    affected?: number | null;
    error?: string | null;
  },
): void {
  history.push({
    id: nextHistoryId++,
    profileId: info.profileId,
    connectionName: info.name,
    database: info.currentDatabase,
    sql,
    startedAt: new Date().toISOString(),
    durationMs: r.durationMs,
    rowCount: r.rowCount ?? null,
    affected: r.affected ?? null,
    success: !r.error,
    error: r.error ?? null,
    source,
  });
}

async function connectionInfo(connectionId: unknown): Promise<ConnectionInfo> {
  const list = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  return (
    list.find((c) => c.connectionId === connectionId) ?? mockError("NOT_CONNECTED", "not connected")
  );
}

// --- errors ------------------------------------------------------------------------

class SqlError extends Error {
  constructor(readonly payload: ErrorPayload) {
    super(payload.message);
  }
}

function payload(p: Partial<ErrorPayload> & { code: string; message: string }): ErrorPayload {
  return {
    detail: null,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: null,
    data: null,
    ...p,
  };
}

/** Position of UTF-16 index `at` in `text`, as the backend reports it (chars, 1-based line/col). */
function positionIn(text: string, at: number) {
  const before = text.slice(0, at);
  const lineStart = before.lastIndexOf("\n") + 1;
  return {
    offset: utf16ToChar(text, at),
    line: before.split("\n").length,
    column: utf16ToChar(before.slice(lineStart), at - lineStart) + 1,
  };
}

function syntaxError(driver: Driver, text: string, at: number, index: number): SqlError {
  const token = /^\S+/.exec(text.slice(at))?.[0]?.replace(/[;,)]+$/, "") ?? "";
  const position = positionIn(text, at);
  if (driver === "mysql")
    return new SqlError(
      payload({
        code: "DB_ERROR",
        message: `You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '${text.slice(at, at + 40).split("\n")[0]}' at line ${position.line}`,
        sqlstate: "42000",
        vendorCode: 1064,
        position,
        statementIndex: index,
      }),
    );
  return new SqlError(
    payload({
      code: "DB_ERROR",
      message:
        driver === "sqlite"
          ? `near "${token}": syntax error`
          : `syntax error at or near "${token}"`,
      sqlstate: driver === "sqlite" ? null : "42601",
      vendorCode: driver === "sqlite" ? 1 : null,
      position,
      statementIndex: index,
    }),
  );
}

function missingTable(
  driver: Driver,
  name: string,
  text: string,
  at: number,
  index: number,
  db: string,
) {
  const position = positionIn(text, at);
  if (driver === "mysql")
    return new SqlError(
      payload({
        code: "DB_ERROR",
        message: `Table '${db}.${name}' doesn't exist`,
        sqlstate: "42S02",
        vendorCode: 1146,
        position,
        statementIndex: index,
      }),
    );
  return new SqlError(
    payload({
      code: "DB_ERROR",
      message: driver === "sqlite" ? `no such table: ${name}` : `relation "${name}" does not exist`,
      sqlstate: driver === "sqlite" ? null : "42P01",
      vendorCode: driver === "sqlite" ? 1 : null,
      position,
      statementIndex: index,
    }),
  );
}

/** A statement past the query time out (the backend's `timeout_payload`). */
function timeoutError(secs: number, index: number): SqlError {
  return new SqlError(
    payload({
      code: "QUERY_TIMEOUT",
      message: `Query timed out after ${secs} s`,
      hint: "The statement was cancelled. The limit is Settings ▸ Connections ▸ Query time out (0 = no limit).",
      statementIndex: index,
    }),
  );
}

function cancelledError(driver: Driver, index: number): SqlError {
  return new SqlError(
    payload({
      code: "CANCELLED",
      message:
        driver === "postgres"
          ? "canceling statement due to user request"
          : driver === "mysql"
            ? "Query execution was interrupted"
            : "interrupted",
      sqlstate: driver === "postgres" ? "57014" : driver === "mysql" ? "70100" : null,
      vendorCode: driver === "mysql" ? 1317 : null,
      statementIndex: index,
    }),
  );
}

// --- tiny SQL "engine" -----------------------------------------------------------------

const KNOWN = new Set([
  "select",
  "with",
  "values",
  "table",
  "insert",
  "update",
  "delete",
  "create",
  "drop",
  "alter",
  "truncate",
  "set",
  "show",
  "begin",
  "start",
  "commit",
  "rollback",
  "savepoint",
  "release",
  "explain",
  "analyze",
  "vacuum",
  "use",
  "grant",
  "revoke",
  "comment",
  "call",
  "do",
  "refresh",
  "reindex",
  "lock",
  "pragma",
  "describe",
  "desc",
  "replace",
  "merge",
  "listen",
  "notify",
]);

interface Ctx {
  info: ConnectionInfo;
  driver: Driver;
  /** The whole submitted text (positions are relative to it). */
  text: string;
  run: Running;
  maxRows: number;
  /** When the running statement times out (`RunOptions.timeoutSecs`); null = never. */
  deadline: number | null;
  timeoutSecs: number;
  send: (e: QueryEvent) => void;
}

interface Exec {
  kind: StatementResult["kind"];
  columns: ResultColumn[];
  rowCount: number;
  resultSets: number;
  truncated: boolean;
  affected: number | null;
  lastInsertId: string | null;
  notices: string[];
}

const DATA_TYPES: Record<Driver, Partial<Record<ValueKind, string>>> = {
  postgres: {
    int: "int4",
    bigInt: "int8",
    decimal: "numeric",
    text: "text",
    bool: "bool",
    timestampTz: "timestamptz",
    float: "float8",
    date: "date",
  },
  mysql: {
    int: "BIGINT",
    bigInt: "BIGINT",
    decimal: "DECIMAL",
    text: "VARCHAR",
    bool: "TINYINT",
    timestampTz: "DATETIME",
    float: "DOUBLE",
    date: "DATE",
  },
  sqlite: {
    int: "INTEGER",
    bigInt: "INTEGER",
    decimal: "NUMERIC",
    text: "TEXT",
    bool: "INTEGER",
    timestampTz: "TEXT",
    float: "REAL",
    date: "TEXT",
  },
};

function resultColumn(
  driver: Driver,
  name: string,
  kind: ValueKind,
  table: string | null = null,
): ResultColumn {
  return { name, dataType: DATA_TYPES[driver][kind] ?? "text", kind, nullable: null, table };
}

/**
 * A column read straight from a table. With `source` it reports its origin (so the result can be
 * editable); MySQL also names the table reference it was read through (its alias).
 */
function fromMeta(
  c: ColumnMeta,
  table: string,
  schema: string,
  source: { driver: Driver; alias: string | null } | null,
): ResultColumn {
  const instance = source?.driver === "mysql" ? (source.alias ?? table) : null;
  return {
    name: c.name,
    dataType: c.dataType,
    kind: c.kind,
    nullable: c.nullable,
    table: instance ?? table,
    origin: source
      ? {
          table: { schema, name: table },
          column: c.name,
          ...(instance !== null && { instance }),
        }
      : null,
  };
}

const IDENT = String.raw`(?:"[^"]+"|\x60[^\x60]+\x60|\[[^\]]+\]|[\p{L}_][\p{L}\p{N}_$]*)`;
const FROM_RE = new RegExp(
  String.raw`\b(?:from|into|update|table)\s+(${IDENT}(?:\s*\.\s*${IDENT})?)`,
  "iu",
);

function unquote(ident: string): { name: string; quoted: boolean } {
  const t = ident.trim();
  const q = t[0];
  if ((q === '"' || q === "`" || q === "[") && t.length >= 2)
    return { name: t.slice(1, -1), quoted: true };
  return { name: t, quoted: false };
}

const NOT_ALIAS = new Set([
  "where",
  "group",
  "order",
  "limit",
  "having",
  "window",
  "offset",
  "fetch",
  "for",
  "natural",
  "left",
  "right",
  "inner",
  "cross",
  "full",
  "join",
  "on",
  "using",
  "union",
  "except",
  "intersect",
]);

/**
 * How a SELECT reads its table: `single` when it reads that one table once (no join, subquery,
 * set operation or second FROM item), the only case where the mock reports column origins, plus
 * the alias it gives the table. The real servers report origins for joins too; the renderer then
 * decides what is editable.
 */
function selectSource(sql: string): { single: boolean; alias: string | null } {
  if (/^\s*table\s+[^;]*;?\s*$/i.test(sql)) return { single: true, alias: null };
  const code = sql.replace(/'(?:[^']|'')*'/g, "''");
  const from = /\bfrom\s+([\s\S]*?)(?:\b(?:where|group|order|limit|having|window)\b|;|$)/i.exec(
    code,
  );
  const single =
    !!from &&
    !/\b(?:join|union|except|intersect)\b/i.test(code) &&
    !/\(\s*(?:select|with|values|table)\b/i.test(code) &&
    !/\bwith\b/i.test(code.slice(0, from.index)) &&
    !from[1]!.includes(",");
  const alias = from
    ? new RegExp(String.raw`^\s*${IDENT}(?:\s*\.\s*${IDENT})?\s+(?:as\s+)?(${IDENT})`, "iu").exec(
        from[1]!,
      )?.[1]
    : undefined;
  const name = alias === undefined ? null : unquote(alias).name;
  return { single, alias: name !== null && !NOT_ALIAS.has(name.toLowerCase()) ? name : null };
}

interface TableHit {
  meta: TableMeta;
  schema: string;
  name: string;
  rows: () => Value[][];
}

/** Finds the table a statement reads or writes (`from x`, `into x`, `update x`). */
function findTable(ctx: Ctx, sql: string, start: number, index: number): TableHit | null {
  const m = FROM_RE.exec(sql);
  if (!m) return null;
  const raw = m[1]!;
  const parts = raw.split(/\s*\.\s*/);
  const nameTok = unquote(parts[parts.length - 1]!);
  const schemaTok = parts.length > 1 ? unquote(parts[0]!).name : null;
  const schema =
    schemaTok ??
    (ctx.driver === "postgres"
      ? "public"
      : ctx.driver === "mysql"
        ? (ctx.info.currentDatabase ?? "tabler_dev")
        : "main");
  const tables = tablesFor(ctx.driver, schema);
  const found = tables.find((t) =>
    nameTok.quoted ? t.name === nameTok.name : t.name.toLowerCase() === nameTok.name.toLowerCase(),
  );
  if (!found) {
    const at = start + m.index + m[0].length - raw.length;
    throw missingTable(ctx.driver, nameTok.name, ctx.text, at, index, schema);
  }
  const meta = tableMetaFor(ctx.driver, schema, found.name)!;
  return {
    meta,
    schema,
    name: found.name,
    rows: () =>
      rowsFor(`${ctx.driver}:${schema}.${found.name}`, meta.columns, meta.rowEstimate ?? 25),
  };
}

/** Splits a select list at top-level commas. */
function splitList(list: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let quote: string | null = null;
  let cur = "";
  for (const ch of list) {
    if (quote) {
      if (ch === quote) quote = null;
    } else if (ch === "'" || ch === '"' || ch === "`") quote = ch;
    else if (ch === "(") depth++;
    else if (ch === ")") depth--;
    else if (ch === "," && depth === 0) {
      out.push(cur.trim());
      cur = "";
      continue;
    }
    cur += ch;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

function literalValue(ctx: Ctx, expr: string): { value: Value; kind: ValueKind; name: string } {
  const e = expr.trim();
  const lower = e.toLowerCase();
  const fn = /^([a-z_]+)\s*\(/i.exec(e)?.[1]?.toLowerCase();
  const pgName = (n: string) => (ctx.driver === "postgres" ? n : e);
  if (/^[+-]?\d+$/.test(e)) {
    const n = Number(e);
    return Number.isSafeInteger(n)
      ? { value: { t: "int", v: n }, kind: "int", name: pgName("?column?") }
      : { value: { t: "bigInt", v: e }, kind: "bigInt", name: pgName("?column?") };
  }
  if (/^[+-]?(\d+\.\d*|\.\d+)$/.test(e))
    return { value: { t: "decimal", v: e }, kind: "decimal", name: pgName("?column?") };
  const str = /^[eE]?'((?:[^']|'')*)'$/s.exec(e);
  if (str)
    return {
      value: { t: "text", v: str[1]!.replace(/''/g, "'") },
      kind: "text",
      name: pgName("?column?"),
    };
  if (lower === "true" || lower === "false")
    return { value: { t: "bool", v: lower === "true" }, kind: "bool", name: pgName("bool") };
  if (lower === "null") return { value: { t: "null" }, kind: "text", name: pgName("?column?") };
  if (fn === "now" || lower === "current_timestamp")
    return {
      value: { t: "timestampTz", v: "2026-10-11 09:41:07.512+07" },
      kind: "timestampTz",
      name: pgName(fn ?? "current_timestamp"),
    };
  if (lower === "current_date")
    return { value: { t: "date", v: "2026-10-11" }, kind: "date", name: pgName("current_date") };
  if (fn === "version")
    return {
      value: { t: "text", v: ctx.info.serverVersion },
      kind: "text",
      name: pgName("version"),
    };
  if (fn === "current_user" || lower === "current_user" || fn === "user")
    return {
      value: { t: "text", v: ctx.info.currentUser ?? "tabler" },
      kind: "text",
      name: pgName("current_user"),
    };
  if (fn === "current_database" || fn === "database")
    return {
      value: { t: "text", v: ctx.info.currentDatabase ?? "" },
      kind: "text",
      name: pgName(fn),
    };
  if (fn === "gen_random_uuid" || fn === "uuid")
    return { value: { t: "uuid", v: crypto.randomUUID() }, kind: "uuid", name: pgName(fn) };
  const sum = /^(\d+)\s*([+*-])\s*(\d+)$/.exec(e);
  if (sum) {
    const [a, op, b] = [Number(sum[1]), sum[2], Number(sum[3])];
    const v = op === "+" ? a + b : op === "-" ? a - b : a * b;
    return { value: { t: "int", v }, kind: "int", name: pgName("?column?") };
  }
  return { value: { t: "text", v: e }, kind: "text", name: pgName(fn ?? "?column?") };
}

/** `select <expr> [as alias], …` without FROM: one row. */
function selectLiterals(ctx: Ctx, list: string): { columns: ResultColumn[]; row: Value[] } {
  const columns: ResultColumn[] = [];
  const row: Value[] = [];
  for (const item of splitList(list)) {
    const alias = /^(.*?)\s+(?:as\s+)?("[^"]+"|`[^`]+`|[\p{L}_][\p{L}\p{N}_]*)$/isu.exec(item);
    const useAlias =
      alias && !/^[eE]?'.*'$/s.test(item) && !/^\d/.test(alias[2]!) && /\s/.test(item);
    const lit = literalValue(ctx, useAlias ? alias[1]! : item);
    columns.push(resultColumn(ctx.driver, useAlias ? unquote(alias[2]!).name : lit.name, lit.kind));
    row.push(lit.value);
  }
  return { columns, row };
}

async function streamRows(
  ctx: Ctx,
  index: number,
  set: number,
  columns: ResultColumn[],
  rows: Value[][],
): Promise<void> {
  ctx.send({ event: "columns", data: { index, set, columns } });
  for (let i = 0; i < rows.length; i += CHUNK) {
    if (ctx.run.cancelled) throw cancelledError(ctx.driver, index);
    ctx.send({ event: "rows", data: { index, set, rows: rows.slice(i, i + CHUNK) } });
    await sleep(rows.length > 2000 ? 8 : 1);
  }
}

function planLines(
  ctx: Ctx,
  sql: string,
  mode: ExplainMode,
  start: number,
  index: number,
): string[] {
  const hit = /\bfrom\b/i.test(sql) ? findTable(ctx, sql, start, index) : null;
  const rows = hit?.meta.rowEstimate ?? 1;
  const limit = /\blimit\s+(\d+)/i.exec(sql)?.[1];
  const where = /\bwhere\s+(.+?)(?:\border\b|\blimit\b|$)/is.exec(sql)?.[1]?.trim();
  const analyze = mode === "analyze";
  if (ctx.driver === "mysql") {
    const scan = hit
      ? `-> Table scan on ${hit.name}  (cost=${(rows * 0.105).toFixed(2)} rows=${rows})`
      : "-> Rows fetched before execution  (cost=0..0 rows=1)";
    const lines = [
      ...(limit
        ? [`-> Limit: ${limit} row(s)  (cost=${(rows * 0.105).toFixed(2)} rows=${limit})`]
        : []),
      ...(where
        ? [
            `    -> Filter: (${where})  (cost=${(rows * 0.105).toFixed(2)} rows=${Math.ceil(rows / 3)})`,
          ]
        : []),
      `${limit || where ? "        " : ""}${scan}${analyze ? `  (actual time=0.031..0.412 rows=${rows} loops=1)` : ""}`,
    ];
    return lines;
  }
  if (ctx.driver === "sqlite") {
    return hit ? [`QUERY PLAN`, `\`--SCAN ${hit.name}`] : ["QUERY PLAN", "`--SCAN CONSTANT ROW"];
  }
  const actual = (n: number) =>
    analyze ? ` (actual time=0.012..0.${String(n).padStart(3, "0")} rows=${n} loops=1)` : "";
  const width = (hit?.meta.columns.length ?? 1) * 18;
  const scan = hit
    ? `Seq Scan on ${hit.name}  (cost=0.00..${(rows * 0.12 + 1).toFixed(2)} rows=${rows} width=${width})${actual(rows)}`
    : `Result  (cost=0.00..0.01 rows=1 width=4)${actual(1)}`;
  const lines = limit
    ? [
        `Limit  (cost=0.00..${(Number(limit) * 0.12).toFixed(2)} rows=${limit} width=${width})${actual(Math.min(rows, Number(limit)))}`,
        `  ->  ${scan}`,
      ]
    : [scan];
  if (where) lines.push(`${limit ? "        " : "  "}Filter: (${where})`);
  if (analyze) lines.push("Planning Time: 0.084 ms", "Execution Time: 0.391 ms");
  return lines;
}

/** `RAISE level 'format', args… USING HINT = '…', DETAIL = '…'` in a PL/pgSQL body. */
const RAISE =
  /\braise\s+(?:(notice|warning|info|log|debug|exception)\s+)?'((?:[^']|'')*)'([^;]*)/gi;

const unquoteLiteral = (s: string) =>
  s
    .trim()
    .replace(/^'([\s\S]*)'$/, "$1")
    .replace(/''/g, "'");

/**
 * PostgreSQL `RAISE` statements of a `DO` block, in order: a notice event for each (as the
 * backend streams `QueryEvent::Notice`), and an error for `RAISE EXCEPTION` (the default level).
 */
function raiseNotices(ctx: Ctx, sql: string, index: number): void {
  if (ctx.driver !== "postgres" || !/^\s*do\b/i.test(sql)) return;
  for (const m of sql.matchAll(RAISE)) {
    const level = (m[1] ?? "exception").toUpperCase();
    const [argText = "", using = ""] = (m[3] ?? "").split(/\busing\b/i);
    const args = splitList(argText.replace(/^\s*,/, "")).map(unquoteLiteral);
    let i = 0;
    const message = (m[2] ?? "").replace(/''/g, "'").replace(/%/g, () => args[i++] ?? "%");
    const option = (name: string) => {
      const hit = new RegExp(`\\b${name}\\s*=\\s*('(?:[^']|'')*')`, "i").exec(using);
      return hit ? unquoteLiteral(hit[1]!) : null;
    };
    if (level === "EXCEPTION")
      throw new SqlError(
        payload({
          code: "DB_ERROR",
          message,
          detail: option("detail"),
          hint: option("hint"),
          sqlstate: "P0001",
          statementIndex: index,
        }),
      );
    ctx.send({
      event: "notice",
      data: {
        index,
        severity: level,
        message,
        detail: option("detail"),
        hint: option("hint"),
        context: "PL/pgSQL function inline_code_block line 1 at RAISE",
      },
    });
  }
}

async function execute(
  ctx: Ctx,
  sql: string,
  start: number,
  index: number,
  explain: ExplainMode | null,
): Promise<Exec> {
  const empty: Exec = {
    kind: "empty",
    columns: [],
    rowCount: 0,
    resultSets: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    notices: [],
  };
  const leadAt = sql.search(/\S/);
  const keyword = /^[\p{L}_]+/u.exec(sql.slice(leadAt))?.[0]?.toLowerCase() ?? "";
  const syntax = /\bsyntax\b/i.exec(sql);
  if (syntax) throw syntaxError(ctx.driver, ctx.text, start + syntax.index, index);
  if (!KNOWN.has(keyword)) throw syntaxError(ctx.driver, ctx.text, start + leadAt, index);

  await sleep(2);
  raiseNotices(ctx, sql, index);

  // EXPLAIN (statement prefix or RunOptions.explain)
  const explainPrefix = /^explain(\s+analyze)?\s+/i.exec(sql);
  if (explainPrefix || explain) {
    const target = explainPrefix ? sql.slice(explainPrefix[0].length) : sql;
    const lines = planLines(
      ctx,
      target,
      explainPrefix?.[1] || explain === "analyze" ? "analyze" : "plan",
      start + (explainPrefix?.[0].length ?? 0),
      index,
    );
    const columns = [
      resultColumn(ctx.driver, ctx.driver === "mysql" ? "EXPLAIN" : "QUERY PLAN", "text"),
    ];
    await streamRows(
      ctx,
      index,
      0,
      columns,
      lines.map((l) => [{ t: "text", v: l }]),
    );
    return { ...empty, kind: "explain", columns, rowCount: lines.length, resultSets: 1 };
  }

  const sleepCall = /\b(?:pg_)?sleep\s*\(\s*([\d.]+)\s*\)/i.exec(sql);
  if (sleepCall) {
    const until = Date.now() + Number(sleepCall[1]) * 1000;
    while (Date.now() < until) {
      if (ctx.run.cancelled) throw cancelledError(ctx.driver, index);
      if (ctx.deadline !== null && Date.now() >= ctx.deadline)
        throw timeoutError(ctx.timeoutSecs, index);
      await sleep(Math.min(50, until - Date.now()));
    }
    if (ctx.run.cancelled) throw cancelledError(ctx.driver, index);
    const pg = ctx.driver === "postgres";
    const columns = [
      {
        ...resultColumn(ctx.driver, pg ? "pg_sleep" : sleepCall[0], pg ? "other" : "int"),
        dataType: pg ? "void" : "BIGINT",
      },
    ];
    await streamRows(ctx, index, 0, columns, [[pg ? { t: "null" } : { t: "int", v: 0 }]]);
    return { ...empty, kind: "rows", columns, rowCount: 1, resultSets: 1 };
  }

  if (keyword === "select" || keyword === "with" || keyword === "table" || keyword === "values") {
    const hit = /\bfrom\b|^\s*table\b/i.test(sql) ? findTable(ctx, sql, start, index) : null;
    if (!hit) {
      const list = sql.replace(/^\s*select\s+/i, "").replace(/;\s*$/, "");
      const { columns, row } = selectLiterals(ctx, list);
      await streamRows(ctx, index, 0, columns, [row]);
      return { ...empty, kind: "rows", columns, rowCount: 1, resultSets: 1 };
    }
    const selectList = /^\s*select\s+(.*?)\s+from\b/is.exec(sql)?.[1]?.trim() ?? "*";
    const all = hit.rows();
    if (/^count\s*\(\s*\*\s*\)$/i.test(selectList)) {
      const columns = [
        resultColumn(ctx.driver, ctx.driver === "postgres" ? "count" : "count(*)", "bigInt"),
      ];
      await streamRows(ctx, index, 0, columns, [[{ t: "int", v: all.length }]]);
      return { ...empty, kind: "rows", columns, rowCount: 1, resultSets: 1 };
    }
    // Pick the named columns when every item is a plain column of the table; else all columns.
    const names =
      selectList === "*"
        ? []
        : splitList(selectList).map((s) => unquote(s.replace(/^.*\./, "")).name);
    const idx = names.map((n) =>
      hit.meta.columns.findIndex((c) => c.name.toLowerCase() === n.toLowerCase()),
    );
    const picked =
      names.length > 0 && idx.every((i) => i >= 0) ? idx : hit.meta.columns.map((_, i) => i);
    const { single, alias } = selectSource(sql);
    const columns = picked.map((i) =>
      fromMeta(
        hit.meta.columns[i]!,
        hit.name,
        hit.schema,
        single ? { driver: ctx.driver, alias } : null,
      ),
    );
    const limit = /\blimit\s+(\d+)(?:\s+offset\s+(\d+))?/i.exec(sql);
    const offset = Number(limit?.[2] ?? 0);
    const wanted = limit ? all.slice(offset, offset + Number(limit[1])) : all.slice(offset);
    const truncated = wanted.length > ctx.maxRows;
    const rows = (truncated ? wanted.slice(0, ctx.maxRows) : wanted).map((r) =>
      picked.map((i) => r[i]!),
    );
    await streamRows(ctx, index, 0, columns, rows);
    return { ...empty, kind: "rows", columns, rowCount: rows.length, resultSets: 1, truncated };
  }

  if (keyword === "call" && ctx.driver === "mysql") {
    // A procedure returning two result sets.
    const users = tableMetaFor(ctx.driver, "tabler_dev", "users")!;
    const tags = tableMetaFor(ctx.driver, "tabler_dev", "tags")!;
    const src = { driver: ctx.driver, alias: null };
    const c1 = users.columns.slice(0, 3).map((c) => fromMeta(c, "users", "tabler_dev", src));
    const c2 = tags.columns.map((c) => fromMeta(c, "tags", "tabler_dev", src));
    const r1 = rowsFor("mysql:tabler_dev.users", users.columns, 50)
      .slice(0, 5)
      .map((r) => r.slice(0, 3));
    const r2 = rowsFor("mysql:tabler_dev.tags", tags.columns, 15);
    await streamRows(ctx, index, 0, c1, r1);
    await streamRows(ctx, index, 1, c2, r2);
    return { ...empty, kind: "rows", columns: c1, rowCount: r1.length + r2.length, resultSets: 2 };
  }

  if (keyword === "show" && /^\s*show\s+tables/i.test(sql)) {
    const db = ctx.info.currentDatabase ?? "tabler_dev";
    const columns = [resultColumn(ctx.driver, `Tables_in_${db}`, "text")];
    const rows: Value[][] = tablesFor(ctx.driver, db).map((t) => [{ t: "text", v: t.name }]);
    await streamRows(ctx, index, 0, columns, rows);
    return { ...empty, kind: "rows", columns, rowCount: rows.length, resultSets: 1 };
  }

  const writes = [
    "insert",
    "update",
    "delete",
    "replace",
    "merge",
    "truncate",
    "create",
    "drop",
    "alter",
  ];
  if (ctx.info.readOnly && writes.includes(keyword)) {
    throw new SqlError(
      payload({
        code: "DB_ERROR",
        message: `cannot execute ${keyword.toUpperCase()} in a read-only transaction`,
        sqlstate: "25006",
        statementIndex: index,
      }),
    );
  }

  if (
    keyword === "insert" ||
    keyword === "update" ||
    keyword === "delete" ||
    keyword === "replace"
  ) {
    const hit = findTable(ctx, sql, start, index);
    let affected: number;
    if (keyword === "insert" || keyword === "replace") {
      const values = /\bvalues\b([\s\S]*)$/i.exec(sql)?.[1] ?? "";
      affected = Math.max(1, splitList(values).filter((v) => v.startsWith("(")).length);
    } else if (!/\bwhere\b/i.test(sql)) {
      affected = hit?.meta.rowEstimate ?? 0;
    } else {
      affected = 1 + ((hit?.name.length ?? 0) % 3);
    }
    return {
      ...empty,
      kind: "affected",
      affected,
      lastInsertId:
        keyword === "insert" && ctx.driver !== "postgres"
          ? String((hit?.meta.rowEstimate ?? 0) + affected)
          : null,
    };
  }

  const notices: string[] = [];
  const dropIfExists = /^\s*drop\s+(table|view|index|schema)\s+if\s+exists\s+(\S+)/i.exec(sql);
  if (dropIfExists && ctx.driver === "postgres")
    notices.push(
      `${dropIfExists[1]!.toLowerCase()} "${unquote(dropIfExists[2]!.replace(/;$/, "")).name}" does not exist, skipping`,
    );
  return { ...empty, notices };
}

// --- commands ------------------------------------------------------------------------

registerMock("run_sql", async ({ connectionId, queryId, sql, options, onEvent }) => {
  const info = await connectionInfo(connectionId);
  const text = String(sql ?? "");
  const opts = {
    maxRows: 10_000,
    stopOnError: true,
    explain: null,
    ...(options as Partial<RunOptions>),
  };
  const channel = onEvent as { send?: (e: QueryEvent) => void } | undefined;
  const run: Running = { cancelled: false };
  const id = String(queryId);
  running.set(id, run);
  const ctx: Ctx = {
    info,
    driver: info.driver,
    text,
    run,
    maxRows: opts.maxRows,
    deadline: null,
    timeoutSecs: opts.timeoutSecs ?? 0,
    send: (e) => channel?.send?.(e),
  };
  const began = performance.now();
  const results: StatementResult[] = [];
  try {
    const statements = splitStatements(text, info.driver);
    for (let index = 0; index < statements.length; index++) {
      const st = statements[index]!;
      const stmtSql = text.slice(st.start, st.end);
      const span = {
        start: utf16ToChar(text, st.start),
        end: utf16ToChar(text, st.end),
        line: st.line,
      };
      const startedAt = new Date().toISOString();
      const t0 = performance.now();
      ctx.send({ event: "started", data: { index, sql: stmtSql, span } });
      const base: StatementResult = {
        index,
        sql: stmtSql,
        span,
        kind: "empty",
        columns: [],
        resultSets: 0,
        rowCount: 0,
        truncated: false,
        affected: null,
        lastInsertId: null,
        startedAt,
        durationMs: 0,
        error: null,
        notices: [],
      };
      try {
        if (run.cancelled) throw cancelledError(info.driver, index);
        ctx.deadline = ctx.timeoutSecs > 0 ? Date.now() + ctx.timeoutSecs * 1000 : null;
        const r = await execute(ctx, stmtSql, st.start, index, opts.explain);
        const result: StatementResult = {
          ...base,
          ...r,
          durationMs: Math.max(1, Math.round(performance.now() - t0)),
        };
        results.push(result);
        ctx.send({ event: "finished", data: { index, result } });
        record(info, stmtSql, "editor", {
          durationMs: result.durationMs,
          rowCount: r.kind === "rows" ? r.rowCount : null,
          affected: r.affected,
        });
      } catch (e) {
        if (!(e instanceof SqlError)) throw e;
        const result: StatementResult = {
          ...base,
          error: e.payload,
          durationMs: Math.round(performance.now() - t0),
        };
        results.push(result);
        ctx.send({ event: "failed", data: { index, error: e.payload } });
        record(info, stmtSql, "editor", {
          durationMs: result.durationMs,
          error: e.payload.message,
        });
        if (
          e.payload.code === "CANCELLED" ||
          e.payload.code === "QUERY_TIMEOUT" ||
          opts.stopOnError
        )
          break;
      }
    }
  } finally {
    running.delete(id);
  }
  ctx.send({
    event: "done",
    data: { totalMs: Math.max(1, Math.round(performance.now() - began)) },
  });
  return results;
});

registerMock("cancel_query", ({ queryId }) => {
  const run = running.get(String(queryId));
  if (run) run.cancelled = true;
  return null;
});

registerMock("explain", async ({ connectionId, sql, mode, queryId }) => {
  const info = await connectionInfo(connectionId);
  const text = String(sql ?? "");
  const run: Running = { cancelled: false };
  const id = queryId ? String(queryId) : crypto.randomUUID();
  running.set(id, run);
  const ctx: Ctx = {
    info,
    driver: info.driver,
    text,
    run,
    maxRows: 10_000,
    deadline: null,
    timeoutSecs: 0,
    send: () => undefined,
  };
  const t0 = performance.now();
  try {
    const st = splitStatements(text, info.driver)[0];
    if (!st) mockError("INVALID_INPUT", "nothing to explain");
    const stmtSql = text.slice(st.start, st.end);
    const leadAt = stmtSql.search(/\S/);
    const keyword = /^[\p{L}_]+/u.exec(stmtSql.slice(leadAt))?.[0]?.toLowerCase() ?? "";
    const syntax = /\bsyntax\b/i.exec(stmtSql);
    if (syntax) throw syntaxError(info.driver, text, st.start + syntax.index, 0);
    if (!KNOWN.has(keyword)) throw syntaxError(info.driver, text, st.start + leadAt, 0);
    await sleep(mode === "analyze" ? 60 : 15);
    if (run.cancelled) throw cancelledError(info.driver, 0);
    const lines = planLines(ctx, stmtSql, mode as ExplainMode, st.start, 0);
    const durationMs = Math.max(1, Math.round(performance.now() - t0));
    record(info, `EXPLAIN${mode === "analyze" ? " ANALYZE" : ""} ${stmtSql}`, "explain", {
      durationMs,
      rowCount: lines.length,
    });
    return {
      columns: [
        resultColumn(info.driver, info.driver === "mysql" ? "EXPLAIN" : "QUERY PLAN", "text"),
      ],
      rows: lines.map((l) => [{ t: "text", v: l }]),
      text: lines.join("\n"),
      durationMs,
    } satisfies ExplainResult;
  } catch (e) {
    if (e instanceof SqlError) {
      record(info, String(sql), "explain", {
        durationMs: Math.round(performance.now() - t0),
        error: e.payload.message,
      });
      throw e.payload;
    }
    throw e;
  } finally {
    running.delete(id);
  }
});

/** The History search (Match Case / Whole Word / Regex), as `appdb.rs` matches it. */
export function historyMatcher(f: {
  search: string | null;
  matchCase: boolean;
  wholeWord: boolean;
  regex: boolean;
}): ((sql: string) => boolean) | null {
  const term = f.search ?? "";
  if (!term) return null;
  if (!f.matchCase && !f.wholeWord && !f.regex) {
    const needle = term.toLowerCase();
    return (sql) => sql.toLowerCase().includes(needle);
  }
  let body = f.regex ? term : term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (f.wholeWord) body = `\\b(?:${body})\\b`;
  let re: RegExp;
  try {
    re = new RegExp(body, f.matchCase ? "u" : "iu");
  } catch (e) {
    return mockError("INVALID_INPUT", `invalid regular expression: ${(e as Error).message}`);
  }
  return (sql) => re.test(sql);
}

registerMock("list_history", ({ filter }) => {
  const f = {
    profileId: null,
    search: null,
    limit: null,
    beforeId: null,
    matchCase: false,
    wholeWord: false,
    regex: false,
    ...(filter as Partial<HistoryFilter>),
  };
  const matches = historyMatcher(f);
  const limit = Math.min(1000, Math.max(1, f.limit ?? 200));
  return history
    .filter(
      (h) =>
        (f.profileId === null || h.profileId === f.profileId) &&
        (f.beforeId === null || h.id < f.beforeId) &&
        (!matches || matches(h.sql)),
    )
    .sort((a, b) => b.id - a.id)
    .slice(0, limit);
});

registerMock("clear_history_before", ({ before, profileId }) => {
  const cutoff = Date.parse(String(before));
  if (Number.isNaN(cutoff)) mockError("INVALID_INPUT", `invalid date ${String(before)}`);
  const n = history.length;
  history = history.filter(
    (h) => !(Date.parse(h.startedAt) < cutoff && (!profileId || h.profileId === profileId)),
  );
  return n - history.length;
});

registerMock("delete_history", ({ ids }) => {
  const drop = new Set((ids as number[]) ?? []);
  history = history.filter((h) => !drop.has(h.id));
  return null;
});

registerMock("clear_history", ({ profileId }) => {
  history = profileId ? history.filter((h) => h.profileId !== profileId) : [];
  return null;
});

registerMock("read_text_file", ({ path }) => {
  const content = files.get(String(path));
  if (content === undefined) mockError("NOT_FOUND", `No such file: ${String(path)}`);
  return content;
});

/** Text of a mock file (the Queries pane's Import… reads them too). */
export function mockFileText(path: string): string | undefined {
  return files.get(path);
}

/** Puts a file in the mock's in-memory file system (the saved queries' `.sql` files). */
export function mockWriteFile(path: string, text: string): void {
  files.set(path, text);
}

/**
 * Files the mock only pretends to hold, to try the large-file prompt in `pnpm dev:mock`: path →
 * size on disk. Their text is generated INSERT lines; reading one whole is refused (the backend
 * reads at most `EDITOR_FILE_MAX_BYTES` at once).
 */
const BIG_FILES = new Map<string, number>([
  ["/Users/me/dumps/huge-dump.sql", 900 * 1024 * 1024],
  ["/Users/me/dumps/big-dump.sql", 100 * 1024 * 1024],
]);
const MOCK_WHOLE_FILE_MAX = 512 * 1024 * 1024;

const utf8Length = (text: string) => new TextEncoder().encode(text).length;

/** The first lines of a generated big file, at most `maxBytes` of them, whole lines only. */
function bigFileHead(maxBytes: number): string {
  const lines: string[] = [];
  let bytes = 0;
  for (let i = 1; ; i++) {
    const line = `INSERT INTO events_big (id, kind) VALUES (${i}, 'click');\n`;
    if (bytes + line.length > maxBytes) break;
    lines.push(line);
    bytes += line.length;
  }
  return lines.join("");
}

registerMock("sql_file_info", ({ path }) => {
  const p = String(path);
  const big = BIG_FILES.get(p);
  if (big !== undefined) return { bytes: big, compressed: false };
  const text = files.get(p);
  if (text === undefined) mockError("NOT_FOUND", `No such file: ${p}`);
  const compressed = /\.gz$/i.test(p);
  // A gzip file is smaller than its text.
  return { bytes: compressed ? Math.ceil(utf8Length(text) / 4) : utf8Length(text), compressed };
});

/** `.sql.gz` files are plain text in the mock (no compression in the browser). */
registerMock("read_sql_file", ({ path }) => {
  const p = String(path);
  if ((BIG_FILES.get(p) ?? 0) > MOCK_WHOLE_FILE_MAX)
    mockError("INVALID_INPUT", `${p} is too big to read whole`);
  const content = files.get(p);
  if (content === undefined) mockError("NOT_FOUND", `No such file: ${p}`);
  return content;
});

registerMock("read_sql_file_head", ({ path, maxBytes }) => {
  const p = String(path);
  const limit = typeof maxBytes === "number" ? maxBytes : null;
  const big = BIG_FILES.get(p);
  if (big !== undefined) {
    if (limit === null) mockError("INVALID_INPUT", `${p} is too big to read whole`);
    return {
      text: bigFileHead(limit),
      totalBytes: big,
      truncated: true,
      compressed: false,
      encoding: null,
    };
  }
  const text = files.get(p);
  if (text === undefined) mockError("NOT_FOUND", `No such file: ${p}`);
  const compressed = /\.gz$/i.test(p);
  const total = compressed ? Math.ceil(utf8Length(text) / 4) : utf8Length(text);
  if (limit === null || utf8Length(text) <= limit)
    return { text, totalBytes: total, truncated: false, compressed, encoding: null };
  // Cut after the last complete line that fits.
  const bytes = new TextEncoder().encode(text).slice(0, limit);
  const head = new TextDecoder().decode(bytes).replace(/\uFFFD$/, "");
  const end = head.lastIndexOf("\n");
  return {
    text: end >= 0 ? head.slice(0, end + 1) : head,
    totalBytes: total,
    truncated: true,
    compressed,
    encoding: null,
  };
});

registerMock("write_sql_file", ({ path, content }) => {
  if (typeof path !== "string" || !path.trim()) mockError("INVALID_INPUT", "path required");
  files.set(path, String(content ?? ""));
  return null;
});

registerMock("write_text_file", ({ path, content }) => {
  if (typeof path !== "string" || !path.trim()) mockError("INVALID_INPUT", "path required");
  files.set(path, String(content ?? ""));
  return null;
});
