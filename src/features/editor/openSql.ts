import { updateQueryTab } from "@/stores/queryTabs";
import { useTabsStore } from "@/stores/tabs";
import { seedEditor } from "./editorStates";
import { baseName, headTitle, type SqlFileRead } from "./files";
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

/**
 * Links a query tab to the file it was read from: the tab is titled after it and saves to it. The
 * head of a big file (`readOnlyHead`) is a copy instead: no file behind the tab, a notice above
 * the editor and a title of its own, so saving can never truncate the original.
 */
export function linkTabToFile(tabId: string, path: string, file: SqlFileRead): void {
  updateQueryTab(tabId, {
    filePath: file.readOnlyHead ? null : path,
    headNotice: file.headNotice,
  });
  useTabsStore.getState().rename(tabId, file.readOnlyHead ? headTitle(path) : baseName(path));
}

/** Opens the text read from `path` (`readSqlFileGuarded`) in a new query tab of the connection. */
export function openFileInNewQueryTab(
  connectionId: string,
  path: string,
  file: SqlFileRead,
  dialect: SqlDialect,
): string {
  const id = useTabsStore.getState().newQuery(connectionId);
  seedEditor(id, file.text, { dialect });
  linkTabToFile(id, path, file);
  return id;
}
