import type { ApplyRequest } from "../../bindings/ApplyRequest";
import type { ApplyResult } from "../../bindings/ApplyResult";
import type { BrowsePage } from "../../bindings/BrowsePage";
import type { BrowseRequest } from "../../bindings/BrowseRequest";
import type { ConnectionProfile } from "../../bindings/ConnectionProfile";
import type { DatabaseInfo } from "../../bindings/DatabaseInfo";
import type { Driver } from "../../bindings/Driver";
import type { FilterGroup } from "../../bindings/FilterGroup";
import type { KillOutcome } from "../../bindings/KillOutcome";
import type { KillScope } from "../../bindings/KillScope";
import type { ProcessRow } from "../../bindings/ProcessRow";
import type { QueryEvent } from "../../bindings/QueryEvent";
import type { RowCount } from "../../bindings/RowCount";
import type { RoutineRef } from "../../bindings/RoutineRef";
import type { RoutineSummary } from "../../bindings/RoutineSummary";
import type { RowKey } from "../../bindings/RowKey";
import type { SchemaInfo } from "../../bindings/SchemaInfo";
import type { SessionCaps } from "../../bindings/SessionCaps";
import type { SessionDetails } from "../../bindings/SessionDetails";
import type { TableMeta } from "../../bindings/TableMeta";
import type { TableRef } from "../../bindings/TableRef";
import type { TableSummary } from "../../bindings/TableSummary";
import type { UserDetail } from "../../bindings/UserDetail";
import type { UserRef } from "../../bindings/UserRef";
import type { UserSummary } from "../../bindings/UserSummary";
import type { Value } from "../../bindings/Value";

/**
 * A driver of the in-memory mock beyond the core three (plan §2.10): each lane's
 * `mock/drivers/<lane>/index.ts` exports a `MockLane` with one per driver, holding its own data
 * model. The mock's handlers send the commands of a connection of that driver here (F9b).
 * Methods that throw use `mockError` (the backend's error codes).
 */
export interface MockDriver {
  driver: Driver;
  /** The version the status pill shows. */
  serverVersion(profile: ConnectionProfile): string;
  /** Profiles `?mockDrivers=<id>` adds to the launcher (the core seed stays as it is). */
  seedProfiles(): ConnectionProfile[];
  /** A session of `profile` (connect and test). */
  connect(profile: ConnectionProfile): MockSession;
  /** The driver's own calls before a session exists (`driver_util`). */
  util?(method: string, args: unknown): unknown;
}

/** One connection of a mock driver. Optional methods the driver does not offer are refused. */
export interface MockSession {
  caps: SessionCaps;
  listDatabases(): DatabaseInfo[];
  listSchemas(): SchemaInfo[];
  listTables(schema: string): TableSummary[];
  tableMeta(table: TableRef): TableMeta;
  /** The statement that creates the table, for Show DDL (refused without it). */
  tableDdl?(table: TableRef): string;
  browse(req: BrowseRequest): BrowsePage;
  count?(table: TableRef, filter: FilterGroup | null): RowCount;
  fetchCell?(table: TableRef, key: RowKey, column: string): Value;
  /** The text Preview shows for the staged changes. */
  preview?(req: ApplyRequest): string[];
  apply?(req: ApplyRequest): ApplyResult;
  /** Runs one statement or command of the editor or console, streaming its events. */
  run(statement: string, emit: (event: QueryEvent) => void): void | Promise<void>;
  /** The session's own calls (`session_call`). */
  call?(method: string, args: unknown): unknown;
  // The backend's hooks of the same names (docs/drivers/FOUNDATION.md), each refused without it.
  /** Running Queries. */
  listProcesses?(): ProcessRow[];
  killProcesses?(ids: string[], scope: KillScope): KillOutcome[];
  /** The sidebar's routines. */
  listRoutines?(schema: string): RoutineSummary[];
  routineDefinition?(routine: RoutineRef): string;
  /** The Users window (reading only). */
  listUsers?(includeNologin: boolean): UserSummary[];
  loadUser?(user: UserRef): UserDetail;
  /** The status bar's TLS state and character set. */
  sessionDetails?(): SessionDetails;
}

/** What a lane's mock module exports as `default`. */
export interface MockLane {
  drivers: readonly MockDriver[];
}
