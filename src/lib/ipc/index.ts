import type { ApplyRequest } from "./bindings/ApplyRequest";
import type { ApplyResult } from "./bindings/ApplyResult";
import type { AppInfo } from "./bindings/AppInfo";
import type { BrowsePage } from "./bindings/BrowsePage";
import type { BrowseRequest } from "./bindings/BrowseRequest";
import type { FilterGroup } from "./bindings/FilterGroup";
import type { RowCount } from "./bindings/RowCount";
import type { RowKey } from "./bindings/RowKey";
import type { Value } from "./bindings/Value";
import type { CompletionMetadata } from "./bindings/CompletionMetadata";
import type { ConnStatus } from "./bindings/ConnStatus";
import type { ConnectionInfo } from "./bindings/ConnectionInfo";
import type { ConnectionProfile } from "./bindings/ConnectionProfile";
import type { ConnectionTag } from "./bindings/ConnectionTag";
import type { ConnectionsFilePreview } from "./bindings/ConnectionsFilePreview";
import type { ExportConnectionsRequest } from "./bindings/ExportConnectionsRequest";
import type { DatabaseInfo } from "./bindings/DatabaseInfo";
import type { ProfileInput } from "./bindings/ProfileInput";
import type { SchemaInfo } from "./bindings/SchemaInfo";
import type { SecretStatus } from "./bindings/SecretStatus";
import type { TableMeta } from "./bindings/TableMeta";
import type { StructureChoices } from "./bindings/StructureChoices";
import type { TableRef } from "./bindings/TableRef";
import type { TableSummary } from "./bindings/TableSummary";
import type { TestResult } from "./bindings/TestResult";
import type { DdlOp } from "./bindings/DdlOp";
import type { DdlPlan } from "./bindings/DdlPlan";
import type { DdlResult } from "./bindings/DdlResult";
import type { ExplainMode } from "./bindings/ExplainMode";
import type { ExplainResult } from "./bindings/ExplainResult";
import type { ExportProgress } from "./bindings/ExportProgress";
import type { ExportRequest } from "./bindings/ExportRequest";
import type { ExportResult } from "./bindings/ExportResult";
import type { ExportTablesProgress } from "./bindings/ExportTablesProgress";
import type { ExportTablesRequest } from "./bindings/ExportTablesRequest";
import type { ExportTablesResult } from "./bindings/ExportTablesResult";
import type { CsvImportProgress } from "./bindings/CsvImportProgress";
import type { CsvImportReport } from "./bindings/CsvImportReport";
import type { CsvImportRequest } from "./bindings/CsvImportRequest";
import type { CsvPreview } from "./bindings/CsvPreview";
import type { CsvPreviewRequest } from "./bindings/CsvPreviewRequest";
import type { SqlDumpPreview } from "./bindings/SqlDumpPreview";
import type { SqlImportProgress } from "./bindings/SqlImportProgress";
import type { SqlImportReport } from "./bindings/SqlImportReport";
import type { SqlImportRequest } from "./bindings/SqlImportRequest";
import type { BackupEvent } from "./bindings/BackupEvent";
import type { BackupJobRequest } from "./bindings/BackupJobRequest";
import type { BackupJobResult } from "./bindings/BackupJobResult";
import type { BackupTarget } from "./bindings/BackupTarget";
import type { Driver } from "./bindings/Driver";
import type { DumpBinary } from "./bindings/DumpBinary";
import type { RestoreFileInfo } from "./bindings/RestoreFileInfo";
import type { HistoryEntry } from "./bindings/HistoryEntry";
import type { HistoryFilter } from "./bindings/HistoryFilter";
import type { QueryFolder } from "./bindings/QueryFolder";
import type { QueryFolderInput } from "./bindings/QueryFolderInput";
import type { SavedQuery } from "./bindings/SavedQuery";
import type { SavedQueryInput } from "./bindings/SavedQueryInput";
import type { SavedQueryLibrary } from "./bindings/SavedQueryLibrary";
import type { QueryEvent } from "./bindings/QueryEvent";
import type { RunOptions } from "./bindings/RunOptions";
import type { StatementResult } from "./bindings/StatementResult";
import type { ObjectOp } from "./bindings/ObjectOp";
import type { OverviewItem } from "./bindings/OverviewItem";
import type { RoutineRef } from "./bindings/RoutineRef";
import type { RoutineSummary } from "./bindings/RoutineSummary";
import type { SequenceSummary } from "./bindings/SequenceSummary";
import type { AdminDdl } from "./bindings/AdminDdl";
import type { AdminResult } from "./bindings/AdminResult";
import type { DatabaseOptions } from "./bindings/DatabaseOptions";
import type { KillOutcome } from "./bindings/KillOutcome";
import type { KillScope } from "./bindings/KillScope";
import type { ProcessRow } from "./bindings/ProcessRow";
import type { UserChange } from "./bindings/UserChange";
import type { UserDetail } from "./bindings/UserDetail";
import type { UserRef } from "./bindings/UserRef";
import type { UserSummary } from "./bindings/UserSummary";
// Read lazily (inside `query.run`): the prefs store itself imports this facade.
import { getPrefs } from "@/stores/prefs";
import {
  invoke,
  makeChannel,
  reportConnectionFailure,
  toIpcError,
  type ErrorPayload,
} from "./transport";

export {
  IpcError,
  isConnectionFailure,
  isMock,
  isTauri,
  onConnectionFailure,
  reportConnectionFailure,
  toIpcError,
} from "./transport";
export type {
  ApplyRequest,
  ApplyResult,
  AppInfo,
  Value,
  BrowsePage,
  BrowseRequest,
  FilterGroup,
  RowCount,
  RowKey,
  CompletionMetadata,
  ConnStatus,
  ConnectionInfo,
  ConnectionProfile,
  ConnectionTag,
  DatabaseInfo,
  ProfileInput,
  SchemaInfo,
  SecretStatus,
  TableMeta,
  TableRef,
  TableSummary,
  TestResult,
};
export type { ColumnMeta } from "./bindings/ColumnMeta";
export type { Driver } from "./bindings/Driver";
export type { SslMode } from "./bindings/SslMode";
export type { SshAuth } from "./bindings/SshAuth";
export type { SshConfig } from "./bindings/SshConfig";
export type { SafeModeLevel } from "./bindings/SafeModeLevel";
export type { PasswordMode } from "./bindings/PasswordMode";
export type { ExportConnectionsRequest } from "./bindings/ExportConnectionsRequest";
export type { ConnectionsFilePreview } from "./bindings/ConnectionsFilePreview";
export type { ImportPreviewItem } from "./bindings/ImportPreviewItem";
export type { TableKind } from "./bindings/TableKind";
export type { ValueKind } from "./bindings/ValueKind";
export type { FilterNode } from "./bindings/FilterNode";
export type { FilterOp } from "./bindings/FilterOp";
export type { Conjunction } from "./bindings/Conjunction";
export type { SortSpec } from "./bindings/SortSpec";
export type { SortDir } from "./bindings/SortDir";
export type { RowIdentity } from "./bindings/RowIdentity";
export type { ForeignKeyMeta } from "./bindings/ForeignKeyMeta";
export type { Dialect } from "./bindings/Dialect";
export type { StagedChange } from "./bindings/StagedChange";
export type {
  BackupEvent,
  BackupJobRequest,
  BackupJobResult,
  BackupTarget,
  DumpBinary,
  RestoreFileInfo,
};
export type { BackupJobKind } from "./bindings/BackupJobKind";
export type { BackupLogLine } from "./bindings/BackupLogLine";
export type { BackupLogStream } from "./bindings/BackupLogStream";
export type { DumpTool } from "./bindings/DumpTool";
export type { ParsedVersion } from "./bindings/ParsedVersion";
export type { RestoreFileKind } from "./bindings/RestoreFileKind";
export type { VersionFlavor } from "./bindings/VersionFlavor";
export type { RowResult } from "./bindings/RowResult";
export type { ConnectionStatusEvent } from "./bindings/ConnectionStatusEvent";
export type { TunnelStatusEvent } from "./bindings/TunnelStatusEvent";
export type { ErrorPayload } from "./bindings/ErrorPayload";
export type { ResultColumn } from "./bindings/ResultColumn";
export type { ColumnOrigin } from "./bindings/ColumnOrigin";
export type { StatementSpan } from "./bindings/StatementSpan";
export type { StatementKind } from "./bindings/StatementKind";
export type { ColumnDef } from "./bindings/ColumnDef";
export type { IndexDef } from "./bindings/IndexDef";
export type { FkDef } from "./bindings/FkDef";
export type { IndexMeta } from "./bindings/IndexMeta";
export type { TriggerMeta } from "./bindings/TriggerMeta";
export type { CheckMeta } from "./bindings/CheckMeta";
export type { CheckDef } from "./bindings/CheckDef";
export type { TableOptions } from "./bindings/TableOptions";
export type { CharsetInfo } from "./bindings/CharsetInfo";
export type { UserTypeInfo } from "./bindings/UserTypeInfo";
export type { StructureChoices } from "./bindings/StructureChoices";
export type { ObjectOp, OverviewItem, RoutineRef, RoutineSummary, SequenceSummary };
export type { ObjectTarget } from "./bindings/ObjectTarget";
export type { RoutineKind } from "./bindings/RoutineKind";
export type {
  AdminDdl,
  AdminResult,
  DatabaseOptions,
  KillOutcome,
  KillScope,
  ProcessRow,
  UserChange,
  UserDetail,
  UserRef,
  UserSummary,
};
export type { CollationOption } from "./bindings/CollationOption";
export type { EncodingOption } from "./bindings/EncodingOption";
export type { ObjectPrivileges } from "./bindings/ObjectPrivileges";
export type { ResourceLimits } from "./bindings/ResourceLimits";
export type { RoleAttributes } from "./bindings/RoleAttributes";
export type { UserState } from "./bindings/UserState";
export type { ExportSource } from "./bindings/ExportSource";
export type { ExportFormat } from "./bindings/ExportFormat";
export type {
  CsvImportProgress,
  CsvImportReport,
  CsvImportRequest,
  CsvPreview,
  CsvPreviewRequest,
  ExportTablesProgress,
  ExportTablesRequest,
  ExportTablesResult,
  SqlDumpPreview,
  SqlImportProgress,
  SqlImportReport,
  SqlImportRequest,
};
export type { CsvQuote } from "./bindings/CsvQuote";
export type { ExportLayout } from "./bindings/ExportLayout";
export type { ExportedTable } from "./bindings/ExportedTable";
export type { CsvColumn } from "./bindings/CsvColumn";
export type { CsvFormat } from "./bindings/CsvFormat";
export type { CsvImportFailure } from "./bindings/CsvImportFailure";
export type { ImportColumnDef } from "./bindings/ImportColumnDef";
export type { ImportPhase } from "./bindings/ImportPhase";
export type { ImportTarget } from "./bindings/ImportTarget";
export type { ImportType } from "./bindings/ImportType";
export type { InsertMode } from "./bindings/InsertMode";
export type { SqlImportError } from "./bindings/SqlImportError";
export type { TextEncoding } from "./bindings/TextEncoding";
export type { QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary };
export type {
  DdlOp,
  DdlPlan,
  DdlResult,
  ExplainMode,
  ExplainResult,
  ExportProgress,
  ExportRequest,
  ExportResult,
  HistoryEntry,
  HistoryFilter,
  QueryEvent,
  RunOptions,
  StatementResult,
};

/**
 * Reports in-band errors of one command run (`reportConnectionFailure`), at most one per run: a
 * `failed` event and the matching `StatementResult.error` carry the same error.
 */
function inBandFailureReporter(
  cmd: string,
  connectionId: string,
): (error: ErrorPayload | null | undefined) => void {
  let reported = false;
  return (error) => {
    if (!reported) reported = reportConnectionFailure(cmd, { connectionId }, error);
  };
}

// ---- SQL log (the console, ⇧⌘C) ------------------------------------------------------------

/** What a logged statement did: read or wrote rows, read or changed the schema, or editor SQL. */
export type SqlLogCategory = "data" | "structure" | "query";

/**
 * One statement (or batch) that a facade call ran. `sql` is the text the backend reports having
 * executed; a command that does not return its SQL is logged as a `-- …` description of the
 * request instead (`meta`), never as made-up SQL.
 */
export interface SqlLogEvent {
  connectionId: string;
  category: SqlLogCategory;
  sql: string;
  /** `sql` describes the request (`-- list tables in public`); it is not the SQL that ran. */
  meta: boolean;
  /** Short tag shown with the entry: how `sql` was wrapped (`EXPLAIN`), `ROLLED BACK`… */
  label?: string;
  /** The backend's timing when it reports one, else the IPC round trip. */
  durationMs?: number;
  /** Rows read (or written, for an export). */
  rows?: number;
  /** Rows changed. */
  affected?: number;
  error?: string;
}

type SqlLogListener = (event: SqlLogEvent) => void;
const sqlLogListeners = new Set<SqlLogListener>();

/** Calls `listener` for every statement run through the facade. Returns the unsubscribe function. */
export function onSqlLog(listener: SqlLogListener): () => void {
  sqlLogListeners.add(listener);
  return () => void sqlLogListeners.delete(listener);
}

function logSql(event: SqlLogEvent): void {
  for (const listener of [...sqlLogListeners]) {
    try {
      listener(event);
    } catch {
      // Logging must never change a command's outcome.
    }
  }
}

type LogFields = Omit<SqlLogEvent, "connectionId" | "category">;

const elapsed = (t0: number) => Math.round(performance.now() - t0);

/** SQL the backend reports having run (several statements are shown one per line). */
function executed(sql: string | string[], fields: Partial<LogFields> = {}): LogFields {
  const text = Array.isArray(sql) ? sql.map((s) => s.trim().replace(/;$/, "")).join(";\n") : sql;
  return { ...fields, sql: text, meta: false };
}

/** A `-- …` description for a command that does not report the SQL it ran. */
function described(text: string): LogFields {
  return { sql: `-- ${text}`, meta: true };
}

function tableName(t: TableRef): string {
  return t.schema ? `${t.schema}.${t.name}` : t.name;
}

const EXPORT_FORMAT = {
  csv: "CSV",
  json: "JSON",
  sqlInsert: "SQL INSERTs",
  xlsx: "Excel",
} as const;

/** How an export is logged: its SQL, or a description of the table; result rows run nothing. */
function exportRequest(req: ExportRequest): LogFields | null {
  const { source } = req;
  if (source.kind === "rows") return null;
  if (source.kind === "sql") return { sql: source.sql, meta: false, label: "EXPORT" };
  return described(`export ${tableName(source.table)} as ${EXPORT_FORMAT[req.format.format]}`);
}

function explainLabel(mode: ExplainMode | null | undefined): string | undefined {
  if (!mode) return undefined;
  return mode === "analyze" ? "EXPLAIN ANALYZE" : "EXPLAIN";
}

/** `f()`, or null when it throws: logging never fails a call (nor a test double of odd shape). */
function safely<T>(f: () => T): T | null {
  try {
    return f();
  } catch {
    return null;
  }
}

/**
 * Runs `call` and logs it under `category`: the `request` entry with the error when the call
 * rejects; on success, `request` with the round-trip time, overridden by what `outcome` makes of
 * the result. `request` or `outcome` returning `null` logs nothing.
 */
async function logged<T>(
  connectionId: string,
  category: SqlLogCategory,
  request: () => LogFields | null,
  call: () => Promise<T>,
  outcome?: (result: T) => Partial<LogFields> | null,
): Promise<T> {
  const t0 = performance.now();
  let result: T;
  try {
    result = await call();
  } catch (err) {
    const fields = safely(request);
    if (fields) {
      const error = toIpcError(err).message;
      logSql({ connectionId, category, ...fields, durationMs: elapsed(t0), error });
    }
    throw err;
  }
  const fields = safely(request);
  const extra = outcome ? safely(() => outcome(result)) : {};
  if (fields && extra)
    logSql({ connectionId, category, ...fields, durationMs: elapsed(t0), ...extra });
  return result;
}

/**
 * Logs the statements of one `run_sql` run from its streamed events, each as it finishes or
 * fails (a long script fills the log statement by statement). `rejected` covers a command that
 * failed as a whole: the statements still running, or the submitted text if none had started.
 */
function runLogger(connectionId: string, text: string, options: Partial<RunOptions> | undefined) {
  const label = explainLabel(options?.explain);
  const running = new Map<number, { sql: string; t0: number }>();
  let started = false;
  const log = (fields: LogFields) => logSql({ connectionId, category: "query", label, ...fields });
  return {
    event(e: QueryEvent) {
      try {
        if (e.event === "started") {
          started = true;
          running.set(e.data.index, { sql: e.data.sql, t0: performance.now() });
        } else if (e.event === "finished") {
          const r = e.data.result;
          running.delete(e.data.index);
          log(
            executed(r.sql, {
              durationMs: r.durationMs,
              rows: r.kind === "rows" || r.kind === "explain" ? r.rowCount : undefined,
              affected: r.affected ?? undefined,
              error: r.error?.message,
            }),
          );
        } else if (e.event === "failed") {
          const st = running.get(e.data.index);
          running.delete(e.data.index);
          log(
            executed(st?.sql ?? text, {
              durationMs: st ? elapsed(st.t0) : undefined,
              error: e.data.error.message,
            }),
          );
        }
      } catch {
        // See `safely`.
      }
    },
    rejected(err: unknown) {
      const error = toIpcError(err).message;
      if (!started) log(executed(text, { error }));
      for (const st of running.values())
        log(executed(st.sql, { durationMs: elapsed(st.t0), error }));
      running.clear();
    },
  };
}

/** The query time out preference in seconds (0 = none), for every `run_sql`. */
function queryTimeoutSecs(): number {
  return safely(() => getPrefs().queryTimeout) ?? 0;
}

/**
 * Logs an admin change (database / schema DDL, user management) like `ddl.apply`: the statements
 * that ran (passwords already masked by the backend), then the failure on its own.
 */
async function loggedAdmin(
  connectionId: string,
  cmd: string,
  request: () => LogFields,
  call: () => Promise<AdminResult>,
): Promise<AdminResult> {
  const result = await logged(connectionId, "structure", request, call, (r) =>
    r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null,
  );
  const failed = result.error ? safely(request) : null;
  if (result.error && failed)
    logSql({
      connectionId,
      category: "structure",
      ...failed,
      durationMs: result.durationMs,
      error: result.error.message,
    });
  reportConnectionFailure(cmd, { connectionId }, result.error);
  return result;
}

const ADMIN_DDL_TEXT: Record<AdminDdl["op"], string> = {
  createDatabase: "create database",
  renameDatabase: "rename database",
  dropDatabase: "drop database",
  createSchema: "create schema",
  dropSchema: "drop schema",
};

function userName(change: UserChange): string {
  const u = change.edited ?? change.original;
  if (!u) return "user";
  return u.host !== null ? `'${u.name}'@'${u.host}'` : u.name;
}

/** Typed facade over every Tauri command. The renderer never calls `invoke` directly. */
export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),

  app: {
    /** Quits for real (the renderer already handled unsaved work); skips the exit guard. */
    quit: () => invoke<void>("quit_app"),
    /**
     * Tells the backend whether any tab holds unsaved work (a Dock / system quit is answered from
     * this without waiting for the renderer); also acknowledges `app:quit-requested`.
     */
    setUnsaved: (unsaved: boolean) => invoke<void>("set_unsaved", { unsaved }),
  },

  menu: {
    /**
     * Rebinds the native menu items' accelerators (menu item id = action id → Tauri accelerator,
     * null for none) after the user remapped them in Settings ▸ Shortcuts.
     */
    setAccelerators: (accelerators: Record<string, string | null>) =>
      invoke<void>("set_menu_accelerators", { accelerators }),
  },

  settings: {
    get: () => invoke<Record<string, unknown>>("get_settings"),
    set: (key: string, value: unknown) => invoke<void>("set_setting", { key, value }),
  },

  profiles: {
    list: () => invoke<ConnectionProfile[]>("list_profiles"),
    get: (id: string) => invoke<ConnectionProfile>("get_profile", { id }),
    save: (input: ProfileInput) => invoke<ConnectionProfile>("save_profile", { input }),
    delete: (id: string) => invoke<void>("delete_profile", { id }),
    duplicate: (id: string) => invoke<ConnectionProfile>("duplicate_profile", { id }),
    reorder: (ids: string[]) => invoke<void>("reorder_profiles", { ids }),
    secretStatus: (id: string) => invoke<SecretStatus>("get_secret_status", { id }),
    test: (input: ProfileInput) => invoke<TestResult>("test_connection", { input }),
    /** Creates an empty SQLite database at `path`; an existing file is left untouched. */
    createSqliteFile: (path: string) => invoke<void>("create_sqlite_database", { path }),
    /** Puts the profiles in `group` (null: no group); returns them as stored. */
    moveToGroup: (ids: string[], group: string | null) =>
      invoke<ConnectionProfile[]>("move_profiles_to_group", { ids, group }),
    /** Writes a `.tablerconnections` file; returns how many connections it holds. */
    exportConnections: (request: ExportConnectionsRequest) =>
      invoke<number>("export_connections", { request }),
    /** Reads a connections file for the import preview (`needsPassword` when it is encrypted). */
    inspectConnectionsFile: (path: string, password: string | null = null) =>
      invoke<ConnectionsFilePreview>("inspect_connections_file", { path, password }),
    /** Imports the chosen connections (ids as in the file) as new profiles. */
    importConnections: (path: string, password: string | null, ids: string[]) =>
      invoke<ConnectionProfile[]>("import_connections", { path, password, ids }),
  },

  connections: {
    /** `password`: typed in the prompt of an "Ask every time" profile (never stored). */
    connect: (profileId: string, password?: string | null) =>
      invoke<ConnectionInfo>("connect", { profileId, password: password ?? null }),
    disconnect: (connectionId: string) => invoke<void>("disconnect", { connectionId }),
    list: () => invoke<ConnectionInfo[]>("list_connections"),
    ping: (connectionId: string) => invoke<ConnStatus>("ping", { connectionId }),
    acceptHostKey: (profileId: string, fingerprint: string, persistToKnownHosts = false) =>
      invoke<void>("accept_host_key", { profileId, fingerprint, persistToKnownHosts }),
    switchDatabase: (connectionId: string, database: string) =>
      invoke<ConnectionInfo>("switch_database", { connectionId, database }),
  },

  browse: {
    rows: (connectionId: string, req: BrowseRequest) =>
      logged(
        connectionId,
        "data",
        () => described(`browse ${tableName(req.table)}`),
        () => invoke<BrowsePage>("browse_table", { connectionId, req }),
        (page) => executed(page.sql, { durationMs: page.durationMs, rows: page.rows.length }),
      ),
    /**
     * Counts rows; an unfiltered table estimated above `estimateAbove` rows (backend default
     * 500,000) reports the planner estimate unless `exact`.
     */
    count: (
      connectionId: string,
      table: TableRef,
      filter: FilterGroup | null,
      exact = false,
      estimateAbove: number | null = null,
    ) =>
      logged(
        connectionId,
        "data",
        () => described(`count rows of ${tableName(table)}${filter ? " (filtered)" : ""}`),
        () =>
          invoke<RowCount>("count_table_rows", {
            connectionId,
            table,
            filter,
            exact,
            estimateAbove,
          }),
        (c) => ({
          ...(c.exact ? {} : described(`estimate the row count of ${tableName(table)}`)),
          rows: c.count,
        }),
      ),
    cell: (connectionId: string, table: TableRef, key: RowKey, column: string) =>
      logged(
        connectionId,
        "data",
        () => described(`fetch the full value of ${column} in ${tableName(table)}`),
        () => invoke<Value>("fetch_cell", { connectionId, table, key, column }),
      ),
  },

  query: {
    /**
     * Runs one or more statements on the connection's editor session, streaming events. Statement
     * errors come back in-band (`failed` events, `StatementResult.error`), so a dropped connection
     * is reported from there (once per run) to `onConnectionFailure`.
     */
    run: async (
      connectionId: string,
      queryId: string,
      sql: string,
      options: Partial<RunOptions>,
      onEvent: (e: QueryEvent) => void,
    ) => {
      const report = inBandFailureReporter("run_sql", connectionId);
      const log = runLogger(connectionId, sql, options);
      let results: StatementResult[];
      try {
        results = await invoke<StatementResult[]>("run_sql", {
          connectionId,
          queryId,
          sql,
          options: {
            maxRows: 10_000,
            stopOnError: true,
            explain: null,
            // Settings ▸ Connections ▸ Query time out (the backend cancels a statement past it).
            timeoutSecs: queryTimeoutSecs(),
            ...options,
          },
          onEvent: await makeChannel<QueryEvent>((e) => {
            if (e.event === "failed") report(e.data.error);
            log.event(e);
            onEvent(e);
          }),
        });
      } catch (err) {
        log.rejected(err);
        throw err;
      }
      for (const r of results) report(r.error);
      return results;
    },
    cancel: (connectionId: string, queryId: string) =>
      invoke<void>("cancel_query", { connectionId, queryId }),
    /** Logged as the submitted statement, labelled with the EXPLAIN form the backend wraps it in. */
    explain: (connectionId: string, sql: string, mode: ExplainMode, queryId?: string) =>
      logged(
        connectionId,
        "query",
        () => ({ sql, meta: false, label: explainLabel(mode) }),
        () =>
          invoke<ExplainResult>("explain", { connectionId, sql, mode, queryId: queryId ?? null }),
        (r) => ({ durationMs: r.durationMs, rows: r.rows.length }),
      ),
  },

  history: {
    list: (filter: Partial<HistoryFilter> = {}) =>
      invoke<HistoryEntry[]>("list_history", {
        filter: { profileId: null, search: null, limit: null, beforeId: null, ...filter },
      }),
    delete: (ids: number[]) => invoke<void>("delete_history", { ids }),
    clear: (profileId: string | null = null) => invoke<void>("clear_history", { profileId }),
  },

  /** Saved queries (the sidebar's Queries pane) and their folders, in the app DB. */
  savedQueries: {
    list: () => invoke<SavedQueryLibrary>("list_saved_queries"),
    save: (input: SavedQueryInput) => invoke<SavedQuery>("save_saved_query", { input }),
    delete: (id: string) => invoke<void>("delete_saved_query", { id }),
    saveFolder: (input: QueryFolderInput) => invoke<QueryFolder>("save_query_folder", { input }),
    /** Also deletes the folders and queries inside it. */
    deleteFolder: (id: string) => invoke<void>("delete_query_folder", { id }),
  },

  ddl: {
    preview: (connectionId: string, table: TableRef | null, ops: DdlOp[]) =>
      invoke<DdlPlan>("preview_ddl", { connectionId, table, ops }),
    /** A failure comes back in `DdlResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, table: TableRef | null, ops: DdlOp[]) => {
      const request = () =>
        described(table ? `apply schema changes to ${tableName(table)}` : "apply schema changes");
      // The statements whose effects persist; a failure is logged on its own after them (the
      // failing statement is not part of `executed`, and a transactional plan rolls back).
      const result = await logged(
        connectionId,
        "structure",
        request,
        () => invoke<DdlResult>("apply_ddl", { connectionId, table, ops }),
        (r) => (r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null),
      );
      const { error, durationMs } = result;
      const failed = error ? safely(request) : null;
      if (error && failed)
        logSql({
          connectionId,
          category: "structure",
          ...failed,
          durationMs,
          error: error.message,
        });
      reportConnectionFailure("apply_ddl", { connectionId }, result.error);
      return result;
    },
    tableDdl: (connectionId: string, table: TableRef) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the DDL of ${tableName(table)}`),
        () => invoke<string>("get_table_ddl", { connectionId, table }),
      ),
  },

  exports: {
    run: async (
      connectionId: string,
      req: ExportRequest,
      onProgress: (p: ExportProgress) => void,
    ) => {
      const channel = await makeChannel<ExportProgress>(onProgress);
      return logged(
        connectionId,
        "data",
        () => exportRequest(req),
        () => invoke<ExportResult>("export_data", { connectionId, req, onProgress: channel }),
        (r) => ({ durationMs: r.durationMs, rows: r.rows }),
      );
    },
    cancel: (exportId: string) => invoke<void>("cancel_export", { exportId }),
    /** Several tables in full (one file each, or one SQL script / workbook); `cancel` stops it. */
    runTables: async (
      connectionId: string,
      req: ExportTablesRequest,
      onProgress: (p: ExportTablesProgress) => void,
    ) => {
      const channel = await makeChannel<ExportTablesProgress>(onProgress);
      return logged(
        connectionId,
        "data",
        () =>
          described(
            `export ${req.tables.length} table${req.tables.length === 1 ? "" : "s"} as ${EXPORT_FORMAT[req.format.format]}`,
          ),
        () =>
          invoke<ExportTablesResult>("export_tables", { connectionId, req, onProgress: channel }),
        (r) => ({ durationMs: r.durationMs, rows: r.rows }),
      );
    },
  },

  // File ▸ Import: the CSV wizard and SQL dumps. `cancel` stops either by its import id.
  imports: {
    previewCsv: (req: CsvPreviewRequest) => invoke<CsvPreview>("preview_csv_import", { req }),
    csv: async (
      connectionId: string,
      req: CsvImportRequest,
      onProgress: (p: CsvImportProgress) => void,
    ) => {
      const channel = await makeChannel<CsvImportProgress>(onProgress);
      return logged(
        connectionId,
        "data",
        () => described(`import CSV into ${tableName(req.target.table)}`),
        () => invoke<CsvImportReport>("import_csv", { connectionId, req, onProgress: channel }),
        (r) => ({
          durationMs: r.durationMs,
          affected: r.rowsImported,
          error: r.failure?.message,
        }),
      );
    },
    previewSql: (connectionId: string, path: string) =>
      invoke<SqlDumpPreview>("preview_sql_dump", { connectionId, req: { path } }),
    sql: async (
      connectionId: string,
      req: SqlImportRequest,
      onProgress: (p: SqlImportProgress) => void,
    ) => {
      const channel = await makeChannel<SqlImportProgress>(onProgress);
      return logged(
        connectionId,
        "query",
        () => described(`import SQL dump ${req.path.slice(req.path.lastIndexOf("/") + 1)}`),
        () =>
          invoke<SqlImportReport>("import_sql_dump", { connectionId, req, onProgress: channel }),
        (r) => ({
          durationMs: r.durationMs,
          error: r.errors[0]?.message,
        }),
      );
    },
    cancel: (importId: string) => invoke<void>("cancel_import", { importId }),
  },

  // Backup / restore through pg_dump, pg_restore, psql, mysqldump and mysql (File ▸ Backup… /
  // Restore…). Jobs run against a saved connection (profile), not an open connection.
  backup: {
    /** The driver's dump / restore binaries found on this Mac, newest version first per tool. */
    binaries: (driver: Driver) => invoke<DumpBinary[]>("backup_list_binaries", { driver }),
    /** Server version and databases, through an open connection or a temporary one. */
    inspectTarget: (profileId: string) =>
      invoke<BackupTarget>("backup_inspect_target", { profileId }),
    /** What a restore file holds (by its first bytes). */
    inspectFile: (path: string) => invoke<RestoreFileInfo>("backup_inspect_file", { path }),
    run: async (req: BackupJobRequest, onEvent: (e: BackupEvent) => void) =>
      invoke<BackupJobResult>("backup_run", {
        req,
        onEvent: await makeChannel<BackupEvent>(onEvent),
      }),
    cancel: (jobId: string) => invoke<void>("backup_cancel", { jobId }),
    /** SQLite: copies the profile's database file to `path`; resolves with its size. */
    copySqlite: (profileId: string, path: string) =>
      invoke<number>("backup_copy_sqlite", { profileId, path }),
  },

  safety: {
    /** Whether `password` is the profile's stored DB password (Safe Mode prompts). */
    verifyPassword: (profileId: string, password: string) =>
      invoke<boolean>("verify_connection_password", { profileId, password }),
  },

  files: {
    readText: (path: string) => invoke<string>("read_text_file", { path }),
    writeText: (path: string, content: string) =>
      invoke<void>("write_text_file", { path, content }),
    /** A whole file as a bytes value (Set Value ▸ Add a File…); at most 64 MB. */
    readBinary: (path: string) => invoke<Value>("read_binary_file", { path }),
  },

  mutate: {
    preview: (connectionId: string, req: ApplyRequest) =>
      invoke<string[]>("preview_changes", { connectionId, req }),
    /** A failure comes back in `ApplyResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, req: ApplyRequest) => {
      // One transaction: a failure rolls every statement back.
      const result = await logged(
        connectionId,
        "data",
        () => described(`apply changes to ${tableName(req.table)}`),
        () => invoke<ApplyResult>("apply_changes", { connectionId, req }),
        (r) =>
          r.sql.length === 0 && !r.error
            ? null
            : executed(r.sql, {
                label: r.error ? "ROLLED BACK" : undefined,
                durationMs: r.durationMs,
                affected: r.error ? undefined : r.results.reduce((n, x) => n + x.affected, 0),
                error: r.error?.message,
              }),
      );
      reportConnectionFailure("apply_changes", { connectionId }, result.error);
      return result;
    },
  },

  // Sidebar objects besides tables (routines, sequences, Item Overview) and the sidebar's object
  // operations (drop / truncate / clone / comment). Introspection is logged as `-- …`.
  objects: {
    routines: (connectionId: string, schema: string) =>
      logged(
        connectionId,
        "structure",
        () => described(`list functions and procedures in ${schema}`),
        () => invoke<RoutineSummary[]>("list_routines", { connectionId, schema }),
      ),
    sequences: (connectionId: string, schema: string) =>
      logged(
        connectionId,
        "structure",
        () => described(`list sequences in ${schema}`),
        () => invoke<SequenceSummary[]>("list_sequences", { connectionId, schema }),
      ),
    /** PostgreSQL's information_schema / pg_catalog / pg_toast (left out of `schema.schemas`). */
    systemSchemas: (connectionId: string) =>
      invoke<SchemaInfo[]>("list_system_schemas", { connectionId }),
    routineDefinition: (connectionId: string, routine: RoutineRef) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the definition of ${routine.name}`),
        () => invoke<string>("get_routine_definition", { connectionId, routine }),
      ),
    sequenceDefinition: (connectionId: string, sequence: TableRef) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the definition of sequence ${tableName(sequence)}`),
        () => invoke<string>("get_sequence_definition", { connectionId, sequence }),
      ),
    overview: (connectionId: string, schema: string) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the item overview of ${schema}`),
        () => invoke<OverviewItem[]>("get_item_overview", { connectionId, schema }),
      ),
    preview: (connectionId: string, op: ObjectOp) =>
      invoke<DdlPlan>("preview_object_op", { connectionId, op }),
    /** A failure comes back in `DdlResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, op: ObjectOp) => {
      const request = () => described(`apply ${op.op}`);
      const result = await logged(
        connectionId,
        "structure",
        request,
        () => invoke<DdlResult>("apply_object_op", { connectionId, op }),
        (r) => (r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null),
      );
      if (result.error)
        logSql({
          connectionId,
          category: "structure",
          ...request(),
          durationMs: result.durationMs,
          error: result.error.message,
        });
      reportConnectionFailure("apply_object_op", { connectionId }, result.error);
      return result;
    },
  },

  // Introspection does not report its SQL: logged as `-- …` descriptions ("structure"). Cached
  // answers (table metadata) are logged too, with their short round trip.
  schema: {
    databases: (connectionId: string) =>
      logged(
        connectionId,
        "structure",
        () => described("list databases"),
        () => invoke<DatabaseInfo[]>("list_databases", { connectionId }),
      ),
    schemas: (connectionId: string) =>
      logged(
        connectionId,
        "structure",
        () => described("list schemas"),
        () => invoke<SchemaInfo[]>("list_schemas", { connectionId }),
      ),
    tables: (connectionId: string, schema: string) =>
      logged(
        connectionId,
        "structure",
        () => described(`list tables in ${schema}`),
        () => invoke<TableSummary[]>("list_tables", { connectionId, schema }),
      ),
    table: (connectionId: string, table: TableRef, refresh = false) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the structure of ${tableName(table)}${refresh ? " (refresh)" : ""}`),
        () => invoke<TableMeta>("get_table", { connectionId, table, refresh }),
      ),
    viewDefinition: (connectionId: string, table: TableRef) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the definition of view ${tableName(table)}`),
        () => invoke<string | null>("get_view_definition", { connectionId, table }),
      ),
    completion: (connectionId: string) =>
      logged(
        connectionId,
        "structure",
        () => described("load autocomplete metadata"),
        () => invoke<CompletionMetadata>("get_completion_metadata", { connectionId }),
      ),
    invalidate: (connectionId: string, table?: TableRef) =>
      invoke<void>("invalidate_schema_cache", { connectionId, table: table ?? null }),
    /** Structure editor pickers: PG user types, MySQL engines and character sets. */
    structureChoices: (connectionId: string) =>
      logged(
        connectionId,
        "structure",
        () => described("load column types, engines and collations"),
        () => invoke<StructureChoices>("get_structure_choices", { connectionId }),
      ),
  },

  // Server administration: the ⌘K database list, PG schemas, Process List…, User Management….
  // User changes have their own command, never `run_sql`: no password reaches the history, and
  // the log only gets the masked statements the backend reports.
  admin: {
    databaseOptions: (connectionId: string) =>
      invoke<DatabaseOptions>("admin_database_options", { connectionId }),
    previewDdl: (connectionId: string, op: AdminDdl) =>
      invoke<string[]>("admin_preview_ddl", { connectionId, op }),
    /** A failure comes back in `AdminResult.error` (reported to `onConnectionFailure` if fatal). */
    applyDdl: (connectionId: string, op: AdminDdl) =>
      loggedAdmin(
        connectionId,
        "admin_apply_ddl",
        () => described(`${ADMIN_DDL_TEXT[op.op]} ${op.name}`),
        () => invoke<AdminResult>("admin_apply_ddl", { connectionId, op }),
      ),
    /** Not logged: Running Queries refreshes every 2 s. */
    processes: (connectionId: string) =>
      invoke<ProcessRow[]>("admin_list_processes", { connectionId }),
    kill: (connectionId: string, ids: number[], scope: KillScope) =>
      logged(
        connectionId,
        "query",
        () =>
          described(
            `kill ${scope === "query" ? "the query of" : "the connection of"} process ${ids.join(", ")}`,
          ),
        () => invoke<KillOutcome[]>("admin_kill_processes", { connectionId, ids, scope }),
      ),
    users: (connectionId: string, includeNologin: boolean) =>
      invoke<UserSummary[]>("admin_list_users", { connectionId, includeNologin }),
    user: (connectionId: string, user: UserRef) =>
      invoke<UserDetail>("admin_get_user", { connectionId, user }),
    /** The statements of `change`, passwords masked. */
    previewUser: (connectionId: string, change: UserChange) =>
      invoke<string[]>("admin_preview_user", { connectionId, change }),
    applyUser: (connectionId: string, change: UserChange) =>
      loggedAdmin(
        connectionId,
        "admin_apply_user",
        () => described(`apply user changes to ${userName(change)}`),
        () => invoke<AdminResult>("admin_apply_user", { connectionId, change }),
      ),
  },
};

export function tableKey(t: TableRef): string {
  return `${t.schema ?? ""}.${t.name}`;
}
