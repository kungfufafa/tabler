/**
 * Messages between the app and a plugin's sandboxed worker. Everything is structured-cloneable:
 * values are plain JSON-like data (see `snapshot.ts`).
 */

export type PlainValue = null | boolean | number | string | PlainValue[];

/** A row: column names and the values in the same order. */
export interface SnapshotRow {
  columns: string[];
  values: PlainValue[];
}

/** What the plugin can read synchronously (`context.currentItem()`, `context.clickedRow()`…). */
export interface PluginSnapshot {
  /** "grid" | "sidebar" | "editor" | "global". */
  source: string;
  /** "PostgreSQL" | "MySQL" | "SQLite", null without a connection. */
  driver: string | null;
  connection: { name: string; readOnly: boolean } | null;
  item: { name: string; schema: string | null; type: string } | null;
  clickedRow: SnapshotRow | null;
  clickedColumn: { name: string; type: string } | null;
  selectedRows: SnapshotRow[];
  sql: { text: string; selected: string; statement: string } | null;
}

/** The services a worker can ask the app for (`context.*`, `SystemService.*`). */
export const BRIDGE_METHODS = [
  "execute",
  "alert",
  "insertToEditor",
  "replaceSelection",
  "insertToClipboard",
  "openURL",
  "notify",
  "loadFile",
] as const;
export type BridgeMethod = (typeof BRIDGE_METHODS)[number];

export function isBridgeMethod(m: unknown): m is BridgeMethod {
  return typeof m === "string" && (BRIDGE_METHODS as readonly string[]).includes(m);
}

/** What `context.execute` resolves to in the worker (before rows get their `raw()` helper). */
export interface PlainExecResult {
  columns: string[];
  rows: PlainValue[][];
  rowsAffected: number | null;
}

/** What `context.loadFile` resolves to. */
export interface PlainLoadedFile {
  name: string;
  text: string;
}

export type HostMessage =
  | {
      type: "run";
      plugin: { identifier: string; name: string };
      code: string;
      /** A function the script defines, called with `context` (TablePlus's `Handler`). */
      handler: string | null;
      snapshot: PluginSnapshot;
    }
  | { type: "reply"; id: number; ok: true; value: unknown }
  | { type: "reply"; id: number; ok: false; error: string };

export type WorkerMessage =
  | { type: "call"; id: number; method: BridgeMethod; args: unknown[] }
  /** The command finished: its code returned and no call is pending. */
  | { type: "done" }
  | { type: "error"; message: string };
