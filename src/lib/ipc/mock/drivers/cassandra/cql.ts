import { split } from "@/features/drivers/portable/scanner";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { DialectLex } from "../../../bindings/DialectLex";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import CQL_LEX from "../../../bindings/lex/cql.json";
import { mockError } from "../../transport";
import { columnsOf, compare, ddlOf, type Keyspace, quoted, rowsOf, same, type Table } from "./data";
import { cell, serverFailure } from "./values";

/**
 * The editor of the mock's Cassandra, as `drivers/cassandra/editor.rs` runs it: cqlsh's
 * `CONSISTENCY`, `SERIAL CONSISTENCY`, `TRACING`, `PAGING` and `EXPAND` change the session's
 * settings and answer with a notice, `USE` switches the keyspace, and the node runs `SELECT`
 * (with the ALLOW FILTERING rule), `INSERT` / `UPDATE` / `DELETE` (lightweight transactions
 * answer `[applied]`), batches, `TRUNCATE` and `DESCRIBE`. Anything else is refused as the mock's
 * limit. A read-only session refuses a write before it runs.
 */

const LEX = CQL_LEX as unknown as DialectLex;

/** What the editor's cqlsh commands change (`session::EditorSettings`), and the keyspace. */
export interface Editor {
  keyspace: string | null;
  consistency: string;
  serial: string;
  tracing: boolean;
  paging: boolean;
  pageSize: number;
  expand: boolean;
}

export interface RunContext {
  keyspaces: Map<string, Keyspace>;
  readOnly: boolean;
  editor: Editor;
}

const MAX_PAGE_SIZE = 10_000;

const INVALID = 8704; // InvalidRequest
const SYNTAX = 8192; // SyntaxError

/** `Consistency`'s `Debug`, which the backend's notices print (`LOCAL_ONE` → `LocalOne`). */
export const levelName = (level: string) =>
  level
    .toLowerCase()
    .split("_")
    .map((w) => w[0]!.toUpperCase() + w.slice(1))
    .join("");

const CONSISTENCY = [
  "ANY",
  "ONE",
  "TWO",
  "THREE",
  "QUORUM",
  "ALL",
  "LOCAL_QUORUM",
  "EACH_QUORUM",
  "LOCAL_ONE",
  "SERIAL",
  "LOCAL_SERIAL",
];

// --- tokens ---------------------------------------------------------------------------------

type Tok =
  | { k: "word"; v: string }
  | { k: "ident"; v: string }
  | { k: "str"; v: string }
  | { k: "num"; v: string }
  | { k: "uuid"; v: string }
  | { k: "blob"; v: string }
  | { k: "coll"; v: string }
  | { k: "punct"; v: string };

const TOKEN =
  /\s+|--[^\n]*|\/\/[^\n]*|\/\*[\s\S]*?\*\/|'(?:[^']|'')*'|"(?:[^"]|"")*"|\$\$[\s\S]*?\$\$|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|0x[0-9a-f]*|-?\d+(?:\.\d+)?(?:e[+-]?\d+)?|[a-z_][a-z0-9_]*|<=|>=|!=|[(),;.=<>*+\-[\]{}:?]/giy;

function tokens(sql: string): Tok[] {
  const out: Tok[] = [];
  TOKEN.lastIndex = 0;
  while (TOKEN.lastIndex < sql.length) {
    const at = TOKEN.lastIndex;
    const m = TOKEN.exec(sql);
    if (!m)
      throw serverFailure(SYNTAX, `line 1:${at} no viable alternative at character '${sql[at]}'`);
    const t = m[0];
    if (/^(\s|--|\/\/|\/\*)/.test(t)) continue;
    if (t.startsWith("'")) out.push({ k: "str", v: t.slice(1, -1).replaceAll("''", "'") });
    else if (t.startsWith("$$")) out.push({ k: "str", v: t.slice(2, -2) });
    else if (t.startsWith('"')) out.push({ k: "ident", v: t.slice(1, -1).replaceAll('""', '"') });
    else if (/^[0-9a-f]{8}-/i.test(t)) out.push({ k: "uuid", v: t.toLowerCase() });
    else if (/^0x/i.test(t)) out.push({ k: "blob", v: t.toLowerCase() });
    else if (/^-?\d/.test(t)) out.push({ k: "num", v: t });
    else if (/^[a-z_]/i.test(t)) out.push({ k: "word", v: t });
    else out.push({ k: "punct", v: t });
  }
  return out;
}

/** A cursor over a statement's tokens. */
class Reader {
  private i = 0;
  constructor(private readonly toks: Tok[]) {}
  peek(n = 0): Tok | undefined {
    return this.toks[this.i + n];
  }
  next(): Tok | undefined {
    return this.toks[this.i++];
  }
  done(): boolean {
    if (this.peek()?.v === ";") this.i++;
    return this.i >= this.toks.length;
  }
  /** Whether the next token is word `w` (any case), taking it when it is. */
  word(w: string): boolean {
    const t = this.peek();
    if (t?.k === "word" && t.v.toUpperCase() === w) {
      this.i++;
      return true;
    }
    return false;
  }
  punct(p: string): boolean {
    const t = this.peek();
    if (t?.k === "punct" && t.v === p) {
      this.i++;
      return true;
    }
    return false;
  }
  expectWord(w: string): void {
    if (!this.word(w)) this.fail();
  }
  expectPunct(p: string): void {
    if (!this.punct(p)) this.fail();
  }
  fail(): never {
    const t = this.peek();
    throw serverFailure(SYNTAX, `line 1: no viable alternative at input '${t?.v ?? "<EOF>"}'`);
  }
  /** A name: a bare word folds to lower case, a quoted one stays as written. */
  name(): string {
    const t = this.next();
    if (t?.k === "word") return t.v.toLowerCase();
    if (t?.k === "ident") return t.v;
    return this.fail();
  }
  /** A literal value (a collection's text kept as JSON). */
  value(): Value {
    const t = this.next();
    if (!t) return this.fail();
    switch (t.k) {
      case "str":
        return { t: "text", v: t.v };
      case "num":
        return /^-?\d+$/.test(t.v) ? { t: "bigInt", v: t.v } : { t: "decimal", v: t.v };
      case "uuid":
        return { t: "uuid", v: t.v };
      case "blob":
        return { t: "text", v: t.v };
      case "word": {
        const w = t.v.toLowerCase();
        if (w === "true" || w === "false") return { t: "bool", v: w === "true" };
        if (w === "null") return { t: "null" };
        return this.fail();
      }
      case "punct":
        if (t.v === "[" || t.v === "{") return { t: "text", v: this.collection(t.v) };
        return this.fail();
      default:
        return this.fail();
    }
  }
  /** `[…]` or `{…}` as JSON text (a set or list as an array, a map as an object). */
  private collection(open: string): string {
    const items: string[] = [];
    let map = false;
    const close = open === "[" ? "]" : "}";
    while (!this.punct(close)) {
      if (items.length > 0) this.expectPunct(",");
      const v = this.value();
      if (this.punct(":")) {
        map = true;
        const w = this.value();
        items.push(`${JSON.stringify(String("v" in v ? v.v : ""))}:${jsonOf(w)}`);
      } else items.push(jsonOf(v));
    }
    return map ? `{${items.join(",")}}` : `[${items.join(",")}]`;
  }
}

function jsonOf(v: Value): string {
  switch (v.t) {
    case "null":
      return "null";
    case "bool":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "text":
      return /^[[{]/.test(v.v) ? v.v : JSON.stringify(v.v);
    default:
      return JSON.stringify("v" in v ? String(v.v) : "");
  }
}

// --- the editor's run -------------------------------------------------------------------------

interface Outcome {
  columns: ResultColumn[];
  rows: Value[][] | null;
  affected: number | null;
  notices: string[];
}

const answer = (notice: string): Outcome => ({
  columns: [],
  rows: null,
  affected: null,
  notices: [notice],
});

/** `cqlsh::parse`: one of cqlsh's commands; null when the statement is none of them. */
function cqlsh(sql: string, e: Editor): Outcome | null {
  let toks: Tok[];
  try {
    toks = tokens(sql);
  } catch {
    return null;
  }
  if (toks.at(-1)?.v === ";") toks = toks.slice(0, -1);
  const words = toks.map((t) => (t.k === "word" || t.k === "num" ? t.v.toUpperCase() : "\0"));
  const name =
    words[0] === "CONSISTENCY" ||
    words[0] === "TRACING" ||
    words[0] === "PAGING" ||
    words[0] === "EXPAND"
      ? words[0]
      : words[0] === "SERIAL" && words[1] === "CONSISTENCY"
        ? "SERIAL CONSISTENCY"
        : null;
  if (name === null) return null;
  const improper = () => mockError("INVALID_INPUT", `Improper ${name} command`);
  const args = words.slice(name === "SERIAL CONSISTENCY" ? 2 : 1);
  if (args.length > 1 || args.includes("\0")) return improper();
  const [arg] = args;
  const onOff = (w: string) => (w === "ON" ? true : w === "OFF" ? false : improper());
  const state = (on: boolean) => (on ? "enabled" : "disabled");
  switch (name) {
    case "CONSISTENCY":
      if (arg === undefined)
        return answer(`Current consistency level is ${levelName(e.consistency)}.`);
      if (!CONSISTENCY.includes(arg)) return improper();
      e.consistency = arg;
      return answer(`Consistency level set to ${levelName(arg)}.`);
    case "SERIAL CONSISTENCY":
      if (arg === undefined)
        return answer(`Current serial consistency level is ${levelName(e.serial)}.`);
      if (arg !== "SERIAL" && arg !== "LOCAL_SERIAL") return improper();
      e.serial = arg;
      return answer(`Serial consistency level set to ${levelName(arg)}.`);
    case "TRACING":
      if (arg === undefined) return answer(`Tracing is currently ${state(e.tracing)}.`);
      e.tracing = onOff(arg);
      return answer(`Tracing is ${state(e.tracing)}.`);
    case "EXPAND":
      if (arg === undefined) return answer(`Expanded output is currently ${state(e.expand)}.`);
      e.expand = onOff(arg);
      return answer(
        `Expanded output is ${state(e.expand)} (the grid shows rows as a table either way).`,
      );
    default: {
      if (arg === "ON") e.paging = true;
      else if (arg === "OFF") e.paging = false;
      else if (arg !== undefined) {
        if (!/^\d+$/.test(arg) || Number(arg) === 0) return improper();
        e.paging = true;
        e.pageSize = Math.min(Number(arg), MAX_PAGE_SIZE);
      }
      return answer(
        e.paging
          ? `Query paging is enabled, page size ${e.pageSize}.`
          : `Query paging is disabled: pages of ${MAX_PAGE_SIZE} rows are read up to the row limit.`,
      );
    }
  }
}

/** `session::use_target`: the keyspace of a `USE`, its name as written. */
function keyspaceOfUse(sql: string): string | null {
  const m = /^\s*use\s+("(?:[^"]|"")+"|[A-Za-z0-9_]+)\s*;?\s*$/i.exec(sql);
  return m ? m[1]! : null;
}

/** `wire::keyspace_name`: a bare name folds to lower case, a quoted one stays as written. */
const keyspaceName = (name: string) =>
  name.startsWith('"') ? name.slice(1, -1).replaceAll('""', '"') : name.toLowerCase();

/** The table a statement names (`ks.t` or `t` in the editor's keyspace). */
function tableNamed(r: Reader, ctx: RunContext): { ks: Keyspace; t: Table } {
  let ksName = ctx.editor.keyspace;
  let name = r.name();
  if (r.punct(".")) {
    ksName = name;
    name = r.name();
  }
  if (ksName === null)
    throw serverFailure(
      INVALID,
      "No keyspace has been specified. USE a keyspace, or explicitly specify keyspace.tablename",
    );
  const ks = ctx.keyspaces.get(ksName);
  if (!ks) throw serverFailure(INVALID, `Keyspace ${ksName} does not exist`);
  const t = ks.tables.find((x) => x.name === name);
  if (!t) throw serverFailure(INVALID, `unconfigured table ${name}`);
  return { ks, t };
}

interface Relation {
  col: ColumnMeta;
  at: number;
  op: string;
  values: Value[];
}

function relations(r: Reader, columns: ColumnMeta[]): Relation[] {
  const out: Relation[] = [];
  do {
    const name = r.name();
    const at = columns.findIndex((c) => c.name === name);
    if (at < 0) throw serverFailure(INVALID, `Undefined column name ${name}`);
    const col = columns[at]!;
    if (r.word("IN")) {
      r.expectPunct("(");
      const values: Value[] = [];
      while (!r.punct(")")) {
        if (values.length > 0) r.expectPunct(",");
        values.push(cell(col, r.value()));
      }
      out.push({ col, at, op: "IN", values });
      continue;
    }
    const t0 = r.next();
    if (t0?.k !== "punct" || !["=", "<", ">", "<=", ">=", "!="].includes(t0.v)) r.fail();
    out.push({ col, at, op: t0.v, values: [cell(col, r.value())] });
  } while (r.word("AND"));
  return out;
}

function holds(rel: Relation, row: Value[]): boolean {
  const v = row[rel.at] ?? { t: "null" };
  if (v.t === "null") return false;
  if (rel.op === "IN") return rel.values.some((x) => same(v, x));
  const c = compare(v, rel.values[0]!);
  switch (rel.op) {
    case "=":
      return c === 0;
    case "!=":
      return c !== 0;
    case "<":
      return c < 0;
    case ">":
      return c > 0;
    case "<=":
      return c <= 0;
    default:
      return c >= 0;
  }
}

/** Every key column named by `=` (or `IN`): the rows a write reaches; refused otherwise. */
function keyRelations(t: Table, rels: Relation[], partitionOnly = false): void {
  const named = (role: string) =>
    t.cols.filter(
      (c) =>
        c.role === role &&
        !rels.some((r) => r.col.name === c.name && (r.op === "=" || r.op === "IN")),
    );
  const pk = named("partition");
  if (pk.length > 0)
    throw serverFailure(
      INVALID,
      `Some partition key parts are missing: ${pk.map((c) => c.name).join(", ")}`,
    );
  const ck = named("clustering");
  if (ck.length > 0 && !partitionOnly)
    throw serverFailure(
      INVALID,
      `Some clustering keys are missing: ${ck.map((c) => c.name).join(", ")}`,
    );
}

const resultColumn = (c: ColumnMeta, table: string): ResultColumn => ({
  name: c.name,
  dataType: c.dataType,
  kind: c.kind,
  nullable: null,
  table,
  origin: null,
});

const APPLIED: ResultColumn = {
  name: "[applied]",
  dataType: "boolean",
  kind: "bool",
  nullable: null,
  table: null,
  origin: null,
};

function select(r: Reader, ctx: RunContext): Outcome {
  r.word("JSON");
  let count = false;
  const names: string[] = [];
  if (r.punct("*")) {
    // every column
  } else if (r.peek()?.v.toUpperCase() === "COUNT" && r.peek(1)?.v === "(") {
    r.next();
    r.expectPunct("(");
    if (!r.punct("*")) r.name();
    r.expectPunct(")");
    count = true;
  } else
    do names.push(r.name());
    while (r.punct(","));
  r.expectWord("FROM");
  const { ks, t } = tableNamed(r, ctx);
  const columns = columnsOf(t);
  const rels = r.word("WHERE") ? relations(r, columns) : [];
  let limit: number | null = null;
  if (r.word("LIMIT")) {
    const n = r.next();
    if (n?.k !== "num" || !/^\d+$/.test(n.v)) r.fail();
    limit = Number(n.v);
  }
  const filtering = r.word("ALLOW") && (r.expectWord("FILTERING"), true);
  if (!r.done()) r.fail();
  // A relation the partition key cannot answer reads the whole cluster (`ALLOW FILTERING`).
  const byKey =
    rels.length === 0 ||
    (t.cols
      .filter((c) => c.role === "partition")
      .every((c) => rels.some((x) => x.col.name === c.name && (x.op === "=" || x.op === "IN"))) &&
      rels.every((x) => x.col.isPk));
  if (!byKey && !filtering)
    throw serverFailure(
      INVALID,
      "Cannot execute this query as it might involve data filtering and thus may have unpredictable performance. If you want to execute this query despite the performance unpredictability, use ALLOW FILTERING",
    );
  let rows = rowsOf(ks, t).filter((row) => rels.every((x) => holds(x, row)));
  if (limit !== null) rows = rows.slice(0, limit);
  if (count)
    return {
      columns: [{ ...APPLIED, name: "count", dataType: "bigint", kind: "bigInt" }],
      rows: [[{ t: "int", v: rows.length }]],
      affected: null,
      notices: [],
    };
  const picked = names.length
    ? names.map((n) => {
        const at = columns.findIndex((c) => c.name === n);
        if (at < 0) throw serverFailure(INVALID, `Undefined column name ${n}`);
        return at;
      })
    : columns.map((_, i) => i);
  return {
    columns: picked.map((i) => resultColumn(columns[i]!, t.name)),
    rows: rows.map((row) => picked.map((i) => row[i] ?? { t: "null" })),
    affected: null,
    notices: [],
  };
}

/** `USING TTL n` / `USING TIMESTAMP n` (accepted, nothing expires in the mock). */
function using(r: Reader): void {
  if (!r.word("USING")) return;
  do {
    if (!r.word("TTL") && !r.word("TIMESTAMP")) r.fail();
    if (r.next()?.k !== "num") r.fail();
  } while (r.word("AND"));
}

const lwt = (applied: boolean): Outcome => ({
  columns: [APPLIED],
  rows: [[{ t: "bool", v: applied }]],
  affected: applied ? 1 : 0,
  notices: [],
});

const written = (): Outcome => ({ columns: [], rows: null, affected: null, notices: [] });

function insert(r: Reader, ctx: RunContext): Outcome {
  r.expectWord("INTO");
  const { t } = tableNamed(r, ctx);
  const columns = columnsOf(t);
  if (columns.some((c) => c.dataType === "counter"))
    throw serverFailure(
      INVALID,
      "INSERT statements are not allowed on counter tables, use UPDATE instead",
    );
  r.expectPunct("(");
  const names: string[] = [];
  do names.push(r.name());
  while (r.punct(","));
  r.expectPunct(")");
  r.expectWord("VALUES");
  r.expectPunct("(");
  const values: Value[] = [];
  do values.push(r.value());
  while (r.punct(","));
  r.expectPunct(")");
  if (names.length !== values.length) throw serverFailure(INVALID, "Unmatched column names/values");
  const ifNotExists = r.word("IF") && (r.expectWord("NOT"), r.expectWord("EXISTS"), true);
  using(r);
  if (!r.done()) r.fail();
  const row: Value[] = columns.map(() => ({ t: "null" }));
  names.forEach((n, i) => {
    const at = columns.findIndex((c) => c.name === n);
    if (at < 0) throw serverFailure(INVALID, `Undefined column name ${n}`);
    row[at] = cell(columns[at]!, values[i]!);
  });
  const missing = columns.filter((c) => c.isPk && row[columns.indexOf(c)]!.t === "null");
  if (missing.length > 0)
    throw serverFailure(
      INVALID,
      `Some ${missing[0]!.extra === "partition key" ? "partition key parts" : "clustering keys"} are missing: ${missing.map((c) => c.name).join(", ")}`,
    );
  const keyAt = columns.flatMap((c, i) => (c.isPk ? [i] : []));
  const at = t.data.findIndex((x) => keyAt.every((i) => same(x[i]!, row[i]!)));
  if (ifNotExists) {
    if (at >= 0) return lwt(false);
    t.data.push(row);
    return lwt(true);
  }
  // An INSERT is an upsert: the cells it names replace the row's.
  if (at < 0) t.data.push(row);
  else
    names.forEach(
      (n) =>
        (t.data[at]![columns.findIndex((c) => c.name === n)] =
          row[columns.findIndex((c) => c.name === n)]!),
    );
  return written();
}

function update(r: Reader, ctx: RunContext): Outcome {
  const { t } = tableNamed(r, ctx);
  const columns = columnsOf(t);
  using(r);
  r.expectWord("SET");
  const sets: { at: number; delta: number | null; value: Value }[] = [];
  do {
    const name = r.name();
    const at = columns.findIndex((c) => c.name === name);
    if (at < 0) throw serverFailure(INVALID, `Undefined column name ${name}`);
    const col = columns[at]!;
    if (col.isPk) throw serverFailure(INVALID, `PRIMARY KEY part ${name} found in SET part`);
    r.expectPunct("=");
    if (col.dataType === "counter") {
      if (r.name() !== name) r.fail();
      const sign = r.punct("+") ? 1 : r.punct("-") ? -1 : r.fail();
      const n = r.next();
      if (n?.k !== "num" || !/^-?\d+$/.test(n.v)) r.fail();
      sets.push({ at, delta: sign * Number(n.v), value: { t: "null" } });
    } else sets.push({ at, delta: null, value: cell(col, r.value()) });
  } while (r.punct(","));
  r.expectWord("WHERE");
  const rels = relations(r, columns);
  keyRelations(t, rels);
  const ifExists = r.word("IF") && (r.expectWord("EXISTS"), true);
  if (!r.done()) r.fail();
  const matching = t.data.filter((row) => rels.every((x) => holds(x, row)));
  if (ifExists && matching.length === 0) return lwt(false);
  if (matching.length === 0) {
    // An UPDATE is an upsert too: the row the key names comes to be.
    const row: Value[] = columns.map(() => ({ t: "null" }));
    for (const x of rels) row[x.at] = x.values[0]!;
    t.data.push(row);
    matching.push(row);
  }
  for (const row of matching)
    for (const s of sets) {
      if (s.delta === null) row[s.at] = s.value;
      else {
        const old = row[s.at];
        const base = old && (old.t === "int" || old.t === "bigInt") ? Number(old.v) : 0;
        row[s.at] = { t: "int", v: base + s.delta };
      }
    }
  return ifExists ? lwt(true) : written();
}

function remove(r: Reader, ctx: RunContext): Outcome {
  if (!r.word("FROM")) {
    // `DELETE col FROM …` empties cells; the mock deletes rows only.
    throw serverFailure(INVALID, "the mock's Cassandra deletes whole rows: DELETE FROM t WHERE …");
  }
  const { t } = tableNamed(r, ctx);
  const columns = columnsOf(t);
  using(r);
  r.expectWord("WHERE");
  const rels = relations(r, columns);
  keyRelations(t, rels, true);
  const ifExists = r.word("IF") && (r.expectWord("EXISTS"), true);
  if (!r.done()) r.fail();
  const before = t.data.length;
  const keep = t.data.filter((row) => !rels.every((x) => holds(x, row)));
  t.data.splice(0, t.data.length, ...keep);
  return ifExists ? lwt(before !== keep.length) : written();
}

/** `DESCRIBE KEYSPACES | TABLES | TABLE t | KEYSPACE k`: the server's rows. */
function describe(r: Reader, ctx: RunContext): Outcome {
  const cols = ["keyspace_name", "type", "name", "create_statement"].map((name): ResultColumn => ({
    name,
    dataType: "text",
    kind: "text",
    nullable: null,
    table: null,
    origin: null,
  }));
  const row = (...v: string[]): Value[] => v.map((x) => ({ t: "text", v: x }));
  const visible = [...ctx.keyspaces.values()].filter((k) => !k.name.startsWith("system"));
  let rows: Value[][];
  if (r.word("KEYSPACES")) rows = visible.map((k) => row(k.name, "keyspace", k.name, ""));
  else if (r.word("TABLES"))
    rows = visible.flatMap((k) =>
      k.tables.map((t) =>
        row(k.name, t.kind === "table" ? "table" : "materialized_view", t.name, ""),
      ),
    );
  else if (r.word("TABLE") || (r.word("MATERIALIZED") && (r.expectWord("VIEW"), true))) {
    const { t } = tableNamed(r, ctx);
    rows = [row(t.keyspace, t.kind === "table" ? "table" : "materialized_view", t.name, ddlOf(t))];
  } else if (r.word("KEYSPACE")) {
    const name = r.name();
    const ks = ctx.keyspaces.get(name);
    if (!ks) throw serverFailure(INVALID, `'${name}' not found in keyspaces`);
    rows = [
      row(
        ks.name,
        "keyspace",
        ks.name,
        `CREATE KEYSPACE ${quoted(ks.name)} WITH replication = ${ks.replication} AND durable_writes = true;`,
      ),
      ...ks.tables.map((t) =>
        row(ks.name, t.kind === "table" ? "table" : "materialized_view", t.name, ddlOf(t)),
      ),
    ];
  } else return r.fail();
  if (!r.done()) r.fail();
  return { columns: cols, rows, affected: null, notices: [] };
}

/**
 * `BEGIN [UNLOGGED | COUNTER] BATCH … APPLY BATCH`: each write in turn, with the tables put back
 * when one fails, since a node validates the whole batch before it applies any of it.
 */
function batch(sql: string, ctx: RunContext): Outcome {
  const m = /^\s*begin\s+(?:(unlogged|counter)\s+)?batch\b([\s\S]*?)\bapply\s+batch\s*;?\s*$/i.exec(
    sql,
  );
  if (!m) throw serverFailure(SYNTAX, "line 1: mismatched input: a batch ends with APPLY BATCH");
  // UPDATE changes cells in place, so the rows are copied, not only the lists.
  const saved = [...ctx.keyspaces.values()].flatMap((ks) =>
    ks.tables.map((t) => ({ t, data: t.data.map((row) => [...row]) })),
  );
  try {
    for (const st of split(LEX.lex, m[2]!)) {
      const r = new Reader(tokens(st.sql));
      const verb = r.next()?.v.toUpperCase();
      if (verb === "INSERT") insert(r, ctx);
      else if (verb === "UPDATE") update(r, ctx);
      else if (verb === "DELETE") remove(r, ctx);
      else throw serverFailure(INVALID, "a batch holds INSERT, UPDATE and DELETE statements only");
    }
  } catch (e) {
    for (const { t, data } of saved) t.data.splice(0, t.data.length, ...data);
    throw e;
  }
  return written();
}

function execute(sql: string, ctx: RunContext): Outcome {
  if (/^\s*begin\b/i.test(sql)) return batch(sql, ctx);
  const r = new Reader(tokens(sql));
  const verb = r.next()?.v.toUpperCase();
  switch (verb) {
    case "SELECT":
      return select(r, ctx);
    case "INSERT":
      return insert(r, ctx);
    case "UPDATE":
      return update(r, ctx);
    case "DELETE":
      return remove(r, ctx);
    case "DESCRIBE":
    case "DESC":
      return describe(r, ctx);
    case "TRUNCATE": {
      r.word("TABLE");
      const { t } = tableNamed(r, ctx);
      if (!r.done()) r.fail();
      t.data.splice(0);
      return written();
    }
    default:
      return mockError(
        "UNSUPPORTED",
        "unsupported: the mock's Cassandra runs SELECT, INSERT, UPDATE, DELETE, batches, TRUNCATE, DESCRIBE, USE and cqlsh's commands",
      );
  }
}

/** Runs one statement of the editor, streaming its events (`editor::run`). */
export function runStatement(
  sql: string,
  ctx: RunContext,
  emit: (e: QueryEvent) => void,
  writes: (sql: string) => boolean,
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
  const finish = (o: Outcome) => {
    const durationMs = Math.max(1, Math.round(performance.now() - started));
    if (o.rows === null) {
      emit({
        event: "finished",
        data: {
          index: 0,
          result: { ...base, affected: o.affected, notices: o.notices, durationMs },
        },
      });
      return;
    }
    emit({ event: "columns", data: { index: 0, set: 0, columns: o.columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows: o.rows } });
    emit({
      event: "finished",
      data: {
        index: 0,
        result: {
          ...base,
          kind: "rows",
          columns: o.columns,
          resultSets: 1,
          rowCount: o.rows.length,
          affected: o.affected,
          notices: o.notices,
          durationMs,
        },
      },
    });
  };
  const command = cqlsh(sql, ctx.editor);
  if (command) return finish(command);
  const use = keyspaceOfUse(sql);
  if (use !== null) {
    const name = keyspaceName(use);
    if (!ctx.keyspaces.has(name)) throw serverFailure(INVALID, `Keyspace '${name}' does not exist`);
    ctx.editor.keyspace = name;
    return finish(answer(`Now using keyspace ${name}.`));
  }
  if (ctx.readOnly && writes(sql)) mockError("INVALID_INPUT", "This connection is read-only");
  const outcome = execute(sql, ctx);
  // A traced statement's trace comes back as notices; the mock's node keeps none.
  if (ctx.editor.tracing) outcome.notices.push("Tracing: the mock's node records no trace events");
  finish(outcome);
}
