import { classifyDialect } from "@/features/drivers/portable/classify";
import { PLUGIN_FILES_REFUSED, readsFiles } from "@/features/drivers/duckdb/files";
import { LEX, LOGGER_REFUSED, switchesLogger } from "@/features/drivers/duckdb/logger";
import type { ApplyRequest } from "../../../bindings/ApplyRequest";
import type { ApplyResult } from "../../../bindings/ApplyResult";
import type { BrowsePage } from "../../../bindings/BrowsePage";
import type { FilterGroup } from "../../../bindings/FilterGroup";
import type { QueryEvent } from "../../../bindings/QueryEvent";
import type { RowKey } from "../../../bindings/RowKey";
import type { RowResult } from "../../../bindings/RowResult";
import type { ServerExpr } from "../../../bindings/ServerExpr";
import type { SessionCaps } from "../../../bindings/SessionCaps";
import type { StagedChange } from "../../../bindings/StagedChange";
import type { StatementResult } from "../../../bindings/StatementResult";
import type { TableRef } from "../../../bindings/TableRef";
import type { Value } from "../../../bindings/Value";
import { matchesGroup, sortRows } from "../../rows";
import { mockError } from "../../transport";
import type { MockSession } from "../types";
import { columnsOf, type Database, NULL, type Table, viewRows } from "./data";
import { clockOf, defaultOf, EngineFailure, engineError, rowProblem, runSql } from "./sql";

/**
 * A session of the mock's DuckDB, mirroring `drivers/duckdb/session.rs`: the caps the backend
 * records in `src-tauri/tests/fixtures/duckdb/caps.json` (edits only where the file takes writes,
 * Switch Database only for a file, `ReadOnlyMode::Server` and `hardened` only for a read-only
 * profile), the attached database with its schemas, offset pages ordered by the sort (else the
 * key), estimates until asked to count, staged commits in one transaction that the first failing
 * change rolls back whole (each statement checked against its expected count), the editor's
 * statements, Show DDL, macros as routines and session details. Every session refuses a
 * statement that switches the logger; a read-only one refuses what the classifier calls a write,
 * before anything runs. The session has no calls of its own.
 */

/** The backend's `READ_ONLY_MESSAGE` (`commands::safety`). */
export const READ_ONLY_MESSAGE = "This connection is read-only";

/** `mutate::plan_rows` on a read-only profile (`AppError::Unsupported`). */
export const READ_ONLY_EDIT = "unsupported: this connection is read-only";

/** `drivers::check_affected`'s words for a change whose row is gone. */
export const GUARD_MESSAGE =
  "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else";

/** Every feature a session may have, in `Feature`'s order (as `SessionCaps::with` sorts them). */
const ORDER: SessionCaps["features"] = [
  "editor",
  "explain",
  "explainAnalyze",
  "cancelOnServer",
  "editRows",
  "insertRows",
  "deleteRows",
  "returning",
  "serverExpressions",
  "exactCount",
  "structureView",
  "tableDdl",
  "viewDefinition",
  "routines",
  "switchDatabase",
  "startupCommands",
  "sessionDetails",
  "mcp",
];

/** `session::EDIT_FEATURES`: only where the database file takes writes. */
export const EDIT_FEATURES: SessionCaps["features"] = [
  "editRows",
  "insertRows",
  "deleteRows",
  "returning",
  "serverExpressions",
];

/** `session::FILTER_OPS`: every filter, ILIKE and `json_contains` included. */
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
  "jsonContains",
  "jsonNotContains",
];

/** What the session knows of its instance (`session::InstanceFacts`). */
export interface InstanceFacts {
  /** `Instance::is_read_only`: what `ReadOnlyMode::Server` may claim. */
  serverReadOnly: boolean;
  hardened: boolean;
  /** `Instance::is_file_read_only`: a read-only profile, or an encrypted file. */
  fileReadOnly: boolean;
  /** A file, whose second sessions share the instance; an in-memory database is not. */
  shared: boolean;
  /** Safe Mode's passcode for a Command Line key, which nobody types. */
  passcode: boolean;
}

/** `DuckdbSession::capabilities`. */
export function duckdbCaps(f: InstanceFacts): SessionCaps {
  const features = ORDER.filter(
    (x) =>
      (f.fileReadOnly ? !EDIT_FEATURES.includes(x) : true) && (f.shared || x !== "switchDatabase"),
  );
  return {
    driver: "duckdb",
    flavor: null,
    dialect: "duckdb",
    language: "sql",
    features,
    namespaces: {
      databases: f.shared,
      schemas: "schemas",
      defaultSchema: "main",
      labels: { database: "Database", schema: "Schema", table: "Table", tables: "Tables" },
    },
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
      serverExprs: ["now", "currentTimestamp", "default"],
      maxParams: 10000,
    },
    safety: {
      readOnly: f.serverReadOnly ? "server" : "classifierOnly",
      hardened: f.hardened,
      confirm: f.passcode ? "passcode" : "password",
    },
    session: { keepaliveSecs: 30, pingBilled: false },
  };
}

/** `"name"` with `"` doubled (`double_quoted`). */
export const quote = (name: string) => `"${name.replaceAll('"', '""')}"`;

/** A value as DuckDB SQL text, for Preview SQL (blobs as `from_hex`). */
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
      return `from_hex('${[...atob(v.v.base64)].map((c) => c.charCodeAt(0).toString(16).padStart(2, "0").toUpperCase()).join("")}')`;
    case "array":
      return `'${JSON.stringify(v.v).replaceAll("'", "''")}'`;
    case "other":
      return `'${v.v.text.replaceAll("'", "''")}'`;
    default:
      return `'${v.v.replaceAll("'", "''")}'`;
  }
}

/** `server_expr` of the dialect: the SQL-standard clocks; null where DuckDB has none. */
function serverExprSql(e: ServerExpr): string | null {
  if (e === "now" || e === "currentTimestamp") return "CURRENT_TIMESTAMP";
  if (e === "currentTime") return "CURRENT_TIME";
  if (e === "localTimestamp") return "LOCALTIMESTAMP";
  // No CURRENT_DATE (ICU's, missing from the bundled build), no precision.
  return null;
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
export interface DuckdbOptions {
  db: Database;
  facts: InstanceFacts;
  /** `conn_options::effective_read_only` of the profile. */
  readOnly: boolean;
}

/** One session of the mock's DuckDB. */
export function duckdbSession(o: DuckdbOptions): MockSession {
  const caps = duckdbCaps(o.facts);
  const { db } = o;
  const has = (f: SessionCaps["features"][number]) => caps.features.includes(f);
  const unsupported = (what: string): never =>
    mockError("UNSUPPORTED", `unsupported: DuckDB: ${what} is not available`);
  const ctx = () => ({ db, hardened: o.facts.hardened, fileReadOnly: o.facts.fileReadOnly });

  const tableOf = (ref: TableRef): Table => {
    const schema = ref.schema ?? "main";
    return (
      db.tables.find((t) => t.schema === schema && t.name === ref.name) ??
      mockError("NOT_FOUND", `not found: ${schema}.${ref.name}`)
    );
  };
  const qualified = (t: Table) => `${quote(t.schema)}.${quote(t.name)}`;
  const rowsOf = (t: Table) => (t.kind === "view" ? viewRows(db, t) : t.rows);

  /** `admit`: the logger on every session, and a read-only session's writes. */
  const admit = (sql: string) => {
    if (switchesLogger(sql)) mockError("INVALID_INPUT", LOGGER_REFUSED);
    if (o.readOnly && classifyDialect(LEX, sql) === "write")
      mockError("INVALID_INPUT", READ_ONLY_MESSAGE);
  };

  /** The grid's reads go through `query`, so a raw filter meets `admit` too. */
  const checkRead = (t: Table, filter: FilterGroup | null) => {
    const raw = filter ? rawFilters(filter) : [];
    if (raw.length === 0) return;
    admit(`SELECT count(*) FROM ${qualified(t)} WHERE ${raw.map((r) => `(${r})`).join(" AND ")}`);
  };

  const filtered = (t: Table, filter: FilterGroup | null) => {
    const columns = columnsOf(t);
    const rows = rowsOf(t);
    return filter ? rows.filter((r) => matchesGroup(filter, r, columns)) : rows;
  };

  const whereKey = (key: RowKey): string =>
    key.kind === "pk"
      ? Object.entries(key.value)
          .map(([k, v]) => (v.t === "null" ? `${quote(k)} IS NULL` : `${quote(k)} = ${literal(v)}`))
          .join(" AND ")
      : "";

  const returning = (t: Table, on: boolean) =>
    on ? ` RETURNING ${t.cols.map((c) => quote(c.name)).join(", ")}` : "";

  /** A cell's SQL: its value, or its server expression (refused where DuckDB has none). */
  const cellSql = (c: string, v: Value | undefined, e: ServerExpr | undefined): string => {
    if (e !== undefined) {
      const sql = serverExprSql(e);
      if (sql === null) mockError("UNSUPPORTED", `unsupported: DuckDB SQL has no ${e} for "${c}"`);
      return sql;
    }
    return literal(v ?? NULL);
  };

  /** `build_update` / `build_insert` / `build_delete` of the portable builder, inlined. */
  const statement = (t: Table, c: StagedChange, back: boolean): string => {
    if (c.op === "delete") return `DELETE FROM ${qualified(t)} WHERE ${whereKey(c.key)}`;
    const exprs = c.exprs ?? {};
    if (c.op === "update") {
      const names = [...new Set([...Object.keys(c.changes), ...Object.keys(exprs)])].sort();
      const sets = names.map((k) => {
        if (exprs[k] === "default" || c.changes[k]?.t === "default")
          mockError(
            "UNSUPPORTED",
            "unsupported: DuckDB SQL cannot SET a column to DEFAULT; enter the value explicitly",
          );
        return `${quote(k)} = ${cellSql(k, c.changes[k], exprs[k])}`;
      });
      return `UPDATE ${qualified(t)} SET ${sets.join(", ")} WHERE ${whereKey(c.key)}${returning(t, back)}`;
    }
    // DEFAULT cells are left out: the column takes its default.
    const names = [...new Set([...Object.keys(c.values), ...Object.keys(exprs)])]
      .sort()
      .filter((k) => exprs[k] !== "default" && c.values[k]?.t !== "default");
    if (names.length === 0)
      return `INSERT INTO ${qualified(t)} DEFAULT VALUES${returning(t, back)}`;
    return `INSERT INTO ${qualified(t)} (${names.map(quote).join(", ")}) VALUES (${names
      .map((k) => cellSql(k, c.values[k], exprs[k]))
      .join(", ")})${returning(t, back)}`;
  };

  /** `plan_rows`: refused on a read-only profile, and by the features the changes need. */
  const plan = (req: ApplyRequest) => {
    if (o.readOnly) mockError("UNSUPPORTED", READ_ONLY_EDIT);
    for (const c of req.changes) {
      unsupportedUnless(
        c.op === "update" ? "editRows" : c.op === "insert" ? "insertRows" : "deleteRows",
      );
      if (c.op !== "insert" && c.key.kind !== "pk")
        unsupported("editing rows without a primary key");
      if (c.op !== "delete" && c.exprs && Object.keys(c.exprs).length > 0)
        unsupportedUnless("serverExpressions");
    }
    const t = tableOf(req.table);
    const back = req.returning && has("returning");
    const order = ordered(req.changes);
    return { t, order, back, sql: order.map((c) => statement(t, c, back)) };
  };
  function unsupportedUnless(f: SessionCaps["features"][number]) {
    if (!has(f))
      unsupported(
        {
          editRows: "editing rows",
          insertRows: "inserting rows",
          deleteRows: "deleting rows",
          serverExpressions: "DEFAULT and server expressions",
        }[f as string] ?? f,
      );
  }

  /** The written row as `RETURNING` gives it back. */
  const returned = (t: Table, row: Value[]) =>
    Object.fromEntries(t.cols.map((c, i) => [c.name, row[i] ?? NULL]));

  const apply = (req: ApplyRequest): ApplyResult => {
    const { t, order, back, sql } = plan(req);
    for (const s of sql) admit(s);
    const before = t.rows.map((r) => r.slice());
    const sequences = new Map(db.sequences);
    const results: RowResult[] = [];
    for (const [i, c] of order.entries()) {
      let failure: EngineFailure | null = null;
      let guard = false;
      let row: Value[] | null = null;
      if (t.kind !== "table")
        failure = new EngineFailure(`Binder Error: Can only update base table!`);
      else if (c.op === "insert") {
        const exprs = c.exprs ?? {};
        row = t.cols.map((col, k) => {
          const e = exprs[col.name];
          const v = c.values[col.name];
          if (e === "default" || v === undefined || v.t === "default") return defaultOf(db, t, k);
          if (e !== undefined) return clockOf(col.kind);
          return v;
        });
        failure = rowProblem(t, row);
        if (!failure) t.rows.push(row);
      } else {
        const at = rowAt(t, c.key);
        if (at < 0) guard = true;
        else if (c.op === "delete") t.rows.splice(at, 1);
        else {
          row = t.rows[at]!.slice();
          const exprs = c.exprs ?? {};
          for (const [k, col] of t.cols.entries()) {
            if (exprs[col.name] !== undefined) row[k] = clockOf(col.kind);
            else if (c.changes[col.name] !== undefined) row[k] = c.changes[col.name]!;
          }
          failure = rowProblem(t, row, at);
          if (!failure) t.rows[at] = row;
        }
      }
      if (failure || guard) {
        // One transaction: ROLLBACK undoes the changes before the failing one.
        t.rows = before;
        db.sequences = sequences;
        const error = engineError(failure ?? new EngineFailure(GUARD_MESSAGE), i);
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
          durationMs: 4,
        };
      }
      results.push({
        rowId: c.rowId,
        op: c.op,
        affected: 1,
        returned: back && row ? returned(t, row) : null,
        error: null,
      });
    }
    return { success: true, results, sql, error: null, durationMs: 4 };
  };

  const run = (sql: string, emit: (event: QueryEvent) => void) => {
    // `run_statement`: the logger and a read-only session's writes, before anything runs.
    admit(sql);
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
      durationMs: 2,
      error: null,
      notices: [],
    };
    emit({ event: "started", data: { index: 0, sql, span } });
    let outcome: ReturnType<typeof runSql>;
    try {
      outcome = runSql(ctx(), sql);
    } catch (err) {
      if (!(err instanceof EngineFailure)) throw err;
      emit({ event: "failed", data: { index: 0, error: engineError(err, 0) } });
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
    // `current_database()`: the catalog's name (the file's stem), not the path.
    currentDatabase: db.name,
    // `admit_plugin_run`: a hardened engine refuses files itself.
    admitPluginRun: (text) => {
      if (!o.facts.hardened && readsFiles(text)) mockError("INVALID_INPUT", PLUGIN_FILES_REFUSED);
    },
    listDatabases: () => [{ name: db.name, owner: null, encoding: null, isCurrent: true }],
    listSchemas: () => [...db.schemas].sort().map((name) => ({ name, isDefault: name === "main" })),
    listTables: (schema) =>
      db.tables
        .filter((t) => t.schema === schema)
        .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
        .map((t) => ({
          name: t.name,
          schema: t.schema,
          kind: t.kind,
          // `estimated_size` of a table; a view has none.
          rowEstimate: t.kind === "view" ? null : t.rows.length,
          sizeBytes: null,
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
        indexes: view ? [] : t.indexes,
        foreignKeys: view ? [] : t.foreignKeys,
        triggers: [],
        comment: t.comment,
        rowEstimate: view ? null : t.rows.length,
        ddl: t.sql,
        checks: view ? [] : t.checks,
        options: null,
      };
    },
    tableDdl: (ref) => {
      const t = tableOf(ref);
      const created = t.indexes.flatMap((i) =>
        i.definition?.startsWith("CREATE") ? [i.definition] : [],
      );
      return [t.sql, ...created].join("\n\n");
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
        // No row id: DuckDB's are not stable, so a table without a primary key is not editable.
        identity: t.pk.length > 0 ? "pk" : "none",
        rowIds: null,
        offset: req.offset,
        limit: req.limit,
        sql: `SELECT ${picked.map((c) => quote(c.name)).join(", ")} FROM ${qualified(t)}${order.length ? ` ORDER BY ${order.join(", ")}` : ""} LIMIT ${req.limit} OFFSET ${req.offset}`,
        durationMs: 3,
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
    listRoutines: (schema) =>
      db.macros
        .filter((m) => m.schema === schema)
        .map((m) => ({
          schema: m.schema,
          name: m.name,
          kind: "function",
          arguments: m.params.join(", "),
          // DROP MACRO drops every overload.
          identityArguments: "",
          returnType: m.table ? "TABLE" : "MACRO",
          language: "SQL",
          comment: m.comment,
        })),
    routineDefinition: (ref) => {
      const m = db.macros.find((x) => x.schema === (ref.schema ?? "main") && x.name === ref.name);
      if (!m) mockError("NOT_FOUND", `not found: macro ${ref.name}`);
      return `CREATE MACRO ${quote(m.schema)}.${quote(m.name)}(${m.params.map(quote).join(", ")}) AS ${m.table ? "TABLE " : ""}${m.body};`;
    },
    // An engine in this process: nothing to encrypt; DuckDB's text is UTF-8.
    sessionDetails: () => ({ tls: false, tlsVersion: null, tlsCipher: null, encoding: "UTF-8" }),
  };
}
