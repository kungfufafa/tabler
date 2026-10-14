import { useConnectionsStore } from "@/stores/connections";
import { onConfirmedClose, useTabsStore } from "@/stores/tabs";

/**
 * Closes every tab of `connectionId` through the regular unsaved-changes guard, then runs
 * `proceed` (disconnect, switch database…). Staged edits, unsaved queries and the connection's
 * staged sidebar operations get the shared "Discard changes?" prompt first, also when the
 * connection has no tab open; cancelling (or "Review…") keeps everything and skips `proceed`.
 * Resolves true when `proceed` ran.
 */
export function closeConnectionTabs(connectionId: string, proceed: () => void): Promise<boolean> {
  const ids = useTabsStore
    .getState()
    .tabs.filter((t) => t.connectionId === connectionId)
    .map((t) => t.id);
  if (useTabsStore.getState().requestCloseConnection(connectionId)) {
    proceed();
    return Promise.resolve(true);
  }
  const request = useTabsStore.getState().closeRequest;
  // Another close prompt is already showing; leave the connection alone.
  if (request?.kind !== "tabs" || !ids.every((id) => request.ids.includes(id)))
    return Promise.resolve(false);
  return new Promise((resolve) => {
    // Only a confirmed Discard goes on: with no tab open, "nothing kept" says nothing.
    let confirmed = false;
    const stopListening = onConfirmedClose((r) => {
      if (r === request) confirmed = true;
    });
    const unsubscribe = useTabsStore.subscribe((s) => {
      if (s.closeRequest === request) return;
      unsubscribe();
      // confirmClose clears the request, THEN tells the hooks and removes the tabs: decide once
      // all of that happened (the hook is still listening until then).
      queueMicrotask(() => {
        stopListening();
        const kept = useTabsStore.getState().tabs.some((t) => ids.includes(t.id));
        const open = useConnectionsStore.getState().byId[connectionId] !== undefined;
        const go = confirmed && !kept && open;
        if (go) proceed();
        resolve(go);
      });
    });
  });
}
