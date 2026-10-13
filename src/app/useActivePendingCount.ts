import { usePendingObjectOps, countOf } from "@/features/objects/pendingOps";
import { useTabPendingCount } from "@/features/objects/tabPending";
import { useConnectionsStore } from "@/stores/connections";
import { useActiveTab } from "@/stores/tabs";

/**
 * What ⌘S / discard act on: the visible tab's staged work (its row edits in the data view, its
 * staged DDL in the structure view — the other view's work stays staged meanwhile —, the staged
 * edits of its editable results in a query tab) plus the active connection's pending item
 * operations (sidebar Truncate… / Delete…, one change per table).
 */
export function useActivePendingCount(): number {
  const tab = useActiveTab();
  const connectionId = useConnectionsStore((s) => s.activeId);
  const ops = usePendingObjectOps(connectionId);
  return useTabPendingCount(tab) + countOf(ops);
}

/** The active connection's pending item operations (one change per table). */
export function useActiveObjectOpCount(): number {
  const connectionId = useConnectionsStore((s) => s.activeId);
  return countOf(usePendingObjectOps(connectionId));
}

/** The visible tab's own staged work (without the connection's pending item operations). */
export function useActiveTabPendingCount(): number {
  return useTabPendingCount(useActiveTab());
}
