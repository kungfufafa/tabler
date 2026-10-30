import { EditorSelection } from "@codemirror/state";
import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { openConnectionOf } from "@/features/connections/openProfile";
import { profileKeys } from "@/features/connections/useConnections";
import { editorOf } from "@/features/editor/language";
import { LARGE_FILE_BYTES, LARGE_GZ_BYTES } from "@/features/editor/largeFile";
import { applyToEditor, markSaved, seedEditor } from "@/features/editor/editorStates";
import { usePaletteStore } from "@/features/palette/store";
import { connectWorkspace, switchDatabase } from "@/features/sidebar/switchDatabase";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { filterMemoryKey } from "@/features/table-tab/filterMemory";
import { recallColumnLayout } from "@/features/table-tab/tableViewMemory";
import { getToolTab, restoreToolParams } from "@/features/tabs/toolTabs";
import { handoffWorkspace, MAIN_WINDOW, windowLabel } from "@/features/tabs/windowHandoff";
import { connectionWorkspaceKey } from "@/features/tabs/workspaces";
import { ipc, tableKey, type TableSummary } from "@/lib/ipc";
import { listenWindowEvent, type Unlisten } from "@/lib/ipc/events";
import { useConnectionsStore } from "./connections";
import { getPrefs } from "./prefs";
import { updateQueryTab } from "./queryTabs";
import {
  applyAutoSave,
  clampCursor,
  queryTabContent,
  workspaceKey,
  type SessionTab,
  type SessionWorkspace,
} from "./session";
import {
  beginRestoring,
  bindRestoring,
  closeRemembered,
  endRestoring,
  installSessionPersistence,
  isRestoring,
  loadSession,
  rememberedWorkspace,
  setRemembered,
} from "./sessionPersist";
import { useTableTabsStore } from "./tableTabs";
import { useTabsStore, type Tab } from "./tabs";
import { useUiStore } from "./ui";

/**
 * Rebuilds saved workspaces: at startup ("Reopen closed workspaces at startup") every workspace
 * that was open is reconnected through the regular open-profile flow and gets its database,
 * schema and tabs back; later, connecting a profile whose workspace was not reopened brings back
 * its SQL Query tabs (and, with the preference on, its table tabs).
 */

const RESTORE_TOAST_ID = "session-restore";

export interface RestoreOptions {
  /**
   * Restore table tabs too (they are checked against the database and skipped when gone), and
   * the tool tabs (they belong to the database too).
   */
  tables: boolean;
  /**
   * Apply "Auto Save SQL Query Tab" to the SQL Query tabs (default). A tab moved from another
   * window keeps its text whatever the setting says.
   */
  autoSave?: boolean;
}

export interface RestoreResult {
  restored: number;
  /** Tabs left out, by title (missing table or file). */
  skipped: string[];
}

/** The schema whose table list a saved table lives in (`null`: the connection's default). */
async function defaultSchema(qc: QueryClient, connectionId: string): Promise<string | null> {
  const schemas = await qc.fetchQuery({
    queryKey: schemaKeys.schemas(connectionId),
    queryFn: () => ipc.schema.schemas(connectionId),
    staleTime: 5 * 60_000,
  });
  return schemas.find((s) => s.isDefault)?.name ?? schemas[0]?.name ?? null;
}

/** Keys (`tableKey`) of the tables that exist, for the schemas the saved table tabs use. */
async function existingTables(
  qc: QueryClient,
  connectionId: string,
  tabs: readonly SessionTab[],
): Promise<Map<string | null, Set<string> | null>> {
  const schemas = new Set<string | null>();
  for (const t of tabs) if (t.kind === "table") schemas.add(t.table.schema);
  const out = new Map<string | null, Set<string> | null>();
  await Promise.all(
    [...schemas].map(async (schema) => {
      try {
        const name = schema ?? (await defaultSchema(qc, connectionId));
        if (name === null) return void out.set(schema, null);
        const tables = await qc.fetchQuery<TableSummary[]>({
          queryKey: schemaKeys.tables(connectionId, name),
          queryFn: () => ipc.schema.tables(connectionId, name),
          staleTime: 60_000,
        });
        out.set(schema, new Set(tables.map((t) => t.name)));
      } catch {
        // Cannot tell: leave those tabs out rather than open tabs that fail.
        out.set(schema, null);
      }
    }),
  );
  return out;
}

async function readFiles(tabs: readonly SessionTab[]): Promise<Map<string, string | null>> {
  const paths = new Set<string>();
  for (const t of tabs) if (t.kind === "query" && t.filePath) paths.add(t.filePath);
  const out = new Map<string, string | null>();
  await Promise.all(
    [...paths].map(async (path) => {
      try {
        // A big file is not loaded at start-up without asking (File ▸ Open asks what to do with
        // it): its tab is left out, like the tab of a file that is gone.
        const info = await ipc.sqlFiles.info(path);
        if (info.bytes > (info.compressed ? LARGE_GZ_BYTES : LARGE_FILE_BYTES)) {
          out.set(path, null);
          return;
        }
        // The same reader as File ▸ Open: also decompresses `.sql.gz` files.
        out.set(path, await ipc.sqlFiles.read(path));
      } catch {
        out.set(path, null);
      }
    }),
  );
  return out;
}

/**
 * Adds a saved workspace's tabs to an open connection, in their saved order after any tab it
 * already has, and shows the tab it showed last when the workspace is still the one on screen.
 * The auto-save setting decides which SQL Query tabs come back; tables that no longer exist and
 * clean file tabs whose file is gone are skipped.
 */
export async function restoreWorkspaceTabs(
  qc: QueryClient,
  connectionId: string,
  ws: SessionWorkspace,
  opts: RestoreOptions,
): Promise<RestoreResult> {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn) return { restored: 0, skipped: [] };
  const saved =
    opts.autoSave === false
      ? { tabs: [...ws.tabs], activeTab: ws.activeTab }
      : applyAutoSave(ws.tabs, getPrefs().autoSaveQueryTabs, ws.activeTab);
  const wanted = saved.tabs.filter((t) => opts.tables || t.kind === "query");
  const activeSaved = saved.activeTab === null ? null : saved.tabs[saved.activeTab];
  const [tables, files] = await Promise.all([
    opts.tables ? existingTables(qc, connectionId, wanted) : Promise.resolve(null),
    readFiles(wanted),
  ]);
  // The connection may have gone away (disconnect) while the checks ran.
  if (!useConnectionsStore.getState().byId[connectionId]) return { restored: 0, skipped: [] };

  const dialect = editorOf(conn.info)?.dialect ?? null;
  const restored: Tab[] = [];
  const skipped: string[] = [];
  let activeId: string | null = null;
  for (const t of wanted) {
    const id = crypto.randomUUID();
    if (t.kind === "table") {
      if (!tables?.get(t.table.schema)?.has(t.table.name)) {
        skipped.push(t.title === t.table.name ? tableKey(t.table).replace(/^\./, "") : t.title);
        continue;
      }
      restored.push({ id, kind: t.view, connectionId, table: { ...t.table }, title: t.title });
      // A tab handed over with its filter bar (a link's windowMode=isolated, Move Tab to New
      // Window): its rows seed the tab's state before it mounts, from the first page. The
      // table's remembered column layout goes along (the tab's own recall skips a seeded tab).
      if (t.filters && t.filters.length > 0) {
        const layout = recallColumnLayout(
          filterMemoryKey(conn.info.profileId, conn.info.currentDatabase, t.table),
        );
        useTableTabsStore.getState().update(id, {
          filters: t.filters.map((f) => ({ ...f })),
          filtersVisible: t.filtersVisible ?? true,
          conjunction: t.conjunction ?? "and",
          offset: 0,
          ...(layout ? { columnState: layout } : {}),
        });
      }
    } else if (t.kind === "tool") {
      // Its feature checks the params (a tool that is gone, or params it refuses: left out).
      const params = getToolTab(t.tool) ? restoreToolParams(t.tool, t.params) : null;
      if (params === null) {
        skipped.push(t.title);
        continue;
      }
      restored.push({ id, kind: "tool", connectionId, tool: t.tool, title: t.title, params });
    } else {
      const content = queryTabContent(t, t.filePath ? files.get(t.filePath) : undefined);
      if (!content) {
        skipped.push(t.title);
        continue;
      }
      // A connection without a core dialect keeps the text, though it has no SQL editor to show.
      seedEditor(id, content.doc, dialect ? { dialect } : undefined);
      markSaved(id, content.saved);
      const cursor = clampCursor(t.cursor, content.doc.length);
      if (cursor > 0) applyToEditor(id, () => ({ selection: EditorSelection.cursor(cursor) }));
      if (t.filePath) updateQueryTab(id, { filePath: t.filePath });
      restored.push({
        id,
        kind: "query",
        connectionId,
        title: t.title,
        ...(content.doc !== content.saved ? { dirty: true } : {}),
      });
    }
    if (t === activeSaved) activeId = id;
  }
  useTabsStore.getState().restoreTabs(restored);
  activeId ??= restored[0]?.id ?? null;
  // Show it unless the user moved on meanwhile (another workspace, or a tab of this one).
  const { tabs, activeId: visibleId } = useTabsStore.getState();
  const visible = tabs.find((t) => t.id === visibleId);
  const onScreen = useConnectionsStore.getState().activeId === connectionId;
  if (activeId && onScreen && visible?.connectionId !== connectionId)
    useTabsStore.getState().activate(activeId);
  return { restored: restored.length, skipped };
}

/**
 * SQL Query tabs came back on another database than the one they were saved on (the switch back
 * failed, or the connection opens on another one): their text is kept, but the user must know
 * where they run now. Table tabs are never reopened on another database.
 */
function reportMoved(
  name: string,
  restored: number,
  database: string | null | undefined,
  saved: string,
): void {
  if (restored === 0) return;
  const tabs = restored === 1 ? "1 SQL tab" : `${restored} SQL tabs`;
  toast.warning(`Restored ${tabs} on ${database ?? "the default database"} (was ${saved})`, {
    description: name,
  });
}

function reportSkipped(name: string, skipped: readonly string[]): void {
  if (skipped.length === 0) return;
  const list = skipped.slice(0, 5).join(", ") + (skipped.length > 5 ? "…" : "");
  toast.warning(`Some tabs of ${name} were not reopened`, {
    description: `No longer there: ${list}.`,
  });
}

/**
 * Waits for the connect `openProfile` started (or found): the profile's open connection, or null
 * once its connect ended without one (the connect flow already told the user why).
 */
export function waitForProfileConnection(profileId: string): Promise<string | null> {
  return new Promise((resolve) => {
    let done = false;
    const unsubscribers: (() => void)[] = [];
    const finish = (id: string | null) => {
      done = true;
      unsubscribers.forEach((u) => u());
      resolve(id);
    };
    const check = () => {
      if (done) return;
      const id = openConnectionOf(profileId);
      if (id) finish(id);
      else if (!usePaletteStore.getState().connecting.includes(profileId)) finish(null);
    };
    unsubscribers.push(useConnectionsStore.subscribe(check), usePaletteStore.subscribe(check));
    check();
  });
}

/**
 * Puts a reopened connection back on its saved database and schema; false when it is not on the
 * saved database (the switch failed, or the connection ended up on another one).
 */
async function restoreLocation(
  qc: QueryClient,
  connectionId: string,
  ws: SessionWorkspace,
): Promise<boolean> {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn) return false;
  if (ws.database && conn.info.driver !== "sqlite" && conn.info.currentDatabase !== ws.database) {
    // Reports its own failure (toast).
    if (!(await switchDatabase(qc, connectionId, ws.database))) return false;
    const now = useConnectionsStore.getState().byId[connectionId]?.info.currentDatabase;
    if (now !== ws.database) return false;
  }
  if (ws.schema) {
    try {
      const schemas = await qc.fetchQuery({
        queryKey: schemaKeys.schemas(connectionId),
        queryFn: () => ipc.schema.schemas(connectionId),
        staleTime: 5 * 60_000,
      });
      if (schemas.some((s) => s.name === ws.schema))
        useConnectionsStore.getState().setCurrentSchema(connectionId, ws.schema);
    } catch {
      // The sidebar falls back to the default schema.
    }
  }
  return true;
}

let started = false;

export interface StartupDeps {
  qc: QueryClient;
  /** The app's open-profile flow (`useOpenProfile`): connects, or shows an open connection. */
  openProfile: (profileId: string) => void;
  /** Resolves once the preferences were read (`loadPrefs`). */
  prefsLoaded: () => Promise<void>;
}

/**
 * Startup: reads the session, starts persisting it, and with "Reopen closed workspaces at startup"
 * on (and nothing opened yet) reconnects the workspaces that were open, one after the other in rail
 * order, then shows the workspace and tab that were on screen. A profile that fails to connect
 * stays closed (its connect toast says why); its tabs come back when it is connected later. Runs
 * once per app session.
 */
export async function restoreAtStartup(deps: StartupDeps): Promise<void> {
  if (started) return;
  started = true;
  const [session] = await Promise.all([loadSession(), deps.prefsLoaded().catch(() => undefined)]);
  // Profiles deleted since: nothing to reopen.
  const profiles = await deps.qc
    .fetchQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 })
    .catch(() => null);
  const exists = (id: string) => !profiles || profiles.some((p) => p.id === id);
  const workspaces = session.workspaces.filter((w) => exists(w.profileId));
  setRemembered(workspaces);
  installSessionPersistence();
  installConnectRestore(deps.qc);

  const pristine =
    useConnectionsStore.getState().order.length === 0 && useTabsStore.getState().tabs.length === 0;
  const toOpen = workspaces.filter((w) => w.open);
  if (!getPrefs().reopenWorkspaces || !pristine || toOpen.length === 0) {
    closeRemembered();
    return;
  }
  toast.loading(toOpen.length === 1 ? "Reopening workspace…" : "Reopening workspaces…", {
    id: RESTORE_TOAST_ID,
  });
  /** Workspace key → its reopened connection. */
  const reopened = new Map<string, string>();
  try {
    for (const ws of toOpen) {
      const token = beginRestoring(ws);
      let connectionId: string | null = null;
      try {
        // A profile's second workspace (another database) gets a connection of its own.
        const second = openConnectionOf(ws.profileId) !== null && ws.database !== null;
        if (second) connectionId = await connectWorkspace(deps.qc, ws.profileId, ws.database);
        else {
          deps.openProfile(ws.profileId);
          connectionId = await waitForProfileConnection(ws.profileId);
        }
        if (!connectionId) continue;
        bindRestoring(token, connectionId);
        reopened.set(workspaceKey(ws), connectionId);
        const located = await restoreLocation(deps.qc, connectionId, ws);
        // Off the saved database only the SQL Query tabs come back (`restored` counts them).
        const { restored, skipped } = await restoreWorkspaceTabs(deps.qc, connectionId, ws, {
          tables: located,
        });
        const info = useConnectionsStore.getState().byId[connectionId]?.info;
        const name = info?.name ?? "the workspace";
        reportSkipped(name, skipped);
        if (!located && ws.database)
          reportMoved(name, restored, info?.currentDatabase, ws.database);
      } catch (err) {
        console.warn("[session] restore failed", err);
      } finally {
        endRestoring(token, connectionId !== null);
      }
    }
  } finally {
    toast.dismiss(RESTORE_TOAST_ID);
  }
  // The workspace that was on screen: the profile's on that database, else its first one.
  const activeProfile = session.activeProfileId;
  let target = activeProfile
    ? reopened.get(
        workspaceKey({ profileId: activeProfile, database: session.activeDatabase ?? null }),
      )
    : undefined;
  if (!target && activeProfile)
    target = toOpen
      .filter((w) => w.profileId === activeProfile)
      .map((w) => reopened.get(workspaceKey(w)))
      .find((id) => id !== undefined);
  if (target && useConnectionsStore.getState().byId[target]) {
    useConnectionsStore.getState().setActive(target);
    useUiStore.getState().setView("workspace");
  }
}

let uninstallConnectRestore: (() => void) | null = null;

type ConnectionsSnapshot = ReturnType<typeof useConnectionsStore.getState>;

/**
 * Whether the new connection `id` of `s` is not the first of its workspace (profile + database):
 * a reconnect (the same update closed the connection of that workspace it replaces), or a second
 * connection of a workspace another connection already holds (it must not restore twice). A
 * profile's other workspace being open does not make it one: a connection on another database
 * is a workspace of its own, with its own remembered tabs.
 */
function isReconnect(s: ConnectionsSnapshot, prev: ConnectionsSnapshot, id: string): boolean {
  const conn = s.byId[id];
  if (!conn) return false;
  const key = connectionWorkspaceKey(conn);
  const replaces = prev.order.some((p) => {
    const old = prev.byId[p];
    return old !== undefined && !s.byId[p] && connectionWorkspaceKey(old) === key;
  });
  if (replaces) return true;
  return s.order.slice(0, s.order.indexOf(id)).some((o) => {
    const other = s.byId[o];
    return other !== undefined && connectionWorkspaceKey(other) === key;
  });
}

/**
 * A profile connected later whose workspace is still remembered (not reopened at startup, or its
 * reconnect failed) gets its tabs back, unless it already has some: SQL Query tabs per the
 * auto-save setting, table tabs too with "Reopen closed workspaces" on and the same database.
 */
export function installConnectRestore(qc: QueryClient): void {
  if (uninstallConnectRestore) return;
  uninstallConnectRestore = useConnectionsStore.subscribe((s, prev) => {
    for (const id of s.order) {
      if (prev.byId[id]) continue;
      const conn = s.byId[id];
      if (!conn) continue;
      const { profileId, currentDatabase } = conn.info;
      // The profile's first connection may land on another database than the saved one and takes
      // that workspace's SQL tabs; one that comes while the profile is already open is a workspace
      // of its own and restores only its own database's entry, never another one's.
      const profileIsOpen = s.order.some(
        (o) => o !== id && s.byId[o]?.info.profileId === profileId,
      );
      const ws = rememberedWorkspace(profileId, currentDatabase, { exact: profileIsOpen });
      // A reconnect swaps the connection id but keeps the workspace (and its tabs); the startup
      // restore handles the profiles it reopens itself.
      if (!ws || isRestoring(profileId) || isReconnect(s, prev, id)) continue;
      const token = beginRestoring(ws);
      bindRestoring(token, id);
      const hasTabs = useTabsStore.getState().tabs.some((t) => t.connectionId === id);
      const sameDatabase = ws.database === null || ws.database === currentDatabase;
      const restore = hasTabs
        ? Promise.resolve<RestoreResult>({ restored: 0, skipped: [] })
        : restoreWorkspaceTabs(qc, id, ws, {
            tables: getPrefs().reopenWorkspaces && sameDatabase,
          });
      void restore
        .then(({ restored, skipped }) => {
          reportSkipped(conn.info.name, skipped);
          // Off the saved database only the SQL Query tabs come back (`restored` counts them).
          if (!sameDatabase && ws.database)
            reportMoved(conn.info.name, restored, currentDatabase, ws.database);
        })
        .catch((err: unknown) => console.warn("[session] restore failed", err))
        .finally(() => endRestoring(token, true));
    }
  });
}

/**
 * This window keeps the session saved from now on: the window that did closed (or every window
 * had, and this one opened after). Reads the saved session (the closed window wrote it last),
 * keeps its workspaces remembered but closed (they went away with their window, so they are not
 * reopened at the next start, but connecting their profile brings their tabs back) and starts
 * saving with this window's workspaces live. Once per window; false when it already keeps it (the
 * main window does from startup).
 */
export async function adoptSession(qc: QueryClient): Promise<boolean> {
  if (started) return false;
  started = true;
  const session = await loadSession();
  setRemembered(session.workspaces);
  closeRemembered();
  installSessionPersistence();
  installConnectRestore(qc);
  return true;
}

/**
 * A window other than the main one keeps the session saved once it is its keeper: at startup when
 * the backend says so, or when the backend tells it the keeper closed (`window:session-owner`).
 * Returns the uninstall function.
 */
export function installSessionOwnership(qc: QueryClient): () => void {
  let gone = false;
  let unlisten: Unlisten | null = null;
  const adopt = () => {
    if (!gone) void adoptSession(qc).catch((err: unknown) => console.warn("[session]", err));
  };
  void listenWindowEvent("window:session-owner", adopt).then((u) => {
    if (gone) u();
    else unlisten = u;
  });
  void windowLabel().then(async (label) => {
    // The main window keeps it from startup (`restoreAtStartup`).
    if (gone || label === MAIN_WINDOW) return;
    if (await ipc.windows.isSessionOwner().catch(() => false)) adopt();
  });
  return () => {
    gone = true;
    unlisten?.();
  };
}

/** Test-only: allow another startup restore and drop the connect hook. */
export function resetSessionRestoreForTests(): void {
  started = false;
  uninstallConnectRestore?.();
  uninstallConnectRestore = null;
}

export interface HandoffDeps {
  qc: QueryClient;
  /** Resolves once the preferences were read (`loadPrefs`). */
  prefsLoaded: () => Promise<void>;
}

/**
 * A window opened with something to show ("Open in New Window", Move Tab to New Window): connects
 * to the profile (on the handed-over database), selects the schema and rebuilds the moved tab.
 * Resolves with the connection, or null for an empty window or a failed connect (reported).
 */
export async function openHandoff(deps: HandoffDeps): Promise<string | null> {
  const handoff = await ipc.windows.takeHandoff().catch(() => null);
  const ws = handoffWorkspace(handoff);
  if (!ws) return null;
  await Promise.all([
    deps.prefsLoaded().catch(() => undefined),
    // An "Ask every time" profile's password mode comes from the list.
    deps.qc
      .fetchQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 })
      .catch(() => null),
  ]);
  const connectionId = await connectWorkspace(deps.qc, ws.profileId, ws.database);
  if (!connectionId) return null;
  if (ws.schema) {
    try {
      const schemas = await deps.qc.fetchQuery({
        queryKey: schemaKeys.schemas(connectionId),
        queryFn: () => ipc.schema.schemas(connectionId),
        staleTime: 5 * 60_000,
      });
      if (schemas.some((s) => s.name === ws.schema))
        useConnectionsStore.getState().setCurrentSchema(connectionId, ws.schema);
    } catch {
      // The sidebar shows the default schema.
    }
  }
  if (ws.tabs.length > 0) {
    const name = useConnectionsStore.getState().byId[connectionId]?.info.name ?? "the workspace";
    const { skipped } = await restoreWorkspaceTabs(deps.qc, connectionId, ws, {
      tables: true,
      autoSave: false,
    });
    reportSkipped(name, skipped);
  }
  return connectionId;
}
