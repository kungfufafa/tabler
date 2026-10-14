import type { QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";
import {
  commitWorkspace,
  discardWorkspace,
  setCommitQueryClient,
  useActiveWorkspaceId,
  useWorkspacePendingCount,
} from "@/features/commit/workspace";
import { openWorkspaceSheet } from "@/features/commit/store";
import { retainRowContexts } from "@/features/commit/rowContext";
import { useAction } from "@/features/palette/actions";
import { useConnectionsStore } from "@/stores/connections";
import { onConfirmedClose, registerConnectionWork, useTabsStore } from "@/stores/tabs";
import {
  discardObjectOps,
  pendingObjectCount,
  retargetObjectOps,
  usePendingOpsStore,
} from "./pendingOps";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// --- the workspace commit's actions --------------------------------------------------------------------

/** The Preview SQL sheet (⇧⌘P) of the connection's whole workspace. */
export function openPendingSheet(connectionId: string): void {
  openWorkspaceSheet(connectionId);
}

/**
 * ⌘S / ⇧⌘P / ⇧⌘⌫ (the action control, the native Commit / Preview / Discard items) for the active
 * connection's workspace: every pending change of every tab and the sidebar's pending operations
 * are committed, previewed and discarded together (`features/commit/workspace.ts`). Registered
 * while the workspace holds changes.
 */
export function usePendingOpActions(qc: QueryClient): void {
  const connectionId = useActiveWorkspaceId();
  const count = useWorkspacePendingCount(connectionId);
  const has = count > 0 && connectionId !== null;
  useEffect(() => {
    setCommitQueryClient(qc);
  }, [qc]);
  useAction("grid.commit", has ? () => void commitWorkspace(connectionId) : null);
  useAction("grid.preview", has ? () => openWorkspaceSheet(connectionId) : null);
  useAction("grid.discard", has ? () => void discardWorkspace(connectionId) : null);
}

// --- lifecycle --------------------------------------------------------------------------------------

let installed = false;

/**
 * Once per app: counts the pending ops as their CONNECTION's unsaved work (closing tabs keeps
 * them; disconnect, a database switch and quit ask first, also with no tab open), drops them when
 * such a close is confirmed with Discard, keeps them with a reconnected connection, and drops
 * them when their connection closes or switches database otherwise (a toast says so).
 */
export function installPendingOps(): void {
  if (installed) return;
  installed = true;
  registerConnectionWork(() => {
    const byId = useConnectionsStore.getState().byId;
    return Object.keys(usePendingOpsStore.getState().byConnection).map((connectionId) => ({
      connectionId,
      title: `Pending item operations on ${byId[connectionId]?.info.name ?? "the connection"}`,
      count: pendingObjectCount(connectionId),
    }));
  });
  onConfirmedClose((request) => {
    for (const entry of request.dirty) {
      if (entry.connectionId === undefined) continue;
      // Even mid-commit: the ops not run yet are skipped.
      const n = discardObjectOps(entry.connectionId);
      if (n > 0 && request.kind === "tabs")
        toast.message(`Discarded ${plural(n, "pending item operation")}`, {
          description: "The connection's tabs were closed with Discard.",
        });
    }
  });
  // What the staged rows of every table tab were staged against (`features/commit/rowContext`)
  // outlives the tab's view, not the tab (recently closed ones keep theirs, like their slices).
  useTabsStore.subscribe((s, prev) => {
    if (s.tabs === prev.tabs && s.closed === prev.closed) return;
    retainRowContexts(new Set([...s.tabs, ...s.closed].map((t) => t.id)));
  });
  // `hasUnsaved` consumers (the window-close guard, the quit reporter) re-check on tab-store
  // changes only: tell them when the pending ops change.
  usePendingOpsStore.subscribe((s, prev) => {
    if (s.byConnection !== prev.byConnection) useTabsStore.setState({});
  });
  useConnectionsStore.subscribe((s, prev) => {
    if (s.byId === prev.byId) return;
    const ops = usePendingOpsStore.getState().byConnection;
    for (const id of Object.keys(ops)) {
      const before = prev.byId[id];
      const now = s.byId[id];
      if (now && before && now.info.currentDatabase === before.info.currentDatabase) continue;
      if (!now && before) {
        // A reconnect swaps the connection for a new one of the same profile and database.
        const next = Object.values(s.byId).find(
          (c) =>
            !prev.byId[c.info.connectionId] &&
            c.info.profileId === before.info.profileId &&
            c.info.currentDatabase === before.info.currentDatabase,
        );
        if (next) {
          retargetObjectOps(id, next.info.connectionId);
          continue;
        }
      }
      const n = discardObjectOps(id);
      if (n > 0)
        toast.message(`Discarded ${plural(n, "pending item operation")}`, {
          description: now
            ? `${now.info.name} switched to another database.`
            : `${before?.info.name ?? "The connection"} was closed.`,
        });
    }
  });
}
