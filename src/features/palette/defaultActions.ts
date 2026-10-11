import { useEffect } from "react";
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
    const unregister = [
      actions.register("tab.close", () => {
        // ⌘W is allowed inside dialogs so Settings can claim it (its handler stacks on top while
        // open). Any other modal (palette, Preview SQL, connection form…) must not close the tab
        // behind it.
        if (resolveScope(document.activeElement) === "dialog") return;
        const id = tabs().activeId;
        if (id) tabs().close(id);
      }),
      actions.register("tab.close-all", () => tabs().closeAll()),
      actions.register("tab.reopen", () => tabs().reopenClosed()),
      actions.register("tab.next", () => tabs().next()),
      actions.register("tab.prev", () => tabs().prev()),
      ...([1, 2, 3, 4, 5, 6, 7, 8, 9] as const).map((n) =>
        actions.register(`tab.goto-${n}`, () => tabs().activateIndex(n - 1)),
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
      actions.register("palette.open", () => usePaletteStore.getState().toggle()),
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
