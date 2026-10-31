import { classifyConsole, parseCommand, splitConsole } from "@/features/drivers/dynamodb/classify";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import { MISSING, N, NULL, S, BOOL, tables, typeOf, type Item, type Table } from "./data";
import {
  compare,
  describeTable,
  itemJson,
  keyNames,
  keyOf,
  keyText,
  MAX_COLUMNS,
  same,
  sampled,
  valueOf,
} from "./model";

/**
 * The mock's console: PartiQL `SELECT` (with `=`, `begins_with` and `AND`), `INSERT … VALUE`,
 * `UPDATE … SET … WHERE` the key, `DELETE … WHERE` the key (with `RETURNING ALL OLD *`), and the
 * API commands `ListTables`, the `Describe…` reads, `Scan`, `Query` (an equality on the
 * partition key), `GetItem`, `PutItem` and `DeleteItem`, answered as the backend's `run_command`
 * answers them: items as a grid of their attributes, any other reply as one JSON cell. A
 * read-only session refuses what the console's classifier calls a write, before anything runs.
 * The shared mock hands a native console its text a line at a time: one command per line.
 */

export interface ConsoleState {
  readOnly: boolean;
  region: string;
}

class MockConsoleError extends Error {
  constructor(
    message: string,
    readonly code = "DB_ERROR",
    readonly detail: string | null = null,
  ) {
    super(message);
  }
}

const fail = (message: string, detail: string | null = null): never => {
  throw new MockConsoleError(message, "DB_ERROR", detail);
};

const tableNamed = (name: string): Table =>
  tables().get(name) ??
  fail(`Requested resource not found: Table: ${name} not found`, "ResourceNotFoundException");

/** What a command answered: items, or a reply shown whole. */
interface Outcome {
  items?: Item[];
  reply?: unknown;
  /** A PartiQL write: the items it changed (null: not known). */
  affected?: number | null;
  notices?: string[];
}

// --- PartiQL ----------------------------------------------------------------------------------

type Tok = { k: "word" | "name" | "str" | "num" | "punct"; v: string };

function lex(text: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < text.length) {
    const c = text[i]!;
    if (/\s/.test(c)) i += 1;
    else if (c === "-" && text[i + 1] === "-") {
      while (i < text.length && text[i] !== "\n" && text[i] !== "\r") i += 1;
    } else if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? text.length : end + 2;
    } else if (c === "'" || c === '"') {
      let v = "";
      i += 1;
      for (;;) {
        if (i >= text.length) fail("unterminated quoted text");
        if (text[i] === c) {
          if (text[i + 1] === c) {
            v += c;
            i += 2;
            continue;
          }
          i += 1;
          break;
        }
        v += text[i];
        i += 1;
      }
      out.push({ k: c === "'" ? "str" : "name", v });
    } else if (/[\d-]/.test(c)) {
      const m = /^-?\d+(\.\d+)?/.exec(text.slice(i));
      if (!m) fail(`unexpected “${c}”`);
      out.push({ k: "num", v: m![0] });
      i += m![0].length;
    } else if (/[A-Za-z_]/.test(c)) {
      const m = /^[A-Za-z_][\w]*/.exec(text.slice(i))!;
      out.push({ k: "word", v: m[0] });
      i += m[0].length;
    } else {
      out.push({ k: "punct", v: c });
      i += 1;
    }
  }
  return out;
}

class Parser {
  private at = 0;
  constructor(private readonly toks: Tok[]) {}
  peek(): Tok | undefined {
    return this.toks[this.at];
  }
  word(w: string): boolean {
    const t = this.peek();
    if (t?.k === "word" && t.v.toUpperCase() === w) {
      this.at += 1;
      return true;
    }
    return false;
  }
  expectWord(w: string): void {
    if (!this.word(w)) fail(`expected ${w}`);
  }
  punct(p: string): boolean {
    if (this.peek()?.k === "punct" && this.peek()!.v === p) {
      this.at += 1;
      return true;
    }
    return false;
  }
  name(): string {
    const t = this.peek();
    if (t?.k !== "name" && t?.k !== "word") return fail("expected a name");
    this.at += 1;
    return t.v;
  }
  literal(): Value {
    const t = this.peek();
    this.at += 1;
    if (t?.k === "str") return S(t.v);
    if (t?.k === "num") return N(t.v);
    if (t?.k === "word" && /^(true|false)$/i.test(t.v)) return BOOL(t.v.toLowerCase() === "true");
    if (t?.k === "word" && /^null$/i.test(t.v)) return NULL;
    return fail("the mock console takes 'text', numbers, true, false and null as values");
  }
  end(): void {
    while (this.punct(";"));
    if (this.at < this.toks.length) fail(`unexpected “${this.peek()!.v}”`);
  }
  done(): boolean {
    return this.at >= this.toks.length || (this.peek()?.k === "punct" && this.peek()!.v === ";");
  }
}

type Cond = (item: Item) => boolean;

function where(p: Parser): Cond[] {
  const conds: Cond[] = [];
  if (!p.word("WHERE")) return conds;
  do {
    if (p.word("BEGINS_WITH")) {
      if (!p.punct("(")) fail("expected (");
      const name = p.name();
      if (!p.punct(",")) fail("expected ,");
      const prefix = p.literal();
      if (!p.punct(")")) fail("expected )");
      conds.push(
        (i) => i[name]?.t === "text" && prefix.t === "text" && i[name].v.startsWith(prefix.v),
      );
    } else {
      const name = p.name();
      if (!p.punct("=")) fail("the mock console compares with = and begins_with");
      const v = p.literal();
      conds.push(
        (i) =>
          same(i[name], v) ||
          (typeOf(v) === "N" && compare(i[name], v) === 0 && i[name] !== undefined),
      );
    }
  } while (p.word("AND"));
  return conds;
}

/** The key a WHERE clause of `=` conditions names (UPDATE and DELETE need the whole key). */
function keyWhere(t: Table, p: Parser): Item {
  const key: Item = {};
  if (!p.word("WHERE")) fail("UPDATE and DELETE need the item's whole key in WHERE");
  do {
    const name = p.name();
    if (!p.punct("=")) fail("expected =");
    key[name] = p.literal();
  } while (p.word("AND"));
  const names = keyNames(t.keys);
  if (Object.keys(key).length !== names.length || !names.every((n) => key[n] !== undefined))
    fail(
      `Where clause does not contain a mandatory equality on all key attributes`,
      "ValidationException",
    );
  return key;
}

const find = (t: Table, key: Item) =>
  t.items.findIndex((i) => keyText(i, t.keys) === keyText(key, t.keys));

function partiql(statement: string): Outcome {
  const p = new Parser(lex(statement));
  if (p.word("SELECT")) {
    const projection: string[] = [];
    if (!p.punct("*"))
      do projection.push(p.name());
      while (p.punct(","));
    p.expectWord("FROM");
    const t = tableNamed(p.name());
    let index: string | null = null;
    if (p.punct(".")) index = p.name();
    const keys = index ? [...t.global, ...t.local].find((i) => i.name === index)?.keys : t.keys;
    if (!keys) fail(`The table does not have the specified index: ${index}`, "ValidationException");
    const conds = where(p);
    p.end();
    let items = t.items.filter((i) => conds.every((c) => c(i)));
    if (index) items = items.filter((i) => keyOf(i, keys!) !== null);
    if (projection.length > 0)
      items = items.map((i) =>
        Object.fromEntries(projection.filter((n) => i[n]).map((n) => [n, i[n]!])),
      );
    return { items };
  }
  if (p.word("INSERT")) {
    p.expectWord("INTO");
    const t = tableNamed(p.name());
    p.expectWord("VALUE");
    if (!p.punct("{")) fail("expected {");
    const item: Item = {};
    do {
      const k = p.literal();
      if (k.t !== "text") fail("an attribute's name is 'text'");
      if (!p.punct(":")) fail("expected :");
      item[(k as { v: string }).v] = p.literal();
    } while (p.punct(","));
    if (!p.punct("}")) fail("expected }");
    p.end();
    const key = keyOf(item, t.keys);
    if (!key)
      fail("One or more parameter values were invalid: Missing the key", "ValidationException");
    if (find(t, key!) >= 0) fail("Duplicate primary key exists in table", "DuplicateItemException");
    t.items.push(item);
    return { affected: 1 };
  }
  if (p.word("UPDATE")) {
    const t = tableNamed(p.name());
    const sets: Item = {};
    const removes: string[] = [];
    for (;;) {
      if (p.word("SET")) {
        do {
          const n = p.name();
          if (!p.punct("=")) fail("expected =");
          sets[n] = p.literal();
        } while (p.punct(","));
      } else if (p.word("REMOVE")) {
        do removes.push(p.name());
        while (p.punct(","));
      } else break;
    }
    const key = keyWhere(t, p);
    p.end();
    const at = find(t, key);
    if (at < 0) fail("The conditional request failed", "ConditionalCheckFailedException");
    const next = { ...t.items[at]!, ...sets };
    for (const r of removes) delete next[r];
    t.items[at] = next;
    return { affected: 1 };
  }
  if (p.word("DELETE")) {
    p.expectWord("FROM");
    const t = tableNamed(p.name());
    const key = keyWhere(t, p);
    let returning = false;
    if (p.word("RETURNING")) {
      p.expectWord("ALL");
      p.expectWord("OLD");
      if (!p.punct("*")) fail("expected *");
      returning = true;
    }
    p.end();
    const at = find(t, key);
    const gone = at >= 0 ? t.items.splice(at, 1) : [];
    // A missing key is no error: only a returned item proves one was deleted.
    return returning ? { items: gone, affected: gone.length > 0 ? 1 : null } : { affected: null };
  }
  return fail(
    "the mock console runs SELECT, INSERT, UPDATE and DELETE, and the API commands ListTables, Describe…, Scan, Query, GetItem, PutItem and DeleteItem",
  );
}

// --- API commands -----------------------------------------------------------------------------

const str = (body: Record<string, unknown>, key: string): string => {
  const v = body[key];
  return typeof v === "string"
    ? v
    : fail(
        `1 validation error detected: Value null at '${key}' failed to satisfy constraint: Member must not be null`,
        "ValidationException",
      );
};

function typedItem(raw: unknown): Item {
  if (typeof raw !== "object" || raw === null)
    fail("an item is a map of AttributeValues", "ValidationException");
  const out: Item = {};
  for (const [k, a] of Object.entries(raw as Record<string, unknown>)) {
    const v = valueOf(a);
    if (!v)
      fail(`Supplied AttributeValue of ${k} is not one the mock reads`, "ValidationException");
    out[k] = v!;
  }
  return out;
}

function api(action: string, body: Record<string, unknown>, region: string): Outcome {
  switch (action) {
    case "ListTables":
      return { reply: { TableNames: [...tables().keys()].sort() } };
    case "DescribeTable":
      return { reply: { Table: describeTable(tableNamed(str(body, "TableName")), region) } };
    case "DescribeTimeToLive": {
      const t = tableNamed(str(body, "TableName"));
      return {
        reply: {
          TimeToLiveDescription: t.ttl
            ? { TimeToLiveStatus: "ENABLED", AttributeName: t.ttl }
            : { TimeToLiveStatus: "DISABLED" },
        },
      };
    }
    case "DescribeContinuousBackups": {
      const t = tableNamed(str(body, "TableName"));
      return {
        reply: {
          ContinuousBackupsDescription: {
            ContinuousBackupsStatus: "ENABLED",
            PointInTimeRecoveryDescription: {
              PointInTimeRecoveryStatus: t.pitr ? "ENABLED" : "DISABLED",
            },
          },
        },
      };
    }
    case "DescribeEndpoints":
      return {
        reply: {
          Endpoints: [{ Address: `dynamodb.${region}.amazonaws.com`, CachePeriodInMinutes: 1440 }],
        },
      };
    case "DescribeLimits":
      return {
        reply: {
          AccountMaxReadCapacityUnits: 80000,
          AccountMaxWriteCapacityUnits: 80000,
          TableMaxReadCapacityUnits: 40000,
          TableMaxWriteCapacityUnits: 40000,
        },
      };
    case "Scan":
    case "Query": {
      const t = tableNamed(str(body, "TableName"));
      let items = [...t.items];
      if (action === "Query") {
        const expr = str(body, "KeyConditionExpression");
        const m = /^\s*(#?\w+)\s*=\s*(:\w+)\s*$/.exec(expr);
        if (!m) fail("the mock console's Query takes one equality on the partition key");
        const names = (body.ExpressionAttributeNames ?? {}) as Record<string, string>;
        const values = (body.ExpressionAttributeValues ?? {}) as Record<string, unknown>;
        const name = m![1]!.startsWith("#") ? names[m![1]!] : m![1]!;
        const v = valueOf(values[m![2]!]);
        if (!name || !v)
          fail(
            "Value provided in ExpressionAttributeValues unused in expressions",
            "ValidationException",
          );
        items = items.filter((i) => same(i[name!], v!));
        const range = t.keys.range?.name;
        if (range)
          items.sort(
            (a, b) => compare(a[range], b[range]) * (body.ScanIndexForward === false ? -1 : 1),
          );
      }
      if (body.ExclusiveStartKey !== undefined) {
        const start = keyText(typedItem(body.ExclusiveStartKey), t.keys);
        items = items.slice(items.findIndex((i) => keyText(i, t.keys) === start) + 1);
      }
      const limit = typeof body.Limit === "number" ? body.Limit : items.length;
      const shown = items.slice(0, limit);
      const last = shown[shown.length - 1];
      const notices =
        shown.length < items.length && last
          ? [
              `More items follow: pass "ExclusiveStartKey": ${JSON.stringify(itemJson(keyOf(last, t.keys)!))}`,
            ]
          : [];
      return { items: shown, notices };
    }
    case "GetItem": {
      const t = tableNamed(str(body, "TableName"));
      const at = find(t, typedItem(body.Key));
      return at >= 0 ? { items: [t.items[at]!] } : { reply: {} };
    }
    case "PutItem": {
      const t = tableNamed(str(body, "TableName"));
      const item = typedItem(body.Item);
      const key = keyOf(item, t.keys);
      if (!key)
        fail("One or more parameter values were invalid: Missing the key", "ValidationException");
      const at = find(t, key!);
      if (at >= 0) t.items[at] = item;
      else t.items.push(item);
      return { reply: {} };
    }
    case "DeleteItem": {
      const t = tableNamed(str(body, "TableName"));
      const at = find(t, typedItem(body.Key));
      if (at >= 0) t.items.splice(at, 1);
      return { reply: {} };
    }
    default:
      throw new MockConsoleError(
        `unsupported: the mock console does not run ${action}`,
        "UNSUPPORTED",
      );
  }
}

// --- the run ----------------------------------------------------------------------------------

/** Items as a grid of their attributes (`items_grid`): first-seen order, at most 500. */
function grid(items: readonly Item[]): { columns: ResultColumn[]; rows: Value[][] } {
  const columns: ResultColumn[] = sampled(items)
    .slice(0, MAX_COLUMNS)
    .map(([name, types]) => {
      const list = (["S", "N", "B", "BOOL", "NULL", "M", "L", "SS", "NS", "BS"] as const).filter(
        (t) => types.has(t),
      );
      const real = list.filter((t) => t !== "NULL");
      const kind =
        real.length !== 1
          ? "other"
          : (
              {
                S: "text",
                N: "decimal",
                B: "bytes",
                BOOL: "bool",
                M: "json",
                L: "json",
                SS: "array",
                NS: "array",
                BS: "array",
              } as const
            )[real[0] as Exclude<(typeof list)[number], "NULL">];
      return { name, dataType: list.join("|"), kind, nullable: null, table: null };
    });
  return { columns, rows: items.map((i) => columns.map((c) => i[c.name] ?? MISSING)) };
}

const error = (message: string, code: string, detail: string | null): ErrorPayload => ({
  code,
  message,
  detail,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: 0,
  data: null,
});

export function runConsole(
  state: ConsoleState,
  statement: string,
  emit: (event: QueryEvent) => void,
): void {
  const span = { start: 0, end: statement.length, line: 1 };
  emit({ event: "started", data: { index: 0, sql: statement, span } });
  const base: StatementResult = {
    index: 0,
    sql: statement,
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
  const failed = (message: string, code: string, detail: string | null = null) =>
    emit({ event: "failed", data: { index: 0, error: error(message, code, detail) } });
  try {
    // The mock is given a line at a time (the backend splits on `;`).
    const commands = splitConsole(statement);
    if (commands.length === 0) {
      emit({ event: "finished", data: { index: 0, result: base } });
      return;
    }
    if (commands.length > 1)
      return failed("the mock console runs one command per line", "UNSUPPORTED");
    const cmd = parseCommand(statement);
    if (state.readOnly && classifyConsole(statement) === "write")
      return failed("This connection is read-only", "INVALID_INPUT");
    const out =
      cmd.kind === "partiql"
        ? partiql(cmd.statement)
        : api(cmd.action, JSON.parse(cmd.text) as Record<string, unknown>, state.region);
    const notices = out.notices ?? [];
    if (out.items) {
      const { columns, rows } = grid(out.items);
      emit({ event: "columns", data: { index: 0, set: 0, columns } });
      emit({ event: "rows", data: { index: 0, set: 0, rows } });
      emit({
        event: "finished",
        data: {
          index: 0,
          result: {
            ...base,
            kind: "rows",
            columns,
            resultSets: 1,
            rowCount: rows.length,
            affected: out.affected ?? null,
            notices,
          },
        },
      });
      return;
    }
    if (out.reply !== undefined) {
      const columns: ResultColumn[] = [
        { name: "reply", dataType: "M", kind: "json", nullable: null, table: null },
      ];
      const rows: Value[][] = [[{ t: "json", v: JSON.stringify(out.reply) }]];
      emit({ event: "columns", data: { index: 0, set: 0, columns } });
      emit({ event: "rows", data: { index: 0, set: 0, rows } });
      emit({
        event: "finished",
        data: {
          index: 0,
          result: { ...base, kind: "rows", columns, resultSets: 1, rowCount: 1, notices },
        },
      });
      return;
    }
    // A write: "1 row affected" where the reply proves it, else "OK".
    const affected = out.affected ?? null;
    emit({
      event: "finished",
      data: {
        index: 0,
        result: { ...base, kind: affected === null ? "empty" : "affected", affected, notices },
      },
    });
  } catch (e) {
    if (e instanceof MockConsoleError) failed(e.message, e.code, e.detail);
    else failed(e instanceof Error ? e.message : String(e), "INVALID_INPUT");
  }
}
