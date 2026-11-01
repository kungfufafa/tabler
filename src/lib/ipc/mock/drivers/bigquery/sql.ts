import { classifyDialect, isSingleStatement } from "@/features/drivers/portable/classify";
import {
  firstReading,
  scan,
  text as tokenText,
  type Token,
} from "@/features/drivers/portable/scanner";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { Value } from "../../../bindings/Value";
import BIGQUERY_LEX from "../../../bindings/lex/bigquery.json";
import { matchesGroup, sortRows } from "../../rows";
import { columnOf, type Dataset, type Field, type Table, timestampText } from "./data";

/**
 * The mock's GoogleSQL: enough of it for the editor and the dry run to feel real. It runs
 * `SELECT` (columns or `*`, `COUNT(*)`, `WHERE` conditions joined by AND, `ORDER BY`, `LIMIT`)
 * over one table, `SELECT` of literals, and `INSERT … VALUES`, `UPDATE … SET … WHERE` and
 * `DELETE … WHERE` on one table; other statements fail as a BigQuery job would. Each query says
 * what it processed and billed (10 MB at least per table, as on-demand pricing bills), and one
 * that would bill more than the cost guard fails with `bytesBilledLimitExceeded`.
 */

export const LEX = BIGQUERY_LEX as unknown as DialectLex;

/** Whether the backend's classifier calls `sql` a write (`sql::writes`). */
export const writes = (sql: string): boolean => classifyDialect(LEX, sql) === "write";

/** `sql::is_single_statement`. */
export const singleStatement = (sql: string): boolean =>
  isSingleStatement(LEX.lex, LEX.classify, sql);

/** A failed job, as `errors::db_error` reports it. */
export class JobFailure extends Error {
  constructor(
    message: string,
    readonly reason: string,
    readonly hint: string | null = null,
  ) {
    super(message);
  }
}

const MB = 2 ** 20;
/** On-demand pricing bills at least 10 MB per table a query reads. */
const MIN_BILLED = 10 * MB;

/** What a query of the mock would do (a dry run stops here). */
export interface Plan {
  kind: "rows" | "affected";
  /** What it scans; 0 for a SELECT of literals. */
  processed: number;
  columns: ResultColumn[];
  run: () => Value[][] | number;
}

/** The session state a statement runs against. */
export interface SqlContext {
  project: string;
  defaultDataset: string | null;
  location: string;
  user: string | null;
  datasets: Map<string, Dataset>;
}

const fail = (message: string, reason = "invalidQuery"): never => {
  throw new JobFailure(message, reason);
};

/** The code tokens of a statement (no spaces, comments or the final `;`). */
function tokensOf(sql: string): { s: ReturnType<typeof scan>; toks: Token[] } {
  const s = scan(LEX.lex, firstReading(LEX.lex), sql);
  const toks = s.tokens.filter((t) => t.kind !== "space" && t.kind !== "comment");
  while (toks.length > 0 && tokenText(s, toks[toks.length - 1]!) === ";") toks.pop();
  return { s, toks };
}

/** A token cursor over one statement. */
class Cursor {
  i = 0;
  constructor(
    readonly s: ReturnType<typeof scan>,
    readonly toks: Token[],
  ) {}
  peek(ahead = 0): string {
    const t = this.toks[this.i + ahead];
    return t ? tokenText(this.s, t) : "";
  }
  word(ahead = 0): string {
    const t = this.toks[this.i + ahead];
    return t?.kind === "word" ? tokenText(this.s, t).toUpperCase() : "";
  }
  kind(): Token["kind"] | null {
    return this.toks[this.i]?.kind ?? null;
  }
  next(): string {
    const v = this.peek();
    this.i += 1;
    return v;
  }
  done(): boolean {
    return this.i >= this.toks.length;
  }
  accept(word: string): boolean {
    if (this.word() !== word) return false;
    this.i += 1;
    return true;
  }
  expect(word: string): void {
    if (!this.accept(word)) this.syntax(word);
  }
  punct(c: string): boolean {
    if (this.peek() !== c || this.kind() !== "punct") return false;
    this.i += 1;
    return true;
  }
  syntax(wanted: string): never {
    const got = this.peek();
    return fail(
      got === ""
        ? `Syntax error: Expected ${wanted} but got end of script`
        : `Syntax error: Expected ${wanted} but got "${got}"`,
    );
  }
  /** A name: a word, or a backticked name that may hold dots (`demo-proj.shop.orders`). */
  name(): string[] {
    const t = this.toks[this.i];
    if (t?.kind === "ident") {
      this.i += 1;
      const raw = tokenText(this.s, t).slice(1, -1).replace(/\\(.)/g, "$1");
      return raw.split(".");
    }
    if (t?.kind !== "word") return this.syntax("a name");
    let part = this.next();
    // An unquoted project id may hold dashes (`demo-proj.shop.orders`).
    while (this.peek() === "-" && this.toks[this.i + 1]?.kind === "word") {
      this.i += 1;
      part += `-${this.next()}`;
    }
    return [part];
  }
  /** A dotted path of names. */
  path(): string[] {
    const parts = this.name();
    while (this.punct(".")) parts.push(...this.name());
    return parts;
  }
}

/** The table a path names, as BigQuery resolves it (the default dataset for one part). */
function resolve(ctx: SqlContext, parts: string[]): Table {
  const name = parts.join(".");
  const [project, dataset, table] =
    parts.length >= 3
      ? [parts.slice(0, -2).join("."), parts[parts.length - 2]!, parts[parts.length - 1]!]
      : parts.length === 2
        ? [ctx.project, parts[0]!, parts[1]!]
        : [ctx.project, ctx.defaultDataset, parts[0]!];
  if (dataset === null)
    return fail(`Table "${name}" must be qualified with a dataset (e.g. dataset.table).`);
  const t = ctx.datasets.get(dataset)?.tables.get(table);
  if (!t)
    return fail(
      `Not found: ${ctx.datasets.has(dataset) ? "Table" : "Dataset"} ${project}:${dataset}${ctx.datasets.has(dataset) ? `.${table}` : ""} was not found in location ${ctx.location}`,
      "notFound",
    );
  return t;
}

/** A literal of the statement as a value; null when the token is none. */
function literal(c: Cursor): Value | null {
  const t = c.toks[c.i];
  if (!t) return null;
  const raw = tokenText(c.s, t);
  if (t.kind === "str") {
    c.i += 1;
    return {
      t: "text",
      v: raw.replace(/^[rRbB]*(['"]{1,3})([\s\S]*)\1$/, "$2").replace(/\\(.)/g, "$1"),
    };
  }
  const negative = raw === "-" && c.toks[c.i + 1]?.kind === "word";
  const num = negative ? `-${tokenText(c.s, c.toks[c.i + 1]!)}` : raw;
  if (t.kind === "word" || negative) {
    if (/^-?\d+$/.test(num)) {
      c.i += negative ? 2 : 1;
      const n = Number(num);
      return Number.isSafeInteger(n) ? { t: "int", v: n } : { t: "bigInt", v: num };
    }
    if (/^-?\d*\.\d+$/.test(num) || /^-?\d+\.\d*$/.test(num)) {
      c.i += negative ? 2 : 1;
      return { t: "decimal", v: num };
    }
    const w = raw.toUpperCase();
    if (w === "TRUE" || w === "FALSE") {
      c.i += 1;
      return { t: "bool", v: w === "TRUE" };
    }
    if (w === "NULL") {
      c.i += 1;
      return { t: "null" };
    }
  }
  return null;
}

/** A value of the statement stored in a column of `kind` (`INSERT`, `UPDATE`). */
function coerce(v: Value, f: Field): Value {
  if (v.t === "null") {
    if (f.required) fail(`Required field ${f.name} cannot be null`);
    return v;
  }
  const s =
    v.t === "text" || v.t === "decimal" || v.t === "bigInt" ? v.v : String("v" in v ? v.v : "");
  switch (f.kind) {
    case "bigInt":
      if (!/^-?\d+$/.test(s)) fail(`Could not cast literal "${s}" to type INT64`);
      return Number.isSafeInteger(Number(s)) ? { t: "int", v: Number(s) } : { t: "bigInt", v: s };
    case "decimal":
      if (!/^-?\d+(\.\d+)?$/.test(s)) fail(`Invalid NUMERIC value: ${s}`);
      return { t: "decimal", v: s };
    case "float":
      return { t: "float", v: Number(s) };
    case "bool":
      return v.t === "bool" ? v : fail(`Could not cast literal "${s}" to type BOOL`);
    case "date":
      if (!/^\d{4}-\d{2}-\d{2}$/.test(s)) fail(`Could not cast literal "${s}" to type DATE`);
      return { t: "date", v: s };
    case "timestampTz": {
      const ms = Date.parse(s.includes("T") || /[+Z]/.test(s) ? s : `${s.replace(" ", "T")}Z`);
      if (Number.isNaN(ms)) fail(`Could not cast literal "${s}" to type TIMESTAMP`);
      return { t: "timestampTz", v: timestampText(ms) };
    }
    case "json":
      return { t: "json", v: s };
    default:
      return { t: "text", v: s };
  }
}

/** `col op literal [AND …]` as the mock's filter. */
function where(c: Cursor, t: Table): FilterGroup {
  const items: FilterNode[] = [];
  do {
    const col = c.name().join(".");
    if (!t.fields.some((f) => f.name === col)) fail(`Unrecognized name: ${col}`);
    if (c.accept("IS")) {
      const not = c.accept("NOT");
      c.expect("NULL");
      items.push({ kind: "condition", column: col, op: not ? "isNotNull" : "isNull", values: [] });
      continue;
    }
    let op: FilterOp;
    if (c.accept("LIKE")) op = "like";
    else {
      const a = c.peek();
      const b = c.peek(1);
      const two = `${a}${b}`;
      if (["!=", "<>", "<=", ">="].includes(two) && c.kind() === "punct") {
        c.i += 2;
        op = two === "<=" ? "le" : two === ">=" ? "ge" : "ne";
      } else if (["=", "<", ">"].includes(a) && c.kind() === "punct") {
        c.i += 1;
        op = a === "=" ? "eq" : a === "<" ? "lt" : "gt";
      } else return c.syntax("a comparison");
    }
    const v = literal(c) ?? c.syntax("a literal");
    items.push({ kind: "condition", column: col, op, values: [v] });
  } while (c.accept("AND"));
  return { conjunction: "and", items };
}

const resultColumn = (f: Field, table: string | null): ResultColumn => ({
  name: f.name,
  dataType: f.type,
  kind: f.kind,
  nullable: !f.required,
  table,
});

/** What reading `columns` of `t` scans: its share of the table's bytes. */
const scanned = (t: Table, columns: number): number =>
  t.fields.length === 0 ? 0 : Math.round((t.bytes * columns) / t.fields.length);

/** What a query that processed `n` bytes bills. */
export const billed = (n: number, tables: number): number =>
  tables === 0 ? 0 : Math.max(MIN_BILLED * tables, Math.ceil(n / MB) * MB);

function select(c: Cursor, ctx: SqlContext): Plan {
  // The select list: `*`, `COUNT(*)`, column names, or literals with no FROM.
  const items: { col?: string; count?: boolean; value?: Value; alias?: string }[] = [];
  do {
    if (c.punct("*")) items.push({ col: "*" });
    else if (c.word() === "COUNT" && c.peek(1) === "(" && c.peek(2) === "*") {
      c.i += 4;
      items.push({ count: true });
    } else if (c.word() === "CURRENT_TIMESTAMP" || c.word() === "SESSION_USER") {
      const fn = c.word();
      c.i += 1;
      if (!c.punct("(") || !c.punct(")")) c.syntax('")"');
      items.push({
        value:
          fn === "SESSION_USER"
            ? ctx.user === null
              ? { t: "null" }
              : { t: "text", v: ctx.user }
            : { t: "timestampTz", v: timestampText(Date.now()) },
        alias: `f${items.length}_`,
      });
    } else {
      const v = literal(c);
      if (v) items.push({ value: v, alias: `f${items.length}_` });
      else items.push({ col: c.name().join(".") });
    }
    if (c.accept("AS")) items[items.length - 1]!.alias = c.name().join(".");
  } while (c.punct(","));

  if (!c.accept("FROM")) {
    if (!c.done()) c.syntax("end of input");
    if (items.some((i) => i.col !== undefined || i.count))
      fail(`Unrecognized name: ${items.find((i) => i.col)?.col ?? "COUNT"}`);
    const columns: ResultColumn[] = items.map((i) => ({
      name: i.alias!,
      dataType: typeOf(i.value!),
      kind: kindOf(i.value!),
      nullable: true,
      table: null,
    }));
    return { kind: "rows", processed: 0, columns, run: () => [items.map((i) => i.value!)] };
  }

  const t = resolve(ctx, c.path());
  const filter = c.accept("WHERE") ? where(c, t) : null;
  const sort: { column: string; dir: "asc" | "desc"; nulls: null }[] = [];
  if (c.accept("ORDER")) {
    c.expect("BY");
    do {
      const column = c.name().join(".");
      if (!t.fields.some((f) => f.name === column)) fail(`Unrecognized name: ${column}`);
      const dir = c.accept("DESC") ? "desc" : (c.accept("ASC"), "asc");
      sort.push({ column, dir, nulls: null });
    } while (c.punct(","));
  }
  let limit: number | null = null;
  if (c.accept("LIMIT")) {
    const v = literal(c);
    if (v?.t !== "int" || v.v < 0) c.syntax("a row count");
    limit = v.t === "int" ? v.v : 0;
  }
  if (!c.done()) c.syntax("end of input");

  const meta = t.fields.map((f, i) => columnOf(f, i + 1, t.primaryKey));
  const filtered = () => (filter ? t.rows.filter((r) => matchesGroup(filter, r, meta)) : t.rows);
  if (items.length === 1 && items[0]!.count) {
    const used = new Set(filter?.items.map((n) => (n.kind === "condition" ? n.column : "")) ?? []);
    return {
      kind: "rows",
      // An unfiltered COUNT(*) of a table reads its metadata.
      processed: scanned(t, used.size),
      columns: [
        {
          name: items[0]!.alias ?? "f0_",
          dataType: "INT64",
          kind: "bigInt",
          nullable: true,
          table: null,
        },
      ],
      run: () => [[{ t: "int", v: filtered().length }]],
    };
  }
  if (items.some((i) => i.count || i.value))
    fail("The mock runs a SELECT of columns or of COUNT(*), not both");
  const wanted = items.flatMap((i) => {
    if (i.col === "*") return t.fields.map((f) => ({ field: f, alias: f.name }));
    const field = t.fields.find((f) => f.name === i.col) ?? fail(`Unrecognized name: ${i.col}`);
    return [{ field, alias: i.alias ?? field.name }];
  });
  const used = new Set([
    ...wanted.map((w) => w.field.name),
    ...(filter?.items.map((n) => (n.kind === "condition" ? n.column : "")) ?? []),
    ...sort.map((s) => s.column),
  ]);
  const columns = wanted.map((w) => ({ ...resultColumn(w.field, t.name), name: w.alias }));
  return {
    kind: "rows",
    processed: scanned(t, used.size),
    columns,
    run: () => {
      const rows = sortRows(filtered(), sort, meta);
      const idx = wanted.map((w) => t.fields.indexOf(w.field));
      return (limit === null ? rows : rows.slice(0, limit)).map((r) =>
        idx.map((i) => r[i] ?? { t: "null" }),
      );
    },
  };
}

function writable(t: Table): Table {
  if (t.type !== "TABLE")
    fail(
      `${t.dataset}.${t.name} is a ${t.type === "EXTERNAL" ? "external table" : "view"}, which cannot be the target of a DML statement`,
    );
  return t;
}

function insert(c: Cursor, ctx: SqlContext): Plan {
  c.accept("INTO");
  const t = writable(resolve(ctx, c.path()));
  let names = t.fields.map((f) => f.name);
  if (c.punct("(")) {
    names = [];
    do names.push(c.name().join("."));
    while (c.punct(","));
    if (!c.punct(")")) c.syntax('")"');
  }
  const fields = names.map(
    (n) =>
      t.fields.find((f) => f.name === n) ??
      fail(`Column ${n} is not present in table ${t.dataset}.${t.name}`),
  );
  c.expect("VALUES");
  const tuples: Value[][] = [];
  do {
    if (!c.punct("(")) c.syntax('"("');
    const vals: Value[] = [];
    do vals.push(literal(c) ?? c.syntax("a literal"));
    while (c.punct(","));
    if (!c.punct(")")) c.syntax('")"');
    if (vals.length !== fields.length)
      fail(`Inserted row has wrong column count; Has ${vals.length}, expected ${fields.length}`);
    tuples.push(vals);
  } while (c.punct(","));
  if (!c.done()) c.syntax("end of input");
  // Checked before anything is stored: a DML statement applies whole or not at all.
  const rows = tuples.map((vals) =>
    t.fields.map((f) => {
      const at = fields.indexOf(f);
      return at < 0 ? coerce({ t: "null" }, f) : coerce(vals[at]!, f);
    }),
  );
  return {
    kind: "affected",
    processed: 0,
    columns: [],
    run: () => {
      t.rows.push(...rows);
      return rows.length;
    },
  };
}

function update(c: Cursor, ctx: SqlContext): Plan {
  const t = writable(resolve(ctx, c.path()));
  c.expect("SET");
  const sets: { at: number; value: Value }[] = [];
  do {
    const name = c.name().join(".");
    const at = t.fields.findIndex((f) => f.name === name);
    if (at < 0) fail(`Unrecognized name: ${name}`);
    if (!c.punct("=")) c.syntax('"="');
    sets.push({ at, value: coerce(literal(c) ?? c.syntax("a literal"), t.fields[at]!) });
  } while (c.punct(","));
  if (!c.accept("WHERE")) fail("UPDATE must have a WHERE clause");
  const filter = where(c, t);
  if (!c.done()) c.syntax("end of input");
  const meta = t.fields.map((f, i) => columnOf(f, i + 1, t.primaryKey));
  return {
    kind: "affected",
    processed: t.bytes,
    columns: [],
    run: () => {
      let n = 0;
      t.rows = t.rows.map((r) => {
        if (!matchesGroup(filter, r, meta)) return r;
        n += 1;
        const copy = [...r];
        for (const s of sets) copy[s.at] = s.value;
        return copy;
      });
      return n;
    },
  };
}

function remove(c: Cursor, ctx: SqlContext): Plan {
  c.accept("FROM");
  const t = writable(resolve(ctx, c.path()));
  if (!c.accept("WHERE")) fail("DELETE must have a WHERE clause");
  const filter = where(c, t);
  if (!c.done()) c.syntax("end of input");
  const meta = t.fields.map((f, i) => columnOf(f, i + 1, t.primaryKey));
  return {
    kind: "affected",
    processed: t.bytes,
    columns: [],
    run: () => {
      const before = t.rows.length;
      t.rows = t.rows.filter((r) => !matchesGroup(filter, r, meta));
      return before - t.rows.length;
    },
  };
}

/** What `sql` would do; throws `JobFailure` where BigQuery would refuse the job. */
export function plan(sql: string, ctx: SqlContext): Plan {
  const { s, toks } = tokensOf(sql);
  const c = new Cursor(s, toks);
  const verb = c.word();
  c.i += 1;
  if (verb === "SELECT") return select(c, ctx);
  if (verb === "INSERT") return insert(c, ctx);
  if (verb === "UPDATE") return update(c, ctx);
  if (verb === "DELETE") return remove(c, ctx);
  return fail(
    verb === ""
      ? "Syntax error: Unexpected end of script"
      : `The mock BigQuery runs SELECT, INSERT, UPDATE and DELETE on one table, not ${verb}`,
  );
}

function typeOf(v: Value): string {
  switch (v.t) {
    case "int":
    case "bigInt":
      return "INT64";
    case "decimal":
      return "NUMERIC";
    case "bool":
      return "BOOL";
    case "timestampTz":
      return "TIMESTAMP";
    case "null":
      return "INT64";
    default:
      return "STRING";
  }
}

function kindOf(v: Value): ResultColumn["kind"] {
  switch (v.t) {
    case "int":
    case "bigInt":
    case "null":
      return "bigInt";
    case "decimal":
      return "decimal";
    case "bool":
      return "bool";
    case "timestampTz":
      return "timestampTz";
    default:
      return "text";
  }
}
