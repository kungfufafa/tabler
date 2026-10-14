import { useActiveWorkspaceId, useWorkspacePendingCount } from "@/features/commit/workspace";

/**
 * What ⌘S / discard act on: every pending change of the active workspace (TablePlus §9) — the
 * staged rows of all its table tabs, the staged edits of its query results, its staged structure
 * and the sidebar's pending item operations (one change per table, routine or sequence).
 */
export function useActivePendingCount(): number {
  return useWorkspacePendingCount(useActiveWorkspaceId());
}
