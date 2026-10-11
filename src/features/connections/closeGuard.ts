import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";

/**
 * Closes every tab of `connectionId` through the regular unsaved-changes guard, then runs
 * `proceed` (disconnect, switch database…). Staged edits and unsaved queries get the shared
 * "Discard changes?" prompt first; cancelling (or "Review…") keeps the tabs and skips `proceed`.
 * Resolves true when `proceed` ran.
 */
export function closeConnectionTabs(connectionId: string, proceed: () => void): Promise<boolean> {
  const ids = useTabsStore
    .getState()
    .tabs.filter((t) => t.connectionId === connectionId)
    .map((t) => t.id);
  if (useTabsStore.getState().requestClose(ids)) {
    proceed();
    return Promise.resolve(true);
  }
  const request = useTabsStore.getState().closeRequest;
  // Another close prompt is already showing; leave the connection alone.
  if (request?.kind !== "tabs" || !ids.every((id) => request.ids.includes(id)))
    return Promise.resolve(false);
  return new Promise((resolve) => {
    const unsubscribe = useTabsStore.subscribe((s) => {
      if (s.closeRequest === request) return;
      unsubscribe();
      // confirmClose clears the request before it removes the tabs: look once both happened.
      queueMicrotask(() => {
        const kept = useTabsStore.getState().tabs.some((t) => ids.includes(t.id));
        const open = useConnectionsStore.getState().byId[connectionId] !== undefined;
        if (!kept && open) proceed();
        resolve(!kept && open);
      });
    });
  });
}
