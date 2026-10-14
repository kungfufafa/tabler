import type { WindowWork } from "@/stores/tabs";
import { dropViewerBuffer, viewerBufferKeys } from "./buffers";
import { viewerTitle } from "./labels";
import { SQL_VIEWER_TOOL } from "./open";
import { STANDALONE, dropDocs, getDoc, setStandaloneOpen, useSqlViewerStore } from "./store";

/** A viewer tab with unsaved changes counts in every close / quit guard (one change). */
export function viewerUnsavedCount(tab: { kind: string; id: string; tool?: string }): number {
  return tab.kind === "tool" && tab.tool === SQL_VIEWER_TOOL && getDoc(tab.id)?.dirty ? 1 : 0;
}

/**
 * The standalone viewer's unsaved changes: it belongs to no tab, so the window's close / quit
 * guard lists it itself (registered with `registerWindowWork`).
 */
export function standaloneUnsavedWork(): WindowWork[] {
  const doc = getDoc(STANDALONE);
  if (!useSqlViewerStore.getState().standaloneOpen || !doc?.dirty) return [];
  return [{ id: "sql-viewer", title: viewerTitle(doc), count: 1 }];
}

/** Drops the buffers of viewer tabs that are gone (closed, moved to another window). */
export function dropClosedViewers(openTabIds: ReadonlySet<string>): void {
  const keys = new Set([...viewerBufferKeys(), ...Object.keys(useSqlViewerStore.getState().docs)]);
  const gone = [...keys].filter((k) => k !== STANDALONE && !openTabIds.has(k));
  for (const k of gone) dropViewerBuffer(k);
  dropDocs(gone);
}

/** Forgets the standalone viewer (it was closed). */
export function closeStandalone(): void {
  setStandaloneOpen(false);
  dropViewerBuffer(STANDALONE);
  dropDocs([STANDALONE]);
}
