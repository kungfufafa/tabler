import { QueryClientContext, type QueryClient } from "@tanstack/react-query";
import { useContext, useEffect } from "react";
import { toast } from "sonner";
import { openConnectionDialog } from "@/features/connections/store";
import { openNewQueryTab } from "@/features/editor/newTab";
import { cyclePane } from "@/features/navigate/panes";
import { openSqlViewer } from "@/features/sqlviewer/open";
import { pendingCount } from "@/features/staging/model";
import { cycleWorkspace } from "@/features/tabs/workspaces";
import { requestWindowClose } from "@/hooks/useUnsavedGuard";
import { resolveScope } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { useTableTabsStore } from "@/stores/tableTabs";
import { isTableTab, useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { reloadSchema } from "./commands";
import { usePaletteStore } from "./store";

/**
 * File ▸ Reload Workspace (⌘R, TablePlus): the active tab reloads as with ⌥⌘R (asking first when
 * it holds staged edits), and the sidebar with it (`view.refresh` observers); the workspace's other
 * table tabs refetch their rows when they show next (one holding staged edits keeps them, and its
 * rows); and the connection's schema caches are dropped: sidebar lists, table structures, enums
 * and the editor's completion.
 */
export async function reloadWorkspace(qc: QueryClient | undefined): Promise<void> {
  const connectionId = useConnectionsStore.getState().activeId;
  await actions.run("view.refresh");
  if (!connectionId) return;
  const { tabs, activeId } = useTabsStore.getState();
  const tables = useTableTabsStore.getState();
  for (const tab of tabs) {
    if (tab.id === activeId || tab.connectionId !== connectionId || !isTableTab(tab)) continue;
    const slice = tables.slices[tab.id];
    if (!slice || pendingCount(slice.staging) > 0) continue;
    tables.update(tab.id, (s) => ({ refreshToken: s.refreshToken + 1 }));
  }
  if (!qc) return;
  try {
    await reloadSchema(qc, connectionId);
  } catch (e) {
    toast.error("Could not reload the workspace", {
      description: e instanceof Error ? e.message : String(e),
    });
  }
}

/**
 * ⌘↩ with the focus outside the SQL editor, a grid and the filter bar (TablePlus): a SQL tab runs
 * its current statement; any other tab, or none, opens the SQL editor (like ⌘E).
 */
export async function openEditorFromTab(): Promise<void> {
  if (useUiStore.getState().view !== "workspace") return;
  const { tabs, activeId } = useTabsStore.getState();
  const tab = tabs.find((t) => t.id === activeId);
  if (tab?.kind === "query") {
    await actions.run("query.run-current");
    return;
  }
  await actions.run("tab.new-query");
}

/** Store-backed actions that are always available (tabs, panels, launcher, palette). */
export function useDefaultActions(): void {
  // The app's query client (absent in unit tests that mount these actions alone).
  const qc = useContext(QueryClientContext);
  useEffect(() => {
    const tabs = useTabsStore.getState;
    const ui = useUiStore.getState;
    const workspace = () => useConnectionsStore.getState().activeId;
    const unregister = [
      actions.register("tab.close", () => {
        // ⌘W is allowed inside dialogs so Settings can claim it (its handler stacks on top while
        // open). Any other modal (palette, Preview SQL, connection form…) must not close the tab
        // behind it, nor may an editor inside one (Bootstrap commands, a trigger's body).
        if (resolveScope(document.activeElement) === "dialog") return;
        // TablePlus ⌘W: "Close tab/window". The tab of the workspace on screen closes; with none to
        // close (the welcome screen or Connection Manager, an empty workspace, a ⌘N window) the
        // window closes through the unsaved-work guard. The Connection Manager hides the open
        // tabs, so ⌘W there must not close one of them unseen.
        const { tabs: open, activeId } = tabs();
        const connectionId = workspace();
        const shown =
          ui().view === "workspace"
            ? open.find(
                (t) =>
                  t.id === activeId && (connectionId === null || t.connectionId === connectionId),
              )
            : undefined;
        if (shown) tabs().close(shown.id);
        else void requestWindowClose();
      }),
      // Tabs belong to the active connection's workspace (TablePlus): these stay inside it.
      actions.register("tab.close-all", () => tabs().closeAll(workspace())),
      actions.register("tab.reopen", () => tabs().reopenClosed()),
      actions.register("tab.next", () => tabs().next(workspace())),
      actions.register("tab.prev", () => tabs().prev(workspace())),
      actions.register("tab.back", () => tabs().back()),
      actions.register("tab.forward", () => tabs().forward()),
      actions.register("workspace.next", () => cycleWorkspace(1)),
      actions.register("workspace.prev", () => cycleWorkspace(-1)),
      ...([1, 2, 3, 4, 5, 6, 7, 8, 9] as const).map((n) =>
        actions.register(`tab.goto-${n}`, () => tabs().activateIndex(n - 1, workspace())),
      ),
      actions.register("tab.new-query", () => {
        const active = useConnectionsStore.getState().activeId;
        if (active) {
          // "Command + T should open" (General): the last script, or an empty tab.
          openNewQueryTab(active);
          ui().setView("workspace");
        }
      }),
      actions.register("ui.toggle-sidebar", () => ui().toggleSidebar()),
      actions.register("ui.toggle-inspector", () => ui().toggleInspector()),
      actions.register("ui.toggle-console", () => ui().toggleConsole()),
      actions.register("ui.next-pane", () => void cyclePane(1)),
      actions.register("ui.prev-pane", () => void cyclePane(-1)),
      actions.register("palette.open", () => usePaletteStore.getState().toggle("all")),
      actions.register("palette.databases", () => {
        // The database list belongs to a connection; without one, list the connections.
        const connected = useConnectionsStore.getState().activeId !== null;
        usePaletteStore.getState().toggle(connected ? "databases" : "connections");
      }),
      actions.register("palette.connections", () =>
        usePaletteStore.getState().toggle("connections"),
      ),
      actions.register("connection.new", () => openConnectionDialog()),
      actions.register("ui.launcher", () =>
        ui().setView(
          ui().view === "launcher" && useConnectionsStore.getState().activeId
            ? "workspace"
            : "launcher",
        ),
      ),
      actions.register("workspace.reload", () => reloadWorkspace(qc)),
      // File ▸ New Workspace: pick a connection (⇧⌘K); it opens in a workspace of its own, or its
      // open workspace comes to the front.
      actions.register("workspace.new", () =>
        usePaletteStore.getState().openPalette("connections"),
      ),
      actions.register("sqlviewer.new", () => openSqlViewer(null)),
      actions.register("query.open-editor", openEditorFromTab),
    ];
    return () => unregister.forEach((u) => u());
  }, [qc]);
}
