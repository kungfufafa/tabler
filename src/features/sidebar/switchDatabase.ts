import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { create } from "zustand";
import { closeConnectionTabs } from "@/features/connections/closeGuard";
import { openConnectionOf } from "@/features/connections/openProfile";
import {
  askConnectionPassword,
  isPromptCancelled,
  takeHandedOverPassword,
} from "@/features/connections/passwordPromptStore";
import {
  hostKeyDeclined,
  profileKeys,
  withHostKeyTrust,
} from "@/features/connections/useConnections";
import { usePaletteStore } from "@/features/palette/store";
import { resetDatabaseCaches } from "@/features/structure/invalidate";
import { connectionWorkspaceKey, moveWorkspace, railOrder } from "@/features/tabs/workspaces";
import { ipc, toIpcError, type ConnectionInfo, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { workspaceKey } from "@/stores/session";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

interface DatabaseSwitchState {
  /** connectionId → database a switch is on its way to. */
  pending: Readonly<Record<string, string>>;
}

/** Database switches in flight, shared by the sidebar picker and the palette (⌘K). */
export const useDatabaseSwitchStore = create<DatabaseSwitchState>()(() => ({ pending: {} }));

/** The database `connectionId` is being switched to, or null. */
export function useSwitchingDatabase(connectionId: string | null): string | null {
  return useDatabaseSwitchStore((s) => (connectionId ? (s.pending[connectionId] ?? null) : null));
}

function setPending(connectionId: string, database: string | null) {
  useDatabaseSwitchStore.setState((s) => {
    const pending = { ...s.pending };
    if (database === null) delete pending[connectionId];
    else pending[connectionId] = database;
    return { pending };
  });
}

const errorMessage = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** One switch at a time: a second one would race the first for the connection's info. */
function busy(connectionId: string): boolean {
  return useDatabaseSwitchStore.getState().pending[connectionId] !== undefined;
}

/**
 * The workspace changed database in place: its saved rail position follows it (the rail orders
 * workspaces by profile and database).
 */
function renameRailKey(from: string, to: string): void {
  const ui = useUiStore.getState();
  if (from === to || !ui.railOrder.includes(from)) return;
  ui.setRailOrder(
    ui.railOrder.includes(to)
      ? ui.railOrder.filter((k) => k !== from)
      : ui.railOrder.map((k) => (k === from ? to : k)),
  );
}

async function apply(qc: QueryClient, connectionId: string, database: string): Promise<boolean> {
  // Two requests can wait on the same "Discard changes?" prompt; only the first one runs.
  if (busy(connectionId)) return false;
  setPending(connectionId, database);
  try {
    const before = useConnectionsStore.getState().byId[connectionId];
    const info = await ipc.connections.switchDatabase(connectionId, database);
    const conns = useConnectionsStore.getState();
    conns.setInfo(info);
    const after = useConnectionsStore.getState().byId[connectionId];
    if (before && after)
      renameRailKey(connectionWorkspaceKey(before), connectionWorkspaceKey(after));
    conns.setCurrentSchema(connectionId, null);
    // Tabs opened while the switch was on its way belong to the old database too.
    useTabsStore.getState().closeForConnection(connectionId);
    // Same connection id, another database: nothing cached for the old one may show again.
    resetDatabaseCaches(qc, connectionId);
    return true;
  } catch (e) {
    toast.error("Could not switch database", { description: errorMessage(e) });
    return false;
  } finally {
    setPending(connectionId, null);
  }
}

/**
 * Points `connectionId` at another database of the same server (PostgreSQL, MySQL). Switching
 * drops every tab of the connection, so tabs holding unsaved work get the "Discard changes?" prompt
 * first; cancelling keeps everything as it is. Failures are reported as a toast. Resolves true
 * once the connection is on `database` (right away when it already is).
 */
export async function switchDatabase(
  qc: QueryClient,
  connectionId: string,
  database: string,
): Promise<boolean> {
  const connection = useConnectionsStore.getState().byId[connectionId];
  if (!connection) return false;
  if (connection.info.currentDatabase === database) return true;
  if (busy(connectionId)) return false;
  let applied: Promise<boolean> = Promise.resolve(false);
  const proceeded = await closeConnectionTabs(connectionId, () => {
    applied = apply(qc, connectionId, database);
  });
  return proceeded && (await applied);
}

// --- per-database workspaces ---------------------------------------------------------------

/** The open connection (workspace) of `profileId` on `database`, if any. */
export function workspaceOf(profileId: string, database: string | null): string | null {
  const s = useConnectionsStore.getState();
  const key = workspaceKey({ profileId, database });
  return (
    s.order.find((id) => {
      const conn = s.byId[id];
      return conn !== undefined && connectionWorkspaceKey(conn) === key;
    }) ?? null
  );
}

function showConnection(connectionId: string): void {
  useConnectionsStore.getState().setActive(connectionId);
  useUiStore.getState().setView("workspace");
}

/**
 * Connects `profileId` straight to `database` (null: the profile's own) in a workspace of its
 * own and shows it. An "Ask every time" profile is asked for its password only when the backend
 * does not have it (it keeps the one typed while any connection of the profile is open, in any
 * window); SSH host keys are confirmed as on any connect. Resolves with
 * the new connection, or null when the connect failed (a toast says why) or was cancelled.
 */
export async function connectWorkspace(
  qc: QueryClient,
  profileId: string,
  database: string | null,
): Promise<string | null> {
  const palette = usePaletteStore.getState();
  // One connect per profile at a time (shared with the palette and the connection list).
  if (!palette.beginConnect(profileId)) return null;
  const profile = qc
    .getQueryData<ConnectionProfile[]>(profileKeys.all)
    ?.find((p) => p.id === profileId);
  const name = profile?.name ?? null;
  const attempt = (password: string | null) =>
    withHostKeyTrust(profileId, "connect", name, () =>
      ipc.connections.connect(profileId, password, database),
    );
  try {
    let info: ConnectionInfo;
    if (profile?.options.passwordMode === "ask" && openConnectionOf(profileId) === null) {
      const handedOver = takeHandedOverPassword(profileId);
      if (handedOver !== undefined) info = await attempt(handedOver);
      else
        try {
          // Another window's connection of the profile may have it (the backend keeps it).
          info = await attempt(null);
        } catch (e) {
          if (toIpcError(e).code !== "AUTH_FAILED") throw e;
          info = await attempt(await askConnectionPassword(profile.name, "connect"));
        }
    } else info = await attempt(null);
    useConnectionsStore.getState().add(info);
    useUiStore.getState().setView("workspace");
    void qc.invalidateQueries({ queryKey: profileKeys.all });
    return info.connectionId;
  } catch (e) {
    if (!hostKeyDeclined(e) && !isPromptCancelled(e) && toIpcError(e).code !== "CANCELLED")
      toast.error(`Could not open ${database ?? name ?? "the connection"}`, {
        description: toIpcError(e).message,
      });
    return null;
  } finally {
    usePaletteStore.getState().endConnect(profileId);
  }
}

/**
 * TablePlus ⌘K → open a database (double-click / ↩): another database of the connection's server
 * opens in a new workspace next to this one (a connection of its own, since a session is on one
 * database); the database's workspace is shown when it is open already. "Open in Current
 * Workspace" (`switchDatabase`) keeps today's behaviour. Resolves true once that workspace shows.
 */
export async function openDatabaseWorkspace(
  qc: QueryClient,
  connectionId: string,
  database: string,
): Promise<boolean> {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn) return false;
  if (conn.info.currentDatabase === database) {
    showConnection(connectionId);
    return true;
  }
  // SQLite has one database per file: nothing to open beside it.
  if (conn.info.driver === "sqlite") return false;
  const open = workspaceOf(conn.info.profileId, database);
  if (open) {
    showConnection(open);
    return true;
  }
  if (busy(connectionId)) return false;
  setPending(connectionId, database);
  let created: string | null;
  try {
    created = await connectWorkspace(qc, conn.info.profileId, database);
  } finally {
    setPending(connectionId, null);
  }
  if (!created) return false;
  // The new workspace sits right after the one it was opened from.
  const rail = railOrder();
  const from = rail.indexOf(created);
  const after = rail.indexOf(connectionId);
  if (from >= 0 && after >= 0 && from !== after + 1)
    moveWorkspace(from, from > after ? after + 1 : after);
  return true;
}
