import type { QueryClient } from "@tanstack/react-query";
import type { TableRef } from "@/lib/ipc";
import { isTableTab, useTabsStore } from "@/stores/tabs";
import { invalidateSchema } from "./invalidate";
import { sameRef } from "./model";
import { useStructureStore } from "./store";

/** Tabs (data and structure) showing `table` on `connectionId`. */
export function tabsOf(connectionId: string, table: TableRef): string[] {
  return useTabsStore
    .getState()
    .tabs.filter((t) => isTableTab(t) && t.connectionId === connectionId && sameRef(t.table, table))
    .map((t) => t.id);
}

/**
 * After `from` was renamed to `newName`: its open tabs follow it (and so do their staged
 * structure edits), its cached metadata goes, and everything listing tables refetches. Returns
 * the new reference.
 */
export function followRename(
  qc: QueryClient,
  connectionId: string,
  from: TableRef,
  newName: string,
): TableRef {
  const ids = tabsOf(connectionId, from);
  const renamed: TableRef = { schema: from.schema, name: newName };
  useStructureStore.getState().retarget(ids, from, renamed);
  useTabsStore.setState((s) => ({
    tabs: s.tabs.map((t) =>
      ids.includes(t.id) && isTableTab(t) ? { ...t, table: renamed, title: newName } : t,
    ),
  }));
  qc.removeQueries({ queryKey: ["table", connectionId, from.schema ?? "", from.name] });
  qc.removeQueries({ queryKey: ["tableDdl", connectionId, from.schema ?? "", from.name] });
  invalidateSchema(qc, connectionId, renamed);
  return renamed;
}
