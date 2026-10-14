import type { ApplyRequest } from "./bindings/ApplyRequest";
import type { ApplyResult } from "./bindings/ApplyResult";
import type { DefinitionRestore } from "./bindings/DefinitionRestore";
import type { GroupPlan } from "./bindings/GroupPlan";
import type { GroupResult } from "./bindings/GroupResult";
import type { GroupStatus } from "./bindings/GroupStatus";
import type { ServerExpr } from "./bindings/ServerExpr";
import type { WorkspaceGroup } from "./bindings/WorkspaceGroup";
import type { WorkspacePlan } from "./bindings/WorkspacePlan";
import type { WorkspaceResult } from "./bindings/WorkspaceResult";
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
import type { DiagramSchema } from "./bindings/DiagramSchema";
import type { MetricsBoard } from "./bindings/MetricsBoard";
import type { MetricsBoardInput } from "./bindings/MetricsBoardInput";
import type { MetricsImport } from "./bindings/MetricsImport";
import type { MetricsQuery } from "./bindings/MetricsQuery";
import type { MetricsResult } from "./bindings/MetricsResult";
import type { ExplainMode } from "./bindings/ExplainMode";
import type { ExplainResult } from "./bindings/ExplainResult";
import type { ExportProgress } from "./bindings/ExportProgress";
import type { ExportRequest } from "./bindings/ExportRequest";
import type { ExportResult } from "./bindings/ExportResult";
import type { ExportResultItem } from "./bindings/ExportResultItem";
import type { ExportResultsRequest } from "./bindings/ExportResultsRequest";
import type { ExportResultsResult } from "./bindings/ExportResultsResult";
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
import type { JsonImportRequest } from "./bindings/JsonImportRequest";
import type { JsonPreview } from "./bindings/JsonPreview";
import type { JsonPreviewRequest } from "./bindings/JsonPreviewRequest";
import type { BackupEvent } from "./bindings/BackupEvent";
import type { BackupJobRequest } from "./bindings/BackupJobRequest";
import type { BackupJobResult } from "./bindings/BackupJobResult";
import type { BackupTarget } from "./bindings/BackupTarget";
import type { Driver } from "./bindings/Driver";
import type { DumpBinary } from "./bindings/DumpBinary";
import type { RestoreFileInfo } from "./bindings/RestoreFileInfo";
import type { HistoryEntry } from "./bindings/HistoryEntry";
import type { HistoryFilter } from "./bindings/HistoryFilter";
import type { LocationChange } from "./bindings/LocationChange";
import type { LocationInspection } from "./bindings/LocationInspection";
import type { LocationKind } from "./bindings/LocationKind";
import type { LocationsInfo } from "./bindings/LocationsInfo";
import type { McpClient } from "./bindings/McpClient";
import type { McpCliResult } from "./bindings/McpCliResult";
import type { McpConfigPreview } from "./bindings/McpConfigPreview";
import type { McpInfo } from "./bindings/McpInfo";
import type { QueryFolder } from "./bindings/QueryFolder";
import type { QueryFolderInput } from "./bindings/QueryFolderInput";
import type { SavedQuery } from "./bindings/SavedQuery";
import type { SavedQueryInput } from "./bindings/SavedQueryInput";
import type { SavedQueryLibrary } from "./bindings/SavedQueryLibrary";
import type { SqlFileHead } from "./bindings/SqlFileHead";
import type { SqlFileInfo } from "./bindings/SqlFileInfo";
import type { QueryEvent } from "./bindings/QueryEvent";
import type { RunOptions } from "./bindings/RunOptions";
import type { StatementResult } from "./bindings/StatementResult";
import type { ObjectOp } from "./bindings/ObjectOp";
import type { OverviewItem } from "./bindings/OverviewItem";
import type { DatabaseItems } from "./bindings/DatabaseItems";
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
import type { PasscodeStatus } from "./bindings/PasscodeStatus";
import type { SessionDetails } from "./bindings/SessionDetails";
import type { WindowHandoff } from "./bindings/WindowHandoff";
import type { RecentEntry } from "./bindings/RecentEntry";
import type { RecentItems } from "./bindings/RecentItems";
import type { OpenRequest } from "./bindings/OpenRequest";
import type { SqliteSshSync } from "./bindings/SqliteSshSync";
import type { AssistantChatRequest } from "./bindings/AssistantChatRequest";
import type { AssistantChatResult } from "./bindings/AssistantChatResult";
import type { AssistantContextPreview } from "./bindings/AssistantContextPreview";
import type { AssistantContextRequest } from "./bindings/AssistantContextRequest";
import type { AssistantEvent } from "./bindings/AssistantEvent";
import type { AssistantKeyStatus } from "./bindings/AssistantKeyStatus";
import type { AssistantProvider } from "./bindings/AssistantProvider";
import type { AssistantProviderConfig } from "./bindings/AssistantProviderConfig";
import type { AssistantTestResult } from "./bindings/AssistantTestResult";
import type { InstalledPlugin } from "./bindings/InstalledPlugin";
import type { PluginList } from "./bindings/PluginList";
import type { PluginMenuEntry } from "./bindings/PluginMenuEntry";
import type { TracedSql } from "./bindings/TracedSql";
import {
  markTraceSeen,
  maskSecrets,
  noteLogged,
  normalizeSql,
  sameStatement,
  takeTwin,
  traceClock,
  traceLoggedSince,
} from "./sqlTrace";
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
export type { OpenRequest } from "./bindings/OpenRequest";
export type { RecentEntry } from "./bindings/RecentEntry";
export type { RecentItems } from "./bindings/RecentItems";
export type { SqliteSshEvent } from "./bindings/SqliteSshEvent";
export type { SqliteSshSync } from "./bindings/SqliteSshSync";
export type { SshPromptKind } from "./bindings/SshPromptKind";
export type { SafeModeLevel } from "./bindings/SafeModeLevel";
export type { PasswordMode } from "./bindings/PasswordMode";
export type { SshPasswordMode } from "./bindings/SshPasswordMode";
export type { MysqlVersion } from "./bindings/MysqlVersion";
export type { SslNegotiation } from "./bindings/SslNegotiation";
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
export type { PasscodeStatus, SessionDetails };
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
export type { SshPromptDone } from "./bindings/SshPromptDone";
export type { SshPromptField } from "./bindings/SshPromptField";
export type { SshPromptRequest } from "./bindings/SshPromptRequest";
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
export type { DiagramSchema };
export type { McpClient, McpCliResult, McpConfigPreview, McpInfo };
export type { McpClientStatus } from "./bindings/McpClientStatus";
export type { MetricsBoard, MetricsBoardInput, MetricsImport, MetricsQuery, MetricsResult };
export type { DiagramTable } from "./bindings/DiagramTable";
export type { DiagramColumn } from "./bindings/DiagramColumn";
export type { DiagramRelation } from "./bindings/DiagramRelation";
export type { ObjectOp, OverviewItem, RoutineRef, RoutineSummary, SequenceSummary };
export type { DatabaseItems };
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
export type { WindowHandoff };
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
  ExportResultItem,
  ExportResultsRequest,
  ExportResultsResult,
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
export type { ExportEncoding } from "./bindings/ExportEncoding";
export type { JsonLayout } from "./bindings/JsonLayout";
export type { JsonImportRequest, JsonPreview, JsonPreviewRequest };
export type { QueryFolder, QueryFolderInput, SavedQuery, SavedQueryInput, SavedQueryLibrary };
export type { SqlFileHead, SqlFileInfo };
export type { LocationChange, LocationInspection, LocationKind, LocationsInfo };
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
 * executed: the statements its sessions recorded (`drain_sql_trace`: introspection SELECTs, counts,
 * exports…) or the SQL a command returns. When a call ran no SQL the backend reported (cached
 * metadata, a SQL dump read from a file) it is logged as a `-- …` description of the request
 * instead (`meta`), never as made-up SQL.
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

/** `text` on one line, cut to a length a log line can show. */
function oneLine(text: string, max = 200): string {
  const line = text.trim().replace(/\s+/g, " ");
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

function tableName(t: TableRef): string {
  return t.schema ? `${t.schema}.${t.name}` : t.name;
}

const EXPORT_FORMAT = {
  csv: "CSV",
  json: "JSON",
  sqlInsert: "SQL INSERTs",
  xlsx: "Excel",
  html: "HTML",
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

/** The statements the connection's session recorded since the last drain (none when it cannot say). */
async function drainTrace(connectionId: string): Promise<TracedSql[]> {
  try {
    const trace = await invoke<TracedSql[]>("drain_sql_trace", { connectionId });
    return Array.isArray(trace) ? trace : [];
  } catch {
    return [];
  }
}

/**
 * Logs what a call ran, after it finished. The statements the backend recorded are logged with
 * their real SQL, category, timing and row counts, except the ones the call's own entry (`own`,
 * real SQL the command returned) already covers. A `-- …` description (`own.meta`) is only logged
 * when the call reported no SQL at all: nothing was recorded for the connection since `since`
 * (concurrent calls drain each other's statements, so this looks at the connection, not the call).
 * `own` null logs nothing of its own, and drops the recorded statements too (a quiet call).
 */
async function logCall(
  connectionId: string,
  since: number,
  own: SqlLogEvent | null,
  quiet = false,
  secrets?: readonly string[],
): Promise<void> {
  try {
    await logCallUnsafe(connectionId, since, own, quiet, secrets);
  } catch {
    // Logging must never change a command's outcome.
  }
}

async function logCallUnsafe(
  connectionId: string,
  since: number,
  call: SqlLogEvent | null,
  quiet: boolean,
  secrets?: readonly string[],
): Promise<void> {
  const trace = await drainTrace(connectionId);
  if (quiet) return;
  // The call's own secrets (a password it sent) never reach the log, whatever carries them.
  const own =
    call && secrets?.length
      ? {
          ...call,
          sql: maskSecrets(call.sql, secrets),
          ...(call.error !== undefined ? { error: maskSecrets(call.error, secrets) } : {}),
        }
      : call;
  const claims = own && !own.meta ? own.sql.split(";\n").map(normalizeSql) : [];
  const paired = claims.map(() => false);
  for (const recorded of trace) {
    const t = secrets?.length
      ? {
          ...recorded,
          sql: maskSecrets(recorded.sql, secrets),
          error: recorded.error !== null ? maskSecrets(recorded.error, secrets) : null,
        }
      : recorded;
    markTraceSeen(connectionId);
    const text = normalizeSql(t.sql);
    // The call logs this statement itself (with its own timing), or another call already did.
    const mine = claims.findIndex((c, i) => !paired[i] && sameStatement(c, text));
    if (mine >= 0) {
      paired[mine] = true;
      continue;
    }
    if (takeTwin(connectionId, text, "trace")) continue;
    logSql({
      connectionId,
      category: t.category === "structure" ? "structure" : "data",
      sql: t.sql,
      meta: false,
      durationMs: t.durationMs,
      ...(t.rows !== null ? { rows: t.rows } : {}),
      ...(t.affected !== null ? { affected: t.affected } : {}),
      ...(t.error !== null ? { error: t.error } : {}),
    });
    noteLogged(connectionId, text, "trace");
  }
  if (!own) return;
  if (own.meta) {
    if (!traceLoggedSince(connectionId, since)) logSql(own);
    return;
  }
  // The trace of another call already logged this very statement: keep that one.
  if (claims.length === 1 && !paired[0] && takeTwin(connectionId, claims[0]!, "own")) return;
  logSql(own);
  claims.forEach((c, i) => noteLogged(connectionId, c, "own", paired[i]));
}

/**
 * Runs `call` and logs it under `category`: the `request` entry with the error when the call
 * rejects; on success, `request` with the round-trip time, overridden by what `outcome` makes of
 * the result. `request` or `outcome` returning `null` logs nothing of its own. Either way the SQL
 * the backend recorded for the call is logged too (see `logCall`), unless `request` returned
 * `null` (a quiet call).
 */
async function logged<T>(
  connectionId: string,
  category: SqlLogCategory,
  request: () => LogFields | null,
  call: () => Promise<T>,
  outcome?: (result: T) => Partial<LogFields> | null,
  secrets?: readonly string[],
): Promise<T> {
  const t0 = performance.now();
  const since = traceClock();
  let result: T;
  try {
    result = await call();
  } catch (err) {
    const fields = safely(request);
    const error = toIpcError(err).message;
    await logCall(
      connectionId,
      since,
      fields ? { connectionId, category, ...fields, durationMs: elapsed(t0), error } : null,
      !fields,
      secrets,
    );
    throw err;
  }
  const fields = safely(request);
  const extra = outcome ? safely(() => outcome(result)) : {};
  await logCall(
    connectionId,
    since,
    fields && extra
      ? { connectionId, category, ...fields, durationMs: elapsed(t0), ...extra }
      : null,
    !fields,
    secrets,
  );
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
 * that ran (passwords masked by the backend, in the result and in the statements its session
 * records), then the failure on its own. `secrets` (the password of a user change) are masked in
 * everything that reaches the log, as a second lock.
 */
async function loggedAdmin(
  connectionId: string,
  cmd: string,
  request: () => LogFields,
  call: () => Promise<AdminResult>,
  secrets?: readonly string[],
): Promise<AdminResult> {
  const since = traceClock();
  const result = await logged(
    connectionId,
    "structure",
    request,
    call,
    (r) => (r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null),
    secrets,
  );
  const failed = result.error ? safely(request) : null;
  // The description of a failure is only worth a line when no SQL was reported for the call.
  if (result.error && failed && !traceLoggedSince(connectionId, since))
    logSql({
      connectionId,
      category: "structure",
      ...failed,
      durationMs: result.durationMs,
      error: maskSecrets(result.error.message, secrets),
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

  windows: {
    /**
     * File ▸ New Window, "Open in New Window", Move Tab to New Window: opens a window; `handoff`
     * (what it opens with) waits for its renderer. Resolves with the new window's label.
     */
    open: (handoff: WindowHandoff | null = null) => invoke<string>("window_open", { handoff }),
    /** What this window was opened with (once; null for the main window and plain new ones). */
    takeHandoff: () => invoke<WindowHandoff | null>("window_take_handoff"),
    /** This window's open connections: the backend closes them when the window goes away. */
    setConnections: (connectionIds: string[]) =>
      invoke<void>("window_set_connections", { connectionIds }),
    /** This window's label ("main" for the first window). */
    label: () => invoke<string>("window_label"),
    /**
     * Whether this window keeps the session saved: the main window until it closes, then the
     * oldest window left (told with `window:session-owner`).
     */
    isSessionOwner: () => invoke<boolean>("window_is_session_owner"),
    /** The user kept their work in the unsaved-changes dialog: a quit under way is off. */
    cancelQuit: () => invoke<void>("cancel_quit"),
  },

  menu: {
    /**
     * Rebinds the native menu items' accelerators (menu item id = action id → Tauri accelerator,
     * null for none) after the user remapped them in Settings ▸ Shortcuts.
     */
    setAccelerators: (accelerators: Record<string, string | null>) =>
      invoke<void>("set_menu_accelerators", { accelerators }),
  },

  // File ▸ Open Recent: what was opened last (shared by every window); the native submenu follows.
  recent: {
    list: () => invoke<RecentItems>("recent_list"),
    /** A connection was opened, an `.sql` file opened or saved: it moves to the top. */
    add: (entry: RecentEntry) => invoke<RecentItems>("recent_add", { entry }),
    clear: () => invoke<void>("recent_clear"),
    /** Rebuilds the submenu (saved connections renamed or deleted). */
    refreshMenu: () => invoke<void>("recent_refresh_menu"),
  },

  settings: {
    get: () => invoke<Record<string, unknown>>("get_settings"),
    set: (key: string, value: unknown) => invoke<void>("set_setting", { key, value }),
  },

  decoder: {
    /** Settings ▸ General ▸ Data Decoder ▸ "Automatically convert PostGIS geometry to WKT". */
    setGeometryWkt: (enabled: boolean) => invoke<void>("set_geometry_wkt", { enabled }),
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
    /**
     * Creates an empty SQLite database at `path`; an existing file is left untouched. With `key`
     * the new file is a SQLCipher database (format of SQLCipher `compatibility`, default 4).
     */
    createSqliteFile: (
      path: string,
      key: string | null = null,
      compatibility: number | null = null,
    ) => invoke<void>("create_sqlite_database", { path, key, compatibility }),
    /** Puts the profiles in `group` (null: no group); returns them as stored. */
    moveToGroup: (ids: string[], group: string | null) =>
      invoke<ConnectionProfile[]>("move_profiles_to_group", { ids, group }),
    /** Writes a `.tablerconnections` file; returns how many connections it holds. */
    exportConnections: (request: ExportConnectionsRequest) =>
      invoke<number>("export_connections", { request }),
    /** Reads a connections file for the import preview (`needsPassword` when it is encrypted). */
    inspectConnectionsFile: (path: string, password: string | null = null) =>
      invoke<ConnectionsFilePreview>("inspect_connections_file", { path, password }),
    /**
     * Imports the chosen connections (ids as in the file) as new profiles. Password commands come
     * in only for `keepPasswordCommands` (ids as in the file); the others ask for the password.
     */
    importConnections: (
      path: string,
      password: string | null,
      ids: string[],
      keepPasswordCommands: string[] = [],
    ) =>
      invoke<ConnectionProfile[]>("import_connections", {
        path,
        password,
        ids,
        keepPasswordCommands,
      }),
  },

  connections: {
    /**
     * `password`: typed in the prompt of an "Ask every time" profile (never stored).
     * `databaseOverride`: open that database instead of the profile's (another workspace).
     */
    connect: (profileId: string, password?: string | null, databaseOverride?: string | null) =>
      invoke<ConnectionInfo>("connect", {
        profileId,
        password: password ?? null,
        databaseOverride: databaseOverride ?? null,
      }),
    disconnect: (connectionId: string) => invoke<void>("disconnect", { connectionId }),
    list: () => invoke<ConnectionInfo[]>("list_connections"),
    ping: (connectionId: string) => invoke<ConnStatus>("ping", { connectionId }),
    acceptHostKey: (profileId: string, fingerprint: string, persistToKnownHosts = false) =>
      invoke<void>("accept_host_key", { profileId, fingerprint, persistToKnownHosts }),
    switchDatabase: (connectionId: string, database: string) =>
      invoke<ConnectionInfo>("switch_database", { connectionId, database }),
    /** View Using Encoding…: the encoding the open session uses now (MySQL / PostgreSQL). */
    getEncoding: (connectionId: string) =>
      invoke<string>("get_connection_encoding", { connectionId }),
    /** Reopens the session with `encoding` (null: the profile's own); the profile is unchanged. */
    setEncoding: (connectionId: string, encoding: string | null) =>
      invoke<ConnectionInfo>("set_connection_encoding", { connectionId, encoding }),
    /**
     * Answers an SSH server's keyboard-interactive (2FA) question (`ssh:prompt`): one answer per
     * prompt, or null to cancel the connect / test waiting for it.
     */
    answerSshPrompt: (requestId: string, answers: string[] | null) =>
      invoke<void>("answer_ssh_prompt", { requestId, answers }),
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
    /**
     * Logged as the statements the backend really ran (its trace): the EXPLAIN-wrapped text, and
     * the transaction it wraps around an Analyze. The submitted statement is only described, when
     * nothing was recorded (it failed before it reached the server).
     */
    explain: (connectionId: string, sql: string, mode: ExplainMode, queryId?: string) =>
      logged(
        connectionId,
        "query",
        () => ({
          ...described(`explain ${oneLine(sql)}`),
          label: explainLabel(mode),
        }),
        () =>
          invoke<ExplainResult>("explain", { connectionId, sql, mode, queryId: queryId ?? null }),
        (r) => ({ durationMs: r.durationMs, rows: r.rows.length }),
      ),
  },

  history: {
    list: (filter: Partial<HistoryFilter> = {}) =>
      invoke<HistoryEntry[]>("list_history", {
        filter: {
          profileId: null,
          search: null,
          limit: null,
          beforeId: null,
          matchCase: false,
          wholeWord: false,
          regex: false,
          ...filter,
        },
      }),
    delete: (ids: number[]) => invoke<void>("delete_history", { ids }),
    clear: (profileId: string | null = null) => invoke<void>("clear_history", { profileId }),
    /** Deletes the history older than `before` (ISO date); returns how many entries went. */
    clearBefore: (before: string, profileId: string | null = null) =>
      invoke<number>("clear_history_before", { before, profileId }),
  },

  /** Saved queries (the sidebar's Queries pane) and their folders: `.sql` files in a folder. */
  savedQueries: {
    list: () => invoke<SavedQueryLibrary>("list_saved_queries"),
    save: (input: SavedQueryInput) => invoke<SavedQuery>("save_saved_query", { input }),
    delete: (id: string) => invoke<void>("delete_saved_query", { id }),
    saveFolder: (input: QueryFolderInput) => invoke<QueryFolder>("save_query_folder", { input }),
    /** Also deletes the folders and queries inside it. */
    deleteFolder: (id: string) => invoke<void>("delete_query_folder", { id }),
    /** "Import…": `.sql` / `.sql.gz` files become queries in `folderId` (null: top level). */
    importFiles: (paths: string[], folderId: string | null = null) =>
      invoke<SavedQuery[]>("import_saved_queries", { paths, folderId }),
    /** "Add Folder to Favorites…": copies a folder of `.sql` files in. */
    addFolderFromDisk: (path: string, parentId: string | null = null) =>
      invoke<QueryFolder>("add_query_folder_from_disk", { path, parentId }),
    /** Show in Finder: a query's file, a folder (`folder`), or (`id` null) the queries folder. */
    reveal: (id: string | null, folder = false) =>
      invoke<void>("reveal_saved_query", { id, folder }),
    /** The `.sql` file behind a saved query (what "Import…" hands to the SQL import wizard). */
    path: (id: string) => invoke<string>("saved_query_path", { id }),
  },

  /** Settings ▸ Locations: the folders of the app database and of the saved queries. */
  locations: {
    get: () => invoke<LocationsInfo>("get_locations"),
    inspect: (kind: LocationKind, path: string) =>
      invoke<LocationInspection>("inspect_location", { kind, path }),
    /** Used from the next start (`path` null: the default folder). */
    setData: (path: string | null, change: LocationChange) =>
      invoke<LocationsInfo>("set_data_location", { path, change }),
    /** Applies at once (`path` null: the default folder). */
    setQueries: (path: string | null, change: LocationChange) =>
      invoke<LocationsInfo>("set_queries_location", { path, change }),
    /** Show in Finder: the app database file (`data`, also the history) or the queries folder. */
    reveal: (kind: LocationKind) => invoke<void>("reveal_location", { kind }),
    restart: () => invoke<void>("restart_app"),
  },

  // Settings ▸ Connections ▸ MCP Server: `tabler --mcp` and registering it with MCP clients.
  mcp: {
    /** The command line of the running app's MCP server and which clients are installed. */
    info: () => invoke<McpInfo>("mcp_info"),
    /** Claude Code / Codex: runs their `mcp add` (when the CLI is found). */
    installCli: (client: McpClient) => invoke<McpCliResult>("mcp_install_cli", { client }),
    /** Cursor / VS Code: the change to their mcp.json, to confirm before `writeConfig`. */
    previewConfig: (client: McpClient) =>
      invoke<McpConfigPreview>("mcp_preview_config", { client }),
    /** Writes the confirmed change; refused when the file no longer reads `expectedBefore`. */
    writeConfig: (client: McpClient, expectedBefore: string | null) =>
      invoke<string>("mcp_write_config", { client, expectedBefore }),
  },

  /** The SQL editor's files: `.sql.gz` is decompressed on open and compressed on save. */
  sqlFiles: {
    /** The whole text, for a file known to be small (a file that is not UTF-8 opens too). */
    read: (path: string) => invoke<string>("read_sql_file", { path }),
    /** Size and compression of a file, without reading it (ask before a big one is opened). */
    info: (path: string) => invoke<SqlFileInfo>("sql_file_info", { path }),
    /**
     * The first `maxBytes` of a file as text, cut after a complete line (`maxBytes` null: all of
     * it). A gzip file is streamed, so only the head is decompressed; `encoding` names what a file
     * that is not UTF-8 was read as.
     */
    readHead: (path: string, maxBytes: number | null) =>
      invoke<SqlFileHead>("read_sql_file_head", { path, maxBytes }),
    write: (path: string, content: string) => invoke<void>("write_sql_file", { path, content }),
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
      const since = traceClock();
      const result = await logged(
        connectionId,
        "structure",
        request,
        () => invoke<DdlResult>("apply_ddl", { connectionId, table, ops }),
        (r) => (r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null),
      );
      const { error, durationMs } = result;
      const failed = error ? safely(request) : null;
      if (error && failed && !traceLoggedSince(connectionId, since))
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
    /**
     * Every result of a multi-statement run in one go (a workbook with a sheet each, one page or
     * script, or a file per result in a folder); progress per result, `cancel` stops it.
     */
    runResults: async (
      connectionId: string,
      req: ExportResultsRequest,
      onProgress: (p: ExportTablesProgress) => void,
    ) => {
      const channel = await makeChannel<ExportTablesProgress>(onProgress);
      return logged(
        connectionId,
        "data",
        () =>
          described(
            `export ${req.results.length} result${req.results.length === 1 ? "" : "s"} as ${EXPORT_FORMAT[req.format.format]}`,
          ),
        () =>
          invoke<ExportResultsResult>("export_results", { connectionId, req, onProgress: channel }),
        (r) => ({ durationMs: r.durationMs, rows: r.rows }),
      );
    },
    /**
     * The tables and views of one database of the connection for the multi-table export (every
     * user schema's; PostgreSQL reads another database through a temporary session). Null = the
     * current database.
     */
    databaseTables: (connectionId: string, database: string | null) =>
      logged(
        connectionId,
        "structure",
        () => described(`list the tables of ${database ?? "the current database"}`),
        () => invoke<TableSummary[]>("export_database_tables", { connectionId, database }),
      ),
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
    /** File ▸ Import ▸ From JSON…: the keys of the first objects as columns, the first rows. */
    previewJson: (req: JsonPreviewRequest) => invoke<JsonPreview>("preview_json_import", { req }),
    json: async (
      connectionId: string,
      req: JsonImportRequest,
      onProgress: (p: CsvImportProgress) => void,
    ) => {
      const channel = await makeChannel<CsvImportProgress>(onProgress);
      return logged(
        connectionId,
        "data",
        () => described(`import JSON into ${tableName(req.target.table)}`),
        () => invoke<CsvImportReport>("import_json", { connectionId, req, onProgress: channel }),
        (r) => ({
          durationMs: r.durationMs,
          affected: r.rowsImported,
          error: r.failure?.message,
        }),
      );
    },
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
    /**
     * SQLite: copies the profile's database file to `path`; resolves with its size. SQLite over
     * SSH copies the local copy an open connection downloaded (the cached copy), or with `fresh`
     * (and without an open connection) downloads the file from the SSH server first.
     */
    copySqlite: (profileId: string, path: string, fresh?: boolean) =>
      invoke<number>("backup_copy_sqlite", { profileId, path, fresh: fresh ?? null }),
    /** "Install with Homebrew…": `brew install <formula>` in a new Terminal window. */
    installWithBrew: (formula: string) => invoke<void>("backup_install_with_brew", { formula }),
  },

  safety: {
    /** Whether `password` is the profile's stored DB password (Safe Mode prompts). */
    verifyPassword: (profileId: string, password: string) =>
      invoke<boolean>("verify_connection_password", { profileId, password }),
  },

  // App lock (Settings ▸ Security ▸ Passcode), Touch ID, and connection security details.
  security: {
    /** Touch ID or the account password ("Tabler is trying to <reason>."); false = cancelled. */
    authenticate: (reason: string) => invoke<boolean>("authenticate_biometric", { reason }),
    passcodeStatus: () => invoke<PasscodeStatus>("passcode_status"),
    /** Sets / changes (`passcode`) or turns off (`null`) the passcode; `current` when one is set. */
    setPasscode: (current: string | null, passcode: string | null) =>
      invoke<void>("set_passcode", { current, passcode }),
    verifyPasscode: (passcode: string) => invoke<boolean>("verify_passcode", { passcode }),
    /** Locks every window (Window ▸ Lock Tabler). */
    lockApp: () => invoke<void>("lock_app"),
    /** Calls `handler` when every window must lock (sleep, screen lock, Lock Tabler). */
    onLock: async (handler: (reason: string) => void): Promise<() => void> => {
      const { isMock: mock } = await import("./transport");
      if (mock) {
        const { mockListen } = await import("./mock/transport");
        return mockListen("lock:system", (reason) => handler(String(reason)));
      }
      const { listen } = await import("@tauri-apps/api/event");
      return listen<string>("lock:system", (e) => handler(e.payload));
    },
    /** The DB password for Copy as URL (after the safe-mode / Touch ID checks); null = none. */
    revealPassword: (profileId: string) =>
      invoke<string | null>("reveal_connection_password", { profileId }),
    /** TLS state and character set of an open session (the status bar's details). */
    sessionDetails: (connectionId: string) =>
      invoke<SessionDetails>("session_details", { connectionId }),
  },

  files: {
    readText: (path: string) => invoke<string>("read_text_file", { path }),
    writeText: (path: string, content: string) =>
      invoke<void>("write_text_file", { path, content }),
    /** A whole file as a bytes value (Set Value ▸ Add a File…); at most 64 MB. */
    readBinary: (path: string) => invoke<Value>("read_binary_file", { path }),
  },

  diagram: {
    /** Generate Diagram…: a schema's tables, columns and foreign keys, in one round trip. */
    schemaRelations: (connectionId: string, schema: string) =>
      logged(
        connectionId,
        "structure",
        () => described(`load the diagram of ${schema}`),
        () => invoke<DiagramSchema>("schema_relations", { connectionId, schema }),
      ),
    /** Writes base64-encoded bytes to `path` (the diagram's Export ▸ PNG…). */
    writeBinaryFile: (path: string, base64: string) =>
      invoke<void>("write_binary_file", { path, base64 }),
  },

  // Metrics Board: boards in the app database, their `.tablerboard` files, object queries.
  metrics: {
    /** The profile's boards plus the boards of every connection (null: all boards). */
    listBoards: (profileId: string | null) =>
      invoke<MetricsBoard[]>("metrics_list_boards", { profileId }),
    saveBoard: (input: MetricsBoardInput) => invoke<MetricsBoard>("metrics_save_board", { input }),
    deleteBoard: (id: string) => invoke<void>("metrics_delete_board", { id }),
    /** Writes boards to a `.tablerboard` file (encrypted with `password`); returns the count. */
    exportBoards: (ids: string[], path: string, password: string | null) =>
      invoke<number>("metrics_export_boards", { ids, path, password }),
    /** Adds the boards of a file for `profileId` (null: every connection). */
    importBoards: (path: string, password: string | null, profileId: string | null) =>
      invoke<MetricsImport>("metrics_import_boards", { path, password, profileId }),
    /**
     * Runs one board object's query (cancel with `query.cancel` and the same id). Logged in the
     * console unless `quiet` (automatic refreshes).
     */
    run: (connectionId: string, queryId: string, query: MetricsQuery, quiet = false) =>
      logged(
        connectionId,
        "query",
        () => (quiet ? null : { sql: query.parts.join("?"), meta: false, label: "METRICS" }),
        () => invoke<MetricsResult>("metrics_run", { connectionId, queryId, query }),
        (r) =>
          quiet
            ? null
            : {
                sql: r.sql,
                durationMs: r.durationMs,
                rows: r.rows.length,
                ...(r.affected !== null ? { affected: r.affected } : {}),
              },
      ),
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

  // The workspace commit (⌘S, TablePlus §9): every pending GUI change of a connection — staged
  // rows of its tabs and results, staged structure, pending sidebar operations, a definition
  // saved from its SQL tab — in one transaction where the engine allows it.
  workspace: {
    preview: (connectionId: string, groups: WorkspaceGroup[]) =>
      invoke<WorkspacePlan>("preview_workspace_changes", { connectionId, groups }),
    /** A failure comes back in `WorkspaceResult.error` (reported to `onConnectionFailure` if fatal). */
    apply: async (connectionId: string, groups: WorkspaceGroup[]) => {
      const result = await logged(
        connectionId,
        groups.every((g) => g.kind === "rows") ? "data" : "structure",
        () => described(`commit ${groups.length} pending change group(s)`),
        () => invoke<WorkspaceResult>("apply_workspace_changes", { connectionId, groups }),
        (r) => {
          const ran = r.groups.flatMap((g) => g.executed);
          const sql = ran.length > 0 ? ran : r.groups.flatMap((g) => g.sql);
          if (sql.length === 0 && !r.error) return null;
          return executed(sql, {
            label: r.error && ran.length === 0 ? "ROLLED BACK" : undefined,
            durationMs: r.durationMs,
            error: r.error?.message,
          });
        },
      );
      reportConnectionFailure("apply_workspace_changes", { connectionId }, result.error);
      return result;
    },
  },

  // Sidebar objects besides tables (routines, sequences, Item Overview) and the sidebar's object
  // operations (drop / truncate / clone / comment). Introspection is logged with its recorded SQL.
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
    /**
     * Every table, view and routine of the connection's database across its schemas (Search in
     * Database…); `includeSystem`: PostgreSQL's system schemas' relations too.
     */
    databaseItems: (connectionId: string, includeSystem = false) =>
      logged(
        connectionId,
        "structure",
        () => described("list every item of the database"),
        () => invoke<DatabaseItems>("list_database_items", { connectionId, includeSystem }),
      ),
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
      const since = traceClock();
      const result = await logged(
        connectionId,
        "structure",
        request,
        () => invoke<DdlResult>("apply_object_op", { connectionId, op }),
        (r) => (r.executed.length > 0 ? executed(r.executed, { durationMs: r.durationMs }) : null),
      );
      if (result.error && !traceLoggedSince(connectionId, since))
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

  // Introspection is logged with the SELECTs the backend recorded ("structure"); a cached answer
  // (table metadata) ran no SQL and is logged as a `-- …` description, with its short round trip.
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
        change.password ? [change.password] : undefined,
      ),
  },

  // Opening Tabler from outside: connection URLs (`open -a Tabler "postgres://…"`, links) and
  // files from Finder; connections opened from a URL stay temporary until saved.
  deeplink: {
    /** The queued URLs and files, oldest first (each is handed out once). */
    takePending: () => invoke<OpenRequest[]>("take_pending_opens"),
    /** Temporary profiles are deleted at the next start unless marked saved (`false`). */
    markTemporary: (profileId: string, temporary: boolean) =>
      invoke<void>("mark_temporary_profile", { profileId, temporary }),
    temporaryProfiles: () => invoke<string[]>("list_temporary_profiles"),
  },

  // SQLite over SSH: the downloaded copy's changes go back to the server.
  sqliteSsh: {
    /** Uploads now; `force` overwrites a remote file that changed since it was downloaded. */
    upload: (connectionId: string, force = false) =>
      invoke<SqliteSshSync>("sqlite_ssh_upload", { connectionId, force }),
  },

  // Plugins ▸ Manage Plugins… (⌘L): the plugins in <app data>/Plugins and the native menu.
  plugins: {
    list: () => invoke<PluginList>("plugins_list"),
    /** Install from Folder…: copies the folder in (replacing the same identifier). */
    install: (path: string) => invoke<InstalledPlugin>("plugins_install", { path }),
    remove: (identifier: string) => invoke<void>("plugins_remove", { identifier }),
    /** A command's script, for the renderer's sandboxed worker. */
    readScript: (identifier: string, script: string) =>
      invoke<string>("plugins_read_script", { identifier, script }),
    revealFolder: () => invoke<void>("plugins_reveal_folder"),
    /** The enabled plugins' commands in the native Plugins menu. */
    setMenu: (entries: PluginMenuEntry[]) => invoke<void>("plugins_set_menu", { entries }),
  },

  // AI Assistant (right sidebar): chat about the active connection's STRUCTURE (never its rows)
  // with Claude, an OpenAI-compatible server or a local Ollama. Keys stay in the Keychain.
  assistant: {
    /** Streams the reply on `onEvent`; provider failures come back in `result.error`. */
    chat: async (req: AssistantChatRequest, onEvent: (event: AssistantEvent) => void) =>
      invoke<AssistantChatResult>("assistant_chat", {
        req,
        onEvent: await makeChannel<AssistantEvent>(onEvent),
      }),
    /** Stops a running chat; false when it already ended. */
    cancel: (requestId: string) => invoke<boolean>("assistant_cancel", { requestId }),
    /** The structure a request with this context sends ("What is sent"). */
    context: (context: AssistantContextRequest) =>
      invoke<AssistantContextPreview>("assistant_context", { context }),
    keyStatus: () => invoke<AssistantKeyStatus>("assistant_key_status"),
    /** Stores the provider's API key in the Keychain; null removes it. */
    setKey: (provider: AssistantProvider, key: string | null) =>
      invoke<void>("assistant_set_key", { provider, key }),
    test: (config: AssistantProviderConfig) =>
      invoke<AssistantTestResult>("assistant_test", { config }),
  },
};

export type { InstalledPlugin, PluginList, PluginMenuEntry };
export type { InvalidPlugin } from "./bindings/InvalidPlugin";
export type { PluginCommand } from "./bindings/PluginCommand";
export type { PluginContextKind } from "./bindings/PluginContextKind";
export type { PluginManifest } from "./bindings/PluginManifest";

export function tableKey(t: TableRef): string {
  return `${t.schema ?? ""}.${t.name}`;
}

export type {
  AssistantChatRequest,
  AssistantChatResult,
  AssistantContextPreview,
  AssistantContextRequest,
  AssistantEvent,
  AssistantKeyStatus,
  AssistantProvider,
  AssistantProviderConfig,
  AssistantTestResult,
};
export type { AssistantFailure } from "./bindings/AssistantFailure";
export type { AssistantFailureKind } from "./bindings/AssistantFailureKind";
export type { AssistantMessage } from "./bindings/AssistantMessage";
export type { AssistantRole } from "./bindings/AssistantRole";
export type {
  DefinitionRestore,
  GroupPlan,
  GroupResult,
  GroupStatus,
  ServerExpr,
  WorkspaceGroup,
  WorkspacePlan,
  WorkspaceResult,
};
