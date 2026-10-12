import { create } from "zustand";
import type { GridColumn, RowStatus } from "@/features/datagrid/types";
import type { RowKey, TableRef, Value } from "@/lib/ipc";
import { useUiStore } from "./ui";

/** The focused cell's whole row (the right sidebar's Details view). */
export interface InspectedRow {
  /** Every column in source order (hidden ones included). */
  columns: GridColumn[];
  cells: Value[];
  /** Source index of the focused column. */
  focused: number;
  /** Ids of columns hidden in the grid. */
  hidden?: string[];
  /** Source column indexes holding a staged edit (their value boxes are highlighted). */
  changed?: ReadonlySet<number>;
  /** Staging status of the row; a row staged for deletion is shown read-only. */
  status?: RowStatus;
  /**
   * Stages `value` into source column `col` of this row, like an inline edit in the grid (table
   * tabs on an editable table only; absent for query results and read-only tables). In a bulk
   * edit it stages the value on every selected row.
   */
  edit?: (col: number, value: Value) => void;
  /**
   * Several rows are selected (TablePlus's bulk edit): `cells` holds the first one's values, and
   * the source columns in `mixed` differ between them ("Multiple values").
   */
  bulk?: BulkSelection;
}

export interface BulkSelection {
  /** Selected rows (at least two). */
  count: number;
  /** Source column indexes whose values differ between the selected rows. */
  mixed: ReadonlySet<number>;
  /** Identity of the selection (its row keys): a new one drops unstaged drafts. */
  id: string;
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
  /**
   * Where the full value of source column `col` of the row can be fetched from, when the columns
   * are not simply the table's (a query result: aliased, computed or other tables' columns). Absent:
   * `fetch` with the column's name.
   */
  fetchAt?: (col: number) => CellFetch | null;
}

/**
 * What the right sidebar shows: `"row"` is the **Details** view (every field of the focused row,
 * editable on table tabs), `"cell"` the **Cell** view (the focused cell's full value, hex dump,
 * full blob load).
 */
export type InspectorMode = "cell" | "row";

interface InspectorState {
  cell: InspectedCell | null;
  mode: InspectorMode;
  set: (cell: InspectedCell | null) => void;
  setMode: (mode: InspectorMode) => void;
}

export const useInspectorStore = create<InspectorState>()((set) => ({
  cell: null,
  // TablePlus opens its row-detail sidebar on the field list.
  mode: "row",
  set: (cell) => set({ cell }),
  setMode: (mode) => set({ mode }),
}));

/**
 * TablePlus's Space in a grid: shows / hides the right-hand detail panel, opening it on the focused
 * row (Details view) rather than on the single cell.
 */
export function toggleRowDetail(): void {
  const ui = useUiStore.getState();
  if (!ui.inspectorOpen) useInspectorStore.getState().setMode("row");
  ui.toggleInspector();
}

/**
 * The inspected cell moved to source column `col` of its row (the Cell view of another field).
 * Returns `cell` itself when it has no such column.
 */
export function cellAtColumn(cell: InspectedCell, col: number): InspectedCell {
  const row = cell.row;
  const column = row?.columns[col];
  if (!row || !column || col === row.focused) return cell;
  return {
    ...cell,
    column,
    value: row.cells[col] ?? { t: "null" },
    row: { ...row, focused: col },
    fetch: cell.fetchAt
      ? cell.fetchAt(col)
      : cell.fetch
        ? { ...cell.fetch, column: column.name }
        : cell.fetch,
  };
}
