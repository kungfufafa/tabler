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

/** One undo step: the rows it touched, as they were before (several rows for batch edits). */
export interface UndoEntry {
  rows: Array<{ rowKey: string; before: RowChange | undefined }>;
  insertOrderBefore?: string[];
}

/** One cell of a batch edit (`setCells`); `col` is the source column index. */
export interface CellEdit {
  rowKey: string;
  col: number;
  value: Value;
  original: Value[] | undefined;
}

export interface StagingState {
  changes: ReadonlyMap<string, RowChange>;
  /** Display order of inserted rows (appended after the fetched page). */
  insertOrder: string[];
  undo: UndoEntry[];
  version: number;
}

export const MAX_UNDO = 200;
