import { commandName, tokenize, CLI_ERROR_TEXT } from "@/features/drivers/redis/cli";
import { classifyArgv, refusedInConsole } from "@/features/drivers/redis/classify";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import type { Server } from "./data";
import { exec, OK, type Reply } from "./engine";

/**
 * The mock's console (`drivers/redis/console.rs`): a command tokenized like redis-cli, refused on
 * a read-only connection when the classifier calls it a write and when the console does not run
 * it (blocking commands, pub/sub, connection state), then run on the console's own database, where
 * `SELECT` and `MULTI` … `EXEC` persist; the reply shaped as the backend's grids (`reply.rs`).
 */

export interface ConsoleState {
  server: Server;
  readOnly: boolean;
  /** The console's database (its `SELECT`). */
  db: number;
  /** The commands queued since `MULTI`; null outside a transaction. */
  queue: string[][] | null;
}

/** The backend's refusal of a write on a read-only connection (`READ_ONLY_MESSAGE`). */
export const READ_ONLY = "This connection is read-only";

const col = (name: string, dataType: string, kind: ValueKind): ResultColumn => ({
  name,
  dataType,
  kind,
  nullable: true,
  table: null,
});

/** A reply as a cell (`values::cell`): nil is NULL, nested replies JSON. */
export function cell(r: Reply): Value {
  switch (r.t) {
    case "nil":
      return { t: "null" };
    case "int":
      return Number.isSafeInteger(r.v) ? { t: "int", v: r.v } : { t: "bigInt", v: String(r.v) };
    case "bulk":
    case "status":
      return { t: "text", v: r.v };
    case "err":
      return { t: "other", v: { ty: "error", text: r.v } };
    case "arr":
      return { t: "json", v: JSON.stringify(plain(r)) };
  }
}

function plain(r: Reply): unknown {
  switch (r.t) {
    case "nil":
      return null;
    case "arr":
      return r.v.map(plain);
    default:
      return r.v;
  }
}

interface Grid {
  columns: ResultColumn[];
  rows: Value[][];
  notices: string[];
}

const has = (argv: string[], word: string) => argv.slice(1).some((a) => a.toUpperCase() === word);

/** A reply as the backend's grid (`reply::shape`). */
export function shape(argv: string[], r: Reply): Grid {
  const name = (argv[0] ?? "").toUpperCase();
  const indexed = (items: Reply[]): Grid => ({
    columns: [col("#", "integer", "int"), col("value", "reply", "other")],
    rows: items.map((v, i) => [{ t: "int", v: i + 1 }, cell(v)]),
    notices: [],
  });
  const pairs = (
    items: Reply[],
    first: string,
    second = "value",
    kind: ValueKind = "other",
  ): Grid => {
    const rows: Value[][] = [];
    for (let i = 0; i + 1 < items.length; i += 2) rows.push([cell(items[i]!), cell(items[i + 1]!)]);
    return {
      columns: [
        col(first, "reply", "text"),
        col(second, second === "score" ? "double" : "reply", kind),
      ],
      rows,
      notices: [],
    };
  };
  if (name === "INFO" && r.t === "bulk") {
    const rows: Value[][] = [];
    let section = "";
    for (const line of r.v.split(/\r?\n/)) {
      if (line.startsWith("# ")) section = line.slice(2).trim();
      else if (line.includes(":")) {
        const at = line.indexOf(":");
        rows.push([
          { t: "text", v: section },
          { t: "text", v: line.slice(0, at) },
          { t: "text", v: line.slice(at + 1) },
        ]);
      }
    }
    return {
      columns: [
        col("section", "reply", "text"),
        col("field", "reply", "text"),
        col("value", "reply", "text"),
      ],
      rows,
      notices: [],
    };
  }
  if (["SCAN", "SSCAN", "HSCAN", "ZSCAN"].includes(name) && r.t === "arr" && r.v.length === 2) {
    const [cursor, items] = r.v as [Reply, Reply];
    const list = items.t === "arr" ? items.v : [];
    const grid =
      name === "HSCAN" && !has(argv, "NOVALUES")
        ? pairs(list, "field")
        : {
            columns: [
              col(
                name === "SCAN" ? "key" : name === "HSCAN" ? "field" : "member",
                "reply",
                "text" as ValueKind,
              ),
            ],
            rows: list.map((v) => [cell(v)]),
            notices: [],
          };
    const c = cursor.t === "bulk" ? cursor.v : "0";
    grid.notices.push(c === "0" ? "cursor 0: the scan is complete" : `next cursor: ${c}`);
    return grid;
  }
  if (r.t === "arr") {
    if (name.startsWith("Z") && has(argv, "WITHSCORES"))
      return pairs(r.v, "member", "score", "float");
    const sub = (argv[1] ?? "").toUpperCase();
    const flat =
      name === "HGETALL" ||
      (name === "CONFIG" && sub === "GET") ||
      (name === "XINFO" && sub === "STREAM") ||
      (name === "MEMORY" && sub === "STATS") ||
      (name === "HRANDFIELD" && has(argv, "WITHVALUES"));
    return flat ? pairs(r.v, "field") : indexed(r.v);
  }
  return { columns: [col("value", "reply", "other")], rows: [[cell(r)]], notices: [] };
}

const failure = (code: string, message: string): ErrorPayload => ({
  code,
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: 0,
  data: null,
});

class ConsoleError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** Runs one command, as the backend's `run` (the reply, or what refuses it). */
function command(state: ConsoleState, argv: string[], raw: Uint8Array[]): Grid {
  const name = commandName(raw) ?? "";
  if (state.readOnly && classifyArgv(raw) === "write")
    throw new ConsoleError("INVALID_INPUT", READ_ONLY);
  const why = refusedInConsole(raw);
  if (why) throw new ConsoleError("UNSUPPORTED", `unsupported: Redis console: ${why}`);
  let reply: Reply;
  if (name === "MULTI") {
    reply = state.queue ? { t: "err", v: "ERR MULTI calls can not be nested" } : OK;
    state.queue ??= [];
  } else if (name === "DISCARD") {
    reply = state.queue ? OK : { t: "err", v: "ERR DISCARD without MULTI" };
    state.queue = null;
  } else if (name === "EXEC") {
    if (!state.queue) reply = { t: "err", v: "ERR EXEC without MULTI" };
    else {
      const queued = state.queue;
      state.queue = null;
      reply = { t: "arr", v: queued.map((q) => one(state, q)) };
    }
  } else if (name === "WATCH" || name === "UNWATCH") {
    reply = OK;
  } else if (state.queue) {
    state.queue.push(argv);
    reply = { t: "status", v: "QUEUED" };
  } else reply = one(state, argv);
  if (reply.t === "err") throw new ConsoleError("DB_ERROR", reply.v);
  return shape(argv, reply);
}

/** One command outside a transaction (also each one an EXEC runs). */
function one(state: ConsoleState, argv: string[]): Reply {
  if ((argv[0] ?? "").toUpperCase() === "SELECT") {
    const n = Number(argv[1]);
    if (!/^\d+$/.test(argv[1] ?? "") || n >= state.server.databases)
      return { t: "err", v: "ERR DB index is out of range" };
    state.db = n;
    return OK;
  }
  return exec(state.server, state.db, argv);
}

export function runConsole(
  state: ConsoleState,
  statement: string,
  emit: (event: QueryEvent) => void,
): void {
  const span = { start: 0, end: statement.length, line: 1 };
  emit({ event: "started", data: { index: 0, sql: statement, span } });
  const startedAt = new Date().toISOString();
  try {
    const t = tokenize(statement);
    if (!t.ok) throw new ConsoleError("INVALID_INPUT", CLI_ERROR_TEXT[t.error]);
    if (t.argv.length === 0) throw new ConsoleError("INVALID_INPUT", "no command");
    const argv = t.argv.map((a) => new TextDecoder("utf-8", { ignoreBOM: true }).decode(a));
    const g = command(state, argv, t.argv);
    emit({ event: "columns", data: { index: 0, set: 0, columns: g.columns } });
    if (g.rows.length > 0) emit({ event: "rows", data: { index: 0, set: 0, rows: g.rows } });
    const result: StatementResult = {
      index: 0,
      sql: statement,
      span,
      kind: "rows",
      columns: g.columns,
      resultSets: 1,
      rowCount: g.rows.length,
      truncated: false,
      affected: null,
      lastInsertId: null,
      startedAt,
      durationMs: 1,
      error: null,
      notices: g.notices,
    };
    emit({ event: "finished", data: { index: 0, result } });
  } catch (e) {
    const err = e instanceof ConsoleError ? e : new ConsoleError("INTERNAL", String(e));
    emit({ event: "failed", data: { index: 0, error: failure(err.code, err.message) } });
  }
}
