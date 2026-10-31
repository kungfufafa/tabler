import { classifyConsole } from "@/features/drivers/elasticsearch/classify";
import { fromPlain, get, toText, type Json } from "@/features/drivers/elasticsearch/json";
import {
  bodyValues,
  parseCommand,
  queryValue,
  type EsRequest,
} from "@/features/drivers/elasticsearch/request";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import { indicesOf, mergedFields, type Cluster, type Doc, type Index } from "./data";
import type { EsFeatures } from "./session";
import { at, cellOf, kindOf } from "./values";

/**
 * The mock's console (`EsSession::run_console`): read-only refusals by the shared classifier
 * first, then the requests the mock's cluster can answer, shaped as the backend's grids
 * (`reply.rs`): hits with their `_source` leaves, `_cat` tables, SQL results, and any other reply
 * as one JSON cell. The shared mock hands a native console its text a line at a time, so a
 * request's body on its own lines runs only in the app.
 */

export interface ConsoleContext {
  cluster: Cluster;
  readOnly: boolean;
  opaque: string;
  features: EsFeatures;
  flavor: "elasticsearch" | "opensearch";
}

/** The backend's refusal of a console write on a read-only connection. */
export const CONSOLE_READ_ONLY =
  "this connection is read-only: the command may write, and it was not run";

interface Grid {
  columns: ResultColumn[];
  rows: Value[][];
  notices: string[];
}

const col = (name: string, dataType: string, kind: ValueKind): ResultColumn => ({
  name,
  dataType,
  kind,
  nullable: true,
  table: null,
});

const jsonGrid = (reply: unknown): Grid => ({
  columns: [col("response", "json", "json")],
  rows: [[{ t: "json", v: toText(fromPlain(reply), 2) }]],
  notices: [],
});

class ConsoleError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly status: number | null = null,
    readonly detail: string | null = null,
  ) {
    super(message);
  }
}

const fail = (message: string, status = 400, detail: string | null = null): never => {
  throw new ConsoleError("DB_ERROR", message, status, detail);
};

export async function runConsole(
  ctx: ConsoleContext,
  statement: string,
  emit: (event: QueryEvent) => void,
): Promise<void> {
  const span = { start: 0, end: statement.length, line: 1 };
  emit({ event: "started", data: { index: 0, sql: statement, span } });
  const started = new Date().toISOString();
  try {
    if (ctx.readOnly && classifyConsole(statement) === "write")
      throw new ConsoleError("INVALID_INPUT", CONSOLE_READ_ONLY);
    let command;
    try {
      command = parseCommand(statement);
    } catch (e) {
      throw new ConsoleError("INVALID_INPUT", (e as Error).message);
    }
    const grids = command.kind === "sql" ? [sqlGrid(ctx, command.sql)] : request(ctx, command.req);
    let rows = 0;
    grids.forEach((g, set) => {
      emit({ event: "columns", data: { index: 0, set, columns: g.columns } });
      if (g.rows.length > 0) emit({ event: "rows", data: { index: 0, set, rows: g.rows } });
      rows += g.rows.length;
    });
    const result: StatementResult = {
      index: 0,
      sql: statement,
      span,
      kind: "rows",
      columns: grids[0]?.columns ?? [],
      resultSets: grids.length,
      rowCount: rows,
      truncated: false,
      affected: null,
      lastInsertId: null,
      startedAt: started,
      durationMs: 2,
      error: null,
      notices: grids.flatMap((g) => g.notices),
    };
    emit({ event: "finished", data: { index: 0, result } });
  } catch (e) {
    const err = e instanceof ConsoleError ? e : new ConsoleError("INTERNAL", String(e));
    const error: ErrorPayload = {
      code: err.code,
      message: err.message,
      detail: err.detail,
      hint: null,
      sqlstate: null,
      vendorCode: err.status,
      position: null,
      statementIndex: 0,
      data: null,
    };
    emit({ event: "failed", data: { index: 0, error } });
  }
}

// ---- requests --------------------------------------------------------------------------------

function targetIndices(ctx: ConsoleContext, target: string): Index[] {
  const found = indicesOf(ctx.cluster, target === "_all" ? "*" : target);
  if (found.length === 0 && !target.includes("*"))
    fail(`no such index [${target}]`, 404, "index_not_found_exception");
  return found;
}

function request(ctx: ConsoleContext, req: EsRequest): Grid[] {
  const [first, second, third] = req.segments;
  const body = req.body ? bodyValues(req.body)[0] : undefined;
  const m = req.method;
  const reading = m === "GET" || m === "HEAD" || m === "POST";
  if (req.segments.length === 0 && (m === "GET" || m === "HEAD"))
    return [
      jsonGrid({
        name: "node-1",
        cluster_name: ctx.cluster.name,
        version: {
          ...(ctx.flavor === "opensearch"
            ? { distribution: "opensearch", number: "2.19.1" }
            : { number: "9.1.5", build_flavor: "default" }),
        },
        tagline:
          ctx.flavor === "opensearch"
            ? "The OpenSearch Project: https://opensearch.org/"
            : "You Know, for Search",
      }),
    ];
  if (first === "_cat" && m === "GET") return [catGrid(ctx, second ?? "", req)];
  if (first === "_cluster" && second === "health" && m === "GET")
    return [jsonGrid({ cluster_name: ctx.cluster.name, status: "green", number_of_nodes: 1 })];
  if (first === "_tasks" && m === "GET")
    return [
      jsonGrid({
        tasks: Object.fromEntries(
          ctx.cluster.tasks.map((t) => [t.id, { action: t.action, description: t.description }]),
        ),
      }),
    ];
  if (first === "_search" && reading) return searchGrids(ctx, "*", req, body);
  if (first === undefined) fail(`no handler for ${m} /`, 405);
  const target = first!;
  if (second === "_search" && third === undefined && reading)
    return searchGrids(ctx, target, req, body);
  if (second === "_count" && reading) {
    const docs = hits(ctx, target, body, req).length;
    return [jsonGrid({ count: docs, _shards: { total: 1, successful: 1, skipped: 0, failed: 0 } })];
  }
  if ((second === "_mapping" || second === "_mappings") && m === "GET") {
    const out: Record<string, unknown> = {};
    for (const i of targetIndices(ctx, target))
      out[i.name] = {
        mappings: {
          properties: Object.fromEntries(i.fields.map((f) => [f.path, { type: f.type }])),
        },
      };
    return [jsonGrid(out)];
  }
  if (second === "_doc" && third !== undefined) {
    const index = targetIndices(ctx, target)[0]!;
    const at_ = index.docs.findIndex((d) => d.id === third);
    const doc = index.docs[at_];
    if (m === "GET" || m === "HEAD")
      return [
        jsonGrid(
          doc
            ? {
                _index: index.name,
                _id: doc.id,
                _seq_no: doc.seqNo,
                _primary_term: doc.primaryTerm,
                found: true,
                _source: doc.source,
              }
            : { _index: index.name, _id: third, found: false },
        ),
      ];
    if (m === "DELETE") {
      if (!doc) return [jsonGrid({ _index: index.name, _id: third, result: "not_found" })];
      index.docs.splice(at_, 1);
      return [
        jsonGrid({ _index: index.name, _id: third, result: "deleted", _seq_no: index.nextSeqNo++ }),
      ];
    }
    if (m === "PUT" || m === "POST") return indexDoc(index, third, body);
  }
  if (second === "_doc" && third === undefined && m === "POST")
    return indexDoc(targetIndices(ctx, target)[0]!, null, body);
  if (
    ["_refresh", "_flush", "_forcemerge"].includes(second ?? "") &&
    (m === "POST" || m === "GET")
  ) {
    targetIndices(ctx, target);
    return [jsonGrid({ _shards: { total: 2, successful: 1, failed: 0 } })];
  }
  if (second === undefined && m === "PUT") {
    if (ctx.cluster.indices.has(target))
      fail(`index [${target}/abc123] already exists`, 400, "resource_already_exists_exception");
    ctx.cluster.indices.set(target, {
      name: target,
      fields: [],
      docs: [],
      hidden: false,
      status: "open",
      window: 10_000,
      nextSeqNo: 0,
    });
    return [jsonGrid({ acknowledged: true, shards_acknowledged: true, index: target })];
  }
  if (second === undefined && m === "DELETE") {
    if (!ctx.cluster.indices.delete(target))
      fail(`no such index [${target}]`, 404, "index_not_found_exception");
    return [jsonGrid({ acknowledged: true })];
  }
  if (second === undefined && (m === "GET" || m === "HEAD")) {
    const out: Record<string, unknown> = {};
    for (const i of targetIndices(ctx, target))
      out[i.name] = { aliases: {}, settings: { index: { number_of_shards: "1" } } };
    return [jsonGrid(out)];
  }
  return fail(
    `the mock cluster does not serve ${m} /${req.segments.join("/")}; the app sends it to the cluster`,
    400,
    "illegal_argument_exception",
  );
}

function indexDoc(index: Index, id: string | null, body: Json | undefined): Grid[] {
  if (body?.k !== "obj")
    fail("request body is required", 400, "action_request_validation_exception");
  const source = JSON.parse(toText(body!)) as Record<string, unknown>;
  const docId = id ?? `mock${index.nextSeqNo}`;
  const old = index.docs.findIndex((d) => d.id === docId);
  const doc: Doc = { id: docId, seqNo: index.nextSeqNo++, primaryTerm: 1, source };
  if (old >= 0) index.docs[old] = doc;
  else index.docs.push(doc);
  return [
    jsonGrid({
      _index: index.name,
      _id: docId,
      result: old >= 0 ? "updated" : "created",
      _seq_no: doc.seqNo,
      _primary_term: 1,
    }),
  ];
}

interface Hit {
  index: Index;
  doc: Doc;
}

/** The hits a search finds: `q=field:value`, a `term` / `match` / `match_all` query, or all. */
function hits(ctx: ConsoleContext, target: string, body: Json | undefined, req: EsRequest): Hit[] {
  const all = targetIndices(ctx, target).flatMap((index) =>
    index.docs.map((doc) => ({ index, doc })),
  );
  const q = queryValue(req, "q");
  const query = get(body, "query");
  const leaf = (kind: string) => {
    const inner = get(query, kind);
    const pair = inner?.k === "obj" ? inner.v[0] : undefined;
    if (!pair) return null;
    const [field, v] = pair;
    const value = v.k === "obj" ? (get(v, "query") ?? get(v, "value")) : v;
    return value === undefined
      ? null
      : { field, value: value.k === "str" || value.k === "num" ? value.v : toText(value) };
  };
  const cond = q
    ? q.includes(":")
      ? { field: q.slice(0, q.indexOf(":")), value: q.slice(q.indexOf(":") + 1) }
      : { field: null, value: q }
    : (leaf("term") ?? leaf("match"));
  if (!cond) return all;
  const text = (h: Hit, path: string) => {
    const v = at(h.doc.source, path);
    return v === undefined ? "" : typeof v === "object" ? JSON.stringify(v) : String(v);
  };
  return all.filter((h) =>
    cond.field === null
      ? JSON.stringify(h.doc.source).toLowerCase().includes(cond.value.toLowerCase())
      : text(h, cond.field.replace(/\.keyword$/, "")).toLowerCase() === cond.value.toLowerCase(),
  );
}

/** Hits as a grid: `_index`, `_id`, `_score`, then the `_source` leaves (`reply::hits_or_json`). */
function searchGrids(
  ctx: ConsoleContext,
  target: string,
  req: EsRequest,
  body: Json | undefined,
): Grid[] {
  const found = hits(ctx, target, body, req);
  const sizeText =
    queryValue(req, "size") ??
    (get(body, "size")?.k === "num" ? (get(body, "size") as { v: string }).v : null);
  const size = sizeText !== null && /^\d+$/.test(sizeText) ? Number(sizeText) : 10;
  const page = found.slice(0, size);
  const fields = mergedFields(targetIndices(ctx, target));
  const columns = [
    col("_index", "keyword", "text"),
    col("_id", "keyword", "text"),
    col("_score", "float", "float"),
    ...fields.map((f) => col(f.path, f.type, kindOf(f.type))),
  ];
  const rows = page.map((h): Value[] => [
    { t: "text", v: h.index.name },
    { t: "text", v: h.doc.id },
    { t: "float", v: 1 },
    ...fields.map((f) => cellOf(at(h.doc.source, f.path), f.type)),
  ]);
  const notices = [`${found.length} hits`];
  if (found.length > page.length) notices.push(`showing the first ${page.length} (size)`);
  return [{ columns, rows, notices }];
}

function catGrid(ctx: ConsoleContext, what: string, req: EsRequest): Grid {
  const text = (v: string | number): Value => ({ t: "text", v: String(v) });
  const table = (names: string[], rows: (string | number)[][]): Grid => ({
    columns: names.map((n) => col(n, "keyword", "text")),
    rows: rows.map((r) => r.map(text)),
    notices: [],
  });
  void req;
  if (what === "indices")
    return table(
      ["health", "status", "index", "pri", "rep", "docs.count", "store.size"],
      [...ctx.cluster.indices.values()]
        .filter((i) => !i.hidden)
        .map((i) => [
          "green",
          i.status,
          i.name,
          1,
          1,
          i.docs.length,
          `${Math.round((4096 + i.docs.length * 310) / 1024)}kb`,
        ]),
    );
  if (what === "aliases")
    return table(
      ["alias", "index", "filter", "routing.index", "routing.search", "is_write_index"],
      [...ctx.cluster.aliases].flatMap(([a, targets]) =>
        targets.map((t) => [a, t, "-", "-", "-", "-"]),
      ),
    );
  if (what === "health")
    return table(
      ["cluster", "status", "node.total", "shards"],
      [[ctx.cluster.name, "green", 1, ctx.cluster.indices.size]],
    );
  return fail(`the mock cluster has no _cat/${what}`, 400, "illegal_argument_exception");
}

// ---- SQL -------------------------------------------------------------------------------------

/** `SHOW TABLES`, `DESCRIBE <index>` and `SELECT <cols> FROM <index> [LIMIT n]`. */
function sqlGrid(ctx: ConsoleContext, sql: string): Grid {
  const text = (v: string): Value => ({ t: "text", v });
  if (/^show\s+tables$/i.test(sql)) {
    const rows: Value[][] = [
      ...[...ctx.cluster.indices.values()]
        .filter((i) => !i.hidden)
        .map((i) => [text(ctx.cluster.name), text(i.name), text("TABLE"), text("INDEX")]),
      ...[...ctx.cluster.aliases.keys()].map((a) => [
        text(ctx.cluster.name),
        text(a),
        text("VIEW"),
        text("ALIAS"),
      ]),
    ];
    return {
      columns: ["catalog", "name", "type", "kind"].map((n) => col(n, "keyword", "text")),
      rows,
      notices: [],
    };
  }
  const describe = /^(?:describe|desc)\s+("?)([^"\s;]+)\1$/i.exec(sql);
  if (describe) {
    const fields = mergedFields(targetIndices(ctx, describe[2]!));
    return {
      columns: ["column", "type", "mapping"].map((n) => col(n, "keyword", "text")),
      rows: fields.map((f) => [text(f.path), text(sqlType(f.type)), text(f.type)]),
      notices: [],
    };
  }
  const select = /^select\s+(.+?)\s+from\s+("?)([^"\s;]+)\2(?:\s+limit\s+(\d+))?$/is.exec(sql);
  if (!select)
    return fail(
      "the mock's SQL runs SELECT … FROM <index> [LIMIT n], SHOW TABLES and DESCRIBE; the app sends the rest to the cluster",
      400,
      "parsing_exception",
    );
  const [, list, , target, limit] = select;
  const fields = mergedFields(targetIndices(ctx, target!));
  const wanted =
    list!.trim() === "*"
      ? fields
      : list!.split(",").map((c) => {
          const name = c.trim().replace(/^"|"$/g, "");
          return (
            fields.find((f) => f.path === name) ??
            fail(`Unknown column [${name}]`, 400, "verification_exception")
          );
        });
  const docs = hits(ctx, target!, undefined, {
    method: "GET",
    segments: [],
    query: [],
    body: null,
  });
  const rows = docs
    .slice(0, limit ? Number(limit) : 1000)
    .map((h) => wanted.map((f) => cellOf(at(h.doc.source, f.path) ?? null, f.type)));
  return {
    columns: wanted.map((f) => col(f.path, sqlType(f.type), kindOf(f.type))),
    rows,
    notices: [],
  };
}

const sqlType = (type: string): string =>
  ({
    long: "long",
    keyword: "keyword",
    text: "text",
    date: "datetime",
    double: "double",
    float: "float",
    boolean: "boolean",
  })[type] ?? type;
