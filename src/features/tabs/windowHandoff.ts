import { toast } from "sonner";
import { getEditorState } from "@/features/editor/editorStates";
import { ipc, toIpcError, type Conjunction, type WindowHandoff } from "@/lib/ipc";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { getQueryTab } from "@/stores/queryTabs";
import { parseTab, type SessionTab, type SessionWorkspace } from "@/stores/session";
import { useTableTabsStore, type FilterRowSpec } from "@/stores/tableTabs";
import { isTableTab, unsavedCount, useTabsStore, type Tab } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

/**
 * Multi-window (TablePlus: File ▸ New Window ⌘N, "Open in New Window" on a connection, Window ▸
 * Move Tab to New Window). Every window has its own renderer stores and its own connections: a
 * new window connects again to the profile, with what to open handed over through the backend
 * (`WindowHandoff`, parked under the new window's label until its renderer takes it). A moved
 * tab travels in the saved-session tab format, so it is rebuilt like a restored tab.
 */

export const MAIN_WINDOW = "main";

/** A query tab's live buffer, as the session saves it. */
export interface HandoffQuery {
  text: string;
  cursor: number;
  filePath: string | null;
}

/** A table tab's filter bar, as it travels to the new window. */
export interface HandoffFilters {
  filters: readonly FilterRowSpec[];
  filtersVisible: boolean;
  conjunction?: Conjunction;
}

/** The filters a live table tab shows now (null: none), copied. */
function liveFilters(tabId: string): HandoffFilters | null {
  const slice = useTableTabsStore.getState().slices[tabId];
  if (!slice || slice.filters.length === 0) return null;
  return {
    filters: slice.filters,
    filtersVisible: slice.filtersVisible,
    conjunction: slice.conjunction,
  };
}

/**
 * A tab in the saved-session format (what the new window rebuilds). A table tab carries its
 * filter bar: `filters` when given (null: none, for a tab that is not open here), else the tab's
 * own rows from the table-tab store. Staged edits never travel.
 */
export function handoffTab(
  tab: Tab,
  query?: HandoffQuery,
  filters?: HandoffFilters | null,
): SessionTab {
  if (tab.kind === "tool")
    return { kind: "tool", tool: tab.tool, title: tab.title, params: tab.params ?? null };
  if (isTableTab(tab)) {
    const bar = filters === undefined ? liveFilters(tab.id) : filters;
    return {
      kind: "table",
      table: { ...tab.table },
      view: tab.kind,
      title: tab.title,
      ...(bar && bar.filters.length > 0
        ? {
            filters: bar.filters.map((f) => ({ ...f })),
            filtersVisible: bar.filtersVisible,
            ...(bar.conjunction === "or" ? { conjunction: "or" as const } : {}),
          }
        : {}),
    };
  }
  const q = query ?? { text: "", cursor: 0, filePath: null };
  // A clean file tab reopens from its file; anything else carries its text.
  const keep = q.filePath === null || tab.dirty === true;
  return {
    kind: "query",
    title: tab.title,
    sql: keep ? q.text : null,
    filePath: q.filePath,
    cursor: Math.max(0, Math.min(q.cursor, q.text.length)),
  };
}

/** What the new window opens: the tab's connection (profile, database, schema) and the tab. */
export function buildHandoff(
  conn: Pick<OpenConnection, "info" | "currentSchema">,
  tab: Tab | null = null,
  query?: HandoffQuery,
): WindowHandoff {
  return {
    profileId: conn.info.profileId,
    // A SQLite profile has one database: its own.
    database: conn.info.driver === "sqlite" ? null : conn.info.currentDatabase,
    schema: conn.currentSchema,
    tab: tab ? handoffTab(tab, query) : null,
  };
}

/** The workspace a received handoff describes (null: an empty window). */
export function handoffWorkspace(handoff: WindowHandoff | null): SessionWorkspace | null {
  if (!handoff?.profileId) return null;
  const tab = handoff.tab === null || handoff.tab === undefined ? null : parseTab(handoff.tab);
  return {
    profileId: handoff.profileId,
    open: true,
    database: handoff.database,
    schema: handoff.schema,
    tabs: tab ? [tab] : [],
    activeTab: tab ? 0 : null,
  };
}

/**
 * Why a tab cannot move to another window, or null: staged row edits, staged structure changes
 * and staged result edits live in this window's stores and would be lost. (A query's unsaved text
 * moves with it.)
 */
export function moveRefusal(tab: Tab): string | null {
  const pending = unsavedCount(tab) - (tab.kind === "query" && tab.dirty ? 1 : 0);
  if (pending <= 0) return null;
  return `“${tab.title}” has ${pending} uncommitted change${pending === 1 ? "" : "s"}: commit or discard ${pending === 1 ? "it" : "them"} first.`;
}

const errorText = (e: unknown) => toIpcError(e).message;

/** File ▸ New Window (⌘N): an empty window (its connection list). */
export async function openNewWindow(): Promise<boolean> {
  try {
    await ipc.windows.open(null);
    return true;
  } catch (e) {
    toast.error("Could not open a new window", { description: errorText(e) });
    return false;
  }
}

/** "Open in New Window" on a connection: a new window connects to it. */
export async function openProfileInNewWindow(
  profileId: string,
  database: string | null = null,
): Promise<boolean> {
  try {
    await ipc.windows.open({ profileId, database, schema: null, tab: null });
    return true;
  } catch (e) {
    toast.error("Could not open a new window", { description: errorText(e) });
    return false;
  }
}

/**
 * Window ▸ Move Tab to New Window: the tab (default: the active one) reopens in a new window on
 * its connection's profile and database, and leaves this one. A query's text, caret and file go
 * with it; a table tab its table and view. Refused (with a toast) while it holds staged changes.
 */
export async function moveTabToNewWindow(tabId?: string | null): Promise<boolean> {
  const tabs = useTabsStore.getState();
  const tab = tabs.tabs.find((t) => t.id === (tabId ?? tabs.activeId));
  if (!tab) return false;
  const conn = useConnectionsStore.getState().byId[tab.connectionId];
  if (!conn) return false;
  const refusal = moveRefusal(tab);
  if (refusal) {
    toast.error("Cannot move the tab", { description: refusal });
    return false;
  }
  const state = tab.kind === "query" ? getEditorState(tab.id) : null;
  const query: HandoffQuery | undefined =
    tab.kind === "query"
      ? {
          text: state?.doc.toString() ?? "",
          cursor: state?.selection.main.head ?? 0,
          filePath: getQueryTab(tab.id).filePath,
        }
      : undefined;
  try {
    await ipc.windows.open(buildHandoff(conn, tab, query));
  } catch (e) {
    toast.error("Could not move the tab to a new window", { description: errorText(e) });
    return false;
  }
  // Its work went along: nothing to ask about.
  useTabsStore.getState().forceClose([tab.id]);
  return true;
}

let labelPromise: Promise<string> | null = null;

/** This window's label (cached; "main" when it cannot be told). */
export function windowLabel(): Promise<string> {
  labelPromise ??= ipc.windows.label().catch(() => MAIN_WINDOW);
  return labelPromise;
}

/** Test-only: forget the cached label. */
export function resetWindowLabelForTests(): void {
  labelPromise = null;
}

/**
 * Tells the backend which connections this window owns whenever that changes, so they are closed
 * with the window. Returns the uninstall function.
 */
export function installConnectionClaims(
  report: (ids: string[]) => Promise<void> = ipc.windows.setConnections,
): () => void {
  let sent: string | null = null;
  const sync = () => {
    const ids = [...useConnectionsStore.getState().order].sort();
    const key = ids.join(",");
    if (key === sent) return;
    sent = key;
    void report(ids).catch(() => {
      if (sent === key) sent = null;
    });
  };
  sync();
  return useConnectionsStore.subscribe(sync);
}

/** Split Right (⇧⌘D outside the SQL editor): the active tab's copy in the right group. */
export function splitRight(): void {
  const tabs = useTabsStore.getState();
  if (!tabs.activeId) return;
  tabs.splitRight(tabs.activeId);
  useUiStore.getState().setView("workspace");
}
