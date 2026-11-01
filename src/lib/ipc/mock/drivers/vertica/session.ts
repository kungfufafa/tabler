import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { KillOutcome } from "../../../bindings/KillOutcome";
import type { ProcessRow } from "../../../bindings/ProcessRow";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { DRIVER_CATALOG } from "../../../bindings/driverCatalog";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { isSingle, refusedReadOnly, writes } from "./classify";
import { type Cluster, columnsOf, exportObjects, NULL, sizeOf, type Table } from "./data";
import { rowProblem, runSql, serverError, ServerFailure } from "./sql";

/**
 * A session of the mock's Vertica, mirroring `drivers/vertica/session.rs`: the same caps, one
 * database with its schemas, offset pages ordered by the sort (else the key), exact counts on
 * request, staged commits in one transaction that the first failing change rolls back whole (each
 * statement checked against its expected count), the editor's statements one at a time, Running
 * Queries over `v_monitor.sessions` with Kill Query (`INTERRUPT_STATEMENT`) and Kill Connection
 * (`CLOSE_SESSION`), routines, Show DDL (`EXPORT_OBJECTS`) and session details. A read-only
 * session refuses what `refused_read_only` refuses before anything runs; every session refuses a
 * text of several statements and a grid filter that writes. The session has no calls of its own.
 */

/** The backend's `READ_ONLY_MESSAGE` (`commands::safety`). */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `mutate::plan_rows` on a read-only profile (`AppError::Unsupported`). */
export const READ_ONLY_EDIT = "unsupported: this connection is read-only";

/** `session::refuse_several`. */
export const SEVERAL_MESSAGE = "Vertica: one statement at a time (this text holds several)";

/** `session::refuse_added_write`. */
export const ADDED_WRITE_MESSAGE =
  "A filter or an exported query can only read rows: this one would write";

/** `drivers::check_affected`'s words for a change whose row is gone. */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

/** `session::NO_DEFINITION`. */
export const NO_DEFINITION =
  "-- The definition is not available: Vertica keeps no text of UDx functions, and EXPORT_OBJECTS returned none for this routine.";

/** `session::FEATURES`, in `Feature`'s order (as `SessionCaps::with` sorts them). */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "explain",
  "cancelOnServer",
  "editRows",
  "insertRows",
  "deleteRows",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "routines",
  "processList",
  "killProcess",
  "timezone",
  "startupCommands",
  "sessionDetails",
  "mcp",
];

/** `session::FILTER_OPS`: every filter but JSON containment. */
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

/** `VerticaSession::capabilities`. */
export function verticaCaps(defaultSchema: string, commandPassword: boolean): SessionCaps {
  const base = DRIVER_CATALOG.baseCaps.vertica as SessionCaps;
  return {
    ...base,
    features: FEATURES,
    namespaces: { ...base.namespaces, databases: true, schemas: "schemas", defaultSchema },
    data: {
      paging: "offset",
      count: "estimate",
      sort: true,
      filterOps: FILTER_OPS,
      rawFilter: "sqlWhere",
      transactions: "atomic",
      affected: "exact",
      defaultKeyword: false,
      upsert: false,
      serverExprs: [],
      maxParams: 1000,
    },
    safety: {
      readOnly: "server",
      hardened: false,
      confirm: commandPassword ? "passcode" : "password",
    },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** `"name"` with `"` doubled (`double_quoted`). */
export const quote = (name: string) => `"${name.replaceAll('"', '""')}"`;

/** A value as Vertica SQL text, for Preview SQL (`quote_exec`: `E''` when it holds a backslash). */
export function literal(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "NULL";
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "bytes":
      return `HEX_TO_BINARY('${[...atob(v.v.base64)].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("")}')`;
    case "array":
      return `'${JSON.stringify(v.v).replaceAll("'", "''")}'`;
    case "other":
      return `'${v.v.text.replaceAll("'", "''")}'`;
    default:
      return v.v.includes("\\")
        ? `E'${v.v.replaceAll("\\", "\\\\").replaceAll("'", "''")}'`
        : `'${v.v.replaceAll("'", "''")}'`;
  }
}

/** Every raw filter of a group, nested groups included. */
function rawFilters(group: FilterGroup): string[] {
  return group.items.flatMap((n) =>
    n.kind === "raw" ? [n.sql] : n.kind === "group" ? rawFilters(n) : [],
  );
}

/** `mutate::ordered`: deletes first (they free unique keys), then updates, then inserts. */
const ordered = (changes: StagedChange[]): StagedChange[] => [
  ...changes.filter((c) => c.op === "delete"),
  ...changes.filter((c) => c.op === "update"),
  ...changes.filter((c) => c.op === "insert"),
];

const cellText = (v: Value): string =>
  v.t === "null" || v.t === "default"
    ? "\u0000null"
    : typeof v.v === "object"
      ? JSON.stringify(v.v)
      : String(v.v);

/** The row a primary key names; -1 when it is gone. */
function rowAt(t: Table, key: RowKey): number {
  if (key.kind !== "pk") return -1;
  return t.rows.findIndex((row) =>
    Object.entries(key.value).every(([name, v]) => {
      const k = t.cols.findIndex((c) => c.name === name);
      return k >= 0 && cellText(row[k] ?? NULL) === cellText(v);
    }),
  );
}

/** A session's own state. */
export interface VerticaOptions {
  cluster: Cluster;
  user: string;
  readOnly: boolean;
  commandPassword: boolean;
  tls: boolean;
  clientLabel: string;
  /** `v_vmart_node0001-…:0x…`, the session's id in `v_monitor.sessions`. */
  sessionId: string;
}

/** One session to the mock's cluster. */
export function verticaSession(o: VerticaOptions): MockSession {
  const caps = verticaCaps("public", o.commandPassword);
  const { cluster } = o;
  const tableOf = (ref: TableRef): Table => {
    const schema = ref.schema ?? "public";
    return (
      cluster.tables.find((t) => t.schema === schema && t.name === ref.name) ??
      mockError("NOT_FOUND", `not found: ${schema}.${ref.name}`)
    );
  };
  const qualified = (t: Table) => `${quote(t.schema)}.${quote(t.name)}`;

  /** `refuse_write`: a read-only session refuses before anything is sent. */
  const refuseWrite = (sql: string) => {
    if (o.readOnly && refusedReadOnly(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };
  const refuseSeveral = (sql: string) => {
    if (!isSingle(sql)) mockError("INVALID_INPUT", SEVERAL_MESSAGE);
  };

  /** The checks of the grid's reads (`read`): several, `refuse_write`, then the filter's write. */
  const checkRead = (t: Table, filter: FilterGroup | null) => {
    const raw = filter ? rawFilters(filter) : [];
    if (raw.length === 0) return;
    const sql = `SELECT COUNT(*) FROM ${qualified(t)} WHERE ${raw.map((r) => `(${r})`).join(" AND ")}`;
    refuseSeveral(sql);
    refuseWrite(sql);
    if (writes(sql)) mockError("INVALID_INPUT", ADDED_WRITE_MESSAGE);
  };

  const filtered = (t: Table, filter: FilterGroup | null) => {
    const columns = columnsOf(t);
    return filter ? t.rows.filter((r) => matchesGroup(filter, r, columns)) : t.rows;
  };

  const whereKey = (key: RowKey): string =>
    key.kind === "pk"
      ? Object.entries(key.value)
          .map(([k, v]) => (v.t === "null" ? `${quote(k)} IS NULL` : `${quote(k)} = ${literal(v)}`))
          .join(" AND ")
      : "";

  /** The cells of a change by column name, as the backend's map orders them. */
  const byName = (cells: Record<string, Value>) =>
    Object.entries(cells).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));

  const statement = (t: Table, c: StagedChange): string => {
    if (c.op === "delete") return `DELETE FROM ${qualified(t)} WHERE ${whereKey(c.key)}`;
    if (c.op === "update")
      return `UPDATE ${qualified(t)} SET ${byName(c.changes)
        .map(([k, v]) => `${quote(k)} = ${literal(v)}`)
        .join(", ")} WHERE ${whereKey(c.key)}`;
    // Vertica has no RETURNING: the grid reads its page again.
    const cols = byName(c.values);
    return `INSERT INTO ${qualified(t)} (${cols.map(([k]) => quote(k)).join(", ")}) VALUES (${cols
      .map(([, v]) => literal(v))
      .join(", ")})`;
  };

  /** `plan_rows`: refused on a read-only profile, and by the features the changes need. */
  const plan = (req: ApplyRequest): { t: Table; order: StagedChange[]; sql: string[] } => {
    if (o.readOnly) mockError("UNSUPPORTED", READ_ONLY_EDIT);
    for (const c of req.changes) {
      if (c.op !== "insert" && c.key.kind !== "pk")
        mockError(
          "UNSUPPORTED",
          "unsupported: Vertica: editing rows without a primary key is not available",
        );
      if (c.op !== "delete" && c.exprs && Object.keys(c.exprs).length > 0)
        mockError(
          "UNSUPPORTED",
          "unsupported: Vertica: DEFAULT and server expressions is not available",
        );
    }
    const t = tableOf(req.table);
    const order = ordered(req.changes);
    return { t, order, sql: order.map((c) => statement(t, c)) };
  };

  const apply = (req: ApplyRequest): ApplyResult => {
    const { t, order, sql } = plan(req);
    // `transaction`: each statement one at a time, refused on a read-only session first.
    for (const s of sql) refuseWrite(s);
    if (t.kind !== "table")
      mockError(
        "DB_ERROR",
        `Cannot modify ${t.kind === "view" ? "view" : "external table"} "${t.name}"`,
      );
    const before = t.rows.map((r) => r.slice());
    // Results follow the statements' order, as `PlannedRows` reports them.
    const results = new Map<string, RowResult>();
    for (const [i, c] of order.entries()) {
      let failure: ServerFailure | null = null;
      let guard = false;
      if (c.op === "insert") {
        const row = t.cols.map((col) => {
          const v = c.values[col.name];
          if (v !== undefined) return v;
          return col.default !== undefined ? runDefault(t, col.name) : NULL;
        });
        failure = rowProblem(t, row);
        if (!failure) t.rows.push(row);
      } else {
        const at = rowAt(t, c.key);
        if (at < 0) guard = true;
        else if (c.op === "delete") t.rows.splice(at, 1);
        else {
          const row = t.rows[at]!.slice();
          for (const [name, v] of Object.entries(c.changes)) {
            const k = t.cols.findIndex((col) => col.name === name);
            if (k >= 0) row[k] = v;
          }
          failure = rowProblem(t, row);
          if (!failure) t.rows[at] = row;
        }
      }
      if (failure || guard) {
        // One transaction: ROLLBACK undoes the changes before the failing one.
        t.rows = before;
        const error = failure
          ? serverError(failure, i)
          : {
              ...serverError(new ServerFailure("", 0, GUARD_MESSAGE), i),
              sqlstate: null,
              vendorCode: null,
            };
        return {
          success: false,
          results: order.map((x) => ({
            rowId: x.rowId,
            op: x.op,
            affected: 0,
            returned: null,
            error: x === c ? error : null,
          })),
          sql,
          error,
          durationMs: 6,
        };
      }
      results.set(c.rowId, { rowId: c.rowId, op: c.op, affected: 1, returned: null, error: null });
    }
    return {
      success: true,
      results: order.map((c) => results.get(c.rowId)!),
      sql,
      error: null,
      durationMs: 6,
    };
  };

  const run = (sql: string, emit: (event: QueryEvent) => void) => {
    // `editor::run`: one statement, and a read-only session's refusal, before anything is sent.
    refuseSeveral(sql);
    refuseWrite(sql);
    const span = { start: 0, end: sql.length, line: 1 };
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
      durationMs: 3,
      error: null,
      notices: [],
    };
    emit({ event: "started", data: { index: 0, sql, span } });
    let outcome: ReturnType<typeof runSql>;
    try {
      outcome = runSql({ cluster, user: o.user, defaultSchema: "public" }, sql);
    } catch (err) {
      if (!(err instanceof ServerFailure)) throw err;
      emit({ event: "failed", data: { index: 0, error: serverError(err, 0) } });
      return;
    }
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
  };

  return {
    caps,
    listDatabases: () => [
      { name: cluster.database, owner: cluster.owner, encoding: "UTF-8", isCurrent: true },
    ],
    listSchemas: () => cluster.schemas.map((name) => ({ name, isDefault: name === "public" })),
    listTables: (schema) =>
      cluster.tables
        .filter((t) => t.schema === schema)
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((t) => ({
          name: t.name,
          schema: t.schema,
          kind: t.kind,
          // The super projections' rows and the projections' bytes (`TABLE_SIZES`).
          rowEstimate: t.projections.length > 0 ? t.rows.length : null,
          sizeBytes: t.projections.length > 0 ? sizeOf(t) : null,
          comment: t.comment,
        })),
    tableMeta: (ref) => {
      const t = tableOf(ref);
      const view = t.kind === "view";
      return {
        table: { schema: t.schema, name: t.name },
        kind: t.kind,
        columns: columnsOf(t),
        primaryKey: t.pk,
        indexes: view ? [] : t.projections,
        foreignKeys: view ? [] : t.foreignKeys,
        triggers: [],
        comment: t.comment,
        rowEstimate: t.projections.length > 0 ? t.rows.length : null,
        ddl: null,
        checks: view
          ? []
          : t.checks.map((c) => ({ name: c.name, expression: c.expression, columns: c.columns })),
        options: null,
      };
    },
    tableDdl: (ref) => exportObjects(tableOf(ref)),
    browse: (req): BrowsePage => {
      const t = tableOf(req.table);
      checkRead(t, req.filter);
      const columns = columnsOf(t);
      const rows = sortRows(filtered(t, req.filter), req.sort, columns);
      const page = rows.slice(req.offset, req.offset + req.limit);
      const picked = req.columns ? columns.filter((c) => req.columns!.includes(c.name)) : columns;
      const at = picked.map((c) => columns.indexOf(c));
      const order = req.sort.length
        ? req.sort.map((s) => `${quote(s.column)} ${s.dir.toUpperCase()}`)
        : t.pk.map((k) => `${quote(k)} ASC`);
      return {
        columns: picked,
        rows: page.map((row) => at.map((i) => row[i] ?? NULL)),
        pkColumns: t.pk,
        // No row id: a table without a primary key is not editable (`EditWithoutKey`).
        identity: t.pk.length > 0 ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${picked.map((c) => quote(c.name)).join(", ")} FROM ${qualified(t)}${order.length ? ` ORDER BY ${order.join(", ")}` : ""} LIMIT ${req.limit} OFFSET ${req.offset}`,
        durationMs: 9,
      };
    },
    count: (ref, filter) => {
      const t = tableOf(ref);
      checkRead(t, filter);
      return { count: filtered(t, filter).length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = tableOf(ref);
      const at = rowAt(t, key);
      const k = t.cols.findIndex((c) => c.name === column);
      if (at < 0 || k < 0) mockError("NOT_FOUND", "not found: the row is gone");
      return t.rows[at]![k] ?? NULL;
    },
    preview: (req) => plan(req).sql,
    apply,
    run,
    listProcesses: () => processes(o),
    killProcesses: (ids, scope) => ids.map((id) => kill(o, id, scope)),
    listRoutines: (schema) =>
      cluster.routines
        .filter((r) => r.schema === schema)
        .map(({ definition: _definition, ...summary }) => summary),
    routineDefinition: (ref) => {
      const r = cluster.routines.find((x) => x.schema === ref.schema && x.name === ref.name);
      if (!r) return NO_DEFINITION;
      return r.definition ?? NO_DEFINITION;
    },
    sessionDetails: () => ({
      tls: o.tls,
      tlsVersion: o.tls ? "TLSv1.3" : null,
      tlsCipher: o.tls ? "TLS13_AES_256_GCM_SHA384" : null,
      encoding: "UTF-8",
    }),
  };
}

/** A column's default, as the server fills it for an insert that leaves it out. */
function runDefault(t: Table, column: string): Value {
  const c = t.cols.find((x) => x.name === column)!;
  if (c.kind === "int") return { t: "int", v: Number(c.default) };
  if (c.kind === "bool") return { t: "bool", v: c.default === "true" };
  return { t: "text", v: c.default ?? "" };
}

// --- v_monitor.sessions --------------------------------------------------------------------------

/** A session of `v_monitor.sessions` (`catalog::SessionRow`). */
interface ClusterSession {
  id: string;
  user: string;
  client: string;
  label: string | null;
  /** `client_type`: Tabler's own sessions say `Tabler`. */
  type: string;
  login: string;
  statementStart: string | null;
  statement: string | null;
  statementId: number | null;
}

/** The other sessions of the cluster, for Running Queries (a kill ends one). */
const OTHERS: ClusterSession[] = [];

function seedOthers() {
  if (OTHERS.length > 0) return;
  OTHERS.push(
    {
      id: "v_vmart_node0001-41205:0x3c1d",
      user: "etl_loader",
      client: "10.0.4.30:52114",
      label: "nightly-load",
      type: "JDBC Driver",
      login: "2026-11-01 01:00:02.118+00",
      statementStart: "2026-11-01 09:10:41.003+00",
      statement:
        "INSERT /*+ DIRECT */ INTO store.store_sales_fact SELECT * FROM staging.pos_batch_2041",
      statementId: 7,
    },
    {
      id: "v_vmart_node0002-41388:0x3c2a",
      user: "analyst",
      client: "10.0.4.17:61022",
      label: null,
      type: "vsql",
      login: "2026-11-01 08:55:40.512+00",
      statementStart: "2026-11-01 09:12:04.117+00",
      statement:
        "SELECT customer_state, SUM(sales_dollar_amount) FROM store.store_sales_fact JOIN public.customer_dimension USING (customer_key) GROUP BY 1",
      statementId: 3,
    },
    {
      id: "v_vmart_node0003-41402:0x3c31",
      user: "dbadmin",
      client: "10.0.4.2:50510",
      label: "Management Console",
      type: "ODBC Driver",
      login: "2026-11-01 07:30:00.000+00",
      statementStart: null,
      statement: null,
      statementId: null,
    },
  );
}

/** Tests: the other sessions as seeded again. */
export function resetSessions(): void {
  OTHERS.splice(0);
}

/** The session that lists them, then a worker of Tabler's own, then the others. */
function sessions(o: VerticaOptions): ClusterSession[] {
  seedOthers();
  return [
    {
      id: o.sessionId,
      user: o.user,
      client: "127.0.0.1:50122",
      label: o.clientLabel,
      type: "Tabler",
      login: "2026-11-01 09:00:00.000+00",
      statementStart: "2026-11-01 09:15:00.000+00",
      statement:
        "SELECT session_id, user_name, client_hostname, client_label, … FROM v_monitor.sessions ORDER BY login_timestamp",
      statementId: 12,
    },
    ...OTHERS,
  ];
}

/** `session::process_row`. */
function processes(o: VerticaOptions): ProcessRow[] {
  return sessions(o).map((s) => ({
    id: s.id,
    user: s.user,
    database: o.cluster.database,
    application: s.label,
    client: s.client,
    state: s.statement !== null ? "active" : "idle",
    command: s.type,
    startedAt: s.statementStart ?? s.login,
    durationMs: null,
    waitEvent: null,
    query: s.statement,
    current: s.id === o.sessionId,
    own: s.type === "Tabler",
  }));
}

/** `session::kill_one`: `INTERRUPT_STATEMENT` with the running statement, or `CLOSE_SESSION`. */
function kill(o: VerticaOptions, id: string, scope: "query" | "connection"): KillOutcome {
  if (o.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
  const all = sessions(o);
  const s = all.find((x) => x.id === id);
  if (scope === "query") {
    if (!s) return { id, ok: false, message: `not found: session ${id}` };
    if (s.statementId === null)
      return { id, ok: false, message: `session ${id} runs no statement` };
    const other = OTHERS.find((x) => x.id === id);
    if (other) {
      other.statement = null;
      other.statementStart = null;
      other.statementId = null;
    }
    return { id, ok: true, message: null };
  }
  if (!s) return { id, ok: false, message: `Session "${id}" not found` };
  const at = OTHERS.findIndex((x) => x.id === id);
  if (at >= 0) OTHERS.splice(at, 1);
  return { id, ok: true, message: null };
}
