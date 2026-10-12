import { useConnectionsStore } from "./connections";
import { useTabsStore, type Tab } from "./tabs";

/**
 * Keeps the visible tab and the active connection describing the same connection, so the
 * sidebar, database picker, status bar, banner, accent colour and ⌘T always follow the tab on
 * screen.
 *
 * - Tab → connection: activating a tab of another open connection (tab strip, ⌘1…9, ⌃Tab, the
 *   palette) makes that connection active.
 * - Connection → tab: activating a connection (switcher, palette, a new connect) shows the tab it
 *   last showed, else its first tab in strip order, else no tab at all (its empty workspace).
 *
 * Both directions call into the other store, so a re-entrancy flag stops the echo. Install once;
 * returns the uninstall function.
 */
export function installActiveSync(): () => void {
  /** The tab each connection showed last, by connection id. */
  const lastTab = new Map<string, string>();
  let syncing = false;

  const run = (fn: () => void) => {
    syncing = true;
    try {
      fn();
    } finally {
      syncing = false;
    }
  };

  const findTab = (tabs: readonly Tab[], id: string | null | undefined) =>
    id ? tabs.find((t) => t.id === id) : undefined;

  /** Drops entries for closed tabs; follows tabs that moved to a new connection (reconnect). */
  const prune = (tabs: readonly Tab[]) => {
    if (lastTab.size === 0) return;
    const byId = new Map(tabs.map((t) => [t.id, t]));
    for (const [connectionId, tabId] of [...lastTab]) {
      const tab = byId.get(tabId);
      if (tab?.connectionId === connectionId) continue;
      lastTab.delete(connectionId);
      if (tab && !lastTab.has(tab.connectionId)) lastTab.set(tab.connectionId, tab.id);
    }
  };

  const initial = useTabsStore.getState();
  const initialTab = findTab(initial.tabs, initial.activeId);
  if (initialTab) lastTab.set(initialTab.connectionId, initialTab.id);

  const unsubscribeTabs = useTabsStore.subscribe((s, prev) => {
    if (s.tabs !== prev.tabs) prune(s.tabs);
    const tab = findTab(s.tabs, s.activeId);
    if (!tab) return;
    const before = findTab(prev.tabs, prev.activeId);
    if (before?.id === tab.id && before.connectionId === tab.connectionId) return;
    lastTab.set(tab.connectionId, tab.id);
    if (syncing) return;
    const connections = useConnectionsStore.getState();
    if (!connections.byId[tab.connectionId] || connections.activeId === tab.connectionId) return;
    run(() => connections.setActive(tab.connectionId));
  });

  const unsubscribeConnections = useConnectionsStore.subscribe((s, prev) => {
    const connectionId = s.activeId;
    if (connectionId === prev.activeId || connectionId === null || syncing) return;
    const tabs = useTabsStore.getState();
    const current = findTab(tabs.tabs, tabs.activeId);
    if (current?.connectionId === connectionId) {
      lastTab.set(connectionId, current.id);
      return;
    }
    const last = findTab(tabs.tabs, lastTab.get(connectionId));
    const target =
      last?.connectionId === connectionId
        ? last
        : tabs.tabs.find((t) => t.connectionId === connectionId);
    run(() => (target ? tabs.activate(target.id) : tabs.clearActive()));
  });

  return () => {
    unsubscribeTabs();
    unsubscribeConnections();
  };
}
