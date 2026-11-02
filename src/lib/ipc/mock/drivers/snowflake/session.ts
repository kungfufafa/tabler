import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { ExplainResult } from "../../../bindings/ExplainResult";
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
import { isSingle, setsAppParameter, setsAutocommit, writes } from "./classify";
import { type Account, bytesOf, columnsOf, getDdl, NULL, rowsOf, type Table } from "./data";
import {
  coerce,
  defaultOf,
  rowProblem,
  runSql,
  serverError,
  ServerFailure,
  type SqlContext,
} from "./sql";

/**
 * A session of the mock's Snowflake, mirroring `drivers/snowflake/session.rs`: the same caps, the
 * account's databases (one per session: Switch Database connects again) and schemas through
 * SHOW, offset pages ordered by the sort (else the key), the metadata's row count until an exact
 * count is asked for (Costly), staged commits in one transaction that the first failing change
 * rolls back whole (each statement checked against its expected count, since keys are
 * informational), the editor's statements one at a time with BEGIN / COMMIT / ROLLBACK, `USE`
 * and AUTOCOMMIT kept on, Running Queries over the user's queries with Kill Query
 * (`SYSTEM$CANCEL_QUERY`; no Kill Connection), routines, GET_DDL and session details. A read-only
 * session refuses what the classifier does not call a read before anything is sent; every
 * session refuses a grid filter that holds a second statement. On a password + MFA profile the
 * editor and the app share one session: the grid refuses while the editor holds a transaction or
 * has moved the session to another database. The session has no calls of its own.
 */

/** The backend's `READ_ONLY_MESSAGE` (`commands::safety`). */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `mutate::plan_rows` on a read-only profile (`AppError::Unsupported`). */
export const READ_ONLY_EDIT = "unsupported: this connection is read-only";

/** `session::refuse_added_statement`. */
export const ADDED_STATEMENT_MESSAGE =
  "A filter or an exported query can only read rows: this one would run another statement";

/** `editor::autocommit_error`. */
export const AUTOCOMMIT_MESSAGE =
  "Snowflake: Tabler keeps AUTOCOMMIT on (its commits and its transaction tracking rely on it): open a transaction with BEGIN and end it with COMMIT or ROLLBACK instead";

/** `editor::ignore_case_error`. */
export const IGNORE_CASE_MESSAGE =
  'Snowflake: Tabler keeps QUOTED_IDENTIFIERS_IGNORE_CASE off on the session its grid and commits use (with it on, a quoted name such as "Orders" would find ORDERS, another table)';

/** `editor::app_parameter_error`. */
export const appParameterMessage = (name: string): string =>
  name === "QUOTED_IDENTIFIERS_IGNORE_CASE"
    ? IGNORE_CASE_MESSAGE
    : `Snowflake: Tabler keeps ${name} = 'AUTO' on the session its grid and commits use (the grid writes dates, times and timestamps in ISO form), and this sign-in asks MFA, so the editor shares that session`;

/** `session::transaction_open_error`. */
export const TRANSACTION_OPEN_MESSAGE =
  "Snowflake: the editor has a transaction open on this session: commit or roll it back first";

/** `session::kill_one` of a whole session. */
export const KILL_CONNECTION_MESSAGE =
  "unsupported: Snowflake cancels one query, not a whole session: use Kill Query";

/** What Snowflake answers a request of two statements (the login's `MULTI_STATEMENT_COUNT = 1`). */
export const SEVERAL_MESSAGE =
  "Actual statement count 2 did not match the desired statement count 1.";

/** `drivers::check_affected`'s words for a change whose row is gone. */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

/** `session::check_database` on a shared session the editor moved. */
export const movedMessage = (current: string | null, own: string | null) =>
  `Snowflake: the editor moved this session to database ${current ?? "(none)"} (the app shares it, as the sign-in asks MFA), and the app's statements would land there: ${own ? `run USE DATABASE ${quote(own)} in the editor` : "reconnect"} first`;

/** `session::FEATURES`, in `Feature`'s order (as `SessionCaps::with` sorts them). */
export const FEATURES: SessionCaps["features"] = [
  "editor",
  "explain",
  "cancelOnServer",
  "format",
  "editRows",
  "insertRows",
  "deleteRows",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "routines",
  "switchDatabase",
  "processList",
  "killProcess",
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

/** `session::capabilities` (`defaultSchema`: the session's schema). */
export function snowflakeCaps(defaultSchema: string | null): SessionCaps {
  const base = DRIVER_CATALOG.baseCaps.snowflake as SessionCaps;
  return {
    ...base,
    features: FEATURES,
    namespaces: { ...base.namespaces, databases: true, schemas: "schemas", defaultSchema },
    data: {
      paging: "offset",
      count: "costly",
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
    safety: { readOnly: "classifierOnly", hardened: false, confirm: "passcode" },
    session: { keepaliveSecs: null, pingBilled: false },
  };
}

/** `"name"` with `"` doubled (`double_quoted`). */
export const quote = (name: string) => `"${name.replaceAll('"', '""')}"`;

/** `spec::single_quoted_backslash`. */
const quoted = (s: string) =>
  `'${s.replaceAll("\\", "\\\\").replaceAll("'", "\\'").replaceAll("\n", "\\n").replaceAll("\r", "\\r")}'`;

/** A value as Snowflake SQL text, for Preview SQL (the dialect's casts around binds). */
export function literal(v: Value): string {
  switch (v.t) {
    case "null":
    case "default":
      return "NULL";
    case "bool":
      return v.v ? "TRUE" : "FALSE";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "bytes":
      return `TO_BINARY('${[...atob(v.v.base64)].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("")}', 'HEX')`;
    case "json":
      return `PARSE_JSON(${quoted(v.v)})`;
    case "array":
      return `PARSE_JSON(${quoted(JSON.stringify(v.v))})`;
    case "other":
      return quoted(v.v.text);
    default:
      return quoted(v.v);
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

/** The rows a primary key names (keys are informational: there may be two). */
function rowsAt(t: Table, key: RowKey): number[] {
  if (key.kind !== "pk") return [];
  const out: number[] = [];
  t.rows.forEach((row, i) => {
    const hit = Object.entries(key.value).every(([name, v]) => {
      const k = t.cols.findIndex((c) => c.name === name);
      return k >= 0 && cellText(row[k] ?? NULL) === cellText(v);
    });
    if (hit) out.push(i);
  });
  return out;
}

/** A session's own state. */
export interface SnowflakeOptions {
  account: Account;
  user: string;
  role: string;
  warehouse: string | null;
  /** The connection's database (one per session). */
  database: string | null;
  schema: string | null;
  readOnly: boolean;
  /** Password + MFA: the editor and the app share one Snowflake session. */
  shared: boolean;
  tls: boolean;
  /** Snowflake's session id, which Running Queries shows as the client. */
  sessionId: string;
}

/** One session to the mock's account. */
export function snowflakeSession(o: SnowflakeOptions): MockSession {
  const caps = snowflakeCaps(o.schema);
  const { account } = o;
  // The editor's session: `USE` moves it; on a shared session the app's statements run there.
  const editor: SqlContext = {
    account,
    user: o.user,
    database: o.database,
    schema: o.schema,
    warehouse: o.warehouse,
    role: o.role,
  };
  /** The tables' rows when the editor's transaction began; null outside one. */
  let tx: Map<Table, Value[][]> | null = null;

  /** `database()`: the app's statements name schema and table, in the connection's database. */
  const database = (): string =>
    o.database ??
    mockError(
      "INVALID_INPUT",
      "Snowflake: the session has no database: choose one in the connection",
    );

  const tableOf = (ref: TableRef): Table => {
    const db = database();
    const schema = ref.schema ?? o.schema;
    if (schema === null) mockError("INVALID_INPUT", `Snowflake: ${ref.name} has no schema`);
    return (
      account.tables.find((t) => t.database === db && t.schema === schema && t.name === ref.name) ??
      mockError(
        "DB_ERROR",
        `SQL compilation error:\nObject '${db}.${schema}.${ref.name}' does not exist or not authorized.`,
      )
    );
  };
  const qualified = (t: Table) => `${quote(t.schema)}.${quote(t.name)}`;

  /** `refuse_write`: a read-only session refuses before anything is sent. */
  const refuseWrite = (sql: string) => {
    if (o.readOnly && writes(sql)) mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };

  /** `check_database`: on a shared session, the editor must not have moved it elsewhere. */
  const checkDatabase = () => {
    if (o.shared && editor.database !== o.database)
      mockError("INVALID_INPUT", movedMessage(editor.database, o.database));
  };

  /** The checks of the grid's reads (`read`): `refuse_write`, a second statement, the database. */
  const checkRead = (t: Table, filter: FilterGroup | null) => {
    const raw = filter ? rawFilters(filter) : [];
    if (raw.length > 0) {
      const sql = `SELECT COUNT(*) FROM ${qualified(t)} WHERE ${raw.map((r) => `(${r})`).join(" AND ")}`;
      refuseWrite(sql);
      if (!isSingle(sql)) mockError("INVALID_INPUT", ADDED_STATEMENT_MESSAGE);
    }
    checkDatabase();
  };

  const filtered = (t: Table, filter: FilterGroup | null) => {
    const columns = columnsOf(t);
    const rows = rowsOf(account, t);
    return filter ? rows.filter((r) => matchesGroup(filter, r, columns)) : rows;
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
    const cols = byName(c.values);
    const names = cols.map(([k]) => quote(k)).join(", ");
    const values = cols.map(([, v]) => literal(v)).join(", ");
    // Snowflake refuses functions in VALUES: a VARIANT goes in through INSERT … SELECT.
    const fn = cols.some(([, v]) => v.t === "json" || v.t === "array" || v.t === "bytes");
    return fn
      ? `INSERT INTO ${qualified(t)} (${names}) SELECT ${values}`
      : `INSERT INTO ${qualified(t)} (${names}) VALUES (${values})`;
  };

  /** `plan_rows`: refused on a read-only profile, and by the features the changes need. */
  const plan = (req: ApplyRequest): { t: Table; order: StagedChange[]; sql: string[] } => {
    if (o.readOnly) mockError("UNSUPPORTED", READ_ONLY_EDIT);
    for (const c of req.changes) {
      if (c.op !== "insert" && c.key.kind !== "pk")
        mockError(
          "UNSUPPORTED",
          "unsupported: Snowflake: editing rows without a primary key is not available",
        );
      if (c.op !== "delete" && c.exprs && Object.keys(c.exprs).length > 0)
        mockError(
          "UNSUPPORTED",
          "unsupported: Snowflake: DEFAULT and server expressions is not available",
        );
    }
    const t = tableOf(req.table);
    const order = ordered(req.changes);
    return { t, order, sql: order.map((c) => statement(t, c)) };
  };

  const apply = (req: ApplyRequest): ApplyResult => {
    const { t, order, sql } = plan(req);
    for (const s of sql) refuseWrite(s);
    // A commit would join the editor's transaction on a shared session.
    if (o.shared && tx !== null) mockError("INVALID_INPUT", TRANSACTION_OPEN_MESSAGE);
    checkDatabase();
    if (t.kind !== "table")
      mockError(
        "DB_ERROR",
        `SQL compilation error:\nObject '${t.name}' does not exist or not authorized.`,
      );
    const before = t.rows.map((r) => r.slice());
    const results = new Map<string, RowResult>();
    for (const [i, c] of order.entries()) {
      let failure: ServerFailure | null = null;
      let matched = 1;
      try {
        if (c.op === "insert") {
          const row = t.cols.map((col) => {
            const v = c.values[col.name];
            if (v !== undefined) return coerce(t, col.name, v);
            return col.default !== undefined || col.autoincrement !== undefined
              ? defaultOf(t, col.name)
              : NULL;
          });
          failure = rowProblem(t, row);
          if (!failure) t.rows.push(row);
        } else {
          const at = rowsAt(t, c.key);
          matched = at.length;
          // `expect_affected = 1`: a key matching no row or two (keys are informational).
          if (matched === 1) {
            const k = at[0]!;
            if (c.op === "delete") t.rows.splice(k, 1);
            else {
              const row = t.rows[k]!.slice();
              for (const [name, v] of Object.entries(c.changes)) {
                const col = t.cols.findIndex((x) => x.name === name);
                if (col >= 0) row[col] = coerce(t, name, v);
              }
              failure = rowProblem(t, row);
              if (!failure) t.rows[k] = row;
            }
          }
        }
      } catch (err) {
        if (!(err instanceof ServerFailure)) throw err;
        failure = err;
      }
      if (failure || matched !== 1) {
        // One transaction: ROLLBACK undoes the changes before the failing one.
        t.rows = before;
        const error = failure
          ? serverError(failure, i)
          : {
              ...serverError(
                new ServerFailure(
                  "",
                  0,
                  matched === 0
                    ? GUARD_MESSAGE
                    : `expected to affect 1 row(s) but matched ${matched}; the row may have been changed or deleted by someone else`,
                ),
                i,
              ),
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
          durationMs: 240,
        };
      }
      results.set(c.rowId, { rowId: c.rowId, op: c.op, affected: 1, returned: null, error: null });
    }
    return {
      success: true,
      results: order.map((c) => results.get(c.rowId)!),
      sql,
      error: null,
      durationMs: 240,
    };
  };

  /** BEGIN / COMMIT / ROLLBACK of the editor; DDL would commit too (none runs here). */
  const followTransaction = (sql: string) => {
    const word = /^\s*(\w+)(?:\s+(\w+))?/.exec(sql);
    const first = word?.[1]?.toUpperCase();
    const second = word?.[2]?.toUpperCase();
    // `BEGIN` followed by statements is a Snowflake Scripting block, not a transaction.
    if (
      (first === "BEGIN" && [undefined, "TRANSACTION", "WORK", "NAME"].includes(second)) ||
      (first === "START" && second === "TRANSACTION")
    ) {
      tx ??= new Map(account.tables.map((t) => [t, t.rows.map((r) => r.slice())]));
    } else if (first === "COMMIT") tx = null;
    else if (first === "ROLLBACK") {
      for (const [t, rows] of tx ?? []) t.rows = rows;
      tx = null;
    }
  };

  const run = (sql: string, emit: (event: QueryEvent) => void) => {
    // `editor::run`: a read-only session's refusal, AUTOCOMMIT and, on a shared session, the
    // app's parameters (the case of quoted names, the input formats), before anything is sent.
    refuseWrite(sql);
    if (setsAutocommit(sql)) mockError("INVALID_INPUT", AUTOCOMMIT_MESSAGE);
    const pinned = o.shared ? setsAppParameter(sql) : null;
    if (pinned !== null) mockError("INVALID_INPUT", appParameterMessage(pinned));
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
      durationMs: 180,
      error: null,
      notices: [],
    };
    emit({ event: "started", data: { index: 0, sql, span } });
    let outcome: ReturnType<typeof runSql>;
    try {
      // One statement per request: Snowflake refuses a second one whole.
      if (!isSingle(sql)) throw new ServerFailure("0A000", 8, SEVERAL_MESSAGE);
      outcome = runSql(editor, sql);
    } catch (err) {
      if (!(err instanceof ServerFailure)) throw err;
      emit({ event: "failed", data: { index: 0, error: serverError(err, 0) } });
      return;
    }
    followTransaction(sql);
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

  const explain = (sql: string, mode: "plan" | "analyze"): ExplainResult => {
    if (mode === "analyze")
      mockError("UNSUPPORTED", "unsupported: Snowflake: EXPLAIN ANALYZE is not available");
    refuseWrite(sql);
    const outcome = (() => {
      try {
        return runSql({ ...editor }, `EXPLAIN USING TEXT ${sql}`);
      } catch (err) {
        if (err instanceof ServerFailure) return mockError("DB_ERROR", err.message);
        throw err;
      }
    })();
    const textOf = outcome.kind === "rows" ? outcome.rows[0]?.[0] : undefined;
    return {
      columns: outcome.kind === "rows" ? outcome.columns : [],
      rows: outcome.kind === "rows" ? outcome.rows : [],
      text: textOf?.t === "text" ? textOf.v : null,
      durationMs: 95,
    };
  };

  const processes = (): ProcessRow[] => {
    if (o.warehouse === null)
      mockError(
        "DB_ERROR",
        "No active warehouse selected in the current session.  Select an active warehouse with the 'use warehouse' command.",
      );
    return account.queries
      .filter((q) => q.user === o.user)
      .map((q) => ({
        id: q.id,
        user: q.user,
        database: null,
        application: q.warehouse,
        client: q.session,
        state: q.status,
        command: null,
        startedAt: q.start,
        durationMs: null,
        waitEvent: null,
        query: q.text,
        current: false,
        own: false,
      }));
  };

  const kill = (id: string, scope: "query" | "connection"): KillOutcome => {
    if (scope === "connection") return { id, ok: false, message: KILL_CONNECTION_MESSAGE };
    // `SYSTEM$CANCEL_QUERY` is a write for the classifier, as every `SYSTEM$` call is.
    if (o.readOnly) return { id, ok: false, message: READ_ONLY_MESSAGE };
    const at = account.queries.findIndex((q) => q.id === id.trim());
    if (at >= 0) account.queries.splice(at, 1);
    return { id, ok: true, message: null };
  };

  return {
    caps,
    currentDatabase: o.database,
    listDatabases: () =>
      account.databases.map((d) => ({
        name: d.name,
        owner: d.owner === "" ? null : d.owner,
        encoding: null,
        isCurrent: d.name === o.database,
      })),
    listSchemas: () => {
      const db = database();
      return account.schemas
        .filter((s) => s.database === db)
        .map((s) => ({ name: s.name, isDefault: s.name === o.schema }));
    },
    listTables: (schema) => {
      const db = database();
      const here = account.tables.filter((t) => t.database === db && t.schema === schema);
      const byName = (a: Table, b: Table) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
      // SHOW TABLES, then SHOW VIEWS.
      return [
        ...here.filter((t) => t.kind !== "view").sort(byName),
        ...here.filter((t) => t.kind === "view").sort(byName),
      ].map((t) => ({
        name: t.name,
        schema: t.schema,
        kind: t.kind,
        rowEstimate: t.kind === "view" ? null : t.rows.length,
        sizeBytes: t.kind === "view" ? null : bytesOf(t),
        comment: t.comment,
      }));
    },
    tableMeta: (ref) => {
      const t = tableOf(ref);
      const view = t.kind === "view";
      return {
        table: { schema: t.schema, name: t.name },
        kind: t.kind,
        columns: columnsOf(t),
        primaryKey: t.pk,
        indexes: view
          ? []
          : [
              ...(t.pk.length > 0
                ? [
                    {
                      name: `SYS_CONSTRAINT_PK_${t.name}`,
                      columns: t.pk,
                      unique: true,
                      primary: true,
                    },
                  ]
                : []),
              ...t.unique.map((u) => ({
                name: u.name,
                columns: u.columns,
                unique: true,
                primary: false,
              })),
            ].map((i) => ({
              ...i,
              method: null,
              predicate: null,
              definition: null,
              comment: null,
              include: [],
            })),
        foreignKeys: view ? [] : t.foreignKeys,
        triggers: [],
        comment: t.comment,
        rowEstimate: view ? null : t.rows.length,
        ddl: null,
        checks: [],
        options: null,
      };
    },
    tableDdl: (ref) => getDdl(tableOf(ref)),
    viewDefinition: (ref) => {
      const t = tableOf(ref);
      return t.kind === "view" ? (t.text ?? null) : null;
    },
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
        durationMs: 310,
      };
    },
    count: (ref, filter, exact) => {
      const t = tableOf(ref);
      // CountMode::Costly: the metadata's row count until an exact count is asked for (a count
      // needs a warehouse).
      if (exact !== true && !filter && t.kind === "table")
        return { count: t.rows.length, exact: false };
      checkRead(t, filter);
      return { count: filtered(t, filter).length, exact: true };
    },
    fetchCell: (ref, key, column) => {
      const t = tableOf(ref);
      checkDatabase();
      const at = rowsAt(t, key);
      const k = t.cols.findIndex((c) => c.name === column);
      if (at.length === 0 || k < 0) mockError("NOT_FOUND", "not found: the row is gone");
      return t.rows[at[0]!]![k] ?? NULL;
    },
    preview: (req) => plan(req).sql,
    apply,
    run,
    explain,
    listProcesses: processes,
    killProcesses: (ids, scope) => ids.map((id) => kill(id, scope)),
    listRoutines: (schema) =>
      account.routines
        .filter((r) => r.database === database() && r.schema === schema)
        .map(({ database: _d, definition: _def, ...summary }) => summary),
    routineDefinition: (ref) => {
      const r = account.routines.find(
        (x) => x.database === database() && x.schema === ref.schema && x.name === ref.name,
      );
      if (!r)
        mockError(
          "DB_ERROR",
          `SQL compilation error:\nObject '${ref.schema}.${ref.name}' does not exist or not authorized.`,
        );
      return r.definition;
    },
    sessionDetails: () => ({ tls: o.tls, tlsVersion: null, tlsCipher: null, encoding: "UTF-8" }),
  };
}
