import type { QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";
import { create } from "zustand";
import { actions, type ActionHandler, type ActionId } from "@/features/palette/actions";
import { invalidateSchema } from "@/features/structure/invalidate";
import { sameRef } from "@/features/structure/model";
import { useStructureStore } from "@/features/structure/store";
import type { TableRef } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { onConfirmedClose, registerUnsavedSource, useTabsStore } from "@/stores/tabs";
import { armCommitChain } from "./commitChain";
import {
  commitObjectOps,
  countOf,
  discardObjectOps,
  hostTabId,
  isCommittingObjectOps,
  opTables,
  pendingObjectCount,
  pendingObjectOps,
  retargetObjectOps,
  usePendingObjectOps,
  usePendingOpsStore,
  type PendingObjectOp,
} from "./pendingOps";
import { activeTab, subscribeTabPending, tabPendingCount } from "./tabPending";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// --- the Preview SQL sheet of the pending ops --------------------------------------------------------

interface PendingSheetState {
  connectionId: string | null;
}

export const usePendingSheetStore = create<PendingSheetState>()(() => ({ connectionId: null }));

export function openPendingSheet(connectionId: string): void {
  usePendingSheetStore.setState({ connectionId });
}

export function closePendingSheet(): void {
  usePendingSheetStore.setState({ connectionId: null });
}

// --- commit / discard ----------------------------------------------------------------------------------

/** Closes the tabs of dropped tables and forgets their cached metadata. */
export function forgetTables(
  qc: QueryClient,
  connectionId: string,
  tables: readonly TableRef[],
): void {
  const ids = useTabsStore
    .getState()
    .tabs.filter(
      (t) =>
        t.kind !== "query" &&
        t.connectionId === connectionId &&
        tables.some((x) => sameRef(t.table, x)),
    )
    .map((t) => t.id);
  if (ids.length > 0) {
    useStructureStore.getState().remove(ids);
    useTabsStore.getState().forceClose(ids);
  }
  for (const t of tables)
    qc.removeQueries({ queryKey: ["table", connectionId, t.schema ?? "", t.name] });
}

function summary(ops: readonly PendingObjectOp[]): string {
  const dropped = ops.filter((o) => o.kind === "drop").flatMap((o) => opTables(o.op));
  const truncated = ops.filter((o) => o.kind === "truncate").flatMap((o) => opTables(o.op));
  const parts: string[] = [];
  const names = (list: TableRef[]) =>
    list.length === 1 ? list[0]!.name : plural(list.length, "table");
  if (dropped.length > 0) parts.push(`deleted ${names(dropped)}`);
  if (truncated.length > 0) parts.push(`truncated ${names(truncated)}`);
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Commits the connection's pending ops, then refreshes what they changed and says how it went. */
export async function runObjectCommit(qc: QueryClient, connectionId: string): Promise<void> {
  const outcome = await commitObjectOps(connectionId);
  const committed =
    outcome.status === "done" || outcome.status === "failed" ? outcome.committed : [];
  for (const o of committed) {
    const tables = opTables(o.op);
    if (o.kind === "drop") forgetTables(qc, connectionId, tables);
    else for (const t of tables) invalidateSchema(qc, connectionId, t);
  }
  if (committed.length > 0 || outcome.status === "failed") invalidateSchema(qc, connectionId);
  if (outcome.status === "done") {
    // Empty when every op was taken out while the commit ran.
    if (committed.length > 0) toast.success(summary(committed));
    if (usePendingSheetStore.getState().connectionId === connectionId) closePendingSheet();
  } else if (outcome.status === "failed") {
    toast.error(
      `Could not commit: ${opTables(outcome.failed.op)
        .map((t) => t.name)
        .join(", ")}`,
      {
        description: `${outcome.error}${
          countOf(pendingObjectOps(connectionId)) > 0 ? " — the rest stays pending." : ""
        }`,
      },
    );
  }
}

export function discardWithToast(connectionId: string, description?: string): void {
  if (isCommittingObjectOps(connectionId)) {
    toast.message("The pending item operations are being committed", {
      description: "Wait for the commit to finish.",
    });
    return;
  }
  const n = discardObjectOps(connectionId);
  if (n > 0) toast.message(`Discarded ${plural(n, "pending item operation")}`, { description });
}

/**
 * ⌘S of a view that keeps its handler while it has nothing staged (the structure view, whose
 * cells stage an edit and dispatch ⌘S in the same tick): commits the connection's pending item
 * operations instead. False when there are none.
 */
export function commitObjectOpsInstead(qc: QueryClient, connectionId: string): boolean {
  if (pendingObjectCount(connectionId) === 0) return false;
  void runObjectCommit(qc, connectionId);
  return true;
}

/** ⇧⌘P counterpart of `commitObjectOpsInstead`: their Preview SQL sheet. */
export function previewObjectOpsInstead(connectionId: string): boolean {
  if (pendingObjectCount(connectionId) === 0) return false;
  openPendingSheet(connectionId);
  return true;
}

/** Commit / Preview / Discard of the action control: the active tab's changes, then the ops. */
export function activeConnectionId(): string | null {
  return useConnectionsStore.getState().activeId;
}

/**
 * ⌘S with staged changes in the active tab AND pending ops: the tab's handler commits its
 * changes (maybe through its own Preview SQL sheet); the ops follow only once that tab reports
 * its commit went through (`reportTabCommit`). Any other outcome drops the follow-up.
 */
export function commitAfterActiveTab(qc: QueryClient, connectionId: string): boolean {
  const tab = activeTab();
  if (!tab || tab.connectionId !== connectionId || tabPendingCount(tab) === 0) return false;
  if (pendingObjectCount(connectionId) === 0) return false;
  const tabId = tab.id;
  armCommitChain({
    tabId,
    pending: () => tabPendingCount(useTabsStore.getState().tabs.find((t) => t.id === tabId)),
    alive: () => useTabsStore.getState().activeId === tabId,
    subscribe: subscribeTabPending,
    then: () => void runObjectCommit(qc, connectionId),
  });
  return true;
}

function useFallbackAction(id: ActionId, handler: ActionHandler | null): void {
  useEffect(() => {
    if (!handler) return;
    return actions.registerFallback(id, handler);
  }, [id, handler]);
}

/**
 * The action control's ⌘S / ⇧⌘P / ⇧⌘⌫ for the pending ops of the active connection: fallback
 * handlers (the active tab's own handlers win while it has staged changes) that act on the ops
 * alone, and observers that add the ops to what the tab's handler does (commit after it, discard
 * with it).
 */
export function usePendingOpActions(qc: QueryClient): void {
  const connectionId = useConnectionsStore((s) => s.activeId);
  const ops = usePendingObjectOps(connectionId);
  const has = ops.length > 0;
  useFallbackAction(
    "grid.commit",
    has && connectionId
      ? () => {
          // The tab's handler is missing although it has changes: they go first anyway.
          if (tabPendingCount(activeTab()) > 0) return;
          return runObjectCommit(qc, connectionId);
        }
      : null,
  );
  useFallbackAction(
    "grid.preview",
    has && connectionId ? () => openPendingSheet(connectionId) : null,
  );
  useFallbackAction(
    "grid.discard",
    has && connectionId ? () => discardWithToast(connectionId) : null,
  );

  useEffect(() => {
    // Observers run before whichever handler takes the action.
    const offCommit = actions.observe("grid.commit", () => {
      const c = activeConnectionId();
      if (c) commitAfterActiveTab(qc, c);
    });
    const offDiscard = actions.observe("grid.discard", () => {
      const c = activeConnectionId();
      if (c && tabPendingCount(activeTab()) > 0) discardWithToast(c);
    });
    const offPreview = actions.observe("grid.preview", () => {
      const c = activeConnectionId();
      const n = pendingObjectCount(c);
      if (c && n > 0 && tabPendingCount(activeTab()) > 0)
        toast.message(`Also pending: ${plural(n, "item operation")}`, {
          description: "Committed with ⌘S after this tab's changes.",
        });
    });
    return () => {
      offCommit();
      offDiscard();
      offPreview();
    };
  }, [qc]);
}

// --- lifecycle --------------------------------------------------------------------------------------

let installed = false;

/**
 * Once per app: counts the pending ops in the unsaved-changes guard (on one tab of their
 * connection, `hostTabId`), drops them when that tab is closed with Discard (the dialog said they
 * would be lost), keeps them with a reconnected connection, and drops them when their connection
 * closes or switches database (a toast says so).
 */
export function installPendingOps(): void {
  if (installed) return;
  installed = true;
  registerUnsavedSource((tab) => {
    const n = pendingObjectCount(tab.connectionId);
    if (n === 0) return 0;
    const s = useTabsStore.getState();
    return hostTabId(s.tabs, s.activeId, tab.connectionId) === tab.id ? n : 0;
  });
  onConfirmedClose((request) => {
    if (request.kind !== "tabs") return;
    const s = useTabsStore.getState();
    for (const id of Object.keys(usePendingOpsStore.getState().byConnection)) {
      const host = hostTabId(s.tabs, s.activeId, id);
      if (host === null || !request.ids.includes(host)) continue;
      // Even mid-commit: the ops not run yet are skipped.
      const n = discardObjectOps(id);
      const title = s.tabs.find((t) => t.id === host)?.title ?? "Their tab";
      if (n > 0)
        toast.message(`Discarded ${plural(n, "pending item operation")}`, {
          description: `“${title}” was closed with Discard.`,
        });
    }
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
