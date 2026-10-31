import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterOp } from "../../../bindings/FilterOp";
import type { IndexMeta } from "../../../bindings/IndexMeta";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SafeConfirm } from "../../../bindings/SafeConfirm";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { TableKind } from "../../../bindings/TableKind";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { TableSummary } from "../../../bindings/TableSummary";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { runConsole } from "./console";
import { databases, MISSING, typeLabel, type Collection, type Doc } from "./data";
import { Lexer, ParseError, parseLiteral, type Lit } from "@/features/drivers/mongodb/shell";
import {
  docJson,
  filterMatches,
  litValue,
  queryMatches,
  sameValue,
  sortDocs,
  valueJson,
} from "./query";

/**
 * A mock MongoDB session, mirroring the backend's (`drivers/mongodb/session.rs`, `calls.rs`):
 * the same caps (one transaction on a replica set, changes one after the other on a standalone
 * server), the session's database as the only schema (or every database but the server's own),
 * sampled columns, offset pages sorted with `_id` breaking ties, the collection's own count unless
 * a filter asks for documents counted, edits by `_id` that a validator may refuse, the console
 * (`console.ts`), the session's calls, Running Queries, and every refusal of a read-only session.
 */

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** The 23 filter operators the backend's `FILTER_OPS` grants. */
export const FILTER_OPS: FilterOp[] = [
  "eq",
  "ne",
  "gt",
  "lt",
  "ge",
  "le",
  "like",
  "notLike",
  "in",
  "notIn",
  "isNull",
  "isNotNull",
  "between",
  "notBetween",
  "contains",
  "notContains",
  "containsCase",
  "notContainsCase",
  "hasPrefix",
  "hasSuffix",
  "hasPrefixCase",
  "hasSuffixCase",
  "ilike",
];

/** `session::capabilities`: `transactions` on a replica set or mongos. */
export function mongoCaps(
  transactions: boolean,
  database: string | null,
  confirm: SafeConfirm,
): SessionCaps {
  return {
    driver: "mongodb",
    flavor: null,
    dialect: null,
    language: "mongoShell",
    // In `Feature`'s order, as the backend sorts them.
    features: [
      "editor",
      "cancelOnServer",
      "editRows",
      "insertRows",
      "deleteRows",
      "serverExpressions",
      "exactCount",
      "structureView",
      "viewDefinition",
      "switchDatabase",
      "processList",
      "killProcess",
    ],
    namespaces: {
      databases: true,
      schemas: "databaseIsSchema",
      defaultSchema: database,
      labels: {
        database: "Database",
        schema: "Database",
        table: "Collection",
        tables: "Collections",
      },
    },
    data: {
      paging: "offset",
      count: "estimate",
      sort: true,
      filterOps: FILTER_OPS,
      rawFilter: "json",
      transactions: transactions ? "atomic" : "sequential",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: ["now", "currentTimestamp"],
      maxParams: 0,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** The server's own databases (the descriptor's `systemSchemas`). */
export const SYSTEM_DATABASES = ["admin", "config", "local"];

const KIND_OF: Record<string, ValueKind> = {
  double: "float",
  string: "text",
  null: "text",
  missing: "text",
  object: "json",
  array: "json",
  binData: "bytes",
  uuid: "uuid",
  bool: "bool",
  date: "timestampTz",
  int: "int",
  long: "bigInt",
  decimal: "decimal",
};

/** The columns of a sample (`schema::infer` + `schema::columns`). */
export function sampledColumns(sample: readonly Doc[]): ColumnMeta[] {
  const fields = new Map<string, { present: number; types: Map<string, number> }>();
  for (const d of sample)
    for (const [k, v] of Object.entries(d)) {
      if (v.t === "other" && v.v.ty === "missing") continue;
      const f = fields.get(k) ?? { present: 0, types: new Map<string, number>() };
      f.present += 1;
      const label = typeLabel(v);
      f.types.set(label, (f.types.get(label) ?? 0) + 1);
      fields.set(k, f);
    }
  const ordered = [...fields].sort(
    ([a, fa], [b, fb]) =>
      Number(b === "_id") - Number(a === "_id") || fb.present - fa.present || (a < b ? -1 : 1),
  );
  const column = (
    name: string,
    dataType: string,
    kind: ValueKind,
    nullable: boolean,
    ordinal: number,
    extra: string | null,
  ): ColumnMeta => ({
    name,
    ordinal,
    dataType,
    kind,
    nullable,
    defaultExpr: null,
    isPk: name === "_id",
    pkOrdinal: name === "_id" ? 1 : null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra,
    baseType: null,
  });
  const out: ColumnMeta[] =
    sample.length === 0 ? [column("_id", "objectId", "other", false, 1, null)] : [];
  for (const [name, f] of ordered) {
    const types = [...f.types].sort(([a, na], [b, nb]) => nb - na || (a < b ? -1 : 1));
    const dominant = types.find(([t]) => t !== "null")?.[0] ?? "null";
    out.push(
      column(
        name,
        types.map(([t]) => t).join("|"),
        KIND_OF[dominant] ?? "other",
        name !== "_id" && (f.types.has("null") || f.present < sample.length),
        out.length + 1,
        `in ${f.present} of ${sample.length} sampled documents`,
      ),
    );
  }
  return out;
}

const tableKind = (c: Collection): TableKind =>
  c.name.startsWith("system.") ? "systemView" : c.kind === "view" ? "view" : "table";

const editable = (c: Collection) => tableKind(c) === "table";

const comment = (c: Collection): string | null =>
  c.kind === "view" ? `view on ${c.viewOn}` : c.kind === "timeseries" ? "time series" : null;

/** `db.getSiblingDB(…).runCommand({…})`, as `mongo::command_text` writes it. */
export const commandText = (db: string | null, body: string): string =>
  db === null
    ? `db.runCommand(${body})`
    : `db.getSiblingDB(${JSON.stringify(db)}).runCommand(${body})`;

function createText(c: Collection): string {
  if (c.kind === "view")
    return `db.createView(${JSON.stringify(c.name)}, ${JSON.stringify(c.viewOn)}, ${c.pipeline})`;
  const options: [string, string][] = [];
  if (c.kind === "timeseries") options.push(["timeseries", '{"timeField": "at"}']);
  if (c.validator) options.push(["validator", c.validator]);
  return options.length === 0
    ? `db.createCollection(${JSON.stringify(c.name)})`
    : `db.createCollection(${JSON.stringify(c.name)}, ${docJson(options)})`;
}

const keysJson = (i: { keys: [string, number | string][] }) =>
  docJson(i.keys.map(([k, d]) => [k, JSON.stringify(d)]));

function indexMeta(c: Collection): IndexMeta[] {
  return c.indexes.map((i) => {
    const options: [string, string][] = [["name", JSON.stringify(i.name)]];
    if (i.unique) options.push(["unique", "true"]);
    return {
      name: i.name,
      columns: i.keys.map(([k]) => k),
      unique: !!i.unique || i.name === "_id_",
      primary: i.name === "_id_",
      method: i.keys.some(([, d]) => typeof d === "string") ? String(i.keys[0]![1]) : null,
      predicate: null,
      definition: `db.getCollection(${JSON.stringify(c.name)}).createIndex(${keysJson(i)}, ${docJson(options)})`,
      comment: null,
    };
  });
}

/** A literal as relaxed Extended JSON, as the backend prints the parsed BSON (`bson_json`). */
function litText(v: Lit): string {
  switch (v.k) {
    case "doc":
      return docJson(v.entries.map(([k, x]) => [k, litText(x)]));
    case "arr":
      return `[${v.items.map(litText).join(", ")}]`;
    default:
      return valueJson(litValue(v));
  }
}

/** An index's default name from its keys (`mongo::index_name`): `a_1_b_-1`, `body_text`. */
function indexName(keys: readonly [string, Lit][]): string {
  return keys
    .map(([k, v]) => `${k}_${v.k === "num" ? String(v.v) : v.k === "str" ? v.v : litText(v)}`)
    .join("_");
}

/** A view's pipeline text as its stages; null when it is not an array (`calls.rs` refuses it). */
function stagesOf(text: string): Lit[] | null {
  try {
    const v = parseLiteral(text);
    return v.k === "arr" ? v.items : null;
  } catch {
    return null;
  }
}

/** What a view shows: its source's documents through its `$match` stages (others are skipped). */
function viewDocs(db: Map<string, Collection>, c: Collection): Doc[] {
  const source = c.viewOn ? db.get(c.viewOn) : undefined;
  if (!source) return [];
  const matches = (stagesOf(c.pipeline ?? "[]") ?? []).flatMap((st) =>
    st.k === "doc" && st.entries.length === 1 && st.entries[0]![0] === "$match"
      ? [st.entries[0]![1]]
      : [],
  );
  return source.docs.filter((d) => matches.every((m) => m.k === "doc" && queryMatches(d, m)));
}

const failure = (
  message: string,
  vendorCode: number | null = null,
  index: number | null = null,
): ErrorPayload => ({
  code: vendorCode === null ? "INVALID_INPUT" : "DB_ERROR",
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode,
  position: null,
  statementIndex: index,
  data: null,
});

const idOf = (key: RowKey): Value =>
  key.kind === "pk" && key.value._id
    ? key.value._id
    : mockError("UNSUPPORTED", "unsupported: MongoDB documents are edited by their _id");

/** The `orders` validator: a total, of zero or more. */
function validates(c: Collection, d: Doc): boolean {
  if (!c.validator) return true;
  const total = d.total;
  if (total === undefined || (total.t === "other" && total.v.ty === "missing")) return false;
  const n =
    total.t === "int" || total.t === "float"
      ? total.v
      : total.t === "decimal"
        ? Number(total.v)
        : NaN;
  return !Number.isNaN(n) && n >= 0;
}

export interface MockMongoOptions {
  readOnly: boolean;
  database: string | null;
  transactions: boolean;
  confirm: SafeConfirm;
  sampleSize: number;
}

/** The session state the console shares with the rest (`use` switches its database). */
export interface MongoState {
  readOnly: boolean;
  /** The console's database (`use`); the session's otherwise. */
  console: string | null;
  database: string | null;
}

export function optionsOf(p: ConnectionProfile): MockMongoOptions {
  const params = p.options.driverParams ?? {};
  const sample = Number(params.sampleSize);
  return {
    readOnly: p.options.readOnly,
    database: p.database?.trim() || null,
    transactions: typeof params.replicaSet === "string" && params.replicaSet !== "",
    confirm: p.options.passwordMode === "command" ? "passcode" : "password",
    sampleSize: Number.isInteger(sample) && sample > 0 ? sample : 100,
  };
}

/** Running Queries: two operations of other clients and the listing itself. */
function seedProcesses(): ProcessRow[] {
  const row = (id: string, app: string, op: string, query: string, ms: number): ProcessRow => ({
    id,
    user: "app@admin",
    database: "shop",
    application: app,
    client: "10.0.4.17:51522",
    state: "active",
    command: op,
    startedAt: "2026-10-31T09:00:00.000Z",
    durationMs: ms,
    waitEvent: null,
    query,
    current: false,
    own: false,
  });
  return [
    row("4011", "orders-api", "query", '{"find": "orders", "filter": {"status": "new"}}', 1250),
    row(
      "4012",
      "reporting",
      "command",
      '{"aggregate": "events", "pipeline": [{"$group": {"_id": "$type"}}]}',
      48_000,
    ),
  ];
}

export function mongoSession(profile: ConnectionProfile): MockSession {
  const o = optionsOf(profile);
  const state: MongoState = { readOnly: o.readOnly, console: o.database, database: o.database };
  const processes = seedProcesses();

  const dbName = (table: TableRef): string =>
    table.schema?.trim() ||
    o.database ||
    mockError("INVALID_INPUT", "MongoDB: choose a database first");
  const dbOf = (name: string) =>
    databases.get(name) ?? mockError("NOT_FOUND", `not found: database ${name}`);
  const coll = (table: TableRef): Collection =>
    dbOf(dbName(table)).get(table.name) ??
    mockError("NOT_FOUND", `not found: collection ${dbName(table)}.${table.name}`);
  const docsOf = (table: TableRef): Doc[] => {
    const c = coll(table);
    return c.kind === "view" ? viewDocs(dbOf(dbName(table)), c) : c.docs;
  };

  const tableMeta = (table: TableRef): TableMeta => {
    const c = coll(table);
    const docs = docsOf(table);
    const columns = sampledColumns(docs.slice(0, o.sampleSize));
    const indexes = indexMeta(c);
    const ddl = [
      `${createText(c)};`,
      ...indexes.filter((i) => !i.primary).map((i) => `${i.definition};`),
    ].join("\n");
    return {
      table: { schema: dbName(table), name: c.name },
      kind: tableKind(c),
      columns,
      primaryKey: editable(c) && columns.some((col) => col.name === "_id") ? ["_id"] : [],
      indexes,
      foreignKeys: [],
      triggers: [],
      comment: comment(c),
      rowEstimate: docs.length,
      ddl,
      checks: c.validator
        ? [{ name: "validator", expression: c.validator, columns: ["total"] }]
        : [],
      options: null,
    };
  };

  /** One change as its command (`mongo::plan_changes`), with the document it leaves. */
  const commandOf = (c: Collection, ch: StagedChange): { body: string; op: string } => {
    const exprs = ch.op === "delete" ? {} : (ch.exprs ?? {});
    const dates = Object.keys(exprs);
    if (ch.op === "insert") {
      const fields = Object.entries(ch.values).filter(
        ([k, v]) => !(v.t === "other" && v.v.ty === "missing") && !dates.includes(k),
      );
      const doc = docJson(fields.map(([k, v]) => [k, valueJson(v)]));
      return {
        op: "insert",
        body: `{"insert": ${JSON.stringify(c.name)}, "documents": [${doc}], "ordered": true}`,
      };
    }
    const q = `{"q": {"_id": ${valueJson(idOf(ch.key))}}`;
    if (ch.op === "delete")
      return {
        op: "delete",
        body: `{"delete": ${JSON.stringify(c.name)}, "deletes": [${q}, "limit": 1}], "ordered": true}`,
      };
    const set = Object.entries(ch.changes).filter(
      ([k, v]) => !(v.t === "other" && v.v.ty === "missing") && !dates.includes(k),
    );
    const unset = Object.entries(ch.changes).filter(
      ([, v]) => v.t === "other" && v.v.ty === "missing",
    );
    const u: [string, string][] = [];
    if (set.length > 0) u.push(["$set", docJson(set.map(([k, v]) => [k, valueJson(v)]))]);
    if (unset.length > 0) u.push(["$unset", docJson(unset.map(([k]) => [k, '""']))]);
    if (dates.length > 0) u.push(["$currentDate", docJson(dates.map((k) => [k, "true"]))]);
    return {
      op: "update",
      body: `{"update": ${JSON.stringify(c.name)}, "updates": [${q}, "u": ${docJson(u)}, "multi": false}], "ordered": true}`,
    };
  };

  const preview = (req: ApplyRequest): string[] => {
    const c = coll(req.table);
    const db = dbName(req.table);
    return req.changes.map((ch) =>
      commandText(db === o.database ? null : db, commandOf(c, ch).body),
    );
  };

  /** Applies one change to `docs`; the error that refuses it, or null. */
  const applyOne = (c: Collection, docs: Doc[], ch: StagedChange): ErrorPayload | null => {
    if (!editable(c))
      return failure(
        `MongoDB: ${c.name} is ${c.kind === "view" ? "a view" : "a system collection"}: its documents cannot be edited`,
      );
    const now = new Date().toISOString();
    const exprs = ch.op === "delete" ? {} : (ch.exprs ?? {});
    const withDates = (d: Doc): Doc => {
      for (const k of Object.keys(exprs)) d[k] = { t: "timestampTz", v: now };
      return d;
    };
    if (ch.op === "insert") {
      const doc: Doc = {};
      for (const [k, v] of Object.entries(ch.values))
        if (!(v.t === "other" && v.v.ty === "missing")) doc[k] = v;
      doc._id ??= {
        t: "other",
        v: { ty: "objectId", text: crypto.randomUUID().replaceAll("-", "").slice(0, 24) },
      };
      withDates(doc);
      if (docs.some((d) => sameValue(d._id ?? MISSING, doc._id!)))
        return failure("E11000 duplicate key error collection: _id_ dup key", 11000);
      if (!validates(c, doc)) return failure("Document failed validation", 121);
      docs.push(doc);
      return null;
    }
    const at = docs.findIndex((d) => sameValue(d._id ?? MISSING, idOf(ch.key)));
    if (at < 0)
      return failure(
        "no document matched: it was changed or deleted since it was loaded; reload and try again",
        0,
      );
    if (ch.op === "delete") {
      docs.splice(at, 1);
      return null;
    }
    const next: Doc = { ...docs[at]! };
    for (const [k, v] of Object.entries(ch.changes))
      if (v.t === "other" && v.v.ty === "missing") delete next[k];
      else next[k] = v;
    withDates(next);
    if (!validates(c, next)) return failure("Document failed validation", 121);
    docs[at] = next;
    return null;
  };

  return {
    caps: mongoCaps(o.transactions, o.database, o.confirm),
    listDatabases: () =>
      [...databases.keys()].sort().map((name) => ({
        name,
        owner: null,
        encoding: null,
        isCurrent: name === o.database,
      })),
    listSchemas: () =>
      o.database
        ? [{ name: o.database, isDefault: true }]
        : [...databases.keys()]
            .filter((d) => !SYSTEM_DATABASES.includes(d))
            .sort()
            .map((name) => ({ name, isDefault: false })),
    listTables: (schema): TableSummary[] => {
      const db =
        schema || o.database || mockError("INVALID_INPUT", "MongoDB: choose a database first");
      return [...dbOf(db).values()]
        .sort((a, b) => (a.name < b.name ? -1 : 1))
        .map((c) => ({
          name: c.name,
          schema: db,
          kind: tableKind(c),
          rowEstimate: null,
          sizeBytes: null,
          comment: comment(c),
        }));
    },
    tableMeta,
    browse: (req): BrowsePage => {
      const meta = tableMeta(req.table);
      const columns = req.columns
        ? meta.columns.filter((c) => c.name === "_id" || req.columns!.includes(c.name))
        : meta.columns;
      const all = sortDocs(
        docsOf(req.table).filter((d) => filterMatches(d, req.filter)),
        req.sort,
      );
      const page = all.slice(req.offset, req.offset + req.limit);
      const find: [string, string][] = [["find", JSON.stringify(req.table.name)]];
      if (req.sort.length > 0)
        find.push([
          "sort",
          docJson([
            ...req.sort.map((s): [string, string] => [s.column, s.dir === "desc" ? "-1" : "1"]),
            ["_id", "1"],
          ]),
        ]);
      if (req.offset > 0) find.push(["skip", String(req.offset)]);
      find.push(["limit", String(req.limit)]);
      return {
        columns,
        rows: page.map((d) => columns.map((c) => d[c.name] ?? MISSING)),
        pkColumns: meta.primaryKey,
        identity: meta.primaryKey.length > 0 ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: commandText(dbName(req.table), docJson(find)),
        durationMs: 2,
        nextCursor: null,
      };
    },
    count: (table, filter) => {
      const docs = docsOf(table);
      // Without a filter, the collection's own count (not exact); else the documents counted.
      if (filter === null || filter.items.length === 0) return { count: docs.length, exact: false };
      return { count: docs.filter((d) => filterMatches(d, filter)).length, exact: true };
    },
    fetchCell: (table, key, column) => {
      const doc = docsOf(table).find((d) => sameValue(d._id ?? MISSING, idOf(key)));
      if (!doc) mockError("NOT_FOUND", "not found: no document with this _id");
      return doc[column] ?? MISSING;
    },
    preview,
    apply: (req): ApplyResult => {
      const sql = preview(req);
      if (state.readOnly)
        return {
          success: false,
          results: [],
          sql,
          error: failure(READ_ONLY_MESSAGE),
          durationMs: 0,
        };
      const c = coll(req.table);
      // A transaction works on a copy, kept only when every change applied.
      const docs = o.transactions ? c.docs.slice() : c.docs;
      let failed: { at: number; error: ErrorPayload } | null = null;
      for (let i = 0; i < req.changes.length && failed === null; i++) {
        const error = applyOne(c, docs, req.changes[i]!);
        if (error) failed = { at: i, error: { ...error, statementIndex: i } };
      }
      if (failed === null && o.transactions) c.docs = docs;
      const notSent = failure("not applied: an earlier change failed");
      const results: RowResult[] = req.changes.map((ch, i) => {
        const error =
          failed && i === failed.at
            ? failed.error
            : failed && i > failed.at && !o.transactions
              ? notSent
              : null;
        const applied = failed === null || (!o.transactions && i < failed.at);
        return {
          rowId: ch.rowId,
          op: commandOf(c, ch).op,
          affected: applied ? 1 : 0,
          returned: null,
          error,
        };
      });
      return {
        success: failed === null,
        results,
        sql,
        error: failed?.error ?? null,
        durationMs: 3,
      };
    },
    run: (statement, emit) => runConsole(state, statement, emit),
    call: (method, args) => call(state, method, args),
    listProcesses: () => [
      ...processes,
      {
        id: "4013",
        user: profile.user ? `${profile.user}@admin` : null,
        database: "admin",
        application: "Tabler",
        client: "127.0.0.1:50422",
        state: "active",
        command: "command",
        startedAt: new Date().toISOString(),
        durationMs: 0,
        waitEvent: null,
        query: '{"aggregate": 1, "pipeline": [{"$currentOp": {}}]}',
        current: true,
        own: true,
      },
    ],
    killProcesses: (ids, scope): KillOutcome[] =>
      ids.map((id) => {
        if (state.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
        if (scope === "connection")
          return {
            id,
            ok: false,
            message: "MongoDB stops operations, not connections: use Kill Query",
          };
        const at = processes.findIndex((p) => p.id === id);
        if (at < 0) return { id, ok: false, message: "operation not found" };
        processes.splice(at, 1);
        return { id, ok: true, message: null };
      }),
  };

  // The calls (`calls.rs`): reads, `preview` of a write, and the writes on a writable session.
  function call(s: MongoState, method: string, raw: unknown): unknown {
    const args = (raw ?? {}) as Record<string, unknown>;
    const arg = (k: string): string => {
      const v = args[k];
      return typeof v === "string" && v.trim() !== ""
        ? v.trim()
        : mockError("INVALID_INPUT", `MongoDB: “${k}” is required`);
    };
    const command = (m: string, a: Record<string, unknown>): { db: string; body: string } => {
      const at = a;
      const get = (k: string) => {
        const v = at[k];
        return typeof v === "string" && v.trim() !== ""
          ? v.trim()
          : mockError("INVALID_INPUT", `MongoDB: “${k}” is required`);
      };
      // An optional document argument (`Args::document`): empty when absent, refused with the
      // line and column (in characters) when it does not parse.
      const document = (k: string): [string, Lit][] => {
        const v = at[k];
        if (typeof v !== "string" || v.trim() === "") return [];
        const text = v.trim();
        const refuse = (message: string, offset: number): never => {
          const before = Array.from(text.slice(0, offset));
          const line = before.filter((c) => c === "\n").length + 1;
          const column = before.length - before.lastIndexOf("\n");
          return mockError(
            "INVALID_INPUT",
            `MongoDB ${k}: ${message} (line ${line}, column ${column})`,
          );
        };
        const lx = new Lexer(text);
        lx.skipTrivia();
        const start = lx.pos;
        let lit: Lit;
        try {
          lit = lx.value();
          if (lx.peek() !== undefined) throw new ParseError("expected the end of the value");
        } catch (e) {
          return refuse(e instanceof Error ? e.message : "?", lx.pos);
        }
        return lit.k === "doc" ? lit.entries : refuse("expected a document `{ … }`", start);
      };
      const oneOf = (k: string, allowed: string[]): string | null => {
        const v = at[k];
        if (typeof v !== "string" || v.trim() === "") return null;
        return allowed.includes(v.trim())
          ? v.trim()
          : mockError("INVALID_INPUT", `MongoDB ${k}: one of ${allowed.join(", ")}`);
      };
      const entriesText = (entries: [string, Lit][]) =>
        docJson(entries.map(([k, x]) => [k, litText(x)]));
      const named = (k: string, body: string) => `{${JSON.stringify(k)}: ${body}}`;
      switch (m) {
        case "dropCollection":
          return { db: get("database"), body: named("drop", JSON.stringify(get("collection"))) };
        case "truncateCollection":
          return {
            db: get("database"),
            body: `{"delete": ${JSON.stringify(get("collection"))}, "deletes": [{"q": {}, "limit": 0}]}`,
          };
        case "renameCollection":
          return {
            db: "admin",
            body: `{"renameCollection": ${JSON.stringify(`${get("database")}.${get("collection")}`)}, "to": ${JSON.stringify(`${get("database")}.${get("to")}`)}, "dropTarget": ${at.dropTarget === true}}`,
          };
        case "createIndex": {
          const keys = document("keys");
          if (keys.length === 0)
            mockError("INVALID_INPUT", "MongoDB index: its keys are required ({field: 1})");
          const db = get("database");
          const options = document("options");
          const spec: [string, Lit][] = [["key", { k: "doc", entries: keys }]];
          if (!options.some(([k]) => k === "name"))
            spec.push(["name", { k: "str", v: indexName(keys) }]);
          return {
            db,
            body: `{"createIndexes": ${JSON.stringify(get("collection"))}, "indexes": [${entriesText([...spec, ...options])}]}`,
          };
        }
        case "dropIndex":
          return {
            db: get("database"),
            body: `{"dropIndexes": ${JSON.stringify(get("collection"))}, "index": ${JSON.stringify(get("name"))}}`,
          };
        case "setValidator": {
          const db = get("database");
          const coll = get("collection");
          const body: [string, string][] = [
            ["collMod", JSON.stringify(coll)],
            ["validator", entriesText(document("validator"))],
          ];
          const level = oneOf("level", ["strict", "moderate", "off"]);
          const action = oneOf("action", ["error", "warn"]);
          if (level !== null) body.push(["validationLevel", JSON.stringify(level)]);
          if (action !== null) body.push(["validationAction", JSON.stringify(action)]);
          return { db, body: docJson(body) };
        }
        case "createCollection": {
          const db = get("database");
          const name = get("name");
          // `mongo::create_collection`: the options extend the command.
          const body = entriesText([["create", { k: "str", v: name }], ...document("options")]);
          return { db, body };
        }
        case "createView": {
          const stages = stagesOf(get("pipeline"));
          if (stages === null)
            mockError("INVALID_INPUT", "MongoDB pipeline: an array of stages ([{$match: …}, …])");
          return {
            db: get("database"),
            body: `{"create": ${JSON.stringify(get("name"))}, "viewOn": ${JSON.stringify(get("viewOn"))}, "pipeline": ${litText({ k: "arr", items: stages })}}`,
          };
        }
        case "dropDatabase":
          return { db: get("database"), body: '{"dropDatabase": 1}' };
        default:
          return mockError("UNSUPPORTED", `unsupported: MongoDB: unknown call “${m}”`);
      }
    };
    switch (method) {
      case "collectionStats": {
        const t = { schema: arg("database"), name: arg("collection") };
        const n = docsOf(t).length;
        return {
          ns: `${t.schema}.${t.name}`,
          count: n,
          storageStats: {
            size: n * 212,
            count: n,
            storageSize: n * 96 + 4096,
            totalIndexSize: 20_480,
          },
        };
      }
      case "databaseStats": {
        const db = dbOf(arg("database"));
        return {
          db: arg("database"),
          collections: db.size,
          objects: [...db.values()].reduce((n, c) => n + c.docs.length, 0),
          ok: 1,
        };
      }
      case "preview": {
        const inner = typeof args.method === "string" ? args.method : "";
        const c = command(inner, (args.args ?? {}) as Record<string, unknown>);
        return { database: c.db, command: commandText(c.db, c.body) };
      }
    }
    // Every other call writes (or is unknown, which the backend counts as a write).
    const c = command(method, args);
    if (s.readOnly) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
    const db = dbOf(arg("database"));
    switch (method) {
      case "dropCollection":
        if (!db.delete(arg("collection"))) mockError("DB_ERROR", "ns not found");
        break;
      case "truncateCollection":
        coll({ schema: arg("database"), name: arg("collection") }).docs.length = 0;
        break;
      case "renameCollection": {
        const from = coll({ schema: arg("database"), name: arg("collection") });
        if (db.has(arg("to"))) mockError("DB_ERROR", "target namespace exists");
        db.delete(from.name);
        db.set(arg("to"), { ...from, name: arg("to") });
        break;
      }
      case "createIndex": {
        const from = coll({ schema: arg("database"), name: arg("collection") });
        // Both parsed already: `command` refused them otherwise.
        const keys = parseLiteral(arg("keys"));
        const options = typeof args.options === "string" ? args.options.trim() : "";
        const opts = options === "" ? null : parseLiteral(options);
        if (keys.k !== "doc") break;
        const given = opts?.k === "doc" ? opts.entries.find(([k]) => k === "name")?.[1] : undefined;
        from.indexes.push({
          name: given?.k === "str" ? given.v : indexName(keys.entries),
          keys: keys.entries.map(([k, v]): [string, number | string] => [
            k,
            v.k === "num" || v.k === "str" ? v.v : litText(v),
          ]),
        });
        break;
      }
      case "dropIndex": {
        const from = coll({ schema: arg("database"), name: arg("collection") });
        const at = from.indexes.findIndex((i) => i.name === arg("name"));
        if (at < 0 || arg("name") === "_id_")
          mockError("DB_ERROR", `index not found with name [${arg("name")}]`);
        from.indexes.splice(at, 1);
        break;
      }
      case "setValidator": {
        const from = coll({ schema: arg("database"), name: arg("collection") });
        const v = typeof args.validator === "string" ? args.validator.trim() : "";
        from.validator = v === "" || v === "{}" ? undefined : v;
        break;
      }
      case "createCollection":
        db.set(arg("name"), {
          name: arg("name"),
          kind: "collection",
          docs: [],
          indexes: [{ name: "_id_", keys: [["_id", 1]] }],
        });
        break;
      case "createView":
        if (db.has(arg("name"))) mockError("DB_ERROR", "namespace exists");
        db.set(arg("name"), {
          name: arg("name"),
          kind: "view",
          docs: [],
          indexes: [],
          viewOn: arg("viewOn"),
          pipeline: arg("pipeline"),
        });
        break;
      case "dropDatabase":
        databases.delete(arg("database"));
        break;
    }
    return { command: commandText(c.db, c.body), reply: { ok: 1 } };
  }
}
