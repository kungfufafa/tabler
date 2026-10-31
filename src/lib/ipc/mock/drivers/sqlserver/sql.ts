import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import { mockError } from "../../transport";
import {
  type Col,
  coerce,
  constraintError,
  type Database,
  display,
  int,
  NULL,
  rowsOf,
  sameValue,
  ServerFailure,
  type Table,
  tableIn,
  text,
} from "./data";

/**
 * A small T-SQL interpreter for the mock's editor: one GO batch at a time (as the backend sends
 * it), its statements without terminators: SELECT (TOP, WHERE, ORDER BY, OFFSET / FETCH,
 * COUNT(*), `@@VERSION` and the usual session functions), INSERT, UPDATE, DELETE, USE and
 * transactions. Errors carry SQL Server's numbers (208 an unknown object, 207 an unknown column,
 * 102 a syntax error). Like the backend, a batch reports a count only when it is one INSERT,
 * UPDATE or DELETE (`tsql::is_lone_dml`).
 */

/** What a batch runs against: the session's database, its transaction and who it is. */
export interface Batch {
  databases: Map<string, Database>;
  /** The session's database (`USE` changes it). */
  current: string;
  /** `@@TRANCOUNT`, and the rows to go back to on ROLLBACK. */
  trancount: number;
  snapshot: Map<Table, Value[][]> | null;
  version: string;
  login: string;
  spid: number;
}

export interface ResultSet {
  columns: ResultColumn[];
  rows: Value[][];
}

export type Outcome =
  { kind: "rows"; sets: ResultSet[] } | { kind: "affected"; affected: number } | { kind: "empty" };

// --- tokens -----------------------------------------------------------------------------------

type Tok =
  | { t: "word"; v: string; u: string }
  | { t: "ident"; v: string }
  | { t: "str"; v: string }
  | { t: "num"; v: string }
  | { t: "hex"; v: string }
  | { t: "var"; v: string; u: string }
  | { t: "punct"; v: string };

function tokens(sql: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  const n = sql.length;
  while (i < n) {
    const c = sql[i]!;
    if (/\s/.test(c)) {
      i++;
    } else if (sql.startsWith("--", i)) {
      while (i < n && sql[i] !== "\n") i++;
    } else if (sql.startsWith("/*", i)) {
      // T-SQL nests block comments.
      let depth = 0;
      do {
        if (sql.startsWith("/*", i)) {
          depth++;
          i += 2;
        } else if (sql.startsWith("*/", i)) {
          depth--;
          i += 2;
        } else i++;
      } while (i < n && depth > 0);
    } else if (c === "[" || c === '"') {
      const close = c === "[" ? "]" : '"';
      let v = "";
      i++;
      while (i < n) {
        if (sql[i] === close) {
          if (sql[i + 1] === close) {
            v += close;
            i += 2;
            continue;
          }
          i++;
          break;
        }
        v += sql[i++];
      }
      out.push({ t: "ident", v });
    } else if (c === "'" || ((c === "N" || c === "n") && sql[i + 1] === "'")) {
      i += c === "'" ? 1 : 2;
      let v = "";
      while (i < n) {
        if (sql[i] === "'") {
          if (sql[i + 1] === "'") {
            v += "'";
            i += 2;
            continue;
          }
          i++;
          break;
        }
        v += sql[i++];
      }
      out.push({ t: "str", v });
    } else if (c === "0" && /[xX]/.test(sql[i + 1] ?? "")) {
      const m = /^0[xX][0-9a-fA-F]*/.exec(sql.slice(i))![0];
      out.push({ t: "hex", v: m });
      i += m.length;
    } else if (/[0-9.]/.test(c) && /[0-9]/.test(c === "." ? (sql[i + 1] ?? "") : c)) {
      const m = /^\d*\.?\d+(?:[eE][-+]?\d+)?|^\d+\.?/.exec(sql.slice(i))![0];
      out.push({ t: "num", v: m });
      i += m.length;
    } else if (c === "@") {
      const m = /^@@?[\p{L}\p{N}_$#@]*/u.exec(sql.slice(i))![0];
      out.push({ t: "var", v: m, u: m.toUpperCase() });
      i += m.length;
    } else if (/[\p{L}_#]/u.test(c)) {
      const m = /^[\p{L}_#][\p{L}\p{N}_$#@]*/u.exec(sql.slice(i))![0];
      out.push({ t: "word", v: m, u: m.toUpperCase() });
      i += m.length;
    } else {
      const two = sql.slice(i, i + 2);
      const v = ["<>", "<=", ">=", "!=", "!<", "!>"].includes(two) ? two : c;
      out.push({ t: "punct", v });
      i += v.length;
    }
  }
  return out;
}

// --- the parser --------------------------------------------------------------------------------

const syntax = (near: Tok | undefined) =>
  new ServerFailure(
    102,
    near ? `Incorrect syntax near '${near.v}'.` : "Incorrect syntax near the end of the batch.",
  );

/** The words that start a statement of a batch (no terminator needed between them). */
const STARTS = new Set([
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "USE",
  "BEGIN",
  "COMMIT",
  "ROLLBACK",
  "SET",
  "PRINT",
  "DECLARE",
]);

/** An expression the interpreter evaluates: a column of the row, or a value of its own. */
type Expr =
  | { e: "col"; name: string }
  | { e: "value"; v: Value }
  | { e: "default" }
  | { e: "count" }
  | { e: "star" };

type Cond =
  | { c: "cmp"; left: Expr; op: string; right: Expr }
  | { c: "null"; left: Expr; not: boolean }
  | { c: "like"; left: Expr; pattern: Expr; not: boolean }
  | { c: "in"; left: Expr; list: Expr[]; not: boolean }
  | { c: "and" | "or"; parts: Cond[] };

/** The words that end a select item or a table name. */
const CLAUSES = new Set([
  "FROM",
  "WHERE",
  "ORDER",
  "GROUP",
  "OFFSET",
  "FETCH",
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "USE",
  "BEGIN",
  "COMMIT",
  "ROLLBACK",
  "SET",
  "PRINT",
  "DECLARE",
  "VALUES",
  "OPTION",
]);

class Parser {
  i = 0;
  constructor(
    readonly toks: Tok[],
    readonly batch: Batch,
  ) {}

  peek(k = 0): Tok | undefined {
    return this.toks[this.i + k];
  }
  next(): Tok {
    const t = this.toks[this.i++];
    if (!t) throw syntax(undefined);
    return t;
  }
  isWord(u: string, k = 0): boolean {
    const t = this.peek(k);
    return t?.t === "word" && t.u === u;
  }
  isPunct(v: string, k = 0): boolean {
    const t = this.peek(k);
    return t?.t === "punct" && t.v === v;
  }
  word(u: string): void {
    if (!this.isWord(u)) throw syntax(this.peek());
    this.i++;
  }
  punct(v: string): void {
    if (!this.isPunct(v)) throw syntax(this.peek());
    this.i++;
  }
  /** A name: a word or a quoted identifier. */
  name(): string {
    const t = this.next();
    if (t.t === "ident" || (t.t === "word" && !CLAUSES.has(t.u))) return t.v;
    throw syntax(t);
  }
  /** `[db.][schema.]name`. */
  table(): { db: Database; table: Table; written: string } {
    const parts = [this.name()];
    while (this.isPunct(".")) {
      this.i++;
      parts.push(this.name());
    }
    const written = parts.join(".");
    const [dbName, schema, name] =
      parts.length === 3
        ? parts
        : parts.length === 2
          ? [this.batch.current, parts[0], parts[1]]
          : [this.batch.current, "dbo", parts[0]];
    const db = [...this.batch.databases.values()].find(
      (d) => d.name.toLowerCase() === dbName!.toLowerCase(),
    );
    const table = db ? tableIn(db, schema!, name!) : null;
    if (!db || !table) throw new ServerFailure(208, `Invalid object name '${written}'.`);
    // `AS x` or a bare alias is read and not needed: one table per statement.
    if (this.isWord("AS")) {
      this.i++;
      this.name();
    } else if (this.aliasNext()) this.name();
    return { db, table, written };
  }

  /** Whether a bare alias comes next: a name that is no clause word. */
  aliasNext(): boolean {
    const t = this.peek();
    return t?.t === "ident" || (t?.t === "word" && !CLAUSES.has(t.u) && t.u !== "WITH");
  }

  expr(): Expr {
    const t = this.next();
    switch (t.t) {
      case "num":
        return { e: "value", v: /[.eE]/.test(t.v) ? { t: "decimal", v: t.v } : int(Number(t.v)) };
      case "str":
        return { e: "value", v: text(t.v) };
      case "hex":
        return {
          e: "value",
          v: {
            t: "bytes",
            v: {
              base64: btoa(
                (t.v.slice(2).match(/../g) ?? [])
                  .map((h) => String.fromCharCode(parseInt(h, 16)))
                  .join(""),
              ),
              len: Math.floor((t.v.length - 2) / 2),
              truncated: false,
            },
          },
        };
      case "var":
        return { e: "value", v: this.variable(t.u) };
      case "ident":
        return this.qualified(t.v);
      case "punct":
        if (t.v === "*") return { e: "star" };
        if (t.v === "-" && this.peek()?.t === "num") {
          const n = this.next().v;
          return {
            e: "value",
            v: /[.eE]/.test(n) ? { t: "decimal", v: `-${n}` } : int(-Number(n)),
          };
        }
        if (t.v === "(") {
          const inner = this.expr();
          this.punct(")");
          return inner;
        }
        throw syntax(t);
      case "word":
        if (t.u === "NULL") return { e: "value", v: NULL };
        if (t.u === "DEFAULT") return { e: "default" };
        if (this.isPunct("(")) return this.call(t.u);
        if (CLAUSES.has(t.u)) throw syntax(t);
        return this.qualified(t.v);
    }
  }

  /** `a.b.c` read as its last part, the column. */
  qualified(first: string): Expr {
    let name = first;
    while (this.isPunct(".")) {
      this.i++;
      const t = this.next();
      if (t.t === "punct" && t.v === "*") return { e: "star" };
      if (t.t !== "word" && t.t !== "ident") throw syntax(t);
      name = t.v;
    }
    return { e: "col", name };
  }

  call(fn: string): Expr {
    this.punct("(");
    if ((fn === "COUNT" || fn === "COUNT_BIG") && this.isPunct("*")) {
      this.i++;
      this.punct(")");
      return { e: "count" };
    }
    const args: Expr[] = [];
    while (!this.isPunct(")")) {
      args.push(this.expr());
      if (this.isPunct(",")) this.i++;
      else break;
    }
    this.punct(")");
    const now = new Date().toISOString().replace("T", " ").replace("Z", "0000");
    switch (fn) {
      case "DB_NAME":
        return { e: "value", v: text(this.batch.current) };
      case "SUSER_SNAME":
      case "SUSER_NAME":
      case "USER_NAME":
        return { e: "value", v: text(this.batch.login) };
      case "SCHEMA_NAME":
        return { e: "value", v: text("dbo") };
      case "GETDATE":
      case "SYSDATETIME":
      case "CURRENT_TIMESTAMP":
        return { e: "value", v: { t: "timestamp", v: now } };
      case "SYSDATETIMEOFFSET":
        return { e: "value", v: { t: "timestampTz", v: `${now} +00:00` } };
      case "NEWID":
        return { e: "value", v: { t: "uuid", v: crypto.randomUUID().toUpperCase() } };
      case "SERVERPROPERTY":
        return { e: "value", v: text(this.batch.version) };
      case "LEN": {
        const a = args[0];
        return { e: "value", v: a?.e === "value" ? int(display(a.v).trimEnd().length) : NULL };
      }
      case "UPPER":
      case "LOWER": {
        const a = args[0];
        if (a?.e !== "value") throw syntax(this.peek(-1));
        const s = display(a.v);
        return { e: "value", v: text(fn === "UPPER" ? s.toUpperCase() : s.toLowerCase()) };
      }
      default:
        throw new ServerFailure(195, `'${fn}' is not a recognized built-in function name.`);
    }
  }

  variable(u: string): Value {
    switch (u) {
      case "@@VERSION":
        return text(
          `Microsoft ${this.batch.version}\n\tCopyright (C) 2022 Microsoft Corporation\n\tDeveloper Edition (64-bit) on Linux (Ubuntu 22.04.4 LTS) <X64>`,
        );
      case "@@SPID":
        return int(this.batch.spid);
      case "@@TRANCOUNT":
        return int(this.batch.trancount);
      case "@@SERVERNAME":
        return text("MOCKSQL01");
      case "@@ROWCOUNT":
        return int(0);
      default:
        throw new ServerFailure(137, `Must declare the scalar variable "${u}".`);
    }
  }

  cond(): Cond {
    const ors: Cond[] = [this.andCond()];
    while (this.isWord("OR")) {
      this.i++;
      ors.push(this.andCond());
    }
    return ors.length === 1 ? ors[0]! : { c: "or", parts: ors };
  }
  andCond(): Cond {
    const ands: Cond[] = [this.term()];
    while (this.isWord("AND")) {
      this.i++;
      ands.push(this.term());
    }
    return ands.length === 1 ? ands[0]! : { c: "and", parts: ands };
  }
  term(): Cond {
    if (this.isPunct("(")) {
      this.i++;
      const inner = this.cond();
      this.punct(")");
      return inner;
    }
    const left = this.expr();
    if (this.isWord("IS")) {
      this.i++;
      const not = this.isWord("NOT");
      if (not) this.i++;
      this.word("NULL");
      return { c: "null", left, not };
    }
    const not = this.isWord("NOT");
    if (not) this.i++;
    if (this.isWord("LIKE")) {
      this.i++;
      return { c: "like", left, pattern: this.expr(), not };
    }
    if (this.isWord("IN")) {
      this.i++;
      this.punct("(");
      const list: Expr[] = [this.expr()];
      while (this.isPunct(",")) {
        this.i++;
        list.push(this.expr());
      }
      this.punct(")");
      return { c: "in", left, list, not };
    }
    if (not) throw syntax(this.peek());
    const op = this.next();
    if (op.t !== "punct" || !["=", "<>", "!=", "<", ">", "<=", ">="].includes(op.v))
      throw syntax(op);
    return { c: "cmp", left, op: op.v, right: this.expr() };
  }

  /** Whether the next token starts another statement of the batch, or ends it. */
  atEnd(): boolean {
    const t = this.peek();
    return (
      t === undefined || (t.t === "punct" && t.v === ";") || (t.t === "word" && STARTS.has(t.u))
    );
  }
}

// --- evaluation --------------------------------------------------------------------------------

function columnIndex(table: Table | null, name: string): number {
  const i = table ? table.cols.findIndex((c) => c.name.toLowerCase() === name.toLowerCase()) : -1;
  if (i < 0) throw new ServerFailure(207, `Invalid column name '${name}'.`);
  return i;
}

function valueOf(e: Expr, table: Table | null, row: Value[] | null): Value {
  switch (e.e) {
    case "value":
      return e.v;
    case "col":
      return row?.[columnIndex(table, e.name)] ?? NULL;
    default:
      throw new ServerFailure(102, "Incorrect syntax near '*'.");
  }
}

/** -1, 0 or 1; null when either side is NULL (UNKNOWN). */
function compare(a: Value, b: Value): number | null {
  if (a.t === "null" || b.t === "null") return null;
  const num = (v: Value) =>
    v.t === "int" || v.t === "float"
      ? v.v
      : v.t === "bigInt" || v.t === "decimal"
        ? Number(v.v)
        : NaN;
  const [na, nb] = [num(a), num(b)];
  if (!Number.isNaN(na) && !Number.isNaN(nb)) {
    if (a.t === "bigInt" || b.t === "bigInt") {
      const [x, y] = [BigInt(display(a).split(".")[0]!), BigInt(display(b).split(".")[0]!)];
      return x < y ? -1 : x > y ? 1 : 0;
    }
    return na < nb ? -1 : na > nb ? 1 : 0;
  }
  const [sa, sb] = [display(a).toLowerCase(), display(b).toLowerCase()];
  return sa < sb ? -1 : sa > sb ? 1 : 0;
}

function holds(c: Cond, table: Table | null, row: Value[]): boolean {
  switch (c.c) {
    case "and":
      return c.parts.every((p) => holds(p, table, row));
    case "or":
      return c.parts.some((p) => holds(p, table, row));
    case "null":
      return (valueOf(c.left, table, row).t === "null") !== c.not;
    case "like": {
      const v = valueOf(c.left, table, row);
      if (v.t === "null") return false;
      const pattern = display(valueOf(c.pattern, table, row))
        .replace(/[.*+?^${}()|\\]/g, "\\$&")
        .replace(/%/g, ".*")
        .replace(/_/g, ".");
      return new RegExp(`^${pattern}$`, "is").test(display(v)) !== c.not;
    }
    case "in": {
      const v = valueOf(c.left, table, row);
      const found = c.list.some((e) => compare(v, valueOf(e, table, row)) === 0);
      return v.t !== "null" && found !== c.not;
    }
    case "cmp": {
      const r = compare(valueOf(c.left, table, row), valueOf(c.right, table, row));
      if (r === null) return false;
      switch (c.op) {
        case "=":
          return r === 0;
        case "<>":
        case "!=":
          return r !== 0;
        case "<":
          return r < 0;
        case ">":
          return r > 0;
        case "<=":
          return r <= 0;
        default:
          return r >= 0;
      }
    }
  }
}

/** A column of a result: the server's base type name, as `decode.rs` reports it. */
const resultColumn = (name: string, c: Col | null, v?: Value): ResultColumn => ({
  name,
  dataType: c
    ? c.type.replace(/\(.*\)$/, "")
    : v?.t === "int"
      ? "int"
      : v?.t === "decimal"
        ? "decimal"
        : "nvarchar",
  kind: c
    ? c.kind
    : v?.t === "int"
      ? "int"
      : v?.t === "decimal"
        ? "decimal"
        : v?.t === "timestamp"
          ? "timestamp"
          : v?.t === "timestampTz"
            ? "timestampTz"
            : v?.t === "uuid"
              ? "uuid"
              : "text",
  nullable: c ? c.nullable : null,
  table: null,
});

function select(p: Parser): ResultSet {
  let top: number | null = null;
  if (p.isWord("TOP")) {
    p.i++;
    const paren = p.isPunct("(");
    if (paren) p.i++;
    const n = p.next();
    if (n.t !== "num") throw syntax(n);
    top = Number(n.v);
    if (paren) p.punct(")");
  }
  const items: { expr: Expr; alias: string | null }[] = [];
  do {
    if (p.isPunct(",")) p.i++;
    const expr = p.expr();
    let alias: string | null = null;
    if (p.isWord("AS")) {
      p.i++;
      alias = p.name();
    } else if (p.aliasNext()) alias = p.name();
    items.push({ expr, alias });
  } while (p.isPunct(","));
  let from: { db: Database; table: Table } | null = null;
  if (p.isWord("FROM")) {
    p.i++;
    from = p.table();
  }
  const table = from?.table ?? null;
  let rows: Value[][] = from ? rowsOf(from.db, from.table).map((r) => r.slice()) : [[]];
  if (p.isWord("WHERE")) {
    p.i++;
    const where = p.cond();
    rows = rows.filter((r) => holds(where, table, r));
  }
  if (p.isWord("ORDER")) {
    p.i++;
    p.word("BY");
    const keys: { at: number; desc: boolean }[] = [];
    do {
      if (p.isPunct(",")) p.i++;
      const e = p.expr();
      if (e.e !== "col") throw syntax(p.peek(-1));
      const at = columnIndex(table, e.name);
      let desc = false;
      if (p.isWord("ASC") || p.isWord("DESC")) desc = p.next().v.toUpperCase() === "DESC";
      keys.push({ at, desc });
    } while (p.isPunct(","));
    rows.sort((a, b) => {
      for (const k of keys) {
        const [x, y] = [a[k.at] ?? NULL, b[k.at] ?? NULL];
        // NULLs sort first in SQL Server's ascending order.
        const r = x.t === "null" ? (y.t === "null" ? 0 : -1) : y.t === "null" ? 1 : compare(x, y)!;
        if (r !== 0) return k.desc ? -r : r;
      }
      return 0;
    });
    if (p.isWord("OFFSET")) {
      p.i++;
      const skip = Number(p.next().v);
      p.word(p.isWord("ROW") ? "ROW" : "ROWS");
      let take = Infinity;
      if (p.isWord("FETCH")) {
        p.i++;
        p.next();
        take = Number(p.next().v);
        p.next();
        p.word("ONLY");
      }
      rows = rows.slice(skip, skip + take);
    }
  }
  if (items.some((it) => it.expr.e === "count")) {
    if (items.length > 1)
      throw new ServerFailure(
        8120,
        "Column is invalid in the select list because it is not contained in either an aggregate function or the GROUP BY clause.",
      );
    return {
      columns: [resultColumn(items[0]!.alias ?? "", null, int(0))],
      rows: [[int(rows.length)]],
    };
  }
  if (top !== null) rows = rows.slice(0, top);
  const columns: ResultColumn[] = [];
  const pick: ((r: Value[]) => Value)[] = [];
  for (const it of items) {
    if (it.expr.e === "star") {
      if (!table) throw new ServerFailure(263, "Must specify table to select from.");
      table.cols.forEach((c, k) => {
        columns.push(resultColumn(c.name, c));
        pick.push((r) => r[k] ?? NULL);
      });
    } else if (it.expr.e === "col") {
      const k = columnIndex(table, it.expr.name);
      columns.push(resultColumn(it.alias ?? table!.cols[k]!.name, table!.cols[k]!));
      pick.push((r) => r[k] ?? NULL);
    } else {
      const v = valueOf(it.expr, table, null);
      columns.push(resultColumn(it.alias ?? "", null, v));
      pick.push(() => v);
    }
  }
  return { columns, rows: rows.map((r) => pick.map((f) => f(r))) };
}

/** A column's default as the server fills it in. */
function defaultOf(c: Col): Value {
  const d = c.default;
  if (d === undefined) return NULL;
  if (d === "newid()") return { t: "uuid", v: crypto.randomUUID().toUpperCase() };
  if (d === "sysdatetime()")
    return { t: "timestamp", v: new Date().toISOString().replace("T", " ").replace("Z", "0000") };
  const s = /^N?'(.*)'$/.exec(d);
  return coerce(c, s ? text(s[1]!) : text(d));
}

/** A value for column `c` of an INSERT or UPDATE. */
function cellFor(t: Table, c: Col, e: Expr, row: Value[] | null): Value {
  if (e.e === "default") return defaultOf(c);
  return coerce(c, valueOf(e, t, row));
}

function refuseView(t: Table) {
  if (t.kind === "view")
    throw new ServerFailure(
      4405,
      `View or function '${t.schema}.${t.name}' is not updatable because the modification affects multiple base tables.`,
    );
}

function insert(p: Parser): number {
  p.word("INSERT");
  if (p.isWord("INTO")) p.i++;
  const { db, table: t } = p.table();
  refuseView(t);
  let cols = t.cols.filter((c) => !c.identity).map((c) => c.name);
  if (p.isPunct("(")) {
    p.i++;
    cols = [p.name()];
    while (p.isPunct(",")) {
      p.i++;
      cols.push(p.name());
    }
    p.punct(")");
  }
  const at = cols.map((name) => columnIndex(t, name));
  for (const k of at)
    if (t.cols[k]!.identity)
      throw new ServerFailure(
        544,
        `Cannot insert explicit value for identity column in table '${t.name}' when IDENTITY_INSERT is set to OFF.`,
      );
  const rows: Expr[][] = [];
  if (p.isWord("DEFAULT")) {
    p.i++;
    p.word("VALUES");
    rows.push([]);
  } else {
    p.word("VALUES");
    do {
      if (p.isPunct(",")) p.i++;
      p.punct("(");
      const values: Expr[] = [p.expr()];
      while (p.isPunct(",")) {
        p.i++;
        values.push(p.expr());
      }
      p.punct(")");
      if (values.length !== at.length)
        throw new ServerFailure(
          values.length > at.length ? 110 : 109,
          `There are ${values.length > at.length ? "more" : "fewer"} columns in the INSERT statement than values specified in the VALUES clause. The number of values in the VALUES clause must match the number of columns specified in the INSERT statement.`,
        );
      rows.push(values);
    } while (p.isPunct(","));
  }
  const added: Value[][] = [];
  for (const values of rows) {
    const row = t.cols.map((c) => (c.identity ? int(t.nextId) : defaultOf(c)));
    values.forEach((e, j) => {
      const k = at[j]!;
      row[k] = cellFor(t, t.cols[k]!, e, null);
    });
    const broken = constraintError(db, { ...t, data: [...t.data, ...added] }, row, -1);
    if (broken) throw broken;
    if (t.cols.some((c) => c.identity)) t.nextId++;
    added.push(row);
  }
  t.data.push(...added);
  return added.length;
}

function update(p: Parser): number {
  p.word("UPDATE");
  const { db, table: t } = p.table();
  refuseView(t);
  p.word("SET");
  const sets: { k: number; e: Expr }[] = [];
  do {
    if (p.isPunct(",")) p.i++;
    const k = columnIndex(t, p.name());
    if (t.cols[k]!.identity)
      throw new ServerFailure(8102, `Cannot update identity column '${t.cols[k]!.name}'.`);
    p.punct("=");
    sets.push({ k, e: p.expr() });
  } while (p.isPunct(","));
  let where: Cond | null = null;
  if (p.isWord("WHERE")) {
    p.i++;
    where = p.cond();
  }
  const hit = t.data.flatMap((r, i) => (!where || holds(where, t, r) ? [i] : []));
  const next = t.data.map((r) => r.slice());
  for (const i of hit)
    for (const { k, e } of sets) next[i]![k] = cellFor(t, t.cols[k]!, e, t.data[i]!);
  for (const i of hit) {
    const broken = constraintError(db, { ...t, data: next }, next[i]!, i);
    if (broken) throw broken;
  }
  t.data.splice(0, t.data.length, ...next);
  return hit.length;
}

function remove(p: Parser): number {
  p.word("DELETE");
  if (p.isWord("FROM")) p.i++;
  const { db, table: t } = p.table();
  refuseView(t);
  let where: Cond | null = null;
  if (p.isWord("WHERE")) {
    p.i++;
    where = p.cond();
  }
  const keep = t.data.filter((r) => where && !holds(where, t, r));
  const gone = t.data.length - keep.length;
  // A row another table still references stops the DELETE (NO ACTION).
  for (const other of db.tables)
    for (const fk of other.foreignKeys) {
      if (fk.refTable.schema !== t.schema || fk.refTable.name !== t.name) continue;
      const refAt = columnIndex(t, fk.refColumns[0]!);
      const fkAt = columnIndex(other, fk.columns[0]!);
      const removed = t.data.filter((r) => !keep.includes(r));
      if (other.data.some((o) => removed.some((r) => sameValue(o[fkAt]!, r[refAt]!))))
        throw new ServerFailure(
          547,
          `The DELETE statement conflicted with the REFERENCE constraint "${fk.name}". The conflict occurred in database "${db.name}", table "${other.schema}.${other.name}", column '${fk.columns[0]}'.`,
        );
    }
  t.data.splice(0, t.data.length, ...keep);
  return gone;
}

/** Every table's rows, for a ROLLBACK. */
function snapshotOf(batch: Batch): Map<Table, Value[][]> {
  const out = new Map<Table, Value[][]>();
  for (const db of batch.databases.values())
    for (const t of db.tables)
      out.set(
        t,
        t.data.map((r) => r.slice()),
      );
  return out;
}

/** Runs one GO batch. Throws a `ServerFailure` (or a mock error) at the first failing statement. */
export function runBatch(sql: string, batch: Batch): Outcome {
  const p = new Parser(tokens(sql), batch);
  const sets: ResultSet[] = [];
  let statements = 0;
  let dml: number | null = null;
  while (p.peek()) {
    if (p.isPunct(";")) {
      p.i++;
      continue;
    }
    const t = p.peek()!;
    if (t.t !== "word") throw syntax(t);
    statements++;
    dml = null;
    switch (t.u) {
      case "SELECT":
        p.i++;
        sets.push(select(p));
        break;
      case "INSERT":
        dml = insert(p);
        break;
      case "UPDATE":
        dml = update(p);
        break;
      case "DELETE":
        dml = remove(p);
        break;
      case "USE": {
        p.i++;
        const name = p.name();
        const db = [...batch.databases.values()].find(
          (d) => d.name.toLowerCase() === name.toLowerCase(),
        );
        if (!db)
          throw new ServerFailure(
            911,
            `Database '${name}' does not exist. Make sure that the name is entered correctly.`,
          );
        batch.current = db.name;
        break;
      }
      case "BEGIN":
        p.i++;
        if (!p.isWord("TRAN") && !p.isWord("TRANSACTION")) throw syntax(p.peek());
        p.i++;
        if (batch.trancount === 0) batch.snapshot = snapshotOf(batch);
        batch.trancount++;
        break;
      case "COMMIT":
      case "ROLLBACK":
        p.i++;
        if (p.isWord("TRAN") || p.isWord("TRANSACTION") || p.isWord("WORK")) p.i++;
        if (batch.trancount === 0)
          throw new ServerFailure(
            t.u === "COMMIT" ? 3902 : 3903,
            `The ${t.u} TRANSACTION request has no corresponding BEGIN TRANSACTION.`,
          );
        if (t.u === "ROLLBACK")
          for (const [table, rows] of batch.snapshot ?? [])
            table.data.splice(0, table.data.length, ...rows);
        batch.trancount = t.u === "COMMIT" ? batch.trancount - 1 : 0;
        if (batch.trancount === 0) batch.snapshot = null;
        break;
      case "SET":
      case "PRINT":
        // Session options and messages: the backend shows no PRINT output either.
        p.i++;
        while (!p.atEnd()) p.i++;
        break;
      default:
        return mockError(
          "UNSUPPORTED",
          `unsupported: the mock's SQL Server runs SELECT, INSERT, UPDATE, DELETE, USE, SET and transactions, not ${t.u}`,
        );
    }
    if (p.peek() && !p.atEnd()) throw syntax(p.peek());
  }
  if (sets.length > 0) return { kind: "rows", sets };
  // `@@ROWCOUNT` after the batch counts a lone INSERT, UPDATE or DELETE only.
  if (statements === 1 && dml !== null) return { kind: "affected", affected: dml };
  return { kind: "empty" };
}
