import { useQueryClient } from "@tanstack/react-query";
import { Info } from "lucide-react";
import { useEffect, useMemo, useRef, type KeyboardEvent } from "react";
import { useGroupRef, usePanelRef, type Layout } from "react-resizable-panels";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { NotAvailable } from "@/features/drivers/NotAvailable";
import { actions, useAction } from "@/features/palette/actions";
import { openAddToQueries } from "@/features/queries/dialogStore";
import { invalidateConnectionSchema, reportHeldTables } from "@/features/structure/invalidate";
import { comboFromEvent } from "@/lib/keys/combo";
import { KEYMAP } from "@/lib/keys/keymap";
import { resolveScope } from "@/lib/keys/useShortcuts";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs } from "@/stores/prefs";
import { getQueryTab, updateQueryTab, useQueryTabBusy, useQueryTabField } from "@/stores/queryTabs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  cancelQuery,
  explainQuery,
  exportQuery,
  formatQuery,
  insertSql,
  minifyQuery,
  openQueryFile,
  runQuery,
  saveQuery,
  saveSelectionAs,
  sqlToKeep,
  toggleHistory,
  toggleResults,
  type ControllerHooks,
} from "./controller";
import { focusEditor, getEditorState, syncDirty } from "./editorStates";
import { HISTORY_KEY } from "./historyGroups";
import { coreLanguage, useEditorLanguage, type EditorLanguage } from "./language";
import { EditorBar } from "./EditorBar";
import { EditorPanes, type EditorPaneHandlers } from "./EditorPanes";
import { changeEditorFontSize } from "./editorSettings";
import { HistorySheet } from "./HistorySheet";
import { installFileAutoSave } from "./fileAutoSave";
import { installQueryTabLifecycle } from "./lifecycle";
import { QueryParamsDialog } from "./QueryParamsDialog";
import { ResultsPane } from "./ResultsPane";
import { activeEditorKey, closeSplit, isSplitOpen, openSplit } from "./split";

installQueryTabLifecycle();
installFileAutoSave();

/**
 * Keys the app-wide shortcut listener leaves alone inside a query tab (it runs first, in the window
 * capture phase, and stops every event it handles, so nothing here dispatches twice):
 * - ⌘R. `view.refresh` is not an editor shortcut, so on the editor bar, the result tabs or the
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

/** Editor / results split (percentages) until the user drags it. */
const DEFAULT_LAYOUT: Layout = { editor: 55, results: 45 };
const COLLAPSED_LAYOUT: Layout = { editor: 100, results: 0 };

const isCollapsedLayout = (layout: Layout) => (layout.results ?? 100) < 0.01;

/**
 * The SQL editor tab, laid out like TablePlus: the CodeMirror editor right under the tab bar (split
 * in two panes with ⇧⌘D), the editor bar under it (⚙ settings, cursor position, run summary, row
 * limit, Beautify, Explain, Run), then the results / messages / plan, which collapse (⌃`) to leave
 * the editor and its bar.
 */
export function QueryTab({ tab }: { tab: Extract<Tab, { kind: "query" }> }) {
  const driver = useConnectionsStore((s) => s.byId[tab.connectionId]?.info.driver);
  // A core dialect, or the language of the session's lane (its SQL dialect or its console).
  const language = useEditorLanguage(tab.connectionId);
  if (language === undefined)
    return (
      <div
        data-slot="lane-loading"
        className="text-muted-foreground flex h-full items-center justify-center p-6 text-xs"
      >
        Loading the editor…
      </div>
    );
  if (language) return <QueryTabView tab={tab} dialect={language} />;
  // A session without an editor: the core lexers must not split or classify its statements.
  return driver ? <NotAvailable driver={driver} what="The SQL editor" /> : null;
}

function QueryTabView({
  tab,
  dialect,
}: {
  tab: Extract<Tab, { kind: "query" }>;
  dialect: EditorLanguage;
}) {
  const tabId = tab.id;
  const qc = useQueryClient();
  const isActive = useTabsStore((s) => s.activeId === tabId);
  const currentDatabase = useConnectionsStore(
    (s) => s.byId[tab.connectionId]?.info.currentDatabase ?? null,
  );
  const busy = useQueryTabBusy(tabId);
  const historyOpen = useQueryTabField(tabId, "historyOpen");
  const layout = useQueryTabField(tabId, "layout");
  const resultsCollapsed = useQueryTabField(tabId, "resultsCollapsed");
  const headNotice = useQueryTabField(tabId, "headNotice");
  const groupRef = useGroupRef();
  const resultsRef = usePanelRef();
  const resultsElement = useRef<HTMLDivElement>(null);

  const handlers = useMemo(() => {
    const hooks: ControllerHooks = {
      onHistoryChanged: () => void qc.invalidateQueries({ queryKey: HISTORY_KEY }),
      // DDL run here: the sidebar, open grids and completion refetch (the backend dropped its
      // metadata cache too); grids holding staged row edits keep their page and say so.
      onSchemaChanged: () => reportHeldTables(invalidateConnectionSchema(qc, tab.connectionId)),
    };
    const handlers: EditorPaneHandlers & {
      insert: (sql: string) => Promise<void>;
      runSql: (sql: string) => Promise<void>;
      fontBigger: () => void;
      fontSmaller: () => void;
    } = {
      run: () => runQuery(tabId, "auto", hooks),
      runCurrent: () => runQuery(tabId, "current", hooks),
      cancel: () => cancelQuery(tabId),
      explain: () => explainQuery(tabId, "plan", hooks),
      analyze: () => explainQuery(tabId, "analyze", hooks),
      format: () => formatQuery(tabId),
      minify: () => minifyQuery(tabId),
      toggleResults: () => toggleResults(tabId),
      history: () => toggleHistory(tabId),
      open: () => openQueryFile(tabId),
      save: () => void saveQuery(tabId),
      saveAs: () => void saveQuery(tabId, { saveAs: true }),
      saveSelection: () => void saveSelectionAs(tabId),
      addToQueries: () => {
        const info = useConnectionsStore.getState().byId[tab.connectionId]?.info;
        openAddToQueries({
          sql: sqlToKeep(tabId),
          connection: info ? { profileId: info.profileId, name: info.name } : null,
        });
      },
      exportResults: () => exportQuery(tabId),
      insert: (sql: string) => insertSql(tabId, sql),
      runSql: (sql: string) => insertSql(tabId, sql, { ...hooks, run: true }),
      fontBigger: () => changeEditorFontSize(getPrefs().editorFontSize, 1),
      fontSmaller: () => changeEditorFontSize(getPrefs().editorFontSize, -1),
      split: () => {
        if (!isSplitOpen(tabId)) return openSplit(tabId);
        closeSplit(tabId);
        focusEditor(tabId);
      },
    };
    return handlers;
  }, [tabId, qc, tab.connectionId]);

  // Shortcuts, the palette and the native menu reach the visible query tab only.
  useAction("query.run", isActive ? handlers.run : null);
  useAction("query.run-current", isActive ? handlers.runCurrent : null);
  useAction("query.cancel", isActive && busy ? handlers.cancel : null);
  useAction("query.explain", isActive ? handlers.explain : null);
  useAction("query.format", isActive ? handlers.format : null);
  useAction("query.minify", isActive ? handlers.minify : null);
  useAction("query.toggle-results", isActive ? handlers.toggleResults : null);
  useAction("query.history", isActive ? handlers.history : null);
  useAction("query.save", isActive ? handlers.save : null);
  useAction("query.open", isActive ? handlers.open : null);
  useAction("query.save-as", isActive ? handlers.saveAs : null);
  useAction("query.save-selection", isActive ? handlers.saveSelection : null);
  useAction("query.add-to-queries", isActive ? handlers.addToQueries : null);
  useAction("query.font-bigger", isActive ? handlers.fontBigger : null);
  useAction("query.font-smaller", isActive ? handlers.fontSmaller : null);
  useAction("query.split", isActive ? handlers.split : null);
  // Registered after the app shell's default (which exports the active table tab), so it wins here.
  useAction("export.open", isActive ? handlers.exportResults : null);

  // A reopened tab (⇧⌘T) comes back clean in the tab store; re-derive from the document.
  useEffect(() => {
    const state = getEditorState(tabId);
    if (state) syncDirty(tabId, state.doc);
  }, [tabId]);

  // The store says whether the results show (⌃`, the toolbar, a new run); the panel follows. The
  // split it reopens to is the last one with the results showing.
  useEffect(() => {
    const panel = resultsRef.current;
    if (!panel) return;
    try {
      if (resultsCollapsed) {
        if (resultsElement.current?.contains(document.activeElement))
          focusEditor(activeEditorKey(tabId));
        panel.collapse();
      } else if (panel.isCollapsed()) {
        groupRef.current?.setLayout(getQueryTab(tabId).layout ?? DEFAULT_LAYOUT);
      }
    } catch {
      // Not laid out yet (no size): the group starts from `defaultLayout`, which already agrees.
    }
  }, [tabId, resultsCollapsed, groupRef, resultsRef]);

  return (
    <div
      data-scope="editor"
      className="bg-surface flex h-full min-h-0 flex-col"
      onKeyDownCapture={onQueryTabKeyDownCapture}
    >
      <ResizablePanelGroup
        orientation="vertical"
        className="min-h-0 flex-1"
        groupRef={groupRef}
        defaultLayout={resultsCollapsed ? COLLAPSED_LAYOUT : (layout ?? undefined)}
        onLayoutChanged={(next) =>
          // Collapsed (⌃`, or dragged shut): keep the split to reopen to.
          updateQueryTab(
            tabId,
            isCollapsedLayout(next)
              ? { resultsCollapsed: true }
              : { layout: next, resultsCollapsed: false },
          )
        }
      >
        <ResizablePanel id="editor" defaultSize={`${DEFAULT_LAYOUT.editor}%`} minSize={96}>
          {/* The bar stays with the editor: it ends up at the bottom when the results collapse. */}
          <div className="flex h-full min-h-0 flex-col">
            {/* The head of a big file: a copy, not linked to the file (saving never truncates it). */}
            {headNotice && (
              <div
                role="status"
                data-testid="head-notice"
                className="border-border bg-muted/50 text-muted-foreground flex shrink-0 items-center gap-1.5 border-b px-3 py-1 text-[11px]"
              >
                <Info aria-hidden className="size-3 shrink-0" />
                <span className="min-w-0 truncate">{headNotice}</span>
              </div>
            )}
            <div className="min-h-0 flex-1">
              <EditorPanes
                tabId={tabId}
                connectionId={tab.connectionId}
                dialect={dialect}
                currentDatabase={currentDatabase}
                handlers={handlers}
              />
            </div>
            <EditorBar tabId={tabId} handlers={handlers} />
          </div>
        </ResizablePanel>
        <ResizableHandle />
        <ResizablePanel
          id="results"
          defaultSize={`${DEFAULT_LAYOUT.results}%`}
          minSize={64}
          collapsible
          panelRef={resultsRef}
          elementRef={resultsElement}
          // A collapsed pane keeps its results, out of the tab order.
          inert={resultsCollapsed}
        >
          <ResultsPane tabId={tabId} dialect={coreLanguage(dialect)} />
        </ResizablePanel>
      </ResizablePanelGroup>
      <HistorySheet
        tabId={tabId}
        open={historyOpen}
        onOpenChange={(open) => toggleHistory(tabId, open)}
        onInsert={(sql) => void handlers.insert(sql)}
        onRun={(sql) => void handlers.runSql(sql)}
      />
      <QueryParamsDialog tabId={tabId} />
    </div>
  );
}
