import { scan, text as tokenText } from "@/features/drivers/portable/scanner";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import CLICKHOUSE_LEX from "../../../bindings/lex/clickhouse.json";
import {
  type Col,
  columnsOf,
  createQuery,
  type Database,
  display,
  enumLabels,
  integer,
  kindOf,
  NULL,
  type Table,
  tableKind,
} from "./data";
import { coerce, ServerFailure } from "./values";

/**
 * The editor's statements on the mock's server: the subset of ClickHouse SQL a demo needs, with
 * the server's errors in its words. SELECT with a column list or `*`, `count()`, `sum`, `min`,
 * `max`, `avg` and `uniqExact` (with GROUP BY on columns), the scalar functions of the session
 * (`version()`, `currentDatabase()`, `currentUser()`, `now()`, `today()`, `timezone()`,
 * `getSetting('…')`), WHERE with AND / OR of comparisons, LIKE, ILIKE, IN and IS NULL, ORDER BY,
 * LIMIT and OFFSET, over a table or `numbers(N)`; SHOW DATABASES / TABLES / CREATE TABLE,
 * DESCRIBE, EXISTS, EXPLAIN, SET (the editor's HTTP session keeps it), INSERT … VALUES,
 * ALTER TABLE … UPDATE / DELETE (mutations), lightweight DELETE, TRUNCATE, DROP, CREATE DATABASE,
 * CREATE TABLE and OPTIMIZE. Anything else is a syntax error.
 */

export const LEX = CLICKHOUSE_LEX as unknown as DialectLex;

/** What the editor's HTTP session (`session_id`) keeps between statements. */
export interface Editor {
  settings: Map<string, string>;
}

/** What a statement runs against. */
export interface SqlContext {
  databases: Map<string, Database>;
  /** The database the requests start in (the profile's, else `default`). */
  current: string;
  user: string;
  version: string;
  editor: Editor;
  /** `KILL QUERY` on the server's process list; the ids it stopped. */
  kill: (queryId: string) => boolean;
}

export type Outcome =
  | { kind: "rows"; columns: ResultColumn[]; rows: Value[][] }
  | { kind: "affected"; affected: number }
  | { kind: "empty" };

// --- tokens ------------------------------------------------------------------------------------

type Tok =
  | { k: "word"; v: string; up: string; at: number }
  | { k: "num"; v: string; at: number }
  | { k: "str"; v: string; at: number }
  | { k: "ident"; v: string; at: number }
  | { k: "punct"; v: string; at: number };

/** Undoes backslash escapes (`\n`, `\'`, `\\`). */
function unescape(s: string): string {
  return s.replace(/\\(.)/gs, (_, c: string) =>
    c === "n" ? "\n" : c === "t" ? "\t" : c === "r" ? "\r" : c === "0" ? "\0" : c,
  );
}

function tokens(sql: string): Tok[] {
  const s = scan(LEX.lex, LEX.lex.readings[0]!, sql);
  const out: Tok[] = [];
  for (const t of s.tokens) {
    const raw = tokenText(s, t);
    if (t.kind === "space" || t.kind === "comment") continue;
    if (t.kind === "word") {
      if (/^\d/.test(raw)) out.push({ k: "num", v: raw, at: t.start });
      else out.push({ k: "word", v: raw, up: raw.toUpperCase(), at: t.start });
    } else if (t.kind === "str") {
      out.push({
        k: "str",
        v: raw.startsWith("$") ? raw.replace(/^\$\w*\$|\$\w*\$$/g, "") : unescape(raw.slice(1, -1)),
        at: t.start,
      });
    } else if (t.kind === "ident")
      out.push({ k: "ident", v: unescape(raw.slice(1, -1)), at: t.start });
    else out.push({ k: "punct", v: raw, at: t.start });
  }
  return out;
}

const syntax = (sql: string, at: number) =>
  new ServerFailure(
    62,
    `Syntax error: failed at position ${at + 1} (${JSON.stringify(sql.slice(at, at + 20) || "end of query")})`,
    "SYNTAX_ERROR",
  );

class Parser {
  i = 0;
  constructor(
    readonly sql: string,
    readonly toks: Tok[],
  ) {}
  peek(o = 0): Tok | undefined {
    return this.toks[this.i + o];
  }
  fail(): never {
    throw syntax(this.sql, this.peek()?.at ?? this.sql.length);
  }
  isWord(w: string, o = 0): boolean {
    const t = this.peek(o);
    return t?.k === "word" && t.up === w;
  }
  word(w: string): boolean {
    if (!this.isWord(w)) return false;
    this.i++;
    return true;
  }
  expectWord(w: string): void {
    if (!this.word(w)) this.fail();
  }
  isPunct(p: string, o = 0): boolean {
    const t = this.peek(o);
    return t?.k === "punct" && t.v === p;
  }
  punct(p: string): boolean {
    if (!this.isPunct(p)) return false;
    this.i++;
    return true;
  }
  expectPunct(p: string): void {
    if (!this.punct(p)) this.fail();
  }
  /** A name: a bare word or a quoted identifier. */
  name(): string {
    const t = this.peek();
    if (t?.k === "word" || t?.k === "ident") {
      this.i++;
      return t.v;
    }
    return this.fail();
  }
  /** `[db.]name`. */
  qualified(current: string): { db: string; name: string } {
    const first = this.name();
    if (this.punct(".")) return { db: first, name: this.name() };
    return { db: current, name: first };
  }
  /** Takes the first of `words` that comes next, if any. */
  anyWord(...words: string[]): boolean {
    return words.some((w) => this.word(w));
  }
  /** `IF [NOT] EXISTS`. */
  ifExists(not: boolean): boolean {
    if (!this.word("IF")) return false;
    if (not) this.expectWord("NOT");
    this.expectWord("EXISTS");
    return true;
  }
  done(): boolean {
    // A trailing FORMAT clause changes only the wire format, which the app reads itself.
    if (this.word("FORMAT")) this.name();
    this.punct(";");
    return this.i >= this.toks.length;
  }
  end(): void {
    if (!this.done()) this.fail();
  }
}

// --- expressions -------------------------------------------------------------------------------

type Expr =
  | { e: "lit"; v: Value }
  | { e: "col"; name: string }
  | { e: "star" }
  | { e: "call"; fn: string; args: Expr[] };

type Cond =
  | { c: "cmp"; op: string; left: Expr; right: Expr }
  | { c: "in"; left: Expr; values: Expr[]; not: boolean }
  | { c: "null"; left: Expr; not: boolean }
  | { c: "and" | "or"; parts: Cond[] }
  | { c: "true" };

function literalOf(p: Parser): Value | null {
  const t = p.peek();
  const neg = t?.k === "punct" && t.v === "-" && p.peek(1)?.k === "num";
  const n = neg ? p.peek(1) : t;
  if (n?.k === "num") {
    p.i += neg ? 2 : 1;
    let digits = n.v;
    if (p.isPunct(".") && p.peek(1)?.k === "num") {
      p.i++;
      digits += `.${p.peek()!.v}`;
      p.i++;
    }
    const s = `${neg ? "-" : ""}${digits}`;
    if (/^-?\d+$/.test(s)) return integer(BigInt(s));
    if (/^-?\d+\.\d+$/.test(s)) return { t: "float", v: Number(s) };
    return p.fail();
  }
  if (t?.k === "str") {
    p.i++;
    return { t: "text", v: t.v };
  }
  if (p.word("NULL")) return NULL;
  if (p.word("TRUE")) return { t: "bool", v: true };
  if (p.word("FALSE")) return { t: "bool", v: false };
  if (p.isPunct("[")) {
    p.i++;
    const items: Value[] = [];
    if (!p.punct("]")) {
      do items.push(literalOf(p) ?? p.fail());
      while (p.punct(","));
      p.expectPunct("]");
    }
    return { t: "array", v: items };
  }
  return null;
}

function exprOf(p: Parser): Expr {
  const lit = literalOf(p);
  if (lit) return { e: "lit", v: lit };
  if (p.punct("*")) return { e: "star" };
  const t = p.peek();
  if ((t?.k === "word" || t?.k === "ident") && p.isPunct("(", 1)) {
    p.i += 2;
    const args: Expr[] = [];
    if (!p.punct(")")) {
      do args.push(exprOf(p));
      while (p.punct(","));
      p.expectPunct(")");
    }
    return { e: "call", fn: t.v, args };
  }
  const name = p.name();
  // `t.col`: the column of the one table a query reads.
  if (p.punct(".")) return { e: "col", name: p.name() };
  return { e: "col", name };
}

const CMP = ["=", "==", "!=", "<>", "<", ">", "<=", ">="];

function condAtom(p: Parser): Cond {
  if (p.punct("(")) {
    const c = condOf(p);
    p.expectPunct(")");
    return c;
  }
  const left = exprOf(p);
  if (p.word("IS")) {
    const not = p.word("NOT");
    p.expectWord("NULL");
    return { c: "null", left, not };
  }
  const not = p.word("NOT");
  if (p.word("IN")) {
    p.expectPunct("(");
    const values: Expr[] = [];
    do values.push(exprOf(p));
    while (p.punct(","));
    p.expectPunct(")");
    return { c: "in", left, values, not };
  }
  for (const op of ["LIKE", "ILIKE"])
    if (p.word(op)) return { c: "cmp", op: not ? `NOT ${op}` : op, left, right: exprOf(p) };
  if (not) p.fail();
  // `<=` and `>=` come as two characters, `!=` and `==` too.
  let op = "";
  const a = p.peek();
  if (a?.k === "punct" && "=<>!".includes(a.v)) {
    op = a.v;
    p.i++;
    const b = p.peek();
    if (b?.k === "punct" && "=>".includes(b.v) && CMP.includes(op + b.v)) {
      op += b.v;
      p.i++;
    }
  }
  if (!CMP.includes(op)) p.fail();
  return { c: "cmp", op, left, right: exprOf(p) };
}

function condOf(p: Parser): Cond {
  const ors: Cond[] = [];
  do {
    const ands: Cond[] = [];
    do ands.push(condAtom(p));
    while (p.word("AND"));
    ors.push(ands.length === 1 ? ands[0]! : { c: "and", parts: ands });
  } while (p.word("OR"));
  return ors.length === 1 ? ors[0]! : { c: "or", parts: ors };
}

// --- evaluation --------------------------------------------------------------------------------

interface Source {
  label: string;
  cols: { name: string; type: string }[];
  rows: Value[][];
}

const AGGREGATES = ["count", "sum", "min", "max", "avg", "uniqexact"];
const isAggregate = (x: Expr) => x.e === "call" && AGGREGATES.includes(x.fn.toLowerCase());

const pad = (n: number) => String(n).padStart(2, "0");
function nowText(): string {
  const d = new Date();
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
}

const unknownIdentifier = (name: string) =>
  new ServerFailure(
    47,
    `Unknown expression identifier \`${name}\` in scope SELECT`,
    "UNKNOWN_IDENTIFIER",
  );

function scalar(x: Expr, src: Source, row: Value[], ctx: SqlContext): Value {
  switch (x.e) {
    case "lit":
      return x.v;
    case "star":
      throw unknownIdentifier("*");
    case "col": {
      const i = src.cols.findIndex((c) => c.name === x.name);
      if (i < 0) throw unknownIdentifier(x.name);
      return row[i] ?? NULL;
    }
    case "call": {
      const fn = x.fn.toLowerCase();
      const args = x.args.map((a) => scalar(a, src, row, ctx));
      switch (fn) {
        case "version":
          return { t: "text", v: ctx.version };
        case "currentdatabase":
          return { t: "text", v: ctx.current };
        case "currentuser":
          return { t: "text", v: ctx.user };
        case "timezone":
          return { t: "text", v: "UTC" };
        case "now":
          return { t: "timestamp", v: nowText() };
        case "today":
          return { t: "date", v: nowText().slice(0, 10) };
        case "getsetting": {
          const name = args[0] ? display(args[0]) : "";
          const v = ctx.editor.settings.get(name);
          if (v !== undefined) return { t: "text", v };
          if (name === "readonly") return { t: "text", v: "0" };
          throw new ServerFailure(115, `Unknown setting '${name}'`, "UNKNOWN_SETTING");
        }
        case "todate":
          return { t: "date", v: display(args[0] ?? NULL).slice(0, 10) };
        case "lower":
          return { t: "text", v: display(args[0] ?? NULL).toLowerCase() };
        case "upper":
          return { t: "text", v: display(args[0] ?? NULL).toUpperCase() };
        case "length":
          return { t: "int", v: display(args[0] ?? NULL).length };
        case "tostring":
          return { t: "text", v: display(args[0] ?? NULL) };
        default:
          throw new ServerFailure(
            46,
            `Function with name \`${x.fn}\` does not exist. In scope SELECT`,
            "UNKNOWN_FUNCTION",
          );
      }
    }
  }
}

function aggregate(x: Expr & { e: "call" }, src: Source, rows: Value[][], ctx: SqlContext): Value {
  const fn = x.fn.toLowerCase();
  const arg = x.args[0];
  if (fn === "count" && (!arg || arg.e === "star")) return { t: "int", v: rows.length };
  if (!arg)
    throw new ServerFailure(
      42,
      `Aggregate function ${x.fn} requires an argument`,
      "NUMBER_OF_ARGUMENTS_DOESNT_MATCH",
    );
  const values = rows.map((r) => scalar(arg, src, r, ctx)).filter((v) => v.t !== "null");
  switch (fn) {
    case "count":
      return { t: "int", v: values.length };
    case "uniqexact":
      return { t: "int", v: new Set(values.map(display)).size };
    case "min":
    case "max": {
      if (values.length === 0) return NULL;
      const sorted = [...values].sort(compareLoose);
      return fn === "min" ? sorted[0]! : sorted.at(-1)!;
    }
    default: {
      const nums = values.map((v) => Number(display(v)));
      if (nums.some(Number.isNaN))
        throw new ServerFailure(
          43,
          `Illegal type of argument for aggregate function ${fn}`,
          "ILLEGAL_TYPE_OF_ARGUMENT",
        );
      const sum = nums.reduce((a, b) => a + b, 0);
      if (fn === "avg")
        return values.length === 0
          ? { t: "float", v: Number.NaN }
          : { t: "float", v: sum / nums.length };
      const exact = values.every((v) => v.t === "decimal")
        ? {
            t: "decimal" as const,
            v: sum.toFixed(Math.max(...values.map((v) => display(v).split(".")[1]?.length ?? 0))),
          }
        : Number.isInteger(sum)
          ? integer(BigInt(sum))
          : { t: "float" as const, v: sum };
      return exact;
    }
  }
}

function likeRegex(pattern: string, flags: string): RegExp {
  const body = pattern.replace(/\\([%_])|([%_])|([.*+?^${}()|[\]\\])/g, (_, esc, wild, special) =>
    esc ? `\\${esc}` : wild ? (wild === "%" ? ".*" : ".") : `\\${special}`,
  );
  return new RegExp(`^${body}$`, `s${flags}`);
}

function test(c: Cond, src: Source, row: Value[], ctx: SqlContext): boolean {
  switch (c.c) {
    case "true":
      return true;
    case "and":
      return c.parts.every((p) => test(p, src, row, ctx));
    case "or":
      return c.parts.some((p) => test(p, src, row, ctx));
    case "null":
      return (scalar(c.left, src, row, ctx).t === "null") !== c.not;
    case "in": {
      const v = scalar(c.left, src, row, ctx);
      const hit = c.values.some((x) => compareLoose(v, scalar(x, src, row, ctx)) === 0);
      return hit !== c.not;
    }
    case "cmp": {
      const l = scalar(c.left, src, row, ctx);
      const r = scalar(c.right, src, row, ctx);
      if (l.t === "null" || r.t === "null") return false;
      if (c.op.endsWith("LIKE")) {
        const hit = likeRegex(display(r), c.op.endsWith("ILIKE") ? "i" : "").test(display(l));
        return c.op.startsWith("NOT") ? !hit : hit;
      }
      const d = compareLoose(l, r);
      switch (c.op) {
        case "=":
        case "==":
          return d === 0;
        case "!=":
        case "<>":
          return d !== 0;
        case "<":
          return d < 0;
        case ">":
          return d > 0;
        case "<=":
          return d <= 0;
        default:
          return d >= 0;
      }
    }
  }
}

/** Compares a cell with a literal as the server converts the literal to the cell's type. */
function compareLoose(a: Value, b: Value): number {
  const num = (v: Value) =>
    ["int", "bigInt", "float", "decimal"].includes(v.t) ? Number(display(v)) : null;
  const x = num(a);
  const y = num(b);
  if (x !== null && y !== null) return x < y ? -1 : x > y ? 1 : 0;
  const s = display(a);
  const t = display(b);
  return s < t ? -1 : s > t ? 1 : 0;
}

function valueKind(v: Value): ValueKind {
  return v.t === "null" || v.t === "default" ? "text" : v.t === "other" ? "other" : v.t;
}

const TYPE_OF: Partial<Record<ValueKind, string>> = {
  int: "UInt64",
  bigInt: "UInt64",
  float: "Float64",
  decimal: "Decimal(38, 2)",
  bool: "Bool",
  text: "String",
  date: "Date",
  timestamp: "DateTime",
  array: "Array(String)",
};

const column = (name: string, type: string, table: string | null = null): ResultColumn => ({
  name,
  dataType: type,
  kind: kindOf(type),
  nullable: type.startsWith("Nullable("),
  table,
});

// --- statements --------------------------------------------------------------------------------

function findTable(ctx: SqlContext, db: string, name: string): Table {
  const d = ctx.databases.get(db);
  if (!d) throw new ServerFailure(81, `Database ${db} does not exist`, "UNKNOWN_DATABASE");
  const t = d.tables.find((x) => x.name === name);
  if (!t) throw new ServerFailure(60, `Table ${db}.${name} does not exist`, "UNKNOWN_TABLE");
  return t;
}

/** The rows a view shows: its SELECT run again over the tables it reads (a materialized view
 * keeps its own). */
function viewRows(t: Table, ctx: SqlContext): Value[][] {
  if (t.engine !== "View" || !t.asSelect) return t.data;
  const out = runSelect(new Parser(t.asSelect, tokens(t.asSelect)), ctx, false);
  return out.rows;
}

function sourceOf(p: Parser, ctx: SqlContext): Source {
  if (!p.word("FROM"))
    return {
      label: "system.one",
      cols: [{ name: "dummy", type: "UInt8" }],
      rows: [[{ t: "int", v: 0 }]],
    };
  const t = p.peek();
  if (t?.k === "word" && p.isPunct("(", 1)) {
    if (t.up !== "NUMBERS")
      throw new ServerFailure(46, `Unknown table function ${t.v}`, "UNKNOWN_FUNCTION");
    p.i += 2;
    const args: number[] = [];
    do args.push(Number(display(literalOf(p) ?? p.fail())));
    while (p.punct(","));
    p.expectPunct(")");
    const [start, count] = args.length > 1 ? [args[0]!, args[1]!] : [0, args[0]!];
    if (count > 1_000_000)
      throw new ServerFailure(
        158,
        "Limit for rows to read exceeded: the mock reads at most 1000000 rows",
        "TOO_MANY_ROWS",
      );
    return {
      label: "numbers",
      cols: [{ name: "number", type: "UInt64" }],
      rows: Array.from({ length: Math.max(0, count) }, (_, i) => [{ t: "int", v: start + i }]),
    };
  }
  const { db, name } = p.qualified(ctx.current);
  const table = findTable(ctx, db, name);
  return {
    label: `${db}.${name}`,
    cols: table.cols.map((c) => ({ name: c.name, type: c.type })),
    rows: viewRows(table, ctx),
  };
}

/** `SELECT … [FROM …] [WHERE …] [GROUP BY …] [ORDER BY …] [LIMIT …]`. */
function runSelect(p: Parser, ctx: SqlContext, toEnd = true): Extract<Outcome, { kind: "rows" }> {
  p.expectWord("SELECT");
  p.word("DISTINCT");
  const items: { x: Expr; alias: string | null; text: string }[] = [];
  do {
    const from = p.peek()?.at ?? 0;
    const x = exprOf(p);
    const to = p.peek()?.at ?? p.sql.length;
    let alias: string | null = null;
    if (p.word("AS")) alias = p.name();
    items.push({ x, alias, text: p.sql.slice(from, to).trim() });
  } while (p.punct(","));
  const src = sourceOf(p, ctx);
  let rows = src.rows;
  if (p.word("WHERE")) {
    const c = condOf(p);
    rows = rows.filter((r) => test(c, src, r, ctx));
  }
  const groupBy: Expr[] = [];
  if (p.word("GROUP")) {
    p.expectWord("BY");
    do groupBy.push(exprOf(p));
    while (p.punct(","));
  }
  const order: { x: Expr; desc: boolean }[] = [];
  if (p.word("ORDER")) {
    p.expectWord("BY");
    do {
      const x = exprOf(p);
      const desc = p.word("DESC") || p.word("DESCENDING");
      if (!desc) p.anyWord("ASC", "ASCENDING");
      order.push({ x, desc });
    } while (p.punct(","));
  }
  let limit = Infinity;
  let offset = 0;
  if (p.word("LIMIT")) {
    const first = Number(display(literalOf(p) ?? p.fail()));
    if (p.punct(",")) {
      offset = first;
      limit = Number(display(literalOf(p) ?? p.fail()));
    } else limit = first;
    if (p.word("OFFSET")) offset = Number(display(literalOf(p) ?? p.fail()));
  }
  if (toEnd) p.end();

  // `*` expands to the source's columns.
  const outCols: { name: string; type: string; x: Expr }[] = items.flatMap((it) =>
    it.x.e === "star"
      ? src.cols.map((c) => ({ name: c.name, type: c.type, x: { e: "col", name: c.name } as Expr }))
      : [{ name: it.alias ?? it.text, type: "", x: it.x }],
  );
  const grouped = groupBy.length > 0 || outCols.some((c) => isAggregate(c.x));
  let table: Value[][];
  if (grouped) {
    const groups = new Map<string, Value[][]>();
    for (const r of rows) {
      const key = JSON.stringify(groupBy.map((g) => scalar(g, src, r, ctx)));
      groups.set(key, [...(groups.get(key) ?? []), r]);
    }
    if (groups.size === 0 && groupBy.length === 0) groups.set("[]", []);
    table = [...groups.values()].map((members) =>
      outCols.map((c) =>
        isAggregate(c.x)
          ? aggregate(c.x as Expr & { e: "call" }, src, members, ctx)
          : members[0]
            ? scalar(c.x, src, members[0], ctx)
            : NULL,
      ),
    );
  } else table = rows.map((r) => outCols.map((c) => scalar(c.x, src, r, ctx)));
  if (order.length > 0) {
    const keyed = table.map((r, i) => ({
      r,
      k: order.map((o) =>
        grouped
          ? (r[outCols.findIndex((c) => JSON.stringify(c.x) === JSON.stringify(o.x))] ?? NULL)
          : scalar(o.x, src, rows[i]!, ctx),
      ),
    }));
    keyed.sort((a, b) => {
      for (const [n, o] of order.entries()) {
        const d = compareLoose(a.k[n]!, b.k[n]!);
        if (d !== 0) return o.desc ? -d : d;
      }
      return 0;
    });
    table = keyed.map((k) => k.r);
  }
  table = table.slice(offset, offset + limit);
  const columns = outCols.map((c, i) => {
    if (c.type !== "") return column(c.name, c.type, src.label);
    const sample = table.find((r) => r[i]?.t !== "null")?.[i];
    return column(c.name, TYPE_OF[sample ? valueKind(sample) : "text"] ?? "String");
  });
  return { kind: "rows", columns, rows: table };
}

const strings = (names: string[], rows: string[][]): Outcome => ({
  kind: "rows",
  columns: names.map((n) => column(n, "String")),
  rows: rows.map((r) => r.map((v) => ({ t: "text", v }) as Value)),
});

function insert(p: Parser, ctx: SqlContext): Outcome {
  p.expectWord("INTO");
  p.word("TABLE");
  const { db, name } = p.qualified(ctx.current);
  const t = findTable(ctx, db, name);
  if (tableKind(t) !== "table")
    throw new ServerFailure(
      48,
      `Method write is not supported by storage ${t.engine}`,
      "NOT_IMPLEMENTED",
    );
  let names = t.cols.filter((c) => !c.default || c.default.kind === "DEFAULT").map((c) => c.name);
  if (p.punct("(")) {
    names = [];
    do names.push(p.name());
    while (p.punct(","));
    p.expectPunct(")");
  }
  const cols = names.map((n) => {
    const c = t.cols.find((x) => x.name === n);
    if (!c)
      throw new ServerFailure(
        16,
        `No such column ${n} in table ${db}.${name}`,
        "NO_SUCH_COLUMN_IN_TABLE",
      );
    if (c.default && c.default.kind !== "DEFAULT")
      throw new ServerFailure(
        44,
        `Cannot insert column ${n}, because it is ${c.default.kind} column`,
        "ILLEGAL_COLUMN",
      );
    return c;
  });
  const tuples: Value[][] = [];
  if (p.word("VALUES")) {
    do {
      p.expectPunct("(");
      const values: Value[] = [];
      do values.push(literalOf(p) ?? p.fail());
      while (p.punct(","));
      p.expectPunct(")");
      tuples.push(values);
    } while (p.punct(","));
  } else if (p.isWord("SELECT")) {
    const out = runSelect(p, ctx, false);
    tuples.push(...out.rows);
  } else p.fail();
  p.end();
  const rows = tuples.map((values) => {
    if (values.length !== cols.length)
      throw new ServerFailure(
        62,
        `Cannot parse expression: expected ${cols.length} values, got ${values.length}`,
        "SYNTAX_ERROR",
      );
    return t.cols.map((c) => {
      const at = cols.indexOf(c);
      return at >= 0 ? coerce(c.type, values[at]!) : defaultOf(c, cols, values);
    });
  });
  t.data.push(...rows);
  return { kind: "affected", affected: rows.length };
}

/** A column's value when an insert leaves it out: its DEFAULT or MATERIALIZED expression. */
export function defaultOf(c: Col, given: Col[], values: Value[]): Value {
  const expr = c.default?.expr ?? "";
  if (expr === "now()") return { t: "timestamp", v: nowText() };
  const of = /^toDate\((\w+)\)$/.exec(expr);
  if (of) {
    const i = given.findIndex((g) => g.name === of[1]);
    const src = i >= 0 ? values[i] : undefined;
    const text = src ? display(src) : nowText();
    return { t: "date", v: text.slice(0, 10) };
  }
  if (c.default) return { t: "text", v: expr };
  // A column without a default takes its type's zero value.
  switch (kindOf(c.type)) {
    case "int":
    case "bigInt":
      return { t: "int", v: 0 };
    case "float":
      return { t: "float", v: 0 };
    case "decimal":
      return { t: "decimal", v: "0" };
    case "array":
      return { t: "array", v: [] };
    case "json":
      return { t: "json", v: "{}" };
    case "enum":
      return { t: "enum", v: enumLabels(c.type)?.[0] ?? "" };
    case "date":
      return { t: "date", v: "1970-01-01" };
    case "timestamp":
      return { t: "timestamp", v: "1970-01-01 00:00:00" };
    default:
      return c.type.startsWith("Nullable(") ? NULL : { t: "text", v: "" };
  }
}

/** `ALTER TABLE t UPDATE … WHERE …` and `ALTER TABLE t DELETE WHERE …`: mutations. */
function alter(p: Parser, ctx: SqlContext): Outcome {
  p.expectWord("TABLE");
  const { db, name } = p.qualified(ctx.current);
  const t = findTable(ctx, db, name);
  const src: Source = { label: name, cols: t.cols, rows: t.data };
  if (p.word("DELETE")) {
    p.expectWord("WHERE");
    const c = condOf(p);
    p.end();
    t.data.splice(0, t.data.length, ...t.data.filter((r) => !test(c, src, r, ctx)));
    return { kind: "empty" };
  }
  if (!p.word("UPDATE")) p.fail();
  const sets: { at: number; x: Expr }[] = [];
  do {
    const col = p.name();
    p.expectPunct("=");
    const at = t.cols.findIndex((c) => c.name === col);
    if (at < 0)
      throw new ServerFailure(16, `There is no column ${col} in table`, "NO_SUCH_COLUMN_IN_TABLE");
    const c = t.cols[at]!;
    if ((c.roles ?? []).length > 0)
      throw new ServerFailure(420, `Cannot UPDATE key column \`${col}\``, "CANNOT_UPDATE_COLUMN");
    if (c.default && c.default.kind !== "DEFAULT")
      throw new ServerFailure(
        420,
        `Cannot UPDATE column \`${col}\` with type ${c.default.kind}`,
        "CANNOT_UPDATE_COLUMN",
      );
    sets.push({ at, x: exprOf(p) });
  } while (p.punct(","));
  p.expectWord("WHERE");
  const c = condOf(p);
  p.end();
  const updated = t.data.map((r) => {
    if (!test(c, src, r, ctx)) return r;
    const next = r.slice();
    for (const s of sets) next[s.at] = coerce(t.cols[s.at]!.type, scalar(s.x, src, r, ctx));
    return next;
  });
  t.data.splice(0, t.data.length, ...updated);
  return { kind: "empty" };
}

function createTable(p: Parser, ctx: SqlContext): Outcome {
  const ifNot = p.ifExists(true);
  const { db, name } = p.qualified(ctx.current);
  const d = ctx.databases.get(db);
  if (!d) throw new ServerFailure(81, `Database ${db} does not exist`, "UNKNOWN_DATABASE");
  if (d.tables.some((x) => x.name === name)) {
    if (ifNot) return { kind: "empty" };
    throw new ServerFailure(57, `Table ${db}.${name} already exists`, "TABLE_ALREADY_EXISTS");
  }
  p.expectPunct("(");
  const cols: Col[] = [];
  do {
    const col = p.name();
    // The type runs to the next comma or closing parenthesis outside parentheses.
    const from = p.peek()?.at ?? p.sql.length;
    let depth = 0;
    while (p.peek() && !(depth === 0 && (p.isPunct(",") || p.isPunct(")")))) {
      if (p.isPunct("(")) depth++;
      if (p.isPunct(")")) depth--;
      p.i++;
    }
    const type = p.sql.slice(from, p.peek()?.at ?? p.sql.length).trim();
    if (type === "") p.fail();
    cols.push({ name: col, type });
  } while (p.punct(","));
  p.expectPunct(")");
  p.expectWord("ENGINE");
  p.expectPunct("=");
  const engine = p.name();
  if (p.punct("("))
    while (!p.punct(")")) {
      if (!p.peek()) p.fail();
      p.i++;
    }
  const rest = p.sql
    .slice(p.peek()?.at ?? p.sql.length)
    .replace(/;\s*$/, "")
    .trim();
  const order = /ORDER\s+BY\s+(\(([^)]*)\)|[\w`]+)/i.exec(rest);
  const key = (order?.[2] ?? order?.[1] ?? "").replaceAll("`", "").trim();
  if (engine.endsWith("MergeTree") && !order)
    throw new ServerFailure(
      42,
      "Storage MergeTree requires 3 to 4 parameters or an ORDER BY clause",
      "NUMBER_OF_ARGUMENTS_DOESNT_MATCH",
    );
  const keyCols =
    key === "tuple()"
      ? []
      : key
          .split(",")
          .map((k) => k.trim())
          .filter(Boolean);
  for (const c of cols) if (keyCols.includes(c.name)) c.roles = ["primary key", "sorting key"];
  d.tables.push({
    database: db,
    name,
    engine,
    engineFull: `${engine} ${rest}`.trim(),
    partitionKey: "",
    sortingKey: keyCols.join(", "),
    primaryKey: keyCols.join(", "),
    samplingKey: "",
    cols,
    data: [],
    comment: "",
    skipping: [],
    projections: [],
  });
  return { kind: "empty" };
}

/**
 * A raw filter of the grid (`RawFilter::SqlWhere`) as the server reads it in the page's WHERE:
 * a test of a row of `t`; a `ServerFailure` when it is no condition the mock reads.
 */
export function conditionOf(sql: string, t: Table, ctx: SqlContext): (row: Value[]) => boolean {
  const p = new Parser(sql, tokens(sql));
  const c = condOf(p);
  if (p.i < p.toks.length) p.fail();
  const src: Source = { label: t.name, cols: t.cols, rows: t.data };
  // An unknown column fails the whole read, as on the server, even with no row to test.
  const exprs = (x: Cond): Expr[] =>
    x.c === "and" || x.c === "or"
      ? x.parts.flatMap(exprs)
      : x.c === "cmp"
        ? [x.left, x.right]
        : x.c === "in"
          ? [x.left, ...x.values]
          : x.c === "null"
            ? [x.left]
            : [];
  const names = (x: Expr): string[] =>
    x.e === "col" ? [x.name] : x.e === "call" ? x.args.flatMap(names) : [];
  const unknown = exprs(c)
    .flatMap(names)
    .find((n) => !t.cols.some((col) => col.name === n));
  if (unknown !== undefined) throw unknownIdentifier(unknown);
  return (row) => test(c, src, row, ctx);
}

/** Runs one statement of the editor. */
export function runStatement(sql: string, ctx: SqlContext): Outcome {
  const p = new Parser(sql, tokens(sql));
  const first = p.peek();
  if (!first) throw new ServerFailure(62, "Empty query", "SYNTAX_ERROR");
  if (first.k !== "word") return p.fail();
  switch (first.up) {
    case "SELECT":
    case "WITH":
      return runSelect(p, ctx);
    case "SHOW": {
      p.i++;
      if (p.word("DATABASES")) {
        p.end();
        return strings(
          ["name"],
          [...ctx.databases.keys()].sort().map((n) => [n]),
        );
      }
      if (p.word("TABLES")) {
        const db = p.word("FROM") || p.word("IN") ? p.name() : ctx.current;
        p.end();
        const d = ctx.databases.get(db);
        if (!d) throw new ServerFailure(81, `Database ${db} does not exist`, "UNKNOWN_DATABASE");
        return strings(["name"], d.tables.map((t) => [t.name]).sort());
      }
      if (p.word("CREATE")) {
        p.anyWord("TABLE", "VIEW", "DICTIONARY");
        const { db, name } = p.qualified(ctx.current);
        p.end();
        return strings(["statement"], [[createQuery(findTable(ctx, db, name))]]);
      }
      if (p.word("SETTINGS")) {
        p.end();
        return strings(["name", "value"], [...ctx.editor.settings].sort());
      }
      return p.fail();
    }
    case "DESCRIBE":
    case "DESC": {
      p.i++;
      p.word("TABLE");
      const { db, name } = p.qualified(ctx.current);
      p.end();
      const t = findTable(ctx, db, name);
      return strings(
        [
          "name",
          "type",
          "default_type",
          "default_expression",
          "comment",
          "codec_expression",
          "ttl_expression",
        ],
        columnsOf(t).map((c, i) => [
          c.name,
          c.dataType,
          t.cols[i]!.default?.kind ?? "",
          c.defaultExpr ?? "",
          c.comment ?? "",
          t.cols[i]!.codec?.replace(/^CODEC\((.*)\)$/, "$1") ?? "",
          "",
        ]),
      );
    }
    case "EXISTS": {
      p.i++;
      p.word("TABLE");
      const { db, name } = p.qualified(ctx.current);
      p.end();
      const found = ctx.databases.get(db)?.tables.some((t) => t.name === name) ?? false;
      return {
        kind: "rows",
        columns: [column("result", "UInt8")],
        rows: [[{ t: "int", v: found ? 1 : 0 }]],
      };
    }
    case "EXPLAIN": {
      p.i++;
      for (const kind of ["PLAN", "AST", "SYNTAX", "PIPELINE", "ESTIMATE"]) if (p.word(kind)) break;
      const at = p.peek()?.at ?? sql.length;
      const inner = sql.slice(at);
      const sub = new Parser(inner, tokens(inner));
      if (!sub.isWord("SELECT")) return p.fail();
      const read = runSelect(sub, ctx);
      const from = /\bFROM\s+([\w.`]+)/i.exec(inner)?.[1]?.replaceAll("`", "") ?? "system.one";
      return strings(
        ["explain"],
        [
          ["Expression ((Project names + Projection))"],
          ["  Limit (preliminary LIMIT (without OFFSET))"],
          [`    ReadFromMergeTree (${from.includes(".") ? from : `${ctx.current}.${from}`})`],
          [`    -- ${read.rows.length} row(s) read by the mock`],
        ],
      );
    }
    case "SET": {
      p.i++;
      do {
        const name = p.name();
        p.expectPunct("=");
        const v = literalOf(p) ?? { t: "text", v: p.name() };
        ctx.editor.settings.set(name, display(v));
      } while (p.punct(","));
      p.end();
      return { kind: "empty" };
    }
    case "INSERT":
      p.i++;
      return insert(p, ctx);
    case "ALTER":
      p.i++;
      return alter(p, ctx);
    case "DELETE": {
      p.i++;
      p.expectWord("FROM");
      const { db, name } = p.qualified(ctx.current);
      const t = findTable(ctx, db, name);
      p.expectWord("WHERE");
      const c = condOf(p);
      p.end();
      const src: Source = { label: name, cols: t.cols, rows: t.data };
      t.data.splice(0, t.data.length, ...t.data.filter((r) => !test(c, src, r, ctx)));
      return { kind: "empty" };
    }
    case "TRUNCATE": {
      p.i++;
      p.word("TABLE");
      const { db, name } = p.qualified(ctx.current);
      p.end();
      findTable(ctx, db, name).data.splice(0);
      return { kind: "empty" };
    }
    case "DROP": {
      p.i++;
      p.anyWord("TABLE", "VIEW", "DICTIONARY");
      const ifExists = p.ifExists(false);
      const { db, name } = p.qualified(ctx.current);
      p.end();
      const d = ctx.databases.get(db);
      const at = d?.tables.findIndex((t) => t.name === name) ?? -1;
      if (at < 0) {
        if (ifExists) return { kind: "empty" };
        throw new ServerFailure(60, `Table ${db}.${name} does not exist`, "UNKNOWN_TABLE");
      }
      d!.tables.splice(at, 1);
      return { kind: "empty" };
    }
    case "CREATE": {
      p.i++;
      if (p.word("DATABASE")) {
        const ifNot = p.ifExists(true);
        const name = p.name();
        if (p.word("ENGINE")) {
          p.expectPunct("=");
          p.name();
        }
        p.end();
        if (ctx.databases.has(name)) {
          if (ifNot) return { kind: "empty" };
          throw new ServerFailure(82, `Database ${name} already exists`, "DATABASE_ALREADY_EXISTS");
        }
        ctx.databases.set(name, { name, engine: "Atomic", tables: [] });
        return { kind: "empty" };
      }
      if (p.word("TABLE")) return createTable(p, ctx);
      return p.fail();
    }
    case "OPTIMIZE": {
      p.i++;
      p.expectWord("TABLE");
      const { db, name } = p.qualified(ctx.current);
      findTable(ctx, db, name);
      p.word("FINAL");
      p.end();
      return { kind: "empty" };
    }
    case "KILL": {
      p.i++;
      p.expectWord("QUERY");
      p.expectWord("WHERE");
      p.expectWord("QUERY_ID");
      p.expectPunct("=");
      const id = literalOf(p);
      if (id?.t !== "text") return p.fail();
      p.anyWord("ASYNC", "SYNC");
      p.end();
      const killed = ctx.kill(id.v);
      return strings(
        ["kill_status", "query_id", "user"],
        killed ? [["waiting", id.v, ctx.user]] : [],
      );
    }
    default:
      return p.fail();
  }
}
