import { loadExportOptions } from "@/features/export/options";
import { openExport, setExportDraft, useExportStore } from "@/features/export/store";
import type { GridColumn } from "@/features/datagrid/types";
import { isMock, type ColumnMeta, type FilterGroup, type TableRef, type Value } from "@/lib/ipc";

/**
 * Set Value ▸ Add a File…: the native open panel. The browser mock has no file dialog, so it asks
 * for a path (see the mock's `read_binary_file`).
 */
export async function pickBinaryFile(): Promise<string | null> {
  if (isMock) {
    const path = window.prompt("Add a file — path", "/Users/me/Pictures/avatar.png");
    return path?.trim() || null;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ title: "Add a File", multiple: false, directory: false });
  return typeof picked === "string" ? picked : null;
}

/**
 * Cell menu ▸ Export Current Page…: the export dialog on the tab's table, preset to "Only current
 * page" (unless an export is already running: the dialog then shows its progress).
 */
export function openExportCurrentPage(connectionId: string, table: TableRef, tabId: string): void {
  const running = useExportStore.getState().run !== null;
  openExport({ connectionId, source: { kind: "table", table, tabId } });
  if (running) return;
  const options = loadExportOptions();
  setExportDraft({
    options,
    scope: "page",
    visibleOnly: true,
    tableName: "",
    batchText: String(options.sql.batchSize),
    allRows: false,
  });
}

/** The `=` filter Quick Look's foreign-key viewer browses the referenced table with. */
export function referencedFilter(column: GridColumn, value: Value): FilterGroup | null {
  const fk = column.fk;
  if (!fk || value.t === "null" || value.t === "default") return null;
  return {
    conjunction: "and",
    items: [{ kind: "condition", column: fk.column, op: "eq", values: [value] }],
  };
}

/** `schema.name` (or the name alone) for labels. */
export function qualifiedName(table: TableRef): string {
  return table.schema ? `${table.schema}.${table.name}` : table.name;
}

/** Column labels for a referenced row. */
export function referencedColumns(
  columns: ColumnMeta[],
): Array<{ name: string; dataType: string }> {
  return columns.map((c) => ({ name: c.name, dataType: c.dataType }));
}
