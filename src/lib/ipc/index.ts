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
import { invoke } from "./transport";

export { IpcError, isMock, isTauri, toIpcError } from "./transport";
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

/** Typed facade over every Tauri command. The renderer never calls `invoke` directly. */
export const ipc = {
  appInfo: () => invoke<AppInfo>("app_info"),

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
  },

  connections: {
    connect: (profileId: string) => invoke<ConnectionInfo>("connect", { profileId }),
    disconnect: (connectionId: string) => invoke<void>("disconnect", { connectionId }),
    list: () => invoke<ConnectionInfo[]>("list_connections"),
    ping: (connectionId: string) => invoke<ConnStatus>("ping", { connectionId }),
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

  mutate: {
    preview: (connectionId: string, req: ApplyRequest) =>
      invoke<string[]>("preview_changes", { connectionId, req }),
    apply: (connectionId: string, req: ApplyRequest) =>
      invoke<ApplyResult>("apply_changes", { connectionId, req }),
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
