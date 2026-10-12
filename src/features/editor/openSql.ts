import { useTabsStore } from "@/stores/tabs";
import { seedEditor } from "./editorStates";
import type { SqlDialect } from "./scanner";

/**
 * Opens `sql` in a new query tab of the connection (e.g. a table's CREATE statement from the
 * structure view) and returns the tab id. The editor is seeded right after the tab is created,
 * before it mounts, so it shows the text from its first render (as `openQueryFile` does).
 */
export function openSqlInNewQueryTab(
  connectionId: string,
  sql: string,
  dialect: SqlDialect,
  title?: string,
): string {
  const tabs = useTabsStore.getState();
  const id = tabs.newQuery(connectionId);
  seedEditor(id, sql, { dialect });
  if (title) tabs.rename(id, title);
  return id;
}
