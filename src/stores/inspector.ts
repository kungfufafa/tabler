import { create } from "zustand";
import type { GridColumn } from "@/features/datagrid/types";
import type { RowKey, TableRef, Value } from "@/lib/ipc";

/** The focused cell's whole row (Row view). */
export interface InspectedRow {
  /** Every column in source order (hidden ones included). */
  columns: GridColumn[];
  cells: Value[];
  /** Source index of the focused column. */
  focused: number;
  /** Ids of columns hidden in the grid. */
  hidden?: string[];
}

/** Where the full value of a (blob) cell can be fetched from (`ipc.browse.cell`). */
export interface CellFetch {
  connectionId: string;
  table: TableRef;
  key: RowKey;
  column: string;
}

export interface InspectedCell {
  column: GridColumn;
  value: Value;
  rowIndex: number;
  source: string;
  row?: InspectedRow;
  /** Only for saved rows of a table (not for new rows or query results). */
  fetch?: CellFetch | null;
}

export type InspectorMode = "cell" | "row";

interface InspectorState {
  cell: InspectedCell | null;
  mode: InspectorMode;
  set: (cell: InspectedCell | null) => void;
  setMode: (mode: InspectorMode) => void;
}

export const useInspectorStore = create<InspectorState>()((set) => ({
  cell: null,
  mode: "cell",
  set: (cell) => set({ cell }),
  setMode: (mode) => set({ mode }),
}));
