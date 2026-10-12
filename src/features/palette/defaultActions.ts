import { useEffect } from "react";
import { openConnectionDialog } from "@/features/connections/store";
import { cycleWorkspace } from "@/features/tabs/workspaces";
import { resolveScope } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { actions } from "./actions";
import { usePaletteStore } from "./store";

/** Store-backed actions that are always available (tabs, panels, launcher, palette). */
export function useDefaultActions(): void {
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
        const id = tabs().activeId;
        if (id) tabs().close(id);
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
          tabs().newQuery(active);
          ui().setView("workspace");
        }
      }),
      actions.register("ui.toggle-sidebar", () => ui().toggleSidebar()),
      actions.register("ui.toggle-inspector", () => ui().toggleInspector()),
      actions.register("ui.toggle-console", () => ui().toggleConsole()),
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
    ];
    return () => unregister.forEach((u) => u());
  }, []);
}
