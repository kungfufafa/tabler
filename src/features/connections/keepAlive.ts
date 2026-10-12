import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { applyStatus, startKeepalive, statusFromPing } from "@/hooks/useConnectionEvents";
import { ipc, toIpcError, type ConnStatus } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { usePrefs } from "@/stores/prefs";

/** TablePlus's "Keep connection alive" interval. */
export const KEEP_ALIVE_INTERVAL_MS = 30_000;

/** The open connections worth pinging: connected ones (a lost one waits for Reconnect). */
export function keepAliveTargets(): string[] {
  const s = useConnectionsStore.getState();
  return s.order.filter((id) => s.byId[id]?.status === "connected");
}

/**
 * "Keep connection alive" (Settings ▸ Connections, on by default): pings every connected
 * session every 30 s (its pool and, while idle, the SQL editor's own connection), so servers and
 * firewalls that drop idle connections (MySQL `wait_timeout`, NAT timeouts) keep them open. A
 * failed ping marks the connection lost, like the status keepalive does. Turning it off leaves
 * that 60 s health check (`useConnectionEvents`) running. Mount once (AppShell).
 */
export function useKeepAlive(): void {
  const enabled = usePrefs((p) => p.keepAlive);
  const qc = useQueryClient();
  useEffect(() => {
    if (!enabled) return;
    return startKeepalive({
      intervalMs: KEEP_ALIVE_INTERVAL_MS,
      ping: (id) => ipc.connections.ping(id),
      connectionIds: keepAliveTargets,
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
