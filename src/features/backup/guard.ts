import { toast } from "sonner";
import { withHostKeyTrust } from "@/features/connections/useConnections";
import { openConnectionOf } from "@/features/connections/openProfile";
import { guardExecution, READ_ONLY_MESSAGE, type GuardRequest } from "@/features/safety/guard";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";

const writes = (request: GuardRequest) => request.kind !== "operation" || request.write;

/**
 * Whether the profile's safe-mode level can ask anything for `request`: silent never asks, the
 * "non-SELECT" levels skip read-only operations (a backup).
 */
export function safeModeMayAsk(profile: ConnectionProfile, request: GuardRequest): boolean {
  const level = profile.options.safeMode ?? "silent";
  if (level === "silent") return false;
  return writes(request) || level === "alert" || level === "safe";
}

/**
 * Opens a connection of the profile for the safe-mode check without changing the workspace: it
 * joins the open connections, the active one (and the welcome screen) stay as they were.
 */
async function openForGuard(profile: ConnectionProfile): Promise<string> {
  const store = useConnectionsStore.getState();
  const previous = store.activeId;
  const info = await withHostKeyTrust(profile.id, "connect", profile.name, () =>
    ipc.connections.connect(profile.id),
  );
  useConnectionsStore.getState().add(info);
  if (previous) useConnectionsStore.getState().setActive(previous);
  return info.connectionId;
}

/**
 * Runs the connection's safe-mode check before a backup or restore (`guardExecution` needs an
 * open connection): through the profile's open connection, or one opened for it when its level
 * can ask (a silent profile needs none). A write (a restore) on a read-only connection is refused
 * outright, open or not. Resolves false when the user declined; rejects when the connection could
 * not be opened.
 */
export async function guardJob(
  profile: ConnectionProfile,
  request: GuardRequest,
): Promise<boolean> {
  if (profile.options.readOnly && writes(request)) {
    toast.error(READ_ONLY_MESSAGE, {
      id: `safe-mode-read-only:${profile.id}`,
      description: "Nothing was restored. Turn off Read-only in Edit Connection… to change data.",
    });
    return false;
  }
  const open = openConnectionOf(profile.id);
  if (open) return guardExecution(open, request);
  if (!safeModeMayAsk(profile, request)) return true;
  return guardExecution(await openForGuard(profile), request);
}
