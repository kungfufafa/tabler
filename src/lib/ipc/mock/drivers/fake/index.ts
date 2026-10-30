import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { Driver } from "../../../bindings/Driver";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { ResultColumn } from "../../../bindings/ResultColumn";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { SqlDialectId } from "../../../bindings/SqlDialectId";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import type { ValueKind } from "../../../bindings/ValueKind";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockSession } from "../types";

/**
 * A fake document store for the foundation's tests (never registered by the mock's glob; a test
 * seeds it with `registerMockDriver`): collections of documents keyed by `_id`, where a document
 * may lack a field (`Other{missing}`), browsed by cursor, counted at a cost, edited one change at
 * a time (`TxMode::Sequential`: a change holding the text "__fail__" fails, the others apply),
 * with a console of `find <c>`, `count <c>`, `insert <c> {json}` and `delete <c> <id>` lines and
 * the calls `stats` (reads) and `compact` (writes). It stands in for whichever lane driver a
 * test names.
 *
 * With a `dialect` it is the second fake session, one of an extension SQL dialect: the same
 * collections as tables, paged by offset, filtered and sorted, edited in a batch that may apply
 * in part (`TxMode::Batch`), `SELECT … FROM <table>` and `DELETE FROM <table> WHERE _id = '<id>'`
 * in its editor, and Show DDL.
 */

type Doc = Record<string, Value>;

/** A missing field: the document has none (not NULL). */
export const MISSING: Value = { t: "other", v: { ty: "missing", text: "" } };

const text = (v: string): Value => ({ t: "text", v });
const int = (v: number): Value => ({ t: "int", v });

/** The collections of a new session: `orders` (25 documents, some without `status`) and `users`. */
function seedCollections(): Map<string, Doc[]> {
  const orders: Doc[] = Array.from({ length: 25 }, (_, i) => {
    const doc: Doc = { _id: text(`o${i + 1}`), total: int((i + 1) * 10) };
    if (i % 5 !== 4) doc.status = text(i % 2 === 0 ? "new" : "paid");
    return doc;
  });
  const users: Doc[] = [
    { _id: text("u1"), name: text("Ada"), tags: { t: "json", v: '["admin"]' } },
    { _id: text("u2"), name: text("Linus"), tags: { t: "json", v: "[]" } },
  ];
  return new Map([
    ["orders", orders],
    ["users", users],
  ]);
}

const KIND_OF: Partial<Record<Value["t"], ValueKind>> = {
  int: "int",
  float: "float",
  bool: "bool",
  text: "text",
  json: "json",
};

/** The columns a collection's documents hold, `_id` first (a sampled schema). */
function columnsOf(docs: readonly Doc[]): ColumnMeta[] {
  const kinds = new Map<string, ValueKind>([["_id", "text"]]);
  for (const doc of docs)
    for (const [name, v] of Object.entries(doc))
      if (!kinds.has(name)) kinds.set(name, KIND_OF[v.t] ?? "other");
  return [...kinds].map(([name, kind], ordinal) => ({
    name,
    ordinal,
    dataType: kind,
    kind,
    nullable: name !== "_id",
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
    extra: null,
  }));
}

const idOf = (doc: Doc): string => {
  const id = doc._id;
  return id?.t === "text" ? id.v : "";
};

function keyId(key: RowKey): string {
  if (key.kind !== "pk") mockError("UNSUPPORTED", "unsupported: documents are edited by _id");
  const id = key.value._id;
  return id?.t === "text" ? id.v : "";
}

const valueText = (v: Value): string =>
  v.t === "null" || v.t === "default" ? "null" : v.t === "other" ? v.v.text : JSON.stringify(v.v);

const failure = (message: string, statementIndex: number | null = null): ErrorPayload => ({
  code: "DB_ERROR",
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex,
  data: null,
});

/**
 * The caps of the fake: a document store's ("Collections", cursors, sequential edits), or with a
 * `dialect` a SQL session's (tables, offsets, batch edits, Show DDL).
 */
export function fakeCaps(driver: Driver, dialect: SqlDialectId | null = null): SessionCaps {
  if (dialect !== null) {
    const doc = fakeCaps(driver);
    return {
      ...doc,
      dialect,
      language: "sql",
      features: [...doc.features, "tableDdl", "exportTables"],
      namespaces: {
        ...doc.namespaces,
        labels: { database: "Database", schema: "Schema", table: "Table", tables: "Tables" },
      },
      data: {
        ...doc.data,
        paging: "offset",
        count: "exact",
        sort: true,
        filterOps: ["eq", "ne", "like", "isNull", "isNotNull"],
        rawFilter: "sqlWhere",
        transactions: "batch",
      },
    };
  }
  return {
    driver,
    flavor: null,
    dialect: null,
    language: "mongoShell",
    features: ["editor", "structureView", "editRows", "insertRows", "deleteRows"],
    namespaces: {
      databases: true,
      schemas: "single",
      defaultSchema: null,
      labels: {
        database: "Database",
        schema: "Schema",
        table: "Collection",
        tables: "Collections",
      },
    },
    data: {
      paging: "cursor",
      count: "costly",
      sort: false,
      filterOps: ["eq", "ne"],
      rawFilter: "json",
      transactions: "sequential",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: [],
      maxParams: 0,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "password" },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

function session(driver: Driver, dialect: SqlDialectId | null): MockSession {
  const collections = seedCollections();
  const docs = (table: TableRef): Doc[] =>
    collections.get(table.name) ?? mockError("NOT_FOUND", `not found: collection ${table.name}`);

  const preview = (req: ApplyRequest): string[] =>
    req.changes.map((c) => {
      const coll = `db.${req.table.name}`;
      if (c.op === "insert") return `${coll}.insertOne(${docText(c.values)})`;
      if (c.op === "delete") return `${coll}.deleteOne({_id: "${keyId(c.key)}"})`;
      return `${coll}.updateOne({_id: "${keyId(c.key)}"}, {$set: ${docText(c.changes)}})`;
    });

  return {
    caps: fakeCaps(driver, dialect),
    listDatabases: () => [{ name: "app", owner: null, encoding: null, isCurrent: true }],
    listSchemas: () => [],
    listTables: () =>
      [...collections].map(([name, rows]) => ({
        name,
        schema: null,
        kind: "table",
        rowEstimate: rows.length,
        sizeBytes: null,
        comment: null,
      })),
    tableMeta: (table): TableMeta => ({
      table,
      kind: "table",
      columns: columnsOf(docs(table)),
      primaryKey: ["_id"],
      indexes: [],
      foreignKeys: [],
      triggers: [],
      comment: null,
      rowEstimate: docs(table).length,
      ddl: null,
      checks: [],
      options: null,
    }),
    ...(dialect !== null
      ? {
          tableDdl: (table: TableRef) =>
            `CREATE TABLE ${table.name} (${columnsOf(docs(table))
              .map((c) => `${c.name} ${c.dataType}`)
              .join(", ")})`,
        }
      : {}),
    browse: (req): BrowsePage => {
      const all = docs(req.table);
      const columns = columnsOf(all);
      // By cursor as a document store, by offset as a SQL session.
      const from = dialect !== null ? req.offset : req.cursor ? Number(req.cursor) : 0;
      const page = all.slice(from, from + req.limit);
      const next = from + page.length;
      return {
        columns,
        rows: page.map((doc) => columns.map((c) => doc[c.name] ?? MISSING)),
        pkColumns: ["_id"],
        identity: "pk",
        rowIds: null,
        offset: dialect !== null ? from : 0,
        limit: req.limit,
        sql:
          dialect !== null
            ? `SELECT * FROM ${req.table.name} LIMIT ${req.limit} OFFSET ${from}`
            : `db.${req.table.name}.find({}).limit(${req.limit})`,
        durationMs: 1,
        nextCursor: dialect === null && next < all.length ? String(next) : null,
      };
    },
    count: (table) => ({ count: docs(table).length, exact: true }),
    fetchCell: (table, key, column) => {
      const doc = docs(table).find((d) => idOf(d) === keyId(key));
      if (!doc) mockError("NOT_FOUND", "not found: the document is gone");
      return doc[column] ?? MISSING;
    },
    preview,
    apply: (req): ApplyResult => {
      const all = docs(req.table);
      // One change at a time: a failed change leaves the ones before it applied.
      const results: RowResult[] = req.changes.map((c) => {
        const values = c.op === "insert" ? c.values : c.op === "update" ? c.changes : {};
        const result: RowResult = {
          rowId: c.rowId,
          op: c.op,
          affected: 1,
          returned: null,
          error: null,
        };
        if (Object.values(values).some((v) => v.t === "text" && v.v === "__fail__"))
          return { ...result, affected: 0, error: failure("document failed validation") };
        if (c.op === "insert") all.push({ ...values });
        else {
          const at = all.findIndex((d) => idOf(d) === keyId(c.key));
          if (at < 0) return { ...result, affected: 0, error: failure("no document matched") };
          if (c.op === "delete") all.splice(at, 1);
          else all[at] = { ...all[at], ...values };
        }
        return result;
      });
      const error = results.find((r) => r.error)?.error ?? null;
      return { success: error === null, results, sql: preview(req), error, durationMs: 1 };
    },
    run: (statement, emit) => run(collections, sqlCommand(statement, dialect), emit),
    call: (method, args) => {
      const name = (args as { collection?: string } | null)?.collection ?? "orders";
      if (method === "stats")
        return { collection: name, count: docs({ schema: null, name }).length };
      if (method === "compact") return { ok: 1 };
      return mockError("UNSUPPORTED", `unsupported: unknown call “${method}”`);
    },
  };
}

/** The console line a statement of the SQL fake stands for (the document fake's as it is). */
function sqlCommand(statement: string, dialect: SqlDialectId | null): string {
  if (dialect === null) return statement;
  const select = /^\s*select\b[\s\S]*?\bfrom\s+(\w+)/i.exec(statement);
  if (select) return `find ${select[1]}`;
  const del = /^\s*delete\s+from\s+(\w+)\s+where\s+_id\s*=\s*'([^']*)'/i.exec(statement);
  if (del) return `delete ${del[1]} ${del[2]}`;
  return statement;
}

const docText = (values: Record<string, Value>): string =>
  `{${Object.entries(values)
    .map(([k, v]) => `${k}: ${valueText(v)}`)
    .join(", ")}}`;

/** Runs one console line (see the module comment), streaming its events like `run_sql`. */
function run(
  collections: Map<string, Doc[]>,
  statement: string,
  emit: (event: QueryEvent) => void,
): void {
  const span = { start: 0, end: statement.length, line: 1 };
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
  emit({ event: "started", data: { index: 0, sql: statement, span } });
  const [verb, name, ...rest] = statement.trim().split(/\s+/);
  const docs = name ? collections.get(name) : undefined;
  const fail = (message: string) => {
    const error = failure(message, 0);
    emit({ event: "failed", data: { index: 0, error } });
  };
  if (!docs) return fail(`unknown collection ${name ?? ""}`.trim());
  if (verb === "find") {
    const cols = columnsOf(docs);
    const columns: ResultColumn[] = cols.map((c) => ({
      name: c.name,
      dataType: c.dataType,
      kind: c.kind,
      nullable: c.nullable,
      table: name ?? null,
    }));
    emit({ event: "columns", data: { index: 0, set: 0, columns } });
    const rows = docs.map((doc) => cols.map((c) => doc[c.name] ?? MISSING));
    emit({ event: "rows", data: { index: 0, set: 0, rows } });
    const result: StatementResult = {
      ...base,
      kind: "rows",
      columns,
      resultSets: 1,
      rowCount: rows.length,
    };
    emit({ event: "finished", data: { index: 0, result } });
    return;
  }
  if (verb === "count") {
    const columns: ResultColumn[] = [
      { name: "count", dataType: "int", kind: "int", nullable: false, table: null },
    ];
    emit({ event: "columns", data: { index: 0, set: 0, columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows: [[int(docs.length)]] } });
    const result: StatementResult = { ...base, kind: "rows", columns, resultSets: 1, rowCount: 1 };
    emit({ event: "finished", data: { index: 0, result } });
    return;
  }
  if (verb === "insert" || verb === "delete") {
    let affected = 0;
    if (verb === "insert") {
      try {
        const parsed = JSON.parse(rest.join(" ")) as Record<string, unknown>;
        const doc: Doc = {};
        for (const [k, v] of Object.entries(parsed))
          doc[k] = typeof v === "number" ? int(v) : text(String(v));
        docs.push(doc);
        affected = 1;
      } catch {
        return fail("insert takes a JSON document");
      }
    } else {
      const at = docs.findIndex((d) => idOf(d) === rest[0]);
      if (at >= 0) {
        docs.splice(at, 1);
        affected = 1;
      }
    }
    const result: StatementResult = { ...base, kind: "affected", affected };
    emit({ event: "finished", data: { index: 0, result } });
    return;
  }
  fail(`unknown command ${verb ?? ""}`.trim());
}

/**
 * The fake as the mock driver `driver` (a lane's driver id: the tests choose one); with a
 * `dialect`, the SQL session of that extension dialect.
 */
export function fakeMockDriver(
  driver: Driver,
  { dialect = null }: { dialect?: SqlDialectId | null } = {},
): MockDriver {
  return {
    driver,
    serverVersion: () => "FakeStore 1.0",
    seedProfiles: () => [
      profile({
        id: `p-fake-${driver}`,
        name: "Fake Store",
        driver,
        host: "127.0.0.1",
        port: 27017,
        user: null,
        database: "app",
        sortOrder: 100,
      }),
    ],
    connect: () => session(driver, dialect),
    util: (method) =>
      method === "listDatabases"
        ? ["app"]
        : mockError("UNSUPPORTED", `unsupported: unknown call “${method}”`),
  };
}
