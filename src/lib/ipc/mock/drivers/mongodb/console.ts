import { classifyShell, Lexer, type Lit } from "@/features/drivers/mongodb/shell";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { StatementResult } from "../../../bindings/StatementResult";
import { databases, MISSING, type Collection, type Doc } from "./data";
import { litValue, queryMatches, sameValue, sortDocs } from "./query";
import { READ_ONLY_MESSAGE, sampledColumns, SYSTEM_DATABASES, type MongoState } from "./session";

/**
 * The mock's console: the common mongosh statements over the mock's documents (`use`, `show`,
 * `find` with its cursor chain, `findOne`, the counts, `distinct`, a few aggregation stages, the
 * inserts, updates and deletes, `getIndexes`, `drop`, `db.stats()`, `runCommand({ping: 1})`),
 * streamed as the backend's `console.rs` streams them. A read-only session refuses what the
 * console's classifier calls a write, before anything runs; what the mock does not know fails
 * with a message saying so.
 */

type Doc_ = Extract<Lit, { k: "doc" }>;

interface Call {
  name: string;
  args: Lit[];
}

/** A statement read as its target and calls (`db.c.find(…).sort(…)`). */
interface Chain {
  db: string | null;
  collection: string | null;
  calls: Call[];
}

class MockShellError extends Error {}

const fail = (message: string): never => {
  throw new MockShellError(message);
};

function readChain(text: string): Chain {
  const lx = new Lexer(text);
  if (lx.ident() !== "db") fail("the mock console runs db.…, use and show");
  const chain: Chain = { db: null, collection: null, calls: [] };
  const parts: string[] = [];
  for (;;) {
    if (lx.eat("[")) {
      chain.collection = lx.string();
      lx.expect("]");
      continue;
    }
    if (!lx.eat(".")) break;
    const name = lx.ident() ?? fail("expected a name");
    if (lx.peek() !== "(") {
      parts.push(name);
      continue;
    }
    lx.expect("(");
    const args = lx.args();
    const first = args[0];
    if (name === "getSiblingDB" && chain.collection === null && first?.k === "str")
      chain.db = first.v;
    else if (name === "getCollection" && first?.k === "str") chain.collection = first.v;
    else {
      if (chain.collection === null && parts.length > 0) chain.collection = parts.join(".");
      chain.calls.push({ name, args });
    }
  }
  lx.end();
  return chain;
}

const docArg = (a: Lit | undefined): Doc_ =>
  a === undefined || a.k === "null"
    ? { k: "doc", entries: [] }
    : a.k === "doc"
      ? a
      : fail("takes a document");

const intArg = (a: Lit | undefined): number =>
  a?.k === "num" && Number.isInteger(a.v) ? a.v : fail("takes a whole number");

/** A document of the shell's syntax as a stored document. */
function docOf(d: Doc_): Doc {
  return Object.fromEntries(d.entries.map(([k, v]) => [k, litValue(v)]));
}

/** `$set`, `$unset` and `$inc` applied to a copy of `d`. */
function updated(d: Doc, u: Doc_): Doc {
  const next = { ...d };
  for (const [op, fields] of u.entries) {
    if (fields.k !== "doc") fail(`${op} takes a document`);
    for (const [k, v] of (fields as Doc_).entries) {
      if (op === "$set") next[k] = litValue(v);
      else if (op === "$unset") delete next[k];
      else if (op === "$inc") {
        const by = litValue(v);
        const cur = next[k];
        const a = cur?.t === "int" || cur?.t === "float" ? cur.v : 0;
        const b = by.t === "int" || by.t === "float" ? by.v : fail("$inc takes a number");
        next[k] =
          Number.isInteger(a + b) && cur?.t !== "float"
            ? { t: "int", v: a + b }
            : { t: "float", v: a + b };
      } else fail(`the mock does not run ${op}`);
    }
  }
  return next;
}

/** The pipeline stages the mock runs. */
function aggregate(docs: Doc[], stages: Lit[]): Doc[] {
  let out = docs;
  for (const stage of stages) {
    const [op, arg] =
      stage.k === "doc" ? (stage.entries[0] ?? ["", stage]) : fail("a stage is a document");
    switch (op) {
      case "$match":
        out = out.filter((d) => queryMatches(d, docArg(arg)));
        break;
      case "$sort":
        out = sortDocs(
          out,
          docArg(arg).entries.map(([column, dir]) => ({
            column,
            dir: dir.k === "num" && dir.v < 0 ? "desc" : "asc",
            nulls: null,
          })),
        );
        break;
      case "$skip":
        out = out.slice(intArg(arg));
        break;
      case "$limit":
        out = out.slice(0, intArg(arg));
        break;
      case "$count":
        out = [{ [arg.k === "str" ? arg.v : "count"]: { t: "int", v: out.length } }];
        break;
      default:
        fail(`the mock runs $match, $sort, $skip, $limit and $count, not ${op}`);
    }
  }
  return out;
}

interface Outcome {
  docs?: Doc[];
  /** One column of values. */
  values?: { name: string; items: Doc[keyof Doc][] };
  affected?: number;
  notices?: string[];
}

function runChain(state: MongoState, chain: Chain): Outcome {
  const dbName =
    chain.db ??
    state.console ??
    [...databases.keys()].find((d) => !SYSTEM_DATABASES.includes(d)) ??
    "test";
  const db = databases.get(dbName);
  const [first, ...rest] = chain.calls;
  if (!first) return fail("expected a method call such as `.find()`");
  if (chain.collection === null) {
    switch (first.name) {
      case "getCollectionNames":
        return {
          values: {
            name: "name",
            items: [...(db?.keys() ?? [])].sort().map((n) => ({ t: "text", v: n })),
          },
        };
      case "stats":
        return {
          docs: [
            {
              db: { t: "text", v: dbName },
              collections: { t: "int", v: db?.size ?? 0 },
              objects: {
                t: "int",
                v: [...(db?.values() ?? [])].reduce((n, c) => n + c.docs.length, 0),
              },
              ok: { t: "float", v: 1 },
            },
          ],
        };
      case "version":
        return { values: { name: "version", items: [{ t: "text", v: "8.0.4" }] } };
      case "runCommand":
      case "adminCommand": {
        const a = first.args[0];
        const name = a?.k === "str" ? a.v : a?.k === "doc" ? a.entries[0]?.[0] : undefined;
        if (name === "ping") return { docs: [{ ok: { t: "float", v: 1 } }] };
        return fail(`the mock runs runCommand({ping: 1}), not ${name ?? "that command"}`);
      }
      default:
        return fail(`db.${first.name}() is not supported by the mock`);
    }
  }
  const coll: Collection =
    db?.get(chain.collection) ??
    (["insertOne", "insertMany"].includes(first.name)
      ? (() => {
          const created: Collection = {
            name: chain.collection!,
            kind: "collection",
            docs: [],
            indexes: [{ name: "_id_", keys: [["_id", 1]] }],
          };
          databases.get(dbName)?.set(created.name, created);
          return created;
        })()
      : fail(`ns does not exist: ${dbName}.${chain.collection}`));
  const [a0, a1] = first.args;
  const matching = () => coll.docs.filter((d) => queryMatches(d, docArg(a0)));
  switch (first.name) {
    case "find": {
      let docs = matching();
      for (const c of rest)
        switch (c.name) {
          case "sort":
            docs = aggregate(docs, [
              { k: "doc", entries: [["$sort", c.args[0] ?? { k: "null" }]] },
            ]);
            break;
          case "skip":
            docs = docs.slice(intArg(c.args[0]));
            break;
          case "limit":
            docs = docs.slice(0, Math.abs(intArg(c.args[0])) || undefined);
            break;
          case "count":
          case "size":
            return { values: { name: "count", items: [{ t: "int", v: docs.length }] } };
          case "toArray":
          case "pretty":
            break;
          default:
            fail(`${c.name}() is not supported by the mock`);
        }
      const projection = docArg(a1).entries.filter(([, v]) => v.k !== "num" || v.v !== 0);
      if (projection.length > 0) {
        const keep = new Set(["_id", ...projection.map(([k]) => k)]);
        docs = docs.map((d) => Object.fromEntries(Object.entries(d).filter(([k]) => keep.has(k))));
      }
      return { docs };
    }
    case "findOne":
      return { docs: matching().slice(0, 1) };
    case "countDocuments":
      return { values: { name: "count", items: [{ t: "int", v: matching().length }] } };
    case "estimatedDocumentCount":
    case "count":
      return {
        values: {
          name: "count",
          items: [{ t: "int", v: first.name === "count" ? matching().length : coll.docs.length }],
        },
      };
    case "distinct": {
      const field = a0?.k === "str" ? a0.v : fail("distinct() takes a field name");
      const seen: Doc[keyof Doc][] = [];
      for (const d of coll.docs.filter((x) => queryMatches(x, docArg(a1)))) {
        const v = d[field];
        if (v && !seen.some((s) => sameValue(s!, v))) seen.push(v);
      }
      return { values: { name: field, items: seen } };
    }
    case "aggregate":
      return { docs: aggregate(coll.docs, a0?.k === "arr" ? a0.items : first.args) };
    case "getIndexes":
      return {
        docs: coll.indexes.map((i) => ({
          v: { t: "int", v: 2 },
          key: { t: "json", v: JSON.stringify(Object.fromEntries(i.keys)) },
          name: { t: "text", v: i.name },
        })),
      };
    case "insertOne":
    case "insertMany": {
      const items =
        first.name === "insertOne"
          ? [docArg(a0)]
          : a0?.k === "arr"
            ? a0.items.map(docArg)
            : fail("insertMany() takes an array of documents");
      for (const item of items) {
        const doc = docOf(item);
        doc._id ??= {
          t: "other",
          v: { ty: "objectId", text: crypto.randomUUID().replaceAll("-", "").slice(0, 24) },
        };
        coll.docs.push(doc);
      }
      return { affected: items.length };
    }
    case "updateOne":
    case "updateMany": {
      const hits = matching();
      const targets = first.name === "updateOne" ? hits.slice(0, 1) : hits;
      let modified = 0;
      for (const d of targets) {
        const next = updated(d, docArg(a1));
        if (JSON.stringify(next) !== JSON.stringify(d)) modified += 1;
        coll.docs[coll.docs.indexOf(d)] = next;
      }
      return {
        affected: modified,
        notices: [`matched ${targets.length}, modified ${modified}, upserted 0`],
      };
    }
    case "deleteOne":
    case "deleteMany": {
      const hits = matching();
      const gone = first.name === "deleteOne" ? hits.slice(0, 1) : hits;
      coll.docs = coll.docs.filter((d) => !gone.includes(d));
      return { affected: gone.length };
    }
    case "drop":
      databases.get(dbName)?.delete(coll.name);
      return { values: { name: "value", items: [{ t: "bool", v: true }] } };
    default:
      return fail(`db.${chain.collection}.${first.name}() is not supported by the mock`);
  }
}

const error = (message: string, code = "DB_ERROR"): ErrorPayload => ({
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

/** Runs one console statement against the mock's documents, streaming `run_sql`'s events. */
export function runConsole(
  state: MongoState,
  statement: string,
  emit: (event: QueryEvent) => void,
): void {
  const span = { start: 0, end: statement.length, line: 1 };
  const started = new Date().toISOString();
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
    startedAt: started,
    durationMs: 1,
    error: null,
    notices: [],
  };
  const finish = (result: StatementResult) =>
    emit({ event: "finished", data: { index: 0, result } });
  if (state.readOnly && classifyShell(statement) === "write") {
    emit({ event: "failed", data: { index: 0, error: error(READ_ONLY_MESSAGE, "INVALID_INPUT") } });
    return;
  }
  const words = statement.trim().split(/\s+/);
  try {
    if (words[0] === "use" && words[1]) {
      state.console = words[1];
      return finish({ ...base, notices: [`switched to db ${words[1]}`] });
    }
    let out: Outcome;
    if (words[0] === "show") {
      if (words[1] === "dbs" || words[1] === "databases")
        out = {
          docs: [...databases.keys()].sort().map((name) => ({
            name: { t: "text", v: name },
            sizeOnDisk: { t: "int", v: 8192 * (databases.get(name)?.size ?? 0) },
            empty: { t: "bool", v: (databases.get(name)?.size ?? 0) === 0 },
          })),
        };
      else if (words[1] === "collections" || words[1] === "tables")
        out = runChain(state, {
          db: null,
          collection: null,
          calls: [{ name: "getCollectionNames", args: [] }],
        });
      else out = fail("show takes dbs, collections, users or roles");
    } else out = runChain(state, readChain(statement));
    if (out.affected !== undefined)
      return finish({
        ...base,
        kind: "affected",
        affected: out.affected,
        notices: out.notices ?? [],
      });
    const docs = out.docs ?? [];
    const columns: ResultColumn[] = out.values
      ? [{ name: out.values.name, dataType: "mixed", kind: "text", nullable: true, table: null }]
      : sampledColumns(docs.slice(0, 200)).map((c) => ({
          name: c.name,
          dataType: c.dataType,
          kind: c.kind,
          nullable: c.nullable,
          table: null,
        }));
    const rows = out.values
      ? out.values.items.map((v) => [v ?? MISSING])
      : docs.map((d) => columns.map((c) => d[c.name] ?? MISSING));
    emit({ event: "columns", data: { index: 0, set: 0, columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows } });
    finish({ ...base, kind: "rows", columns, resultSets: 1, rowCount: rows.length });
  } catch (e) {
    // The mock's refusals are thrown as `{code, message}` (`mockError`).
    const message =
      e instanceof Error ? e.message : String((e as { message?: unknown } | null)?.message ?? e);
    emit({ event: "failed", data: { index: 0, error: error(message) } });
  }
}
