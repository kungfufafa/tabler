import { openSqlFile } from "@/features/deeplink/open";
import { openSqlViewer } from "@/features/sqlviewer/open";
import { useConnectionsStore } from "@/stores/connections";
import type { RecentPick } from "./menuIds";

/**
 * Opens an Open Recent entry: a connection is shown (or connected); a file opens in a query tab of
 * the active connection (its tab when it is open already), or in the SQL Viewer when no
 * connection is open.
 */
export function openRecent(pick: RecentPick, openProfile: (profileId: string) => void): void {
  if (pick.kind === "connection") {
    openProfile(pick.profileId);
    return;
  }
  const active = useConnectionsStore.getState().activeId;
  if (active) void openSqlFile(active, pick.path);
  else openSqlViewer(pick.path);
}
