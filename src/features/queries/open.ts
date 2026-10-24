import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { notAvailableFor } from "@/features/drivers/catalog";
import { dialectOf, insertSql, runQuery } from "@/features/editor/controller";
import { HISTORY_KEY } from "@/features/editor/historyGroups";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { openImport } from "@/features/import/store";
import { ipc, toIpcError, type SavedQuery } from "@/lib/ipc";
import type { OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

function hooks(qc: QueryClient, connectionId: string) {
  return {
    onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onSchemaChanged: () => reportHeldTables(invalidateConnectionSchema(qc, connectionId)),
  };
}

/**
 * "Open in New Tab": a new query tab of the connection, titled after the query; null when the
 * connection has no SQL editor (a driver beyond the core ones).
 */
export function openSavedQuery(connection: OpenConnection, query: SavedQuery): string | null {
  const { connectionId, driver } = connection.info;
  const dialect = dialectOf(driver);
  if (!dialect) {
    toast.error(notAvailableFor(driver, "The SQL editor"));
    return null;
  }
  const tabId = openSqlInNewQueryTab(connectionId, query.sql, dialect, query.name);
  useUiStore.getState().setView("workspace");
  return tabId;
}

/** "Run": opens the query in a new tab and runs all of it there. */
export function runSavedQuery(
  qc: QueryClient,
  connection: OpenConnection,
  query: SavedQuery,
): void {
  const tabId = openSavedQuery(connection, query);
  if (tabId) void runQuery(tabId, "auto", hooks(qc, connection.info.connectionId));
}

/**
 * "Insert to SQL Editor": at the cursor of the connection's active query tab, or into a new query
 * tab when the active tab is something else.
 */
export function insertSavedQuery(connection: OpenConnection, query: SavedQuery): void {
  const tabs = useTabsStore.getState();
  const active = tabs.tabs.find((t) => t.id === tabs.activeId);
  if (active?.kind === "query" && active.connectionId === connection.info.connectionId) {
    useUiStore.getState().setView("workspace");
    void insertSql(active.id, query.sql);
    return;
  }
  openSavedQuery(connection, query);
}

/** "Copy" (⌘C): the query's SQL. */
export function copySavedQuery(query: SavedQuery): void {
  void writeClipboard(query.sql).then(
    () => toast.success("Copied SQL"),
    () => toast.error("Could not copy"),
  );
}

/**
 * "Import…": runs the query's `.sql` file through the SQL import wizard on the connection (the
 * wizard asks to confirm, streams the file and reports errors), instead of opening it in the
 * editor. Queries live as `.sql` files in the queries folder, so the file is what the wizard
 * reads: it is never copied or sent through the editor.
 */
export async function importSavedQuery(
  connection: OpenConnection,
  query: SavedQuery,
): Promise<void> {
  try {
    const path = await ipc.savedQueries.path(query.id);
    openImport({ connectionId: connection.info.connectionId, kind: "sql", table: null }, path);
  } catch (e) {
    toast.error("Could not import the query", { description: toIpcError(e).message });
  }
}
