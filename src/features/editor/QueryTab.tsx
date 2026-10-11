import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, type KeyboardEvent } from "react";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { actions, useAction } from "@/features/palette/actions";
import { comboFromEvent } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { resolveScope } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { updateQueryTab, useQueryTabBusy, useQueryTabField } from "@/stores/queryTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  cancelQuery,
  dialectOf,
  explainQuery,
  exportQuery,
  formatQuery,
  insertSql,
  openQueryFile,
  runQuery,
  saveQuery,
  toggleHistory,
  type ControllerHooks,
} from "./controller";
import { getEditorState, syncDirty } from "./editorStates";
import { HISTORY_KEY } from "./historyGroups";
import { HistorySheet } from "./HistorySheet";
import { installQueryTabLifecycle } from "./lifecycle";
import { QueryToolbar, type ToolbarHandlers } from "./QueryToolbar";
import { ResultsPane } from "./ResultsPane";
import { SqlEditor } from "./SqlEditor";

installQueryTabLifecycle();

/**
 * Keys the app-wide shortcut listener leaves alone inside a query tab (it runs first, in the window
 * capture phase, and stops every event it handles, so nothing here dispatches twice):
 * - ⌘R. `view.refresh` is not an editor shortcut, so on the toolbar, the result tabs or the
 *   history sheet nothing would stop the webview from reloading and losing every buffer.
 * - The query shortcuts while the result grid has focus: its "grid" scope has no query bindings,
 *   yet ⌘↩ / ⌘. / ⌘S… should keep working for the tab the grid belongs to.
 */
function onQueryTabKeyDownCapture(e: KeyboardEvent<HTMLElement>): void {
  if (e.defaultPrevented) return;
  const combo = comboFromEvent(e.nativeEvent);
  if (combo === "Mod+R") {
    e.preventDefault();
    return;
  }
  if (resolveScope(e.target instanceof Element ? e.target : null) !== "grid") return;
  const def = KEYMAP.find((d) => d.scope === "editor" && d.keys === combo);
  if (!def || !actions.has(def.id)) return;
  e.preventDefault();
  e.stopPropagation();
  void actions.run(def.id);
}

/** The SQL editor tab: toolbar, CodeMirror editor, results / messages / plan, history. */
export function QueryTab({ tab }: { tab: Extract<Tab, { kind: "query" }> }) {
  const tabId = tab.id;
  const qc = useQueryClient();
  const isActive = useTabsStore((s) => s.activeId === tabId);
  const driver = useConnectionsStore((s) => s.byId[tab.connectionId]?.info.driver);
  const currentDatabase = useConnectionsStore(
    (s) => s.byId[tab.connectionId]?.info.currentDatabase ?? null,
  );
  const dialect = dialectOf(driver);
  const busy = useQueryTabBusy(tabId);
  const historyOpen = useQueryTabField(tabId, "historyOpen");
  const layout = useQueryTabField(tabId, "layout");

  const handlers = useMemo(() => {
    const hooks: ControllerHooks = {
      onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
    };
    return {
      run: () => runQuery(tabId, "auto", hooks),
      runCurrent: () => runQuery(tabId, "current", hooks),
      cancel: () => cancelQuery(tabId),
      explain: () => explainQuery(tabId, "plan", hooks),
      analyze: () => explainQuery(tabId, "analyze", hooks),
      format: () => formatQuery(tabId),
      history: () => toggleHistory(tabId),
      open: () => openQueryFile(tabId),
      save: () => void saveQuery(tabId),
      saveAs: () => void saveQuery(tabId, { saveAs: true }),
      exportResults: () => exportQuery(tabId),
      insert: (sql: string) => insertSql(tabId, sql),
      runSql: (sql: string) => insertSql(tabId, sql, { ...hooks, run: true }),
    };
  }, [tabId, qc]);

  // Shortcuts, the palette and the native menu reach the visible query tab only.
  useAction("query.run", isActive ? handlers.run : null);
  useAction("query.run-current", isActive ? handlers.runCurrent : null);
  useAction("query.cancel", isActive && busy ? handlers.cancel : null);
  useAction("query.explain", isActive ? handlers.explain : null);
  useAction("query.format", isActive ? handlers.format : null);
  useAction("query.history", isActive ? handlers.history : null);
  useAction("query.save", isActive ? handlers.save : null);
  useAction("query.open", isActive ? handlers.open : null);
  // Registered after the app shell's default (which exports the active table tab), so it wins here.
  useAction("export.open", isActive ? handlers.exportResults : null);

  // A reopened tab (⇧⌘T) comes back clean in the tab store; re-derive from the document.
  useEffect(() => {
    const state = getEditorState(tabId);
    if (state) syncDirty(tabId, state.doc);
  }, [tabId]);

  const toolbar: ToolbarHandlers = handlers;

  return (
    <div
      data-scope="editor"
      className="bg-surface flex h-full min-h-0 flex-col"
      onKeyDownCapture={onQueryTabKeyDownCapture}
    >
      <QueryToolbar tabId={tabId} connectionId={tab.connectionId} handlers={toolbar} />
      <ResizablePanelGroup
        orientation="vertical"
        className="min-h-0 flex-1"
        defaultLayout={layout ?? undefined}
        onLayoutChanged={(next) => updateQueryTab(tabId, { layout: next })}
      >
        <ResizablePanel id="editor" defaultSize="55%" minSize={64}>
          <SqlEditor
            tabId={tabId}
            connectionId={tab.connectionId}
            dialect={dialect}
            currentDatabase={currentDatabase}
          />
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel id="results" defaultSize="45%" minSize={64}>
          <ResultsPane tabId={tabId} dialect={dialect} />
        </ResizablePanel>
      </ResizablePanelGroup>
      <HistorySheet
        tabId={tabId}
        open={historyOpen}
        onOpenChange={(open) => toggleHistory(tabId, open)}
        onInsert={(sql) => void handlers.insert(sql)}
        onRun={(sql) => void handlers.runSql(sql)}
      />
    </div>
  );
}
