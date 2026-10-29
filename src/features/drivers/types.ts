import type { SQLDialect } from "@codemirror/lang-sql";
import type { Extension } from "@codemirror/state";
import type { ComponentType, ReactNode } from "react";
import type { UseFormReturn } from "react-hook-form";
import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { StatementRange } from "@/features/editor/scanner";
import type { StatementClass } from "@/features/safety/classify";
import type {
  ColumnMeta,
  ConnectionInfo,
  ConnectionProfile,
  DatabaseKind,
  Driver,
  SessionCaps,
  SqlDialectId,
  SslMode,
  TableKind,
  TableRef,
  Value,
} from "@/lib/ipc";

/**
 * What a lane gives the renderer (plan §2.10): its folder's `index.ts` exports one `LaneModule`
 * as `default`, which `registry.ts` loads lazily, so a lane's error breaks only that lane. The
 * core drivers have none: their UI is the app's own. Every part is optional; what a lane leaves
 * out, the app does the core way or not at all, as the session's caps say.
 */
export interface LaneModule {
  /** The new-connection form of each database of the lane (one tile each). */
  kinds?: Partial<Record<DatabaseKind, KindUi>>;
  /** The views and console of each driver of the lane. */
  drivers?: Partial<Record<Driver, DriverUi>>;
  /** The editor support of each SQL dialect of the lane. */
  dialects?: Partial<Record<SqlDialectId, DialectUi>>;
}

/** The fields of the connection form every driver may use (`connections/rows`). */
export type SharedRow = "hostPort" | "user" | "secret" | "database" | "file" | "ssl" | "socket";

/** A problem of the form a lane's `validate` reports, on the field it names. */
export interface FormIssue {
  path: keyof ConnectionFormValues | `driverParams.${string}`;
  message: string;
}

/** What a lane's own fields (`KindUi.Fields`) are given. */
export interface KindFieldsProps {
  form: UseFormReturn<ConnectionFormValues>;
  /** The database the form is for. */
  kind: DatabaseKind;
  /** Whether the profile has a stored secret (the field then says "leave empty to keep"). */
  hasStoredPassword: boolean;
}

/** The new-connection form of one database (one tile of the picker). */
export interface KindUi {
  /** The shared rows the form shows, in order, before the lane's own `Fields`. */
  rows: SharedRow[];
  /** Labels of shared rows where the database names them otherwise ("Account", "Project"). */
  labels?: Partial<Record<SharedRow, string>>;
  /** The SSL modes to offer as (mode, label); null: none (TLS is set in the lane's fields). */
  sslModes?: ReadonlyArray<readonly [SslMode, string]> | null;
  /** Form values a new profile starts from (the port, a driver param). */
  defaults?: () => Partial<ConnectionFormValues>;
  /** The lane's own fields (loaded with the lane). */
  Fields?: ComponentType<KindFieldsProps>;
  /** Problems of the values beyond the shared rows' (required params, a malformed account). */
  validate?: (values: ConnectionFormValues) => FormIssue[];
  /**
   * The profile to save: writes the lane's `options.driverParams` and clears the common fields
   * the database does not use (`base` is what the shared rows give).
   */
  toProfile?: (values: ConnectionFormValues, base: ConnectionProfile) => ConnectionProfile;
  /** The form values of a saved profile beyond the shared rows' (its driver params). */
  fromProfile?: (profile: ConnectionProfile) => Partial<ConnectionFormValues>;
  /** Connection URLs of the database: the schemes, and reading and writing one. */
  url?: {
    schemes: readonly string[];
    parse(url: string): ParsedConnectionUrl | null;
    format(profile: ConnectionProfile): string | null;
  };
  /** The connection list's one-line detail ("account.region / warehouse"). */
  summary?: (profile: ConnectionProfile) => string;
}

/** A table, collection or index the sidebar lists, as an object menu sees it. */
export interface SidebarItemRef {
  table: TableRef;
  kind: TableKind;
  /** The database it is in, where the session has several. */
  database: string | null;
}

/** What an object menu's items act with. */
export interface ObjectMenuContext {
  connectionId: string;
  info: ConnectionInfo;
  caps: SessionCaps;
  /**
   * The session's own call (`ipc.sessionCall`); a call that may write is confirmed by Safe Mode
   * (`guardExecution`) first, and refused when the connection is read-only.
   */
  call: <T = unknown>(method: string, args?: unknown) => Promise<T>;
  /** Reloads the sidebar's listing after a change. */
  refresh: () => void;
}

/** An item of a lane's object menu (sidebar ▸ right click). */
export interface ObjectMenuItem {
  id: string;
  label: string;
  /** Shown in red and confirmed first ("Flush Index…"). */
  destructive?: boolean;
  disabled?: boolean;
  run: () => void | Promise<void>;
}

/** What a lane's table view (`DriverUi.TableView`) gets in place of the grid. */
export interface TableViewProps {
  connectionId: string;
  table: TableRef;
  caps: SessionCaps;
}

/** What a lane's inspector (`DriverUi.Inspector`) gets for the selected row. */
export interface InspectorSlotProps {
  connectionId: string;
  table: TableRef;
  columns: readonly ColumnMeta[];
  /** The selected row's cells by column; null when no row is selected. */
  row: Readonly<Record<string, Value>> | null;
  /** Stages new cell values for the row (absent when the row cannot be edited). */
  onStage?: (changes: Record<string, Value>) => void;
}

/** What a lane's structure view (`DriverUi.StructureView`) gets in place of the Structure tab. */
export interface StructureViewProps {
  connectionId: string;
  table: TableRef;
  caps: SessionCaps;
  /** The Structure tab's toolbar area, for the lane's own actions. */
  toolbar?: ReactNode;
}

/** The console of a driver whose language is not SQL (redis-cli, a mongosh subset…). */
export interface ConsoleUi {
  /** The editor's language (a `StreamLanguage` from `@codemirror/language`). */
  language: () => Extension;
  /** The commands of the text, as "Run current statement" finds them (UTF-16 offsets). */
  split: (text: string) => StatementRange[];
  /** Whether a command reads or writes, for Safe Mode (unknown commands write). */
  classify: (command: string) => StatementClass;
  /** The extension of the console's files ("redis", "js"). */
  fileExtension: string;
  placeholder?: string;
}

/** The views and console of a driver beyond the core three. */
export interface DriverUi {
  console?: ConsoleUi;
  /** Replaces the data grid of a table tab. */
  TableView?: ComponentType<TableViewProps>;
  /** Replaces the row inspector (a document editor for a document store). */
  Inspector?: ComponentType<InspectorSlotProps>;
  /** Replaces the Structure tab. */
  StructureView?: ComponentType<StructureViewProps>;
  /** The driver's items of the sidebar's object menu. */
  objectMenu?: (item: SidebarItemRef, ctx: ObjectMenuContext) => ObjectMenuItem[];
}

/** The editor support of an extension SQL dialect. */
export interface DialectUi {
  label: string;
  /** CodeMirror's dialect (keywords, quoting) for highlighting and completion. */
  codemirror: () => SQLDialect;
  /** sql-formatter's language ("transactsql", "clickhouse"…); null: Format is off. */
  formatter: string | null;
  functions: readonly string[];
  columnTypes: readonly string[];
  quoteIdent: (name: string) => string;
}
