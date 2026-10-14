import type { QueryClient } from "@tanstack/react-query";
import { activeGridMenuContext, type GridMenuContext } from "@/features/datagrid/menuExtensions";
import type { GridColumn } from "@/features/datagrid/types";
import type { TableKind, TableSummary, Value } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { isTableTab, useTabsStore } from "@/stores/tabs";
import { editorSql } from "./editorText";
import type { PluginContextKind, PluginInvocation, PluginItem, PluginRowData } from "./types";

let queryClient: QueryClient | null = null;

/** The app's query client (cached table lists tell a table from a view). Set by the host. */
export function setPluginQueryClient(qc: QueryClient | null): void {
  queryClient = qc;
}

/** The cached summary of `name` on the connection (the current schema's first). */
function cachedTable(
  connectionId: string,
  name: string,
  schema: string | null,
): TableSummary | null {
  const lists =
    queryClient?.getQueriesData<TableSummary[]>({ queryKey: ["tables", connectionId] }) ?? [];
  const matches = lists.flatMap(([, data]) => (data ?? []).filter((t) => t.name === name));
  return matches.find((t) => (t.schema ?? null) === schema) ?? matches[0] ?? null;
}

function itemOf(connectionId: string, name: string, schema: string | null): PluginItem {
  const t = cachedTable(connectionId, name, schema);
  return { name, schema: t?.schema ?? schema, type: (t?.kind ?? "table") as TableKind };
}

const rowOf = (columns: GridColumn[], cells: Value[]): PluginRowData => ({ columns, cells });

function applyGrid(inv: PluginInvocation, grid: GridMenuContext): void {
  if (grid.focusedRow) {
    inv.clickedRow = rowOf(grid.columns, grid.focusedRow);
    inv.clickedColumn = grid.focusedColumn;
  }
  inv.selectedRows = grid.selectedRows().map((cells) => rowOf(grid.columns, cells));
  inv.stageClickedCell = grid.stageFocused;
  const ctx = grid.copyContext;
  if (ctx) {
    inv.driver ??= ctx.dialect;
    inv.item ??= inv.connectionId
      ? itemOf(inv.connectionId, ctx.table, ctx.schema)
      : { name: ctx.table, schema: ctx.schema, type: "table" };
  }
}

/** The focused row of the right sidebar (a grid that is not the active one reported it). */
function applyInspector(inv: PluginInvocation): void {
  const row = useInspectorStore.getState().cell?.row;
  if (!row) return;
  inv.clickedRow = rowOf(row.columns, row.cells);
  inv.clickedColumn = row.focused;
  inv.selectedRows = [inv.clickedRow];
  const edit = row.edit;
  if (edit) inv.stageClickedCell = (value) => edit(row.focused, value);
}

/**
 * Where a command run from the Plugins menu, the palette or its key works: the active tab (its
 * table, its editor's SQL), the active grid's focused / selected rows.
 */
export function focusInvocation(
  source: PluginContextKind = "global",
  grid: GridMenuContext | null = activeGridMenuContext(),
): PluginInvocation {
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  const connections = useConnectionsStore.getState();
  const connectionId = tab?.connectionId ?? connections.activeId;
  const inv: PluginInvocation = {
    source,
    connectionId,
    driver: connectionId ? (connections.byId[connectionId]?.info.driver ?? null) : null,
    item: null,
    clickedRow: null,
    clickedColumn: null,
    selectedRows: [],
    editorTabId: null,
    sql: null,
  };
  if (tab?.kind === "query") {
    inv.editorTabId = tab.id;
    inv.sql = editorSql(tab.id);
  }
  if (isTableTab(tab) && connectionId)
    inv.item = itemOf(connectionId, tab.table.name, tab.table.schema);
  if (grid) applyGrid(inv, grid);
  else if (isTableTab(tab)) applyInspector(inv);
  return inv;
}

/**
 * A command chosen in a SQL editor's context menu: that query tab's editor (its text, the selection
 * as `context.selectedSql()`, the statement at the cursor), no grid rows. Editor insertions go to
 * the pane that was right-clicked.
 */
export function editorInvocation(tabId: string): PluginInvocation {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId);
  const connections = useConnectionsStore.getState();
  const connectionId = tab?.connectionId ?? connections.activeId;
  const query = tab?.kind === "query";
  return {
    source: "editor",
    connectionId,
    driver: connectionId ? (connections.byId[connectionId]?.info.driver ?? null) : null,
    item: null,
    clickedRow: null,
    clickedColumn: null,
    selectedRows: [],
    editorTabId: query ? tabId : null,
    sql: query ? editorSql(tabId) : null,
  };
}

/** A command chosen in a grid's cell menu: that grid's rows. */
export function gridInvocation(grid: GridMenuContext): PluginInvocation {
  return focusInvocation("grid", grid);
}

/** The sidebar item a context menu was opened on. */
export interface SidebarTarget {
  connectionId: string;
  item: PluginItem;
}

let sidebarTarget: SidebarTarget | null = null;

/**
 * Remembers the sidebar row a context menu opens on (capture-phase `contextmenu`): its name, and
 * from the cached table lists its schema and kind.
 */
export function captureSidebarTarget(target: EventTarget | null): void {
  const row =
    target instanceof Element
      ? target.closest<HTMLElement>("[data-scope='sidebar'] [data-row-index]")
      : null;
  if (!row) return;
  const name = row.querySelector("span.truncate")?.textContent?.trim();
  const connections = useConnectionsStore.getState();
  const connectionId = connections.activeId;
  if (!name || !connectionId) {
    sidebarTarget = null;
    return;
  }
  const schema = connections.byId[connectionId]?.currentSchema ?? null;
  sidebarTarget = { connectionId, item: itemOf(connectionId, name, schema) };
}

export function lastSidebarTarget(): SidebarTarget | null {
  return sidebarTarget;
}

/** A command chosen in the sidebar's item menu: that item, no grid rows. */
export function sidebarInvocation(target: SidebarTarget | null = sidebarTarget): PluginInvocation {
  const inv = focusInvocation("sidebar", null);
  inv.clickedRow = null;
  inv.clickedColumn = null;
  inv.selectedRows = [];
  inv.stageClickedCell = undefined;
  if (target) {
    inv.connectionId = target.connectionId;
    inv.driver =
      useConnectionsStore.getState().byId[target.connectionId]?.info.driver ?? inv.driver;
    inv.item = target.item;
  }
  return inv;
}
