import { ipc, type RecentEntry } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useQueryTabsStore } from "@/stores/queryTabs";

/**
 * File ▸ Open Recent is kept by the backend (shared by every window): the renderer reports what it
 * opens. Failures only cost a menu entry, so they are logged, never shown.
 */
function report(entry: RecentEntry): void {
  ipc.recent.add(entry).catch((e: unknown) => console.warn("[recent] could not record", entry, e));
}

/** An `.sql` file was opened or saved (SQL editor, SQL Viewer). */
export function trackRecentFile(path: string): void {
  if (path.trim() !== "") report({ kind: "file", path });
}

/** A saved connection was opened. */
export function trackRecentConnection(profileId: string): void {
  if (profileId !== "") report({ kind: "connection", profileId });
}

/**
 * Records connections as they open (any way: the welcome screen, ⇧⌘K, a URL, session restore) and
 * the SQL editor's files as a query tab's file changes (File ▸ Open, Save, Save As, Open Recent).
 * Returns the uninstall function.
 */
export function installRecentTracking(): () => void {
  const offConnections = useConnectionsStore.subscribe((s, prev) => {
    if (s.byId === prev.byId) return;
    for (const id of s.order) {
      const conn = s.byId[id];
      if (conn && !prev.byId[id]) trackRecentConnection(conn.info.profileId);
    }
  });
  const offFiles = useQueryTabsStore.subscribe((s, prev) => {
    if (s.slices === prev.slices) return;
    for (const [tabId, slice] of Object.entries(s.slices)) {
      const path = slice.filePath;
      if (path && path !== (prev.slices[tabId]?.filePath ?? null)) trackRecentFile(path);
    }
  });
  return () => {
    offConnections();
    offFiles();
  };
}
