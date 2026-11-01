import { classifyDialect } from "@/features/drivers/portable/classify";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SafeConfirm } from "../../../bindings/SafeConfirm";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { matchesGroup } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import {
  columnsOf,
  createQuery,
  type Database,
  display,
  hasRows,
  KEY_ROLES,
  metaOf,
  NULL,
  primaryKeyOf,
  type Table,
  tableKind,
} from "./data";
import { conditionOf, defaultOf, type Editor, LEX, runStatement, type SqlContext } from "./sql";
import { backticked, coerce, dbPayload, literal, payload, ServerFailure } from "./values";

/**
 * A session of the mock's ClickHouse, mirroring `drivers/clickhouse/session.rs`: the same caps
 * (`tests/fixtures/clickhouse/caps.json` holds the backend's), the server as the one database
 * and its databases as the schemas, offset pages, `total_rows` as the estimate until a count is
 * asked for, edits by primary key only applied one by one, each update and delete under a count
 * of the rows its key matches (ClickHouse keys are not unique), the editor in an HTTP session of
 * its own that keeps its `SET`s, Running Queries with Kill Query only, Show DDL and session
 * details. A read-only session refuses what the classifier calls a write (or does not know)
 * before anything runs, on every path, as the backend does before `readonly=2` would.
 */

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `sql::writes`: what the ClickHouse classifier calls a write, or does not know. */
export const writes = (sql: string) => classifyDialect(LEX, sql) === "write";

/** `session::FEATURES` on top of the base's Editor and StructureView, in `Feature`'s order. */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "explain",
  "cancelOnServer",
  "editRows",
  "insertRows",
  "deleteRows",
  "serverExpressions",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "processList",
  "killProcess",
  "sessionDetails",
  "mcp",
];

/** `ClickhouseSession::capabilities`. */
export function clickhouseCaps(
  defaultSchema: string,
  confirm: SafeConfirm,
  readOnly: boolean,
): SessionCaps {
  return {
    driver: "clickhouse",
    flavor: null,
    dialect: "clickhouse",
    language: "sql",
    features: FEATURES,
    namespaces: {
      databases: false,
      schemas: "schemas",
      defaultSchema,
      labels: { database: "Database", schema: "Database", table: "Table", tables: "Tables" },
    },
    data: {
      paging: "offset",
      count: "estimate",
      sort: true,
      filterOps: [
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
      ],
      rawFilter: "sqlWhere",
      transactions: "sequential",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: ["now", "currentTimestamp", "currentDate", "default"],
      maxParams: 1000,
    },
    // `hardened`: `readonly=2` goes with every request of a read-only session.
    safety: { readOnly: "perRequest", hardened: readOnly, confirm },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** What a session was opened with. */
export interface ClickhouseOptions {
  databases: Map<string, Database>;
  /** The profile's host: the sidebar's one database. */
  host: string;
  /** The database the requests start in. */
  database: string;
  user: string;
  version: string;
  readOnly: boolean;
  confirm: SafeConfirm;
  tls: boolean;
}

// --- the server's other queries (Running Queries) ------------------------------------------------

interface Process {
  queryId: string;
  user: string;
  address: string;
  elapsedSecs: number;
  database: string;
  query: string;
}

const OTHERS: Process[] = [];
function seedOthers() {
  if (OTHERS.length > 0) return;
  OTHERS.push(
    {
      queryId: "5f0c2a8e-31d4-4b1e-9d6a-7f3a2c1b9e01",
      user: "etl",
      address: "::ffff:10.0.4.30",
      elapsedSecs: 84.2,
      database: "analytics",
      query: "INSERT INTO analytics.page_views SELECT * FROM s3('https://bucket/views/*.parquet')",
    },
    {
      queryId: "grafana-7781",
      user: "grafana",
      address: "::ffff:10.0.4.17",
      elapsedSecs: 2.4,
      database: "shop",
      query:
        "SELECT toStartOfHour(created_at) AS t, count() FROM shop.orders GROUP BY t ORDER BY t",
    },
  );
}

/** Tests: the other queries as seeded again. */
export function resetProcesses(): void {
  OTHERS.splice(0);
}

/** `KILL QUERY … ASYNC`: no row and no error for an id that runs nowhere. */
function killQuery(id: string): boolean {
  seedOthers();
  const at = OTHERS.findIndex((p) => p.queryId === id);
  if (at < 0) return false;
  OTHERS.splice(at, 1);
  return true;
}

let queries = 0;
/** `client::new_query_id`: Tabler's own requests carry ids of their own. */
const newQueryId = () => `tabler-${(++queries).toString(16).padStart(8, "0")}-mock`;

// --- the grid's statements (`dialect::clickhouse`) -----------------------------------------------

const unsupported = (message: string): never => mockError("UNSUPPORTED", `unsupported: ${message}`);

/** `commands::mutate::change_features` against the caps (`SessionCaps::require`). */
function requireFeatures(caps: SessionCaps, changes: StagedChange[]): void {
  const keyless = (k: RowKey) => k.kind !== "pk";
  for (const c of changes) {
    const needs: [SessionCaps["features"][number], string][] = [];
    if (c.op === "update") needs.push(["editRows", "editing rows"]);
    if (c.op === "insert") needs.push(["insertRows", "inserting rows"]);
    if (c.op === "delete") needs.push(["deleteRows", "deleting rows"]);
    if (c.op !== "insert" && keyless(c.key))
      needs.push(["editWithoutKey", "editing rows without a primary key"]);
    if (c.op !== "delete" && Object.keys(c.exprs ?? {}).length > 0)
      needs.push(["serverExpressions", "DEFAULT and server expressions"]);
    for (const [f, label] of needs)
      if (!caps.features.includes(f)) unsupported(`ClickHouse: ${label} is not available`);
  }
}

/** `sqlgen::mutate::ordered`: deletes, then updates, then inserts. */
const ordered = (changes: StagedChange[]): StagedChange[] => [
  ...changes.filter((c) => c.op === "delete"),
  ...changes.filter((c) => c.op === "update"),
  ...changes.filter((c) => c.op === "insert"),
];

/** The value of a server expression for a column (`dialect::clickhouse::server_expr`). */
function clockOf(expr: string, col: ColumnMeta): { sql: string; value: Value } {
  const now = new Date().toISOString().replace("T", " ").slice(0, 19);
  const fsp = /^(now|currentTimestamp)\((\d+)\)$/.exec(expr);
  if (expr === "now" || expr === "currentTimestamp" || fsp)
    return {
      sql: fsp ? `now64(${fsp[2]})` : "now()",
      value: col.kind === "date" ? { t: "date", v: now.slice(0, 10) } : { t: "timestamp", v: now },
    };
  if (expr === "currentDate") return { sql: "today()", value: { t: "date", v: now.slice(0, 10) } };
  return unsupported(`ClickHouse SQL has no ${expr} for "${col.name}"`);
}

const inKey = (c: ColumnMeta) => c.isPk || KEY_ROLES.some((r) => c.extra?.includes(r));

/** A change's statement, checked as the backend's builders check it. */
interface Planned {
  change: StagedChange;
  sql: string;
  /** The cells an update or insert writes, by column. */
  cells: Map<string, Value>;
}

function plan(t: Table, change: StagedChange): Planned {
  const columns = columnsOf(t);
  const col = (name: string) =>
    columns.find((c) => c.name === name) ?? mockError("INVALID_INPUT", `unknown column "${name}"`);
  const name = `${backticked(t.database)}.${backticked(t.name)}`;
  const condition = (key: RowKey): string => {
    if (key.kind !== "pk")
      return unsupported("ClickHouse SQL: a row of a table without a primary key cannot be edited");
    const parts = Object.entries(key.value).sort(([a], [b]) => (a < b ? -1 : 1));
    if (parts.length === 0) mockError("INVALID_INPUT", "row key is empty");
    return parts
      .map(([k, v]) => {
        col(k);
        return v.t === "null" ? `${backticked(k)} IS NULL` : `${backticked(k)} = ${literal(v)}`;
      })
      .join(" AND ");
  };
  const cells = new Map<string, Value>();
  if (change.op === "delete")
    return { change, sql: `DELETE FROM ${name} WHERE ${condition(change.key)}`, cells };
  const given: [string, Value | { expr: string }][] = [
    ...Object.entries(change.op === "update" ? change.changes : change.values),
    ...Object.entries(change.exprs ?? {}).map(
      ([k, e]) => [k, { expr: e }] as [string, { expr: string }],
    ),
  ];
  given.sort(([a], [b]) => (a < b ? -1 : 1));
  const parts: [string, string][] = [];
  for (const [k, cell] of given) {
    const c = col(k);
    const isDefault = "expr" in cell ? cell.expr === "default" : cell.t === "default";
    if (change.op === "update") {
      if (inKey(c))
        unsupported(
          `"${k}" is part of the table's key (ORDER BY, PARTITION BY, PRIMARY KEY or SAMPLE BY), which ClickHouse cannot update`,
        );
      if (c.isGenerated)
        unsupported(`"${k}" is computed by the server (MATERIALIZED or ALIAS) and cannot be set`);
      if (isDefault)
        unsupported(`ClickHouse SQL cannot set "${k}" to DEFAULT; enter the value explicitly`);
    } else {
      if (isDefault) continue;
      if (c.isGenerated)
        unsupported(`"${k}" is computed by the server (MATERIALIZED or ALIAS) and cannot be set`);
    }
    if ("expr" in cell) {
      const clock = clockOf(cell.expr, c);
      parts.push([k, clock.sql]);
      cells.set(k, clock.value);
    } else {
      parts.push([k, literal(cell)]);
      cells.set(k, cell);
    }
  }
  if (change.op === "update") {
    if (parts.length === 0) mockError("INVALID_INPUT", "update has no changed columns");
    return {
      change,
      sql: `ALTER TABLE ${name} UPDATE ${parts.map(([k, v]) => `${backticked(k)} = ${v}`).join(", ")} WHERE ${condition(change.key)}`,
      cells,
    };
  }
  if (parts.length === 0)
    unsupported("ClickHouse SQL cannot insert a row of defaults only; enter at least one value");
  return {
    change,
    sql: `INSERT INTO ${name} (${parts.map(([k]) => backticked(k)).join(", ")}) SELECT ${parts.map(([, v]) => v).join(", ")}`,
    cells,
  };
}

/** The rows a key matches (ClickHouse keys are not unique). */
function matching(t: Table, key: RowKey): number[] {
  if (key.kind !== "pk") return [];
  const at = Object.entries(key.value).map(
    ([k, v]) => [t.cols.findIndex((c) => c.name === k), v] as const,
  );
  return t.data.flatMap((row, i) =>
    at.every(([c, v]) => c >= 0 && display(row[c] ?? NULL) === display(v)) ? [i] : [],
  );
}

/** A filter's text in the page's SQL (structured filters shortened). */
function whereText(group: FilterGroup): string {
  const part = (n: FilterNode): string =>
    n.kind === "raw"
      ? `(${n.sql.trim()})`
      : n.kind === "group"
        ? `(${whereText(n)})`
        : n.kind === "anyColumn"
          ? "(any column …)"
          : `${backticked(n.column)} ${n.op} ${n.values.map(literal).join(", ")}`;
  return group.items.map(part).join(group.conjunction === "and" ? " AND " : " OR ");
}

/** Every raw filter of a group, nested groups included. */
function rawFilters(group: FilterGroup): string[] {
  return group.items.flatMap((n) =>
    n.kind === "raw" ? [n.sql] : n.kind === "group" ? rawFilters(n) : [],
  );
}

// --- the session -------------------------------------------------------------------------------

/** One session to the mock's server. */
export function clickhouseSession(o: ClickhouseOptions): MockSession {
  const caps = clickhouseCaps(o.database, o.confirm, o.readOnly);
  const editor: Editor = { settings: new Map() };
  const ctx: SqlContext = {
    databases: o.databases,
    current: o.database,
    user: o.user,
    version: o.version,
    editor,
    kill: killQuery,
  };

  const tableOf = (ref: TableRef): Table => {
    const db = ref.schema ?? o.database;
    return (
      o.databases.get(db)?.tables.find((t) => t.name === ref.name) ??
      mockError("NOT_FOUND", `not found: table ${ref.name}`)
    );
  };

  /** `refuse_write`: a read-only session refuses before anything is sent. */
  const refuseWrite = (sql: string) => {
    if (o.readOnly && writes(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };

  /** A server error of a read as the backend reports it (`Db`, its text with its name). */
  const serverRead = <T>(work: () => T): T => {
    try {
      return work();
    } catch (e) {
      if (e instanceof ServerFailure) return mockError("DB_ERROR", e.message);
      throw e;
    }
  };

  /** The rows of a table a filter keeps, raw conditions read as the server reads them. */
  const filtered = (t: Table, filter: FilterGroup | null): Value[][] => {
    const name = `${backticked(t.database)}.${backticked(t.name)}`;
    if (!filter) return t.data;
    // `read`'s `refuse_write` of the whole SELECT: only a raw condition can make it a write.
    const texts = rawFilters(filter).filter((sql) => sql.trim() !== "");
    if (texts.length > 0)
      refuseWrite(`SELECT count() FROM ${name} WHERE ${texts.map((r) => `(${r})`).join(" AND ")}`);
    const columns = columnsOf(t);
    const raws = new Map(texts.map((sql) => [sql, conditionOf(sql, t, ctx)]));
    const node = (n: FilterNode, row: Value[]): boolean =>
      n.kind === "raw"
        ? n.sql.trim() === "" || raws.get(n.sql)!(row)
        : n.kind === "group"
          ? group(n, row)
          : matchesGroup({ conjunction: "and", items: [n] }, row, columns);
    const group = (g: FilterGroup, row: Value[]): boolean =>
      g.items.length === 0 ||
      (g.conjunction === "and"
        ? g.items.every((n) => node(n, row))
        : g.items.some((n) => node(n, row)));
    return t.data.filter((row) => group(filter, row));
  };

  const previewChanges = (req: ApplyRequest): Planned[] => {
    // `plan_rows`: a read-only profile is refused before anything is read.
    if (o.readOnly) unsupported("this connection is read-only");
    requireFeatures(caps, req.changes);
    const t = tableOf(req.table);
    return ordered(req.changes).map((c) => plan(t, c));
  };

  return {
    caps,
    listDatabases: () => [{ name: o.host, owner: null, encoding: null, isCurrent: true }],
    listSchemas: () =>
      [...o.databases.keys()].sort().map((name) => ({ name, isDefault: name === o.database })),
    listTables: (schema) =>
      [...(o.databases.get(schema)?.tables ?? [])]
        .sort((a, b) => (a.name < b.name ? -1 : 1))
        .map((t) => ({
          name: t.name,
          schema,
          kind: tableKind(t),
          rowEstimate: hasRows(t) ? t.data.length : null,
          sizeBytes: hasRows(t) ? 4096 + t.data.length * 61 : null,
          comment: t.comment === "" ? null : t.comment,
        })),
    tableMeta: (ref) => metaOf(tableOf(ref)),
    tableDdl: (ref) => createQuery(tableOf(ref)),
    browse: (req): BrowsePage => {
      const t = tableOf(req.table);
      const columns = columnsOf(t);
      let rows = serverRead(() => filtered(t, req.filter));
      const pk = primaryKeyOf(t);
      const order = req.sort.length
        ? req.sort
        : pk.map((column) => ({ column, dir: "asc" as const }));
      const at = order.map((s) => ({ i: columns.findIndex((c) => c.name === s.column), d: s.dir }));
      rows = [...rows].sort((a, b) => {
        for (const { i, d } of at) {
          const x = display(a[i] ?? NULL);
          const y = display(b[i] ?? NULL);
          const n = Number(x) - Number(y);
          const c = Number.isNaN(n) ? (x < y ? -1 : x > y ? 1 : 0) : Math.sign(n);
          if (c !== 0) return d === "desc" ? -c : c;
        }
        return 0;
      });
      const page = rows.slice(req.offset, req.offset + req.limit);
      const picked = req.columns ? columns.filter((c) => req.columns!.includes(c.name)) : columns;
      const idx = picked.map((c) => columns.indexOf(c));
      const where =
        req.filter && req.filter.items.length > 0 ? ` WHERE ${whereText(req.filter)}` : "";
      const orderBy = order.length
        ? ` ORDER BY ${order.map((s) => `${backticked(s.column)} ${s.dir.toUpperCase()}`).join(", ")}`
        : "";
      return {
        columns: picked,
        rows: page.map((row) => idx.map((i) => row[i] ?? NULL)),
        pkColumns: pk,
        // Without a primary key a row cannot be edited (no `EditWithoutKey`).
        identity: pk.length > 0 ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${picked.map((c) => backticked(c.name)).join(", ")} FROM ${backticked(t.database)}.${backticked(t.name)}${where}${orderBy} LIMIT ${req.limit} OFFSET ${req.offset}`,
        durationMs: 9,
      };
    },
    count: (ref, filter) => {
      const t = tableOf(ref);
      return { count: serverRead(() => filtered(t, filter)).length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = tableOf(ref);
      const rows = matching(t, key);
      const col = t.cols.findIndex((c) => c.name === column);
      if (rows.length === 0 || col < 0) mockError("NOT_FOUND", "not found: the row is gone");
      return t.data[rows[0]!]![col] ?? NULL;
    },
    preview: (req) => previewChanges(req).map((p) => p.sql),
    apply: (req): ApplyResult => {
      const planned = previewChanges(req);
      const t = tableOf(req.table);
      const sql = planned.map((p) => p.sql);
      const started = performance.now();
      const ms = () => Math.max(1, Math.round(performance.now() - started));
      const fail = (i: number, error: ErrorPayload): ApplyResult => {
        // `partial_results`: those before stay applied, the failed one says why, the rest were
        // not sent.
        const notSent = payload("INVALID_INPUT", "not applied: an earlier change failed");
        const results: RowResult[] = planned.map((q, j) => ({
          rowId: q.change.rowId,
          op: q.change.op,
          affected: 0,
          returned: null,
          error: j < i ? null : j === i ? error : notSent,
        }));
        return { success: false, results, sql, error, durationMs: ms() };
      };
      for (const [i, p] of planned.entries()) {
        const c = p.change;
        try {
          if (c.op === "insert") {
            const given = t.cols.filter((col) => p.cells.has(col.name));
            const values = given.map((col) => p.cells.get(col.name)!);
            const row = t.cols.map((col) =>
              p.cells.has(col.name)
                ? coerce(col.type, p.cells.get(col.name)!)
                : defaultOf(col, given, values),
            );
            t.data.push(row);
            continue;
          }
          // `Client::apply`: the key's rows are counted right before the change.
          const rows = matching(t, c.key);
          if (rows.length !== 1)
            return fail(
              i,
              payload(
                "DB_ERROR",
                `the row's key matches ${rows.length} row(s), not 1: ClickHouse keys are not unique, so the change was not applied`,
                { statementIndex: i },
              ),
            );
          const at = rows[0]!;
          if (c.op === "delete") {
            t.data.splice(at, 1);
            continue;
          }
          const next = t.data[at]!.slice();
          for (const [k, v] of p.cells) {
            const col = t.cols.findIndex((x) => x.name === k);
            next[col] = coerce(t.cols[col]!.type, v);
          }
          t.data[at] = next;
        } catch (e) {
          if (!(e instanceof ServerFailure)) throw e;
          return fail(i, dbPayload(e, i));
        }
      }
      return {
        success: true,
        results: planned.map((p) => ({
          rowId: p.change.rowId,
          op: p.change.op,
          affected: 1,
          returned: null,
          error: null,
        })),
        sql,
        error: null,
        durationMs: ms(),
      };
    },
    run: (statement, emit) => runOne(statement, ctx, o.readOnly, emit),
    listProcesses: (): ProcessRow[] => {
      seedOthers();
      const listing = newQueryId();
      const own: Process = {
        queryId: listing,
        user: o.user,
        address: "::ffff:127.0.0.1",
        elapsedSecs: 0.001,
        database: o.database,
        query:
          "SELECT query_id, user, address, elapsed, read_rows, memory_usage, current_database, query FROM system.processes ORDER BY elapsed DESC",
      };
      return [...OTHERS, own]
        .sort((a, b) => b.elapsedSecs - a.elapsedSecs)
        .map((p) => ({
          id: p.queryId,
          user: p.user,
          database: p.database,
          application: null,
          client: p.address,
          state: null,
          command: null,
          startedAt: null,
          durationMs: Math.trunc(p.elapsedSecs * 1000),
          waitEvent: null,
          query: p.query,
          current: p.queryId === listing,
          own: p.queryId.startsWith("tabler-"),
        }));
    },
    killProcesses: (ids, scope) =>
      ids.map((id) => {
        if (o.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
        if (scope === "connection")
          return {
            id,
            ok: false,
            message:
              "unsupported: ClickHouse stops queries, not connections (each request is one): use Kill Query",
          };
        killQuery(id);
        return { id, ok: true, message: null };
      }),
    sessionDetails: () => ({ tls: o.tls, tlsVersion: null, tlsCipher: null, encoding: "UTF-8" }),
  };
}

/** Runs one statement in the editor's HTTP session, streaming its events. */
function runOne(sql: string, ctx: SqlContext, readOnly: boolean, emit: (e: QueryEvent) => void) {
  const span = { start: 0, end: sql.length, line: 1 };
  const started = performance.now();
  emit({ event: "started", data: { index: 0, sql, span } });
  // `run_statement`: a read-only session refuses before sending.
  if (readOnly && writes(sql)) {
    emit({
      event: "failed",
      data: { index: 0, error: payload("INVALID_INPUT", READ_ONLY_MESSAGE) },
    });
    return;
  }
  let outcome: ReturnType<typeof runStatement>;
  try {
    outcome = runStatement(sql, ctx);
  } catch (e) {
    if (!(e instanceof ServerFailure)) throw e;
    emit({ event: "failed", data: { index: 0, error: dbPayload(e, 0) } });
    return;
  }
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
    durationMs: Math.max(1, Math.round(performance.now() - started)),
    error: null,
    notices: [],
  };
  if (outcome.kind === "rows") {
    emit({ event: "columns", data: { index: 0, set: 0, columns: outcome.columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows: outcome.rows } });
    emit({
      event: "finished",
      data: {
        index: 0,
        result: {
          ...base,
          kind: "rows",
          columns: outcome.columns,
          resultSets: 1,
          rowCount: outcome.rows.length,
        },
      },
    });
    return;
  }
  emit({
    event: "finished",
    data: {
      index: 0,
      result:
        outcome.kind === "affected"
          ? { ...base, kind: "affected", affected: outcome.affected }
          : base,
    },
  });
}
