import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { BrowseRequest } from "../../../bindings/BrowseRequest";
import type { ColumnMeta } from "../../../bindings/ColumnMeta";
import type { ErrorPayload } from "../../../bindings/ErrorPayload";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { FilterNode } from "../../../bindings/FilterNode";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowKey } from "../../../bindings/RowKey";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableMeta } from "../../../bindings/TableMeta";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import {
  columnOf,
  type Dataset,
  LISTED_PROJECTS,
  ROUTINE_DDL,
  type Table,
  tableDdl,
  tableKind,
  timestampText,
} from "./data";
import { billed, JobFailure, plan, type SqlContext, singleStatement, writes } from "./sql";

/**
 * The mock's BigQuery session (`drivers/bigquery/session.rs`): the backend's caps (recorded in
 * `src-tauri/tests/fixtures/bigquery/caps.json`), projects as databases and datasets as schemas,
 * cursor pages (a plain page of a native table read for free, a sorted or filtered page or any
 * page of a view from one query job whose later pages follow), counts, whole cells, the editor
 * with its cost notices and cost guard, Running Queries and kills, Show DDL, routines, and the
 * `dryRun` call. No row edits (EditRows is off), so there is no preview or apply. A read-only
 * session refuses what the classifier calls a write, and kills, before anything runs.
 */

/** The backend's `READ_ONLY_MESSAGE`. */
export const READ_ONLY_MESSAGE = "This connection is read-only";
/** `session::QUERY_BROWSE_LIMIT`. */
export const QUERY_BROWSE_LIMIT = 1_000_000;
/** `session::DRY_RUN`. */
export const DRY_RUN = "dryRun";

/** `session::FEATURES`, in `Feature::ALL`'s order (as `SessionCaps::with` keeps them). */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "cancelOnServer",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "routines",
  "processList",
  "killProcess",
];

/** `session::FILTER_OPS`. */
export const FILTER_OPS: SessionCaps["data"]["filterOps"] = [
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

/**
 * `BigquerySession::capabilities`: the same for every profile but the default dataset (read-only
 * is the classifier's, and the confirm is the passcode, since nobody types a token).
 */
export function bigqueryCaps(defaultDataset: string | null): SessionCaps {
  return {
    driver: "bigquery",
    flavor: null,
    dialect: "bigquery",
    language: "sql",
    features: [...FEATURES],
    namespaces: {
      databases: true,
      schemas: "schemas",
      defaultSchema: defaultDataset,
      labels: { database: "Project", schema: "Dataset", table: "Table", tables: "Tables" },
    },
    data: {
      paging: "cursor",
      count: "costly",
      sort: true,
      filterOps: [...FILTER_OPS],
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: [],
      maxParams: 10_000,
    },
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "passcode" },
    session: { keepaliveSecs: null, pingBilled: false },
  };
}

export interface BigqueryOptions {
  project: string;
  defaultDataset: string | null;
  location: string;
  extraProjects: string[];
  /** The cost guard; null: none. */
  maximumBytesBilled: number | null;
  useQueryCache: boolean;
  readOnly: boolean;
  user: string | null;
  datasets: Map<string, Dataset>;
}

// --- Running Queries: other people's jobs, shared by the mock's sessions ----------------------

const OTHERS: ProcessRow[] = [];

function seedOthers(): void {
  const started = Date.now() - 95_000;
  OTHERS.splice(
    0,
    OTHERS.length,
    {
      id: "demo-proj:US.bquxjob_5f1e2a7b_18c9d0e4a11",
      user: "analyst@example.com",
      database: "demo-proj",
      application: null,
      client: null,
      state: "RUNNING",
      command: "SELECT",
      startedAt: timestampText(started),
      durationMs: 95_000,
      waitEvent: null,
      query:
        "SELECT user_id, COUNT(*) AS n FROM `demo-proj.analytics.events` GROUP BY user_id ORDER BY n DESC",
      current: false,
      own: false,
    },
    {
      id: "demo-proj:US.scheduled_query_6650a1f2_0000",
      user: "scheduler@demo-proj.iam.gserviceaccount.com",
      database: "demo-proj",
      application: null,
      client: null,
      state: "PENDING",
      command: "MERGE",
      startedAt: null,
      durationMs: null,
      waitEvent: null,
      query: "MERGE `demo-proj.analytics.daily_revenue` T USING …",
      current: false,
      own: false,
    },
  );
}
seedOthers();

/** Tests only: the jobs of other people as they were. */
export function resetProcesses(): void {
  seedOthers();
}

/** `catalog::parse_process_id`: `project:LOCATION.job_id` (the last colon ends the project). */
export function parseProcessId(
  id: string,
): { project: string; location: string | null; job: string } | null {
  const at = id.lastIndexOf(":");
  if (at < 0) return null;
  const project = id.slice(0, at);
  const rest = id.slice(at + 1);
  const dot = rest.indexOf(".");
  const [location, job] = dot < 0 ? [null, rest] : [rest.slice(0, dot), rest.slice(dot + 1)];
  return project === "" || job === "" ? null : { project, location, job };
}

// --- helpers ------------------------------------------------------------------------------------

const backticked = (name: string): string =>
  `\`${name.replaceAll("\\", "\\\\").replaceAll("`", "\\`")}\``;

const payload = (
  code: string,
  message: string,
  extra: Partial<ErrorPayload> = {},
): ErrorPayload => ({
  code,
  message,
  detail: null,
  hint: null,
  sqlstate: null,
  vendorCode: null,
  position: null,
  statementIndex: null,
  data: null,
  ...extra,
});

/** A job error as `errors::db_error` reports it (HTTP 400 for a job's error). */
const jobPayload = (e: JobFailure, statementIndex: number | null): ErrorPayload =>
  payload("DB_ERROR", e.message, {
    detail: `reason: ${e.reason}`,
    hint: e.hint,
    vendorCode: 400,
    statementIndex,
  });

/** `session::bytes_text`. */
export function bytesText(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let unit = 0;
  while (v >= 1024 && unit + 1 < units.length) {
    v /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${n} B` : `${v.toFixed(1)} ${units[unit]}`;
}

/** `session::cost_notice`. */
export function costNotice(processed: number, bill: number, cached: boolean): string {
  return `BigQuery processed ${bytesText(processed)}, billed ${bytesText(bill)}${cached ? " (cached result)" : ""}`;
}

const metaColumns = (t: Table): ColumnMeta[] =>
  t.fields.map((f, i) => columnOf(f, i + 1, t.primaryKey));

/** One query job's results, whose pages a cursor reads. */
interface Job {
  fingerprint: string;
  columns: ColumnMeta[];
  rows: Value[][];
}

/** `cursor::stale`. */
const stale = (): never =>
  mockError(
    "INVALID_INPUT",
    "BigQuery: this page no longer matches the table, filter or sort; reload the first page",
  );

/** The WHERE text of the grid's filter, roughly as `sql::select` writes it. */
function whereText(group: FilterGroup): string {
  const lit = (v: Value): string =>
    v.t === "null"
      ? "NULL"
      : v.t === "text" || v.t === "date" || v.t === "timestampTz" || v.t === "timestamp"
        ? `'${v.v.replaceAll("\\", "\\\\").replaceAll("'", "\\'")}'`
        : "v" in v
          ? String(v.v)
          : "NULL";
  const node = (n: FilterNode): string => {
    if (n.kind === "raw") return `(${n.sql})`;
    if (n.kind === "group") return `(${whereText(n)})`;
    if (n.kind === "anyColumn") return `(any column ${n.op} ${n.values.map(lit).join(", ")})`;
    const col = backticked(n.column);
    const [a, b] = n.values;
    const ops: Partial<Record<string, string>> = {
      eq: "=",
      ne: "!=",
      gt: ">",
      lt: "<",
      ge: ">=",
      le: "<=",
      like: "LIKE",
      notLike: "NOT LIKE",
    };
    if (n.op === "isNull") return `${col} IS NULL`;
    if (n.op === "isNotNull") return `${col} IS NOT NULL`;
    if (n.op === "between" && a && b) return `${col} BETWEEN ${lit(a)} AND ${lit(b)}`;
    if (n.op === "in" || n.op === "notIn")
      return `${col} ${n.op === "in" ? "IN" : "NOT IN"} (${n.values.map(lit).join(", ")})`;
    return `CAST(${col} AS STRING) ${ops[n.op] ?? n.op} ${a ? lit(a) : "NULL"}`;
  };
  return group.items.map(node).join(` ${group.conjunction.toUpperCase()} `);
}

/** The raw WHERE pieces of a filter. */
function rawPieces(group: FilterGroup | null): string[] {
  return (group?.items ?? []).flatMap((n) =>
    n.kind === "raw" ? [n.sql] : n.kind === "group" ? rawPieces(n) : [],
  );
}

// --- the session -------------------------------------------------------------------------------

export function bigquerySession(o: BigqueryOptions): MockSession {
  const caps = bigqueryCaps(o.defaultDataset);
  const ctx: SqlContext = {
    project: o.project,
    defaultDataset: o.defaultDataset,
    location: o.location,
    user: o.user,
    datasets: o.datasets,
  };
  const jobs = new Map<string, Job>();
  let jobSeq = 0;
  // The query cache: a statement repeated with no DML since is free.
  const cache = new Set<string>();

  const datasetOf = (table: TableRef): Dataset => {
    const name = table.schema || o.defaultDataset;
    if (!name) return mockError("INVALID_INPUT", "BigQuery: choose a dataset first");
    return (
      o.datasets.get(name) ??
      mockError(
        "NOT_FOUND",
        `not found: BigQuery (HTTP 404): Not found: Dataset ${o.project}:${name}`,
      )
    );
  };
  const tableOf = (ref: TableRef): Table => {
    const d = datasetOf(ref);
    return (
      d.tables.get(ref.name) ??
      mockError(
        "NOT_FOUND",
        `not found: BigQuery (HTTP 404): Not found: Table ${o.project}:${d.name}.${ref.name}`,
      )
    );
  };
  const refuseWrite = (sql: string) => {
    if (o.readOnly && writes(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };
  /** `refuse_added_statement`: the grid's raw WHERE may only read. */
  const refuseAdded = (sql: string) => {
    if (writes(sql) || !singleStatement(sql))
      mockError(
        "INVALID_INPUT",
        "A filter can only read rows: this one would run another statement",
      );
  };
  /** The cost guard of a query that processes `processed` bytes over `tables` tables. */
  const guard = (processed: number, tables: number): number => {
    const bill = billed(processed, tables);
    if (o.maximumBytesBilled !== null && bill > o.maximumBytesBilled)
      throw new JobFailure(
        `Query exceeded limit for bytes billed: ${o.maximumBytesBilled}. ${bill} or higher required.`,
        "bytesBilledLimitExceeded",
        "The query would bill more bytes than the connection's Maximum bytes billed allows.",
      );
    return bill;
  };
  const asMockError = (e: unknown): never => {
    if (e instanceof JobFailure) {
      const p = jobPayload(e, null);
      throw { code: p.code, message: p.message, detail: p.detail, hint: p.hint };
    }
    throw e;
  };

  const tableMeta = (ref: TableRef): TableMeta => {
    const t = tableOf(ref);
    return {
      table: { schema: t.dataset, name: t.name },
      kind: tableKind(t),
      columns: metaColumns(t),
      primaryKey: [...t.primaryKey],
      indexes: t.indexes,
      foreignKeys: t.foreignKeys,
      triggers: [],
      comment: t.description,
      // `numRows`: views have none.
      rowEstimate: t.type === "TABLE" ? t.rows.length : null,
      ddl: null,
      checks: [],
      options: null,
    };
  };

  return {
    caps,
    listDatabases: () => {
      const names = [o.project];
      for (const p of [...o.extraProjects, ...LISTED_PROJECTS.map((l) => l.id)])
        if (p !== "" && !names.includes(p)) names.push(p);
      return names.map((name) => ({
        name,
        owner: null,
        encoding: null,
        isCurrent: name === o.project,
      }));
    },
    listSchemas: () =>
      [...o.datasets.keys()].sort().map((name) => ({ name, isDefault: name === o.defaultDataset })),
    listTables: (schema) =>
      [...datasetOf({ schema, name: "" }).tables.values()]
        .map((t) => ({
          name: t.name,
          schema,
          kind: tableKind(t),
          // `tables.list` has no row counts.
          rowEstimate: null,
          sizeBytes: null,
          comment: null,
        }))
        .sort((a, b) => a.name.localeCompare(b.name)),
    tableMeta,
    // Show DDL: an INFORMATION_SCHEMA read, which a read-only session may run.
    tableDdl: (ref) => tableDdl(o.project, tableOf(ref)),

    browse: (req: BrowseRequest): BrowsePage => {
      const started = performance.now();
      if (!req.cursor && req.offset > 0)
        mockError(
          "INVALID_INPUT",
          "BigQuery pages follow one another: there is no offset to start from",
        );
      const t = tableOf(req.table);
      const all = metaColumns(t);
      const columns = req.columns ? all.filter((c) => req.columns!.includes(c.name)) : all;
      const pick = columns.map((c) => all.indexOf(c));
      const limit = Math.max(1, req.limit);
      const plain = t.type === "TABLE" && !req.filter && req.sort.length === 0;
      const page = (
        rows: Value[][],
        from: number,
        cursorOf: (next: number) => string,
        sql: string,
      ): BrowsePage => {
        const slice = rows.slice(from, from + limit);
        const next = from + slice.length;
        return {
          columns,
          rows: slice,
          pkColumns: [...t.primaryKey],
          identity: t.primaryKey.length > 0 ? "pk" : "none",
          rowIds: null,
          offset: req.offset,
          limit: req.limit,
          sql,
          durationMs: Math.max(1, Math.round(performance.now() - started)),
          nextCursor: next < rows.length ? cursorOf(next) : null,
        };
      };
      if (plain) {
        // `tabledata.list`: free, its page token the cursor.
        const fp = `data:${o.project}.${t.dataset}.${t.name}`;
        let from = 0;
        if (req.cursor) {
          const m = /^(data:[^|]*)\|(\d+)$/.exec(req.cursor);
          if (!m || m[1] !== fp) stale();
          from = Number(m![2]);
        }
        const rows = t.rows.map((r) => pick.map((i) => r[i] ?? { t: "null" as const }));
        return page(
          rows,
          from,
          (next) => `${fp}|${next}`,
          `SELECT * FROM ${[o.project, t.dataset, t.name].map(backticked).join(".")} -- read with tabledata.list: no job, nothing billed`,
        );
      }
      // One query job (under the cost guard); its later pages are read from it for free.
      const where =
        req.filter && req.filter.items.length > 0 ? ` WHERE ${whereText(req.filter)}` : "";
      const order = req.sort.length
        ? ` ORDER BY ${req.sort.map((s) => `${backticked(s.column)} ${s.dir.toUpperCase()}`).join(", ")}`
        : "";
      const sql = `SELECT ${columns.map((c) => backticked(c.name)).join(", ")} FROM ${backticked(t.dataset)}.${backticked(t.name)}${where}${order} LIMIT ${QUERY_BROWSE_LIMIT}`;
      for (const piece of rawPieces(req.filter)) refuseAdded(`SELECT 1 FROM t WHERE ${piece}`);
      refuseWrite(sql);
      let job: { id: string; job: Job } | null = null;
      let from = 0;
      if (req.cursor) {
        const m = /^job:([^|]+)\|(\d+)$/.exec(req.cursor);
        const found = m ? jobs.get(m[1]!) : undefined;
        if (!m || !found || found.fingerprint !== sql) stale();
        job = { id: m![1]!, job: found! };
        from = Number(m![2]);
      } else {
        try {
          guard(Math.round((t.bytes * columns.length) / Math.max(1, all.length)), 1);
        } catch (e) {
          asMockError(e);
        }
        const meta = all;
        let rows = req.filter ? t.rows.filter((r) => matchesGroup(req.filter!, r, meta)) : t.rows;
        rows = sortRows(rows, req.sort, meta).slice(0, QUERY_BROWSE_LIMIT);
        const id = `tabler_mock_${++jobSeq}`;
        job = {
          id,
          job: {
            fingerprint: sql,
            columns,
            rows: rows.map((r) => pick.map((i) => r[i] ?? { t: "null" as const })),
          },
        };
        jobs.set(id, job.job);
      }
      return page(job.job.rows, from, (next) => `job:${job.id}|${next}`, sql);
    },

    // `exact`: what `count_table_rows` was asked. The shared handler does not pass it yet, so
    // until it does every count is the exact one (a change request to the lead).
    count: (ref, filter, exact?: boolean) => {
      const t = tableOf(ref);
      // CountMode::Costly: an unfiltered table answers its numRows until asked to count.
      if (exact === false && !filter && t.type === "TABLE")
        return { count: t.rows.length, exact: false };
      for (const piece of rawPieces(filter)) refuseAdded(`SELECT 1 FROM t WHERE ${piece}`);
      const rows = filter ? t.rows.filter((r) => matchesGroup(filter, r, metaColumns(t))) : t.rows;
      return { count: rows.length, exact: true };
    },

    fetchCell: (ref, key: RowKey, column) => {
      const t = tableOf(ref);
      const at = t.fields.findIndex((f) => f.name === column);
      if (at < 0) mockError("INVALID_INPUT", `unknown column "${column}"`);
      if (key.kind === "rowid") mockError("INVALID_INPUT", "BigQuery rows have no row id");
      const items: FilterNode[] = Object.entries(key.value).map(([c, v]) =>
        v.t === "null"
          ? { kind: "condition", column: c, op: "isNull", values: [] }
          : { kind: "condition", column: c, op: "eq", values: [v] },
      );
      const row = t.rows.find((r) =>
        matchesGroup({ conjunction: "and", items }, r, metaColumns(t)),
      );
      if (!row) mockError("NOT_FOUND", "not found: the row no longer exists");
      return row![at] ?? { t: "null" };
    },

    run: (sql, emit) => runOne(sql, emit),

    call: (method, args) => {
      if (method !== DRY_RUN)
        mockError("UNSUPPORTED", `unsupported: BigQuery: unknown call “${method}”`);
      // `{sql}`, or `{table, project?}`: a full scan the backend builds (`full_scan_query`).
      const a = (args ?? {}) as { sql?: unknown; table?: unknown; project?: unknown };
      const text = (v: unknown): string | null =>
        typeof v === "string" && v.trim() !== "" ? v : null;
      let sql: string | null;
      if (a.table !== undefined) {
        const t = (a.table ?? {}) as { schema?: unknown; name?: unknown };
        const dataset = text(t.schema);
        const name = text(t.name);
        if (dataset === null || name === null)
          mockError("INVALID_INPUT", "BigQuery: dryRun needs the table's dataset and name");
        const project = text(a.project);
        const parts = project === null ? [dataset!, name!] : [project, dataset!, name!];
        sql = `SELECT * FROM ${parts.map(backticked).join(".")}`;
      } else sql = text(a.sql);
      if (sql === null) mockError("INVALID_INPUT", "BigQuery: dryRun needs the sql");
      // A dry run runs nothing, whatever the statement: no read-only check, no cost guard.
      try {
        const p = plan(sql as string, ctx);
        return {
          bytesProcessed: String(p.processed),
          columns: p.columns.map((c) => ({ name: c.name, dataType: c.dataType })),
        };
      } catch (e) {
        return asMockError(e);
      }
    },

    listProcesses: () => [...OTHERS],
    killProcesses: (ids) =>
      ids.map((id) => {
        if (o.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
        if (!parseProcessId(id)) return { id, ok: false, message: `“${id}” is not a BigQuery job` };
        // `jobs.cancel`: a job that is gone is no error.
        const at = OTHERS.findIndex((p) => p.id === id);
        if (at >= 0) OTHERS.splice(at, 1);
        return { id, ok: true, message: null };
      }),

    listRoutines: (schema) => [...datasetOf({ schema, name: "" }).routines],
    routineDefinition: (routine) => {
      const ddl = ROUTINE_DDL[routine.name];
      if (
        !ddl ||
        !datasetOf({ schema: routine.schema, name: "" }).routines.some(
          (r) => r.name === routine.name,
        )
      )
        mockError("NOT_FOUND", `not found: the DDL of ${routine.name}`);
      return ddl!;
    },
  };

  /** One statement of the editor in the session's BigQuery session, streaming its events. */
  function runOne(sql: string, emit: (e: QueryEvent) => void): void {
    const span = { start: 0, end: sql.length, line: 1 };
    const t0 = performance.now();
    emit({ event: "started", data: { index: 0, sql, span } });
    // `run_statement`: a read-only session refuses before sending.
    if (o.readOnly && writes(sql)) {
      emit({
        event: "failed",
        data: {
          index: 0,
          error: payload("INVALID_INPUT", READ_ONLY_MESSAGE, { statementIndex: 0 }),
        },
      });
      return;
    }
    let outcome: Value[][] | number;
    let notice: string;
    let p: ReturnType<typeof plan>;
    try {
      p = plan(sql, ctx);
      const key = sql.trim();
      const cached = o.useQueryCache && p.kind === "rows" && cache.has(key);
      const bill = cached
        ? 0
        : guard(p.processed, p.processed > 0 || p.kind === "affected" ? 1 : 0);
      outcome = p.run();
      notice = costNotice(cached ? 0 : p.processed, bill, cached);
      if (p.kind === "affected") cache.clear();
      else if (o.useQueryCache) cache.add(key);
    } catch (e) {
      if (!(e instanceof JobFailure)) throw e;
      emit({ event: "failed", data: { index: 0, error: jobPayload(e, 0) } });
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
      durationMs: Math.max(1, Math.round(performance.now() - t0)),
      error: null,
      notices: [notice],
    };
    if (typeof outcome === "number") {
      emit({
        event: "finished",
        data: { index: 0, result: { ...base, kind: "affected", affected: outcome } },
      });
      return;
    }
    emit({ event: "columns", data: { index: 0, set: 0, columns: p.columns } });
    emit({ event: "rows", data: { index: 0, set: 0, rows: outcome } });
    emit({
      event: "finished",
      data: {
        index: 0,
        result: {
          ...base,
          kind: "rows",
          columns: p.columns,
          resultSets: 1,
          rowCount: outcome.length,
        },
      },
    });
  }
}
