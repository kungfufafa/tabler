import { FileCode2 } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect } from "react";
import { registerToolTab, type ToolTabProps } from "@/features/tabs/toolTabs";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";
import { loadViewerFile } from "./files";
import { viewerTabTitle } from "./labels";
import {
  SQL_VIEWER_TOOL,
  defaultDialect,
  keepsViewerTab,
  openPathFromViewerTab,
  restoreViewerParams,
  type SqlViewerParams,
} from "./open";
import { SqlViewer } from "./SqlViewer";
import { ensureDoc, getDoc, newDoc, useSqlViewerStore } from "./store";

/** A SQL Viewer in a tab of the active workspace (it only borrows the workspace: nothing runs). */
export function SqlViewerTab({ tab, params, focused }: ToolTabProps<SqlViewerParams | null>) {
  const key = tab.id;
  const initialPath = params?.path ?? null;
  const hasDoc = useSqlViewerStore((s) => s.docs[key] !== undefined);
  const path = useSqlViewerStore((s) => s.docs[key]?.path ?? null);

  // The buffer is created (and its file read) once per tab, before the editor mounts.
  useLayoutEffect(() => {
    if (getDoc(key)) return;
    const driver = useConnectionsStore.getState().byId[tab.connectionId]?.info.driver;
    ensureDoc(key, () => newDoc(initialPath, defaultDialect(driver)));
    if (initialPath) void loadViewerFile(key, initialPath);
    // Params only matter for the first mount; later changes come from the viewer itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  // Saved under another name / opened another file: the tab (title, session params) follows.
  useEffect(() => {
    if (!hasDoc || path === initialPath) return;
    const title = viewerTabTitle(path);
    useTabsStore.setState((s) => ({
      tabs: s.tabs.map((t) =>
        t.id === key && t.kind === "tool" ? { ...t, title, params: { path } } : t,
      ),
    }));
  }, [key, hasDoc, path, initialPath]);

  const onOpenPath = useCallback((p: string) => openPathFromViewerTab(key, p), [key]);

  if (!hasDoc) return null;
  return <SqlViewer viewerKey={key} active={focused} onOpenPath={onOpenPath} />;
}

registerToolTab<SqlViewerParams | null>(SQL_VIEWER_TOOL, {
  component: SqlViewerTab,
  icon: FileCode2,
  title: (p) => viewerTabTitle(p?.path ?? null),
  // Only a viewer of a file comes back (an untitled buffer is not kept).
  persist: keepsViewerTab,
  restore: restoreViewerParams,
});
