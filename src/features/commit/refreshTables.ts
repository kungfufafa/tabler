import { pendingCount } from "@/features/staging/model";
import type { TableRef } from "@/lib/ipc";
import { useTableTabsStore } from "@/stores/tableTabs";
import { isTableTab, sameTable, useTabsStore } from "@/stores/tabs";

/**
 * After rows of `table` were committed (from a table tab, or from an editable query result): every
 * OTHER open table tab of the connection on that table reloads (`refreshToken`, which its row and
 * count queries are keyed by), so a split view or a second tab of the same table does not keep a
 * stale page until ⌘R (TablePlus: a commit "refreshes the open tables"). A tab holding staged edits
 * of its own keeps its page, its rows must not shift under the user; tables that were not written
 * are not reloaded (b256), nor are tabs that never loaded. `except` is the tab(s) that committed
 * (it refreshes itself; several can be named). Returns the tabs reloaded.
 */
export function refreshOpenTableTabs(
  connectionId: string,
  table: TableRef,
  except: string | readonly string[] | null = null,
): string[] {
  const skip = new Set(typeof except === "string" ? [except] : (except ?? []));
  const slices = useTableTabsStore.getState().slices;
  const refreshed: string[] = [];
  for (const tab of useTabsStore.getState().tabs) {
    if (!isTableTab(tab) || tab.connectionId !== connectionId || skip.has(tab.id)) continue;
    if (!sameTable(tab.table, table)) continue;
    const slice = slices[tab.id];
    if (!slice || pendingCount(slice.staging) > 0) continue;
    useTableTabsStore.getState().update(tab.id, (s) => ({ refreshToken: s.refreshToken + 1 }));
    refreshed.push(tab.id);
  }
  return refreshed;
}
