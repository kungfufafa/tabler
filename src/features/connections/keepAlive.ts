import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import {
  applyStatus,
  pingsFreely,
  startKeepalive,
  statusFromPing,
} from "@/hooks/useConnectionEvents";
import { capsOf } from "@/lib/caps";
import { ipc, toIpcError, type ConnStatus } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { usePrefs } from "@/stores/prefs";

/** TablePlus's "Keep connection alive" interval. */
export const KEEP_ALIVE_INTERVAL_MS = 30_000;

/**
 * The open connections worth pinging: connected ones (a lost one waits for Reconnect) whose
 * session may be pinged freely (`LifecycleCaps.keepaliveSecs`; never one whose ping would bill
 * or wake it).
 */
export function keepAliveTargets(): string[] {
  const s = useConnectionsStore.getState();
  return s.order.filter((id) => s.byId[id]?.status === "connected" && pingsFreely(s.byId[id]));
}

/**
 * `keepAliveTargets` on a beat at `now`, without the ones whose own interval (longer than the
 * beat's, say 60 s) has not gone by since `sent` (their last ping, which this records).
 */
function dueTargets(sent: Map<string, number>, now: number): string[] {
  const { byId } = useConnectionsStore.getState();
  const due = keepAliveTargets().filter((id) => {
    const secs = byId[id] ? capsOf(byId[id].info).session.keepaliveSecs : null;
    const last = sent.get(id);
    // A beat may fire a little early: half a beat of slack.
    return last === undefined || now - last >= (secs ?? 0) * 1000 - KEEP_ALIVE_INTERVAL_MS / 2;
  });
  for (const id of due) sent.set(id, now);
  return due;
}

/**
 * "Keep connection alive" (Settings ▸ Connections, on by default): pings every connected
 * session every 30 s, or at its session's longer interval (its pool and, while idle, the SQL
 * editor's own connection), so servers and firewalls that drop idle connections (MySQL
 * `wait_timeout`, NAT timeouts) keep them open. A failed ping marks the connection lost, like the
 * status keepalive does. Turning it off leaves that 60 s health check (`useConnectionEvents`)
 * running. Mount once (AppShell).
 */
export function useKeepAlive(): void {
  const enabled = usePrefs((p) => p.keepAlive);
  const qc = useQueryClient();
  useEffect(() => {
    if (!enabled) return;
    const sent = new Map<string, number>();
    return startKeepalive({
      intervalMs: KEEP_ALIVE_INTERVAL_MS,
      ping: (id) => ipc.connections.ping(id),
      connectionIds: () => dueTargets(sent, Date.now()),
      onResult: (id: string, status: ConnStatus | null, error?: unknown) => {
        const conn = useConnectionsStore.getState().byId[id];
        if (!conn) return;
        const next = statusFromPing(conn.status, status);
        if (!next) return;
        const message =
          next !== "lost"
            ? null
            : status === null
              ? toIpcError(error).message
              : "The session is no longer open on the server.";
        applyStatus(id, next, message, qc);
      },
    });
  }, [enabled, qc]);
}
