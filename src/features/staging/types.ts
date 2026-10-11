import type { RowStatus } from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";

export interface RowChange {
  status: RowStatus;
  /** Snapshot of the fetched row (absent for inserted rows). */
  original?: Value[];
  /** Column index → new value. */
  patch: ReadonlyMap<number, Value>;
  /** Last apply error for this row. */
  error?: string;
}

export interface UndoEntry {
  rowKey: string;
  before: RowChange | undefined;
  insertOrderBefore?: string[];
}

export interface StagingState {
  changes: ReadonlyMap<string, RowChange>;
  /** Display order of inserted rows (appended after the fetched page). */
  insertOrder: string[];
  undo: UndoEntry[];
  version: number;
}

export const MAX_UNDO = 200;
