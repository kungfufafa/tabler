import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { create } from "zustand";
import { closeConnectionTabs } from "@/features/connections/closeGuard";
import { resetDatabaseCaches } from "@/features/structure/invalidate";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";

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

async function apply(qc: QueryClient, connectionId: string, database: string): Promise<boolean> {
  // Two requests can wait on the same "Discard changes?" prompt; only the first one runs.
  if (busy(connectionId)) return false;
  setPending(connectionId, database);
  try {
    const info = await ipc.connections.switchDatabase(connectionId, database);
    const conns = useConnectionsStore.getState();
    conns.setInfo(info);
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
