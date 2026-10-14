import type { GridColumn } from "@/features/datagrid/types";
import type { Driver, PluginCommand, PluginContextKind, TableKind, Value } from "@/lib/ipc";

export type { PluginContextKind };

/** The table / view a command works on (the clicked sidebar item, the tab's table). */
export interface PluginItem {
  name: string;
  schema: string | null;
  type: TableKind;
}

/** A grid row: its columns (every one, hidden ones included) and their values. */
export interface PluginRowData {
  columns: GridColumn[];
  cells: Value[];
}

/** The SQL editor of a query tab: its whole text, the selection, the statement at the cursor. */
export interface PluginSql {
  text: string;
  selected: string;
  statement: string;
}

/** Everything a command can read about where it runs (TablePlus's `context`). */
export interface PluginInvocation {
  /** Where it was started: a grid's / the sidebar's menu, the editor, or the Plugins menu. */
  source: PluginContextKind;
  connectionId: string | null;
  driver: Driver | null;
  item: PluginItem | null;
  /** The clicked (focused) row of the grid. */
  clickedRow: PluginRowData | null;
  /** Index of the clicked column in `clickedRow.columns`. */
  clickedColumn: number | null;
  selectedRows: PluginRowData[];
  /** The query tab whose editor `sql` comes from and editor insertions go to. */
  editorTabId: string | null;
  sql: PluginSql | null;
  /** Stages `value` into the clicked cell (editable table grids only). */
  stageClickedCell?: (value: Value) => void;
}

/** A built-in plugin command: TypeScript running in the app (not sandboxed, trusted). */
export interface BuiltinCommand {
  name: string;
  key?: string;
  contexts: PluginContextKind[];
  /** A keymap action that owns the key (Settings ▸ Shortcuts can remap it). */
  action?: "plugins.sql-format";
  run: (inv: PluginInvocation, host: HostServices) => void | Promise<void>;
}

export interface BuiltinPlugin {
  identifier: string;
  name: string;
  version: string;
  author: string;
  description: string;
  commands: BuiltinCommand[];
}

/** One plugin as the manager and the menus see it, built-in or installed. */
export interface PluginEntry {
  identifier: string;
  name: string;
  version: string;
  author: string;
  description: string;
  builtin: boolean;
  /** Installed plugins: the manifest's commands. */
  commands: Array<Pick<PluginCommand, "name" | "key" | "contexts">>;
  /** Installed plugins: their folder. */
  folder?: string;
}

/** `<identifier>#<command index>`: how menus, the palette and keys name a command. */
export type RunId = string;

export function runIdOf(identifier: string, index: number): RunId {
  return `${identifier}#${index}`;
}

export function parseRunId(run: string): { identifier: string; index: number } | null {
  const at = run.lastIndexOf("#");
  if (at <= 0) return null;
  const index = Number(run.slice(at + 1));
  if (!Number.isInteger(index) || index < 0) return null;
  return { identifier: run.slice(0, at), index };
}

/** What a plugin's `context.execute` returns (rows of the last statement that returned rows). */
export interface ExecResult {
  columns: string[];
  rows: Value[][];
  /** Rows the last statement changed, when it reports it. */
  rowsAffected: number | null;
  statements: number;
}

/** A file the user picked for `context.loadFile()`. */
export interface LoadedFile {
  name: string;
  text: string;
}

/** What `runInWorker` hands the services of one sandboxed run (see `createHostServices`). */
export interface RunHooks {
  /** Pauses the run's time-out while the user answers something (Safe Mode). */
  userWait: <T>(p: Promise<T>) => Promise<T>;
  /** A query started for the run (cancelled when the run is stopped). */
  onQuery: (connectionId: string, queryId: string) => void;
  /** Tags the dialogs the run opens: they are dismissed when it ends. */
  runId: number;
  /**
   * Shows one dialog for the run (an alert, a link confirmation, the file dialog): the time-out
   * pauses while it is open, and a run that used up its dialogs ends instead of opening another.
   */
  dialog: <T>(show: () => Promise<T>) => Promise<T>;
  /** Whether the run already ended (stopped by a limit, failed, done). */
  stopped: () => boolean;
}

/** The services a command can use (the bridge offers the same ones to sandboxed plugins). */
export interface HostServices {
  /** Runs SQL on the invocation's connection, through Safe Mode (`guardExecution`). */
  execute: (sql: string) => Promise<ExecResult>;
  alert: (title: string, message: string) => Promise<void>;
  insertToEditor: (text: string) => Promise<void>;
  replaceSelection: (text: string) => Promise<void>;
  insertToClipboard: (text: string) => Promise<void>;
  /** http / https only; asks the first time per plugin. */
  openURL: (url: string) => Promise<void>;
  notify: (title: string, message: string) => void;
  /**
   * Opens the native file dialog and returns the text file the user picked (at most 5 MB, UTF-8).
   * Once per command run; rejects with `PluginCancelled` when the dialog is cancelled.
   */
  loadFile: () => Promise<LoadedFile>;
}
