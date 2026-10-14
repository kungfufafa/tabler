import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { dialectOf, insertSql, runQuery } from "@/features/editor/controller";
import { HISTORY_KEY } from "@/features/editor/historyGroups";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import type { OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

/** What a SQL code block of a reply offers: Insert into Editor, Run in New Tab, Copy. */

function hooks(qc: QueryClient, connectionId: string) {
  return {
    onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    onSchemaChanged: () => reportHeldTables(invalidateConnectionSchema(qc, connectionId)),
  };
}

/**
 * At the cursor of the connection's active query tab, or into a new query tab when the active
 * tab is something else. Returns the tab id.
 */
export function insertIntoEditor(connection: OpenConnection, sql: string): string {
  const { connectionId, driver } = connection.info;
  const tabs = useTabsStore.getState();
  const active = tabs.tabs.find((t) => t.id === tabs.activeId);
  useUiStore.getState().setView("workspace");
  if (active?.kind === "query" && active.connectionId === connectionId) {
    void insertSql(active.id, sql);
    return active.id;
  }
  return openSqlInNewQueryTab(connectionId, sql, dialectOf(driver), "Assistant");
}

/**
 * Opens the SQL in a new query tab and runs it there, through the editor's normal run (the
 * connection's safe mode asks first when it should).
 */
export function runInNewTab(qc: QueryClient, connection: OpenConnection, sql: string): string {
  const { connectionId, driver } = connection.info;
  const tabId = openSqlInNewQueryTab(connectionId, sql, dialectOf(driver), "Assistant");
  useUiStore.getState().setView("workspace");
  void runQuery(tabId, "auto", hooks(qc, connectionId));
  return tabId;
}

export function copyText(text: string, what = "SQL"): void {
  void writeClipboard(text).then(
    () => toast.success(`Copied ${what}`),
    () => toast.error("Could not copy"),
  );
}
