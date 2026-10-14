import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { registerGridMenuExtension } from "@/features/datagrid/menuExtensions";
import { registerEditorMenuExtension } from "@/features/editor/menuExtensions";
import { registerTableMenuExtension } from "@/features/objects/menu";
import { actions } from "@/features/palette/actions";
import { registerPaletteItems } from "@/features/palette/extraItems";
import { usePrefs } from "@/stores/prefs";
import { SQL_FORMATTER } from "./builtin/sqlFormatter";
import { DummyDataDialog } from "./DummyDataDialog";
import { listenPluginRuns } from "./events";
import {
  editorPluginMenu,
  gridPluginMenu,
  onPluginKey,
  palettePluginItems,
  runFromFocus,
  sidebarPluginEntries,
} from "./integration";
import { captureSidebarTarget, setPluginQueryClient } from "./invocation";
import { installPluginMenuSync } from "./menuSync";
import { PluginManager } from "./PluginManager";
import { isPluginEnabled } from "./pluginPrefs";
import { PluginPrompts } from "./PluginPrompts";
import { openPluginManager, refreshPlugins } from "./store";
import { runIdOf } from "./types";

const SQL_FORMAT_RUN = runIdOf(SQL_FORMATTER.identifier, 0);

/**
 * Plugins (TablePlus's Plugins ▸, ⌘L): loads the installed plugins, keeps the native Plugins menu,
 * the grid / sidebar menus, the palette and the plugins' keys in step, and shows the manager and
 * the plugins' dialogs. Mount once.
 */
export function PluginsHost() {
  const qc = useQueryClient();
  useEffect(() => {
    setPluginQueryClient(qc);
    return () => setPluginQueryClient(null);
  }, [qc]);

  useEffect(() => {
    void refreshPlugins();
    const off = [
      actions.register("plugins.manage", openPluginManager),
      installPluginMenuSync(),
      registerGridMenuExtension(gridPluginMenu),
      registerEditorMenuExtension(editorPluginMenu),
      registerTableMenuExtension(() => sidebarPluginEntries()),
      registerPaletteItems(palettePluginItems),
    ];
    const onContextMenu = (e: MouseEvent) => captureSidebarTarget(e.target);
    window.addEventListener("contextmenu", onContextMenu, { capture: true });
    // After the app's own shortcuts (a window capture listener), before the focused view's.
    document.addEventListener("keydown", onPluginKey, { capture: true });
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenPluginRuns((runId) => runFromFocus(runId)).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      off.forEach((u) => u());
      window.removeEventListener("contextmenu", onContextMenu, { capture: true });
      document.removeEventListener("keydown", onPluginKey, { capture: true });
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Plugins ▸ SQL Format (⌃I in the SQL editor), while the SQL Formatter plugin is enabled.
  const sqlFormat = usePrefs((p) =>
    isPluginEnabled(p.pluginStates, SQL_FORMATTER.identifier, true),
  );
  useEffect(
    () =>
      sqlFormat
        ? actions.register("plugins.sql-format", () => runFromFocus(SQL_FORMAT_RUN))
        : undefined,
    [sqlFormat],
  );

  return (
    <>
      <PluginManager />
      <PluginPrompts />
      <DummyDataDialog />
    </>
  );
}
