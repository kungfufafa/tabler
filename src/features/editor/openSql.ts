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
  dialect: SqlDialect | null,
  title?: string,
): string {
  const tabs = useTabsStore.getState();
  const id = tabs.newQuery(connectionId);
  // A lane's language (null) is set when the editor mounts.
  seedEditor(id, sql, dialect ? { dialect } : undefined);
  if (title) tabs.rename(id, title);
  return id;
}

/**
 * The query tab each sidebar object (a routine, a sequence) was last opened in, by `objectKey`.
 * A single click opens the object, so clicking it again while browsing must show that tab rather
 * than pile up copies of it.
 */
const objectTabs = new Map<string, string>();

/**
 * The open query tab of the connection that `objectKey` was opened in (`openSqlInObjectTab`),
 * activated; null when there is none (never opened, or the tab was closed since).
 */
export function activateObjectTab(connectionId: string, objectKey: string): string | null {
  const key = `${connectionId}\u0000${objectKey}`;
  const id = objectTabs.get(key);
  if (id === undefined) return null;
  const tabs = useTabsStore.getState();
  const tab = tabs.tabs.find((t) => t.id === id);
  if (!tab || tab.kind !== "query" || tab.connectionId !== connectionId) {
    objectTabs.delete(key);
    return null;
  }
  tabs.activate(id);
  return id;
}

/**
 * `openSqlInNewQueryTab` for a sidebar object: remembers the tab under `objectKey` so the next
 * open of the same object shows it again (`activateObjectTab`).
 */
export function openSqlInObjectTab(
  connectionId: string,
  objectKey: string,
  sql: string,
  dialect: SqlDialect | null,
  title?: string,
): string {
  const id = openSqlInNewQueryTab(connectionId, sql, dialect, title);
  objectTabs.set(`${connectionId}\u0000${objectKey}`, id);
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
  dialect: SqlDialect | null,
): string {
  const id = useTabsStore.getState().newQuery(connectionId);
  seedEditor(id, file.text, dialect ? { dialect } : undefined);
  linkTabToFile(id, path, file);
  return id;
}
