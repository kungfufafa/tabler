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
import type { DatabaseInfo } from "./bindings/DatabaseInfo";
import type { ProfileInput } from "./bindings/ProfileInput";
import type { SchemaInfo } from "./bindings/SchemaInfo";
import type { SecretStatus } from "./bindings/SecretStatus";
import type { TableMeta } from "./bindings/TableMeta";
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
import type { HistoryEntry } from "./bindings/HistoryEntry";
import type { HistoryFilter } from "./bindings/HistoryFilter";
import type { QueryEvent } from "./bindings/QueryEvent";
import type { RunOptions } from "./bindings/RunOptions";
import type { StatementResult } from "./bindings/StatementResult";
import { invoke, makeChannel, reportConnectionFailure, type ErrorPayload } from "./transport";

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
export type { RowResult } from "./bindings/RowResult";
export type { ConnectionStatusEvent } from "./bindings/ConnectionStatusEvent";
export type { TunnelStatusEvent } from "./bindings/TunnelStatusEvent";
export type { ErrorPayload } from "./bindings/ErrorPayload";
export type { ResultColumn } from "./bindings/ResultColumn";
export type { StatementSpan } from "./bindings/StatementSpan";
export type { StatementKind } from "./bindings/StatementKind";
export type { ColumnDef } from "./bindings/ColumnDef";
export type { IndexDef } from "./bindings/IndexDef";
export type { FkDef } from "./bindings/FkDef";
export type { IndexMeta } from "./bindings/IndexMeta";
export type { TriggerMeta } from "./bindings/TriggerMeta";
export type { ExportSource } from "./bindings/ExportSource";
export type { ExportFormat } from "./bindings/ExportFormat";
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
  },

  connections: {
    connect: (profileId: string) => invoke<ConnectionInfo>("connect", { profileId }),
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
      invoke<BrowsePage>("browse_table", { connectionId, req }),
    count: (connectionId: string, table: TableRef, filter: FilterGroup | null, exact = false) =>
      invoke<RowCount>("count_table_rows", { connectionId, table, filter, exact }),
    cell: (connectionId: string, table: TableRef, key: RowKey, column: string) =>
      invoke<Value>("fetch_cell", { connectionId, table, key, column }),
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
      const results = await invoke<StatementResult[]>("run_sql", {
        connectionId,
        queryId,
        sql,
        options: { maxRows: 10_000, stopOnError: true, explain: null, ...options },
        onEvent: await makeChannel<QueryEvent>((e) => {
          if (e.event === "failed") report(e.data.error);
          onEvent(e);
        }),
      });
      for (const r of results) report(r.error);
      return results;
    },
    cancel: (connectionId: string, queryId: string) =>
      invoke<void>("cancel_query", { connectionId, queryId }),
    explain: (connectionId: string, sql: string, mode: ExplainMode, queryId?: string) =>
      invoke<ExplainResult>("explain", { connectionId, sql, mode, queryId: queryId ?? null }),
  },

  history: {
    list: (filter: Partial<HistoryFilter> = {}) =>
      invoke<HistoryEntry[]>("list_history", {
        filter: { profileId: null, search: null, limit: null, beforeId: null, ...filter },
      }),
    delete: (ids: number[]) => invoke<void>("delete_history", { ids }),
    clear: (profileId: string | null = null) => invoke<void>("clear_history", { profileId }),
  },

  ddl: {
    preview: (connectionId: string, table: TableRef | null, ops: DdlOp[]) =>
      invoke<DdlPlan>("preview_ddl", { connectionId, table, ops }),
    /** A failure comes back in `DdlResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, table: TableRef | null, ops: DdlOp[]) => {
      const result = await invoke<DdlResult>("apply_ddl", { connectionId, table, ops });
      reportConnectionFailure("apply_ddl", { connectionId }, result.error);
      return result;
    },
    tableDdl: (connectionId: string, table: TableRef) =>
      invoke<string>("get_table_ddl", { connectionId, table }),
  },

  exports: {
    run: async (
      connectionId: string,
      req: ExportRequest,
      onProgress: (p: ExportProgress) => void,
    ) =>
      invoke<ExportResult>("export_data", {
        connectionId,
        req,
        onProgress: await makeChannel<ExportProgress>(onProgress),
      }),
    cancel: (exportId: string) => invoke<void>("cancel_export", { exportId }),
  },

  files: {
    readText: (path: string) => invoke<string>("read_text_file", { path }),
    writeText: (path: string, content: string) =>
      invoke<void>("write_text_file", { path, content }),
  },

  mutate: {
    preview: (connectionId: string, req: ApplyRequest) =>
      invoke<string[]>("preview_changes", { connectionId, req }),
    /** A failure comes back in `ApplyResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, req: ApplyRequest) => {
      const result = await invoke<ApplyResult>("apply_changes", { connectionId, req });
      reportConnectionFailure("apply_changes", { connectionId }, result.error);
      return result;
    },
  },

  schema: {
    databases: (connectionId: string) => invoke<DatabaseInfo[]>("list_databases", { connectionId }),
    schemas: (connectionId: string) => invoke<SchemaInfo[]>("list_schemas", { connectionId }),
    tables: (connectionId: string, schema: string) =>
      invoke<TableSummary[]>("list_tables", { connectionId, schema }),
    table: (connectionId: string, table: TableRef, refresh = false) =>
      invoke<TableMeta>("get_table", { connectionId, table, refresh }),
    viewDefinition: (connectionId: string, table: TableRef) =>
      invoke<string | null>("get_view_definition", { connectionId, table }),
    completion: (connectionId: string) =>
      invoke<CompletionMetadata>("get_completion_metadata", { connectionId }),
    invalidate: (connectionId: string, table?: TableRef) =>
      invoke<void>("invalidate_schema_cache", { connectionId, table: table ?? null }),
  },
};

export function tableKey(t: TableRef): string {
  return `${t.schema ?? ""}.${t.name}`;
}
