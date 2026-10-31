import { toast } from "sonner";
import {
  CORE_DRIVERS,
  coreDialect,
  isCoreDriver,
  notAvailableFor,
  sessionKind,
} from "@/features/drivers/catalog";
import type {
  ConnectionInfo,
  DatabaseKind,
  Dialect,
  Driver,
  Feature,
  NamespaceLabels,
  SessionCaps,
  TxMode,
} from "@/lib/ipc";
import { DRIVER_CATALOG } from "@/lib/ipc/bindings/driverCatalog";
import { useConnectionsStore } from "@/stores/connections";

/**
 * What a connection's session offers (plan A9): the caps it reported at connect
 * (`ConnectionInfo.capabilities`), or, from a backend that reports none, the caps its driver
 * starts from. The backend enforces them (`require(Feature)`); the renderer hides what a session
 * lacks, ANDed with its own checks, so the core drivers, which get every feature, look as before.
 */

/** The caps a connection of each driver starts from (`SessionCaps::base`). */
export const BASE_CAPS: Readonly<Record<Driver, SessionCaps>> = DRIVER_CATALOG.baseCaps;

type WithCaps = Pick<ConnectionInfo, "driver" | "capabilities">;

/** The caps of a connection. */
export function capsOf(info: WithCaps): SessionCaps {
  return info.capabilities ?? BASE_CAPS[info.driver];
}

/** Whether a connection (or its caps) offers `feature`. */
export function has(of: WithCaps | SessionCaps, feature: Feature): boolean {
  const caps = "features" in of ? of : capsOf(of);
  return caps.features.includes(feature);
}

/**
 * Whether the session is a core one: a core driver speaking its own dialect (PostgreSQL, MySQL /
 * MariaDB, SQLite, and the PostgreSQL flavors that speak PostgreSQL). Its data and namespace caps
 * are not read in v1; a Redshift session (PostgreSQL's driver, its own dialect) is not one.
 */
export function isCoreSession(caps: SessionCaps): boolean {
  return isCoreDriver(caps.driver) && caps.dialect === caps.driver;
}

/** The caps of an open connection, kept current; null when it is not open. */
export function useCaps(connectionId: string | null): SessionCaps | null {
  return useConnectionsStore((s) => {
    const conn = connectionId === null ? undefined : s.byId[connectionId];
    return conn ? capsOf(conn.info) : null;
  });
}

/** The caps of an open connection, read outside React; null when it is not open. */
export function connectionCaps(connectionId: string): SessionCaps | null {
  const conn = useConnectionsStore.getState().byId[connectionId];
  return conn ? capsOf(conn.info) : null;
}

/**
 * The core SQL dialect a session speaks: PostgreSQL's for the flavors that speak it, SQLite's
 * for LibSQL and D1; null for an extension dialect (Redshift, T-SQL…) or a native store, whose
 * text the core lexers, generators and completions must not handle.
 */
export function sessionDialect(caps: SessionCaps): Dialect | null {
  return CORE_DRIVERS.find((d) => d === caps.dialect) ?? null;
}

/** The core SQL dialect of a connection (see `sessionDialect`). */
export function connectionDialect(info: WithCaps): Dialect | null {
  return sessionDialect(capsOf(info));
}

/**
 * `connectionDialect` read outside React by the connection's id (an action, a commit): the
 * session's while it is open (none for a Redshift session, although its driver is PostgreSQL's),
 * `driver`'s own while it is not.
 */
export function dialectFor(connectionId: string | null, driver: Driver): Dialect | null {
  const conn =
    connectionId === null ? undefined : useConnectionsStore.getState().byId[connectionId];
  return conn ? connectionDialect(conn.info) : coreDialect(driver);
}

/**
 * `sessionKind` read outside React by the connection's id: the open session's database, the
 * driver's own while it is not open.
 */
export function kindFor(connectionId: string | null, driver: Driver): DatabaseKind {
  const conn =
    connectionId === null ? undefined : useConnectionsStore.getState().byId[connectionId];
  return sessionKind(conn?.info ?? { driver, capabilities: null });
}

/** What the sidebar and the tabs call the session's levels ("Collections", "Indexes"). */
export function nounsOf(caps: SessionCaps): NamespaceLabels {
  return caps.namespaces.labels;
}

/**
 * Whether a commit in `mode` can stop partway with some changes applied (critique D2): every mode
 * but the all-or-nothing ones. Such a commit answers a result per change, and the renderer
 * unstages the applied ones, so a retry never sends a non-idempotent write twice.
 */
export function appliesPartially(mode: TxMode): boolean {
  return mode !== "atomic" && mode !== "dataOnly";
}

/** A feature's name in a refusal, as the backend's `Feature::label` says it. */
const FEATURE_LABEL: Readonly<Record<Feature, string>> = {
  editor: "The query editor",
  explain: "Explain",
  explainAnalyze: "Explain Analyze",
  cancelOnServer: "Cancelling on the server",
  format: "Format SQL",
  editRows: "Editing rows",
  insertRows: "Inserting rows",
  deleteRows: "Deleting rows",
  editWithoutKey: "Editing rows without a primary key",
  returning: "Reading saved rows back",
  serverExpressions: "DEFAULT and server expressions",
  exactCount: "Exact row counts",
  copyAsInsert: "Copy as INSERT",
  workspaceCommit: "Committing the workspace",
  structureView: "The structure view",
  structureEdit: "Editing the structure",
  createTable: "Creating tables",
  dropTable: "Dropping tables",
  truncateTable: "Truncating tables",
  renameTable: "Renaming tables",
  tableDdl: "Show DDL",
  viewDefinition: "View definitions",
  routines: "Functions and procedures",
  sequences: "Sequences",
  triggers: "Triggers",
  diagram: "The diagram",
  switchDatabase: "Switching databases",
  createDatabase: "Creating databases",
  dropDatabase: "Dropping databases",
  renameDatabase: "Renaming databases",
  createSchema: "Creating schemas",
  dropSchema: "Dropping schemas",
  processList: "The process list",
  killProcess: "Killing processes",
  users: "User management",
  exportTables: "Exporting tables",
  exportSql: "SQL export",
  importCsv: "CSV import",
  importJson: "JSON import",
  importSqlDump: "SQL dump import",
  backup: "Backup",
  restore: "Restore",
  encoding: "Connection encodings",
  timezone: "Time zones",
  startupCommands: "Startup commands",
  sessionDetails: "Session details",
  metrics: "Metrics boards",
  assistant: "The AI assistant",
  mcp: "MCP",
  laravelMigration: "Laravel migrations",
  dummyData: "Dummy Data",
};

/** "Exporting tables", "The process list": a feature as the subject of a sentence. */
export function featureLabel(feature: Feature): string {
  return FEATURE_LABEL[feature];
}

/**
 * Whether an opener may go on for the connection: every one of `features` is offered, or the
 * connection is not open (the opened dialog then says what is wrong). A refusal is toasted
 * ("Exporting tables is not available for Redis connections"), the way the backend would refuse.
 */
export function allowFeature(connectionId: string, ...features: Feature[]): boolean {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn) return true;
  const caps = capsOf(conn.info);
  const missing = features.find((f) => !caps.features.includes(f));
  if (missing === undefined) return true;
  toast.error(notAvailableFor(sessionKind(conn.info), featureLabel(missing)));
  return false;
}
