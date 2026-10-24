import { coreDialect } from "@/features/drivers/catalog";
import type { SqlDialect } from "@/features/editor/scanner";
import { findToolTab, openToolTab } from "@/features/tabs/toolTabs";
import type { Driver } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { focusViewer } from "./buffers";
import { loadViewerFile } from "./files";
import { STANDALONE, ensureDoc, newDoc, patchDoc, setStandaloneOpen } from "./store";

/** The tool tab kind of the SQL Viewer (`registerToolTab`). */
export const SQL_VIEWER_TOOL = "sqlviewer";

/** A viewer tab's params: the file it shows (null: untitled). Saved with the session. */
export interface SqlViewerParams {
  path: string | null;
}

/** Checks restored params: only viewers of a file come back (an untitled buffer is not kept). */
export function restoreViewerParams(raw: unknown): SqlViewerParams | null {
  const path = (raw as { path?: unknown } | null)?.path;
  return typeof path === "string" && path.trim() !== "" ? { path } : null;
}

/**
 * Whether a viewer tab is saved with the session: the same test the restore makes, so a tab that
 * could not come back (an untitled viewer) is not saved to be reported as "No longer there".
 */
export function keepsViewerTab(params: SqlViewerParams | null): boolean {
  return restoreViewerParams(params) !== null;
}

/**
 * The highlighting a new viewer starts with: the active connection's, else PostgreSQL. The viewer
 * has the core dialects only, so another driver's connection starts it on PostgreSQL too (its
 * Dialect menu changes it; nothing runs from a viewer).
 */
export function defaultDialect(driver: Driver | null | undefined): SqlDialect {
  return (driver ? coreDialect(driver) : null) ?? "postgres";
}

/**
 * File ▸ New SQL Viewer (⇧⌘O), or a `.sql` file to view: a viewer tab in the active workspace
 * (the tab that shows `path` already, when there is one), or the standalone viewer when no
 * connection is open (tabs belong to a connection's workspace).
 */
export function openSqlViewer(path: string | null = null): void {
  const active = useConnectionsStore.getState().activeId;
  if (active) {
    openToolTab<SqlViewerParams>(active, SQL_VIEWER_TOOL, { path }, { reuse: path !== null });
    return;
  }
  openStandaloneViewer(path);
}

/** Shows the standalone viewer, with `path` read into it (once its unsaved changes are dealt with). */
export function openStandaloneViewer(path: string | null = null): void {
  const doc = ensureDoc(STANDALONE, () => newDoc(null));
  setStandaloneOpen(true);
  if (!path || doc.path === path) return;
  if (doc.dirty) patchDoc(STANDALONE, { pendingPath: path });
  else void loadViewerFile(STANDALONE, path);
}

/**
 * A file picked with Open… in viewer tab `tabId`: the tab that shows it already comes to the
 * front; a clean viewer reads it in place; one with unsaved changes leaves them and the file
 * opens in a new viewer tab.
 */
export function openPathFromViewerTab(tabId: string, path: string): void {
  const store = useTabsStore.getState();
  const tab = store.tabs.find((t) => t.id === tabId);
  if (!tab) return;
  const other = findToolTab(store.tabs, tab.connectionId, SQL_VIEWER_TOOL, { path });
  if (other && other.id !== tabId) {
    store.activate(other.id);
    return;
  }
  const doc = ensureDoc(tabId, () => newDoc(null));
  if (doc.dirty) {
    openToolTab<SqlViewerParams>(tab.connectionId, SQL_VIEWER_TOOL, { path }, { reuse: false });
    return;
  }
  void loadViewerFile(tabId, path).then(() => focusViewer(tabId));
}
