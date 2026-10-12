import { pendingForTab, useResultsStore } from "@/features/results/store";
import { pendingCount } from "@/features/staging/model";
import { useStructureStore } from "@/features/structure/store";
import { useActiveTab } from "@/stores/tabs";
import { useTableTabsStore } from "@/stores/tableTabs";

/**
 * What ⌘S / discard act on in the visible tab: its staged row edits in the data view, its staged
 * DDL in the structure view (the other view's work stays staged meanwhile), the staged edits of
 * its editable results in a query tab.
 */
export function useActivePendingCount(): number {
  const tab = useActiveTab();
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
