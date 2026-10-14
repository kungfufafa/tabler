import { pendingForTab, useResultsStore } from "@/features/results/store";
import { pendingCount } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { useTabsStore, type Tab } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";

/**
 * What ⌘S / discard act on in `tab`: its staged row edits in the data view, its staged DDL in the
 * structure view (the other view's work stays staged meanwhile), the staged edits of its editable
 * results in a query tab. Read outside React (`useActivePendingCount` is the hook).
 */
export function tabPendingCount(tab: Tab | null | undefined): number {
  if (!tab) return 0;
  if (tab.kind === "table") {
    const slice = useTableTabsStore.getState().slices[tab.id];
    return slice ? pendingCount(slice.staging) : 0;
  }
  if (tab.kind === "structure") return useStructureStore.getState().byTab[tab.id]?.ops.length ?? 0;
  return pendingForTab(useResultsStore.getState(), tab.id);
}

/** `tabPendingCount` as a hook (re-renders when the count changes). */
export function useTabPendingCount(tab: Tab | null | undefined): number {
  const rows = useTableTabsStore((s) => {
    if (tab?.kind !== "table") return 0;
    const slice = s.slices[tab.id];
    return slice ? pendingCount(slice.staging) : 0;
  });
  const ddl = useStructureStore((s) =>
    tab?.kind === "structure" ? (s.byTab[tab.id]?.ops.length ?? 0) : 0,
  );
  const results = useResultsStore((s) => (tab?.kind === "query" ? pendingForTab(s, tab.id) : 0));
  return rows + ddl + results;
}

export function activeTab(): Tab | null {
  const s = useTabsStore.getState();
  return s.tabs.find((t) => t.id === s.activeId) ?? null;
}

/** Calls `fn` whenever a store `tabPendingCount` reads (or the active tab) changes. */
export function subscribeTabPending(fn: () => void): () => void {
  const off = [
    useTabsStore.subscribe(fn),
    useTableTabsStore.subscribe(fn),
    useStructureStore.subscribe(fn),
    useResultsStore.subscribe(fn),
  ];
  return () => {
    for (const f of off) f();
  };
}
