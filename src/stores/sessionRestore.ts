import { EditorSelection } from "@codemirror/state";
import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { openConnectionOf } from "@/features/connections/openProfile";
import { profileKeys } from "@/features/connections/useConnections";
import { dialectOf } from "@/features/editor/controller";
import { applyToEditor, markSaved, seedEditor } from "@/features/editor/editorStates";
import { usePaletteStore } from "@/features/palette/store";
import { switchDatabase } from "@/features/sidebar/switchDatabase";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, tableKey, type TableSummary } from "@/lib/ipc";
import { useConnectionsStore } from "./connections";
import { getPrefs } from "./prefs";
import { updateQueryTab } from "./queryTabs";
import {
  applyAutoSave,
  clampCursor,
  queryTabContent,
  type SessionTab,
  type SessionWorkspace,
} from "./session";
import {
  beginRestoring,
  closeRemembered,
  endRestoring,
  installSessionPersistence,
  isRestoring,
  loadSession,
  rememberedWorkspace,
  setRemembered,
} from "./sessionPersist";
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
  /** Restore table tabs too (they are checked against the database and skipped when gone). */
  tables: boolean;
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
        out.set(path, await ipc.files.readText(path));
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
  const saved = applyAutoSave(ws.tabs, getPrefs().autoSaveQueryTabs, ws.activeTab);
  const wanted = saved.tabs.filter((t) => opts.tables || t.kind === "query");
  const activeSaved = saved.activeTab === null ? null : saved.tabs[saved.activeTab];
  const [tables, files] = await Promise.all([
    opts.tables ? existingTables(qc, connectionId, wanted) : Promise.resolve(null),
    readFiles(wanted),
  ]);
  // The connection may have gone away (disconnect) while the checks ran.
  if (!useConnectionsStore.getState().byId[connectionId]) return { restored: 0, skipped: [] };

  const dialect = dialectOf(conn.info.driver);
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
    } else {
      const content = queryTabContent(t, t.filePath ? files.get(t.filePath) : undefined);
      if (!content) {
        skipped.push(t.title);
        continue;
      }
      seedEditor(id, content.doc, { dialect });
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
  const reopened = new Map<string, string>();
  try {
    for (const ws of toOpen) {
      beginRestoring(ws.profileId);
      let connectionId: string | null = null;
      try {
        deps.openProfile(ws.profileId);
        connectionId = await waitForProfileConnection(ws.profileId);
        if (!connectionId) continue;
        reopened.set(ws.profileId, connectionId);
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
        endRestoring(ws.profileId, connectionId !== null);
      }
    }
  } finally {
    toast.dismiss(RESTORE_TOAST_ID);
  }
  const target = session.activeProfileId ? reopened.get(session.activeProfileId) : undefined;
  if (target && useConnectionsStore.getState().byId[target]) {
    useConnectionsStore.getState().setActive(target);
    useUiStore.getState().setView("workspace");
  }
}

let uninstallConnectRestore: (() => void) | null = null;

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
      const ws = rememberedWorkspace(profileId);
      // A reconnect swaps the connection id but keeps the workspace (and its tabs); the startup
      // restore handles the profiles it reopens itself.
      const replaced = prev.order.some((p) => prev.byId[p]?.info.profileId === profileId);
      if (!ws || replaced || isRestoring(profileId)) continue;
      beginRestoring(profileId);
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
        .finally(() => endRestoring(profileId, true));
    }
  });
}

/** Test-only: allow another startup restore and drop the connect hook. */
export function resetSessionRestoreForTests(): void {
  started = false;
  uninstallConnectRestore?.();
  uninstallConnectRestore = null;
}
