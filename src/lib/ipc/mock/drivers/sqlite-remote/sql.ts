import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { SortSpec } from "../../../bindings/SortSpec";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import { matchesGroup, sortRows } from "../../rows";
import {
  columnsOf,
  constraintError,
  newRow,
  sqliteError,
  tableOf,
  type Engine,
  type Table,
} from "./data";

/**
 * The SQL the mock's LibSQL and D1 servers run in the editor: SELECT (columns, `*` or
 * `count(*)`, one table, a WHERE of AND-joined comparisons, ORDER BY, LIMIT / OFFSET, or
 * literals with no table), INSERT … VALUES, UPDATE … SET … WHERE, DELETE, EXPLAIN QUERY PLAN,
 * `PRAGMA table_info(t)` and `sqlite_version()`. D1 refuses BEGIN and SAVEPOINT as D1 does;
 * LibSQL takes them (the mock keeps no transaction). Anything else fails, saying so.
 */

type Token = { kind: "word" | "ident" | "string" | "number" | "op"; text: string };

const TOKEN =
  /\s+|--[^\n]*|\/\*[\s\S]*?\*\/|'(?:[^']|'')*'|"(?:[^"]|"")*"|`(?:[^`]|``)*`|\[[^\]]*\]|\d+(?:\.\d+)?(?:e[+-]?\d+)?|[A-Za-z_][A-Za-z0-9_$]*|<=|>=|!=|<>|==|[(),;*=<>.+-]/gy;

/** The tokens of a statement, or null when it holds something the mock does not read. */
function tokenize(sql: string): Token[] | null {
  const out: Token[] = [];
  TOKEN.lastIndex = 0;
  while (TOKEN.lastIndex < sql.length) {
    const at = TOKEN.lastIndex;
    const m = TOKEN.exec(sql);
    if (!m || m.index !== at) return null;
    const t = m[0];
    if (/^\s|^--|^\/\*/.test(t)) continue;
    if (t.startsWith("'")) out.push({ kind: "string", text: t.slice(1, -1).replaceAll("''", "'") });
    else if (/^["`[]/.test(t)) out.push({ kind: "ident", text: t.slice(1, -1) });
    else if (/^\d/.test(t)) out.push({ kind: "number", text: t });
    else if (/^[A-Za-z_]/.test(t)) out.push({ kind: "word", text: t });
    else out.push({ kind: "op", text: t });
  }
  while (out.at(-1)?.text === ";") out.pop();
  return out;
}

class Unsupported extends Error {}

class Reader {
  private at = 0;
  constructor(private readonly tokens: Token[]) {}
  peek(n = 0): Token | undefined {
    return this.tokens[this.at + n];
  }
  done(): boolean {
    return this.at >= this.tokens.length;
  }
  isWord(word: string, n = 0): boolean {
    const t = this.peek(n);
    return t?.kind === "word" && t.text.toUpperCase() === word;
  }
  take(): Token {
    const t = this.tokens[this.at++];
    if (!t) throw new Unsupported("incomplete input");
    return t;
  }
  word(word: string): void {
    if (!this.isWord(word)) throw new Unsupported(`expected ${word}`);
    this.at++;
  }
  maybeWord(word: string): boolean {
    if (!this.isWord(word)) return false;
    this.at++;
    return true;
  }
  op(op: string): void {
    if (this.peek()?.text !== op) throw new Unsupported(`expected ${op}`);
    this.at++;
  }
  maybeOp(op: string): boolean {
    if (this.peek()?.text !== op) return false;
    this.at++;
    return true;
  }
  /** A name, schema-qualified names read as their last part (`main.t`). */
  name(): string {
    const t = this.take();
    if (t.kind !== "word" && t.kind !== "ident") throw new Unsupported("expected a name");
    if (this.maybeOp(".")) return this.name();
    return t.text;
  }
  /** A literal: a number (with a sign), a string, NULL, TRUE or FALSE. */
  literal(): Value {
    const sign = this.maybeOp("-") ? -1 : (this.maybeOp("+"), 1);
    const t = this.take();
    if (t.kind === "number") {
      if (/[.e]/i.test(t.text)) return { t: "float", v: sign * Number(t.text) };
      const n = BigInt(t.text) * BigInt(sign);
      return n > BigInt(Number.MAX_SAFE_INTEGER) || n < BigInt(Number.MIN_SAFE_INTEGER)
        ? { t: "bigInt", v: String(n) }
        : { t: "int", v: Number(n) };
    }
    if (sign === -1) throw new Unsupported("expected a number");
    if (t.kind === "string") return { t: "text", v: t.text };
    const word = t.kind === "word" ? t.text.toUpperCase() : "";
    if (word === "NULL") return { t: "null" };
    if (word === "TRUE" || word === "FALSE") return { t: "int", v: word === "TRUE" ? 1 : 0 };
    throw new Unsupported("expected a value");
  }
}

const COMPARE: Record<string, FilterOp> = {
  "=": "eq",
  "==": "eq",
  "!=": "ne",
  "<>": "ne",
  ">": "gt",
  "<": "lt",
  ">=": "ge",
  "<=": "le",
};

/** `col op literal [AND …]` as a filter the mock's grid matches with. */
function where(r: Reader): FilterGroup {
  const group: FilterGroup = { conjunction: "and", items: [] };
  do {
    const column = r.name();
    if (r.maybeWord("IS")) {
      const not = r.maybeWord("NOT");
      r.word("NULL");
      group.items.push({ kind: "condition", column, op: not ? "isNotNull" : "isNull", values: [] });
      continue;
    }
    const not = r.maybeWord("NOT");
    if (r.maybeWord("LIKE")) {
      group.items.push({
        kind: "condition",
        column,
        op: not ? "notLike" : "like",
        values: [r.literal()],
      });
      continue;
    }
    const op = COMPARE[r.take().text];
    if (!op || not) throw new Unsupported("expected a comparison");
    group.items.push({ kind: "condition", column, op, values: [r.literal()] });
  } while (r.maybeWord("AND"));
  return group;
}

/** What one statement gave: rows, a count of changed rows, or nothing. */
export type Outcome =
  | { kind: "rows"; columns: ResultColumn[]; rows: Value[][] }
  | { kind: "affected"; affected: number; lastInsertId: string | null }
  | { kind: "empty" };

const resultColumn = (
  name: string,
  dataType: string,
  kind: ResultColumn["kind"],
): ResultColumn => ({
  name,
  dataType,
  kind,
  nullable: true,
  // Neither Hrana nor D1's API names a result column's table: editor results stay read-only.
  table: null,
});

const VERSION: Record<Engine, string> = { libsql: "3.45.1", d1: "3.45.0" };

function select(r: Reader, tables: Map<string, Table>, engine: Engine): Outcome {
  if (r.isWord("SQLITE_VERSION")) {
    r.take();
    r.op("(");
    r.op(")");
    if (!r.done()) throw new Unsupported("sqlite_version() takes no table");
    return {
      kind: "rows",
      columns: [resultColumn("sqlite_version()", "", "text")],
      rows: [[{ t: "text", v: VERSION[engine] }]],
    };
  }
  let count = false;
  let star = false;
  const names: string[] = [];
  const literals: Value[] = [];
  if (r.isWord("COUNT") && r.peek(1)?.text === "(") {
    r.take();
    r.op("(");
    r.op("*");
    r.op(")");
    count = true;
  } else if (r.maybeOp("*")) star = true;
  else
    do {
      const t = r.peek();
      if (t?.kind === "word" && !["NULL", "TRUE", "FALSE"].includes(t.text.toUpperCase()))
        names.push(r.name());
      else if (t?.kind === "ident") names.push(r.name());
      else literals.push(r.literal());
    } while (r.maybeOp(","));
  if (!r.maybeWord("FROM")) {
    if (count || star || names.length > 0 || !r.done()) throw new Unsupported("expected FROM");
    return {
      kind: "rows",
      columns: literals.map((v, i) =>
        resultColumn(
          `column${i + 1}`,
          "",
          v.t === "int" ? "int" : v.t === "float" ? "float" : "text",
        ),
      ),
      rows: [literals],
    };
  }
  if (literals.length > 0) throw new Unsupported("values next to a table");
  const t = tableOf(tables, r.name());
  const columns = columnsOf(t);
  let rows = t.rows();
  if (r.maybeWord("WHERE")) {
    const filter = where(r);
    rows = rows.filter((row) => matchesGroup(filter, row, columns));
  }
  if (r.maybeWord("ORDER")) {
    r.word("BY");
    const sort: SortSpec[] = [];
    do {
      const column = r.name();
      const dir = r.maybeWord("DESC") ? "desc" : (r.maybeWord("ASC"), "asc");
      sort.push({ column, dir, nulls: null });
    } while (r.maybeOp(","));
    rows = sortRows(rows, sort, columns);
  }
  if (r.maybeWord("LIMIT")) {
    const limit = r.literal();
    const offset = r.maybeWord("OFFSET") ? r.literal() : { t: "int" as const, v: 0 };
    if (limit.t !== "int" || offset.t !== "int") throw new Unsupported("LIMIT takes integers");
    rows = rows.slice(offset.v, limit.v < 0 ? undefined : offset.v + limit.v);
  }
  if (!r.done()) throw new Unsupported("more after the statement");
  if (count)
    return {
      kind: "rows",
      columns: [resultColumn("count(*)", "", "int")],
      rows: [[{ t: "int", v: rows.length }]],
    };
  const picked = star ? columns.map((c) => c.name) : names;
  const at = picked.map((name) => {
    const i = columns.findIndex((c) => c.name === name);
    if (i < 0) throw new SqlFailure(`no such column: ${name}`, 1);
    return i;
  });
  return {
    kind: "rows",
    columns: at.map((i) => resultColumn(columns[i]!.name, columns[i]!.dataType, columns[i]!.kind)),
    rows: rows.map((row) => at.map((i) => row[i] ?? { t: "null" })),
  };
}

/** A statement SQLite refuses: its message and (extended) result code. */
export class SqlFailure extends Error {
  constructor(
    message: string,
    readonly code: number,
  ) {
    super(message);
  }
}

function insert(r: Reader, tables: Map<string, Table>): Outcome {
  r.word("INTO");
  const t = tableOf(tables, r.name());
  if (t.kind === "view") throw new SqlFailure(`cannot modify ${t.name} because it is a view`, 1);
  const names: string[] = [];
  if (r.maybeOp("(")) {
    do names.push(r.name());
    while (r.maybeOp(","));
    r.op(")");
  } else names.push(...t.cols.map((c) => c.name));
  r.word("VALUES");
  const added: Value[][] = [];
  do {
    r.op("(");
    const values: Value[] = [];
    do values.push(r.literal());
    while (r.maybeOp(","));
    r.op(")");
    if (values.length !== names.length)
      throw new SqlFailure(`${values.length} values for ${names.length} columns`, 1);
    for (const n of names)
      if (!t.cols.some((c) => c.name === n))
        throw new SqlFailure(`table ${t.name} has no column named ${n}`, 1);
    added.push(newRow(t, Object.fromEntries(names.map((n, i) => [n, values[i]!]))));
  } while (r.maybeOp(","));
  if (!r.done()) throw new Unsupported("more after the statement");
  // One statement is atomic: every row checked before any is stored.
  const before = t.data.length;
  for (const row of added) {
    const broken = constraintError(t, row, -1);
    if (broken) {
      t.data.splice(before);
      t.rowids.splice(before);
      throw new SqlFailure(broken.message, broken.code);
    }
    t.data.push(row);
    t.rowids.push(t.nextRowid++);
  }
  const last = t.rowids.at(-1) ?? null;
  return {
    kind: "affected",
    affected: added.length,
    lastInsertId: last === null ? null : String(last),
  };
}

function update(r: Reader, tables: Map<string, Table>): Outcome {
  const t = tableOf(tables, r.name());
  if (t.kind === "view") throw new SqlFailure(`cannot modify ${t.name} because it is a view`, 1);
  r.word("SET");
  const sets: [number, Value][] = [];
  do {
    const name = r.name();
    const i = t.cols.findIndex((c) => c.name === name);
    if (i < 0) throw new SqlFailure(`no such column: ${name}`, 1);
    r.op("=");
    sets.push([i, r.literal()]);
  } while (r.maybeOp(","));
  const columns = columnsOf(t);
  const filter = r.maybeWord("WHERE") ? where(r) : null;
  if (!r.done()) throw new Unsupported("more after the statement");
  const hits = t.data.flatMap((row, i) =>
    !filter || matchesGroup(filter, row, columns) ? [i] : [],
  );
  const before = t.data.map((row) => row.slice());
  for (const i of hits) {
    const row = t.data[i]!.slice();
    for (const [c, v] of sets) row[c] = v;
    t.data[i] = row;
    const broken = constraintError(t, row, i);
    if (broken) {
      t.data.splice(0, t.data.length, ...before);
      throw new SqlFailure(broken.message, broken.code);
    }
  }
  return { kind: "affected", affected: hits.length, lastInsertId: null };
}

function remove(r: Reader, tables: Map<string, Table>): Outcome {
  r.word("FROM");
  const t = tableOf(tables, r.name());
  if (t.kind === "view") throw new SqlFailure(`cannot modify ${t.name} because it is a view`, 1);
  const columns = columnsOf(t);
  const filter = r.maybeWord("WHERE") ? where(r) : null;
  if (!r.done()) throw new Unsupported("more after the statement");
  let affected = 0;
  for (let i = t.data.length - 1; i >= 0; i--)
    if (!filter || matchesGroup(filter, t.data[i]!, columns)) {
      t.data.splice(i, 1);
      t.rowids.splice(i, 1);
      affected++;
    }
  return { kind: "affected", affected, lastInsertId: null };
}

function tableInfo(r: Reader, tables: Map<string, Table>): Outcome {
  r.op("(");
  const t = tableOf(tables, r.name());
  r.op(")");
  if (!r.done()) throw new Unsupported("more after the statement");
  const cols: [string, string, ResultColumn["kind"]][] = [
    ["cid", "", "int"],
    ["name", "", "text"],
    ["type", "", "text"],
    ["notnull", "", "int"],
    ["dflt_value", "", "text"],
    ["pk", "", "int"],
  ];
  return {
    kind: "rows",
    columns: cols.map(([n, d, k]) => resultColumn(n, d, k)),
    rows: t.cols.map((c, i): Value[] => [
      { t: "int", v: i },
      { t: "text", v: c.name },
      { t: "text", v: c.type },
      { t: "int", v: c.notNull ? 1 : 0 },
      c.default === undefined ? { t: "null" } : { t: "text", v: c.default },
      { t: "int", v: c.pk ? 1 : 0 },
    ]),
  };
}

/** D1's refusal of SQL transactions (workerd's words). */
export const D1_NO_TRANSACTIONS =
  "To execute a transaction, please use the state.storage.transaction() or state.storage.transactionSync() APIs instead of the SQL BEGIN TRANSACTION or SAVEPOINT statements";

const TX_WORDS = new Set(["BEGIN", "COMMIT", "END", "ROLLBACK", "SAVEPOINT", "RELEASE"]);

/** Runs one statement on `tables`; throws `SqlFailure` for what SQLite refuses. */
export function execute(sql: string, tables: Map<string, Table>, engine: Engine): Outcome {
  const tokens = tokenize(sql);
  if (!tokens) throw new Unsupported("unreadable");
  const r = new Reader(tokens);
  const first = r.peek()?.text.toUpperCase() ?? "";
  try {
    if (TX_WORDS.has(first)) {
      // SQLITE_AUTH: workerd's authorizer refuses them.
      if (engine === "d1") throw new SqlFailure(D1_NO_TRANSACTIONS, 23);
      return { kind: "empty" };
    }
    r.take();
    switch (first) {
      case "SELECT":
        return select(r, tables, engine);
      case "INSERT":
        return insert(r, tables);
      case "UPDATE":
        return update(r, tables);
      case "DELETE":
        return remove(r, tables);
      case "PRAGMA":
        if (!r.isWord("TABLE_INFO")) break;
        r.take();
        return tableInfo(r, tables);
      case "EXPLAIN": {
        r.word("QUERY");
        r.word("PLAN");
        r.word("SELECT");
        const from = tokens.findIndex((x) => x.kind === "word" && x.text.toUpperCase() === "FROM");
        const table = from >= 0 ? tokens[from + 1]?.text : undefined;
        const filtered = tokens.some((x) => x.kind === "word" && x.text.toUpperCase() === "WHERE");
        const detail = table
          ? `${filtered ? "SEARCH" : "SCAN"} ${table}${filtered ? " USING INTEGER PRIMARY KEY (rowid=?)" : ""}`
          : "SCAN CONSTANT ROW";
        return {
          kind: "rows",
          columns: [
            resultColumn("id", "", "int"),
            resultColumn("parent", "", "int"),
            resultColumn("notused", "", "int"),
            resultColumn("detail", "", "text"),
          ],
          rows: [
            [
              { t: "int", v: 2 },
              { t: "int", v: 0 },
              { t: "int", v: 0 },
              { t: "text", v: detail },
            ],
          ],
        };
      }
    }
  } catch (err) {
    if (err instanceof Unsupported) throw unsupported();
    throw err;
  }
  throw unsupported();
}

const unsupported = () =>
  new SqlFailure(
    "the mock runs SELECT, INSERT, UPDATE, DELETE, EXPLAIN QUERY PLAN and PRAGMA table_info only",
    1,
  );

/** Streams one statement's run as `run_sql` does (its own `done` is left to the batch). */
export function runStatement(
  sql: string,
  tables: Map<string, Table>,
  engine: Engine,
  emit: (event: QueryEvent) => void,
): void {
  const span = { start: 0, end: sql.length, line: 1 };
  const started = performance.now();
  emit({ event: "started", data: { index: 0, sql, span } });
  const base: StatementResult = {
    index: 0,
    sql,
    span,
    kind: "empty",
    columns: [],
    resultSets: 0,
    rowCount: 0,
    truncated: false,
    affected: null,
    lastInsertId: null,
    startedAt: new Date().toISOString(),
    durationMs: 1,
    error: null,
    notices: [],
  };
  let outcome: Outcome;
  try {
    outcome = execute(sql, tables, engine);
  } catch (err) {
    const error: ErrorPayload =
      err instanceof SqlFailure
        ? sqliteError(engine, err.message, err.code, 0)
        : sqliteError(engine, String(err), 1, 0);
    emit({ event: "failed", data: { index: 0, error } });
    return;
  }
  const durationMs = Math.max(1, Math.round(performance.now() - started));
  if (outcome.kind === "rows") {
    emit({ event: "columns", data: { index: 0, set: 0, columns: outcome.columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows: outcome.rows } });
    const result: StatementResult = {
      ...base,
      kind: "rows",
      columns: outcome.columns,
      resultSets: 1,
      rowCount: outcome.rows.length,
      durationMs,
    };
    emit({ event: "finished", data: { index: 0, result } });
    return;
  }
  const result: StatementResult =
    outcome.kind === "affected"
      ? {
          ...base,
          kind: "affected",
          affected: outcome.affected,
          lastInsertId: outcome.lastInsertId,
          durationMs,
        }
      : { ...base, durationMs };
  emit({ event: "finished", data: { index: 0, result } });
}
