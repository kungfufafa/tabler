import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { profileKeys } from "@/features/connections/useConnections";
import {
  IpcError,
  ipc,
  toIpcError,
  type ConnStatus,
  type ConnectionInfo,
  type TunnelStatusEvent,
} from "@/lib/ipc";
import { listenEvent } from "@/lib/ipc/events";
import { useConnectionsStore, type OpenConnection } from "@/stores/connections";
import { useTabsStore } from "@/stores/tabs";

export const KEEPALIVE_INTERVAL_MS = 60_000;
/**
 * How long a ping may take before the connection counts as lost. A half-open socket (sleep, network
 * switch, a firewall dropping packets) leaves the backend's `SELECT 1` waiting for the kernel's TCP
 * retransmission timeout, which takes minutes.
 */
export const PING_TIMEOUT_MS = 8_000;

export function pingTimeoutError(ms: number = PING_TIMEOUT_MS): IpcError {
  return new IpcError({
    code: "TIMEOUT",
    message: `The server did not answer within ${Math.round(ms / 1000)} s.`,
  });
}

/** Races a ping against a deadline; rejects with `pingTimeoutError` when the server stays silent. */
export function pingWithDeadline(
  ping: Promise<ConnStatus>,
  ms: number = PING_TIMEOUT_MS,
): Promise<ConnStatus> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(pingTimeoutError(ms)), ms);
  });
  return Promise.race([ping, deadline]).finally(() => clearTimeout(timer));
}

/**
 * After a connection comes back without a new session, the query keys stay the same, so whatever
 * failed while it was down would keep showing its error (retries and focus refetches are off).
 */
function refetchFailed(qc: QueryClient | undefined, connectionId: string): void {
  void qc?.invalidateQueries({
    predicate: (q) => q.state.status === "error" && q.queryKey.includes(connectionId),
  });
}

/** One toast slot per profile, so "lost" / "reconnected" replace each other instead of stacking. */
const statusToastId = (profileId: string) => `connection-status:${profileId}`;

// ---- SSH tunnel status ----------------------------------------------------------------------

/**
 * How long a tunnel that went down while its connection still looks healthy waits for the
 * connection's "lost" status before it is announced anyway. A deliberate disconnect also closes
 * the tunnel (and reports it) just before the connection leaves the store; waiting lets that case
 * stay silent.
 */
export const TUNNEL_DOWN_GRACE_MS = 1_500;

interface PendingTunnelDown {
  message: string | null;
  timer: ReturnType<typeof setTimeout>;
}

/** Tunnels reported down whose connection has not been marked lost yet, by connection id. */
const pendingTunnelDown = new Map<string, PendingTunnelDown>();
/** Connections whose tunnel failure was announced; cleared when the tunnel is up again. */
const tunnelAnnounced = new Set<string>();

function takePendingTunnel(connectionId: string): PendingTunnelDown | undefined {
  const pending = pendingTunnelDown.get(connectionId);
  if (pending) {
    clearTimeout(pending.timer);
    pendingTunnelDown.delete(connectionId);
  }
  return pending;
}

function announceTunnelDown(connectionId: string, message: string | null): void {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn || conn.status === "disconnected" || tunnelAnnounced.has(connectionId)) return;
  tunnelAnnounced.add(connectionId);
  // Same slot as "Connection … lost", which it explains (the banner keeps the details).
  toast.error(`SSH tunnel to ${conn.info.name} closed`, {
    id: statusToastId(conn.info.profileId),
    description: message ?? undefined,
  });
}

/**
 * Handles a `tunnel:status` event: a tunnel that went down (or failed) under an open connection is
 * announced once; "up" (after a reconnect) only resets that. The backend flips the connection to
 * "lost" itself, which the banner shows.
 */
export function applyTunnelStatus(e: TunnelStatusEvent): void {
  const id = e.connectionId;
  if (e.status === "up") {
    takePendingTunnel(id);
    tunnelAnnounced.delete(id);
    return;
  }
  const conn = useConnectionsStore.getState().byId[id];
  if (!conn || conn.status === "disconnected") return;
  if (conn.status !== "connected") {
    takePendingTunnel(id);
    announceTunnelDown(id, e.message);
    return;
  }
  if (pendingTunnelDown.has(id)) return;
  const timer = setTimeout(() => {
    pendingTunnelDown.delete(id);
    announceTunnelDown(id, e.message);
  }, TUNNEL_DOWN_GRACE_MS);
  pendingTunnelDown.set(id, { message: e.message, timer });
}

/** Test helper: forget pending / announced tunnel failures. */
export function resetTunnelState(): void {
  for (const p of pendingTunnelDown.values()) clearTimeout(p.timer);
  pendingTunnelDown.clear();
  tunnelAnnounced.clear();
}

/**
 * Writes a status into the store and announces meaningful transitions. With `qc`, a recovery also
 * refetches the connection's queries that failed while it was down.
 */
export function applyStatus(
  connectionId: string,
  status: ConnStatus,
  message: string | null = null,
  qc?: QueryClient,
): void {
  const store = useConnectionsStore.getState();
  const conn = store.byId[connectionId];
  if (status === "disconnected") takePendingTunnel(connectionId);
  if (!conn || (conn.status === status && conn.message === message)) return;
  store.setStatus(connectionId, status, message);
  const id = statusToastId(conn.info.profileId);
  const tunnel = status === "lost" ? takePendingTunnel(connectionId) : undefined;
  if (tunnel) {
    // The SSH tunnel went first: say so instead of a generic "connection lost".
    announceTunnelDown(connectionId, tunnel.message);
  } else if (status === "lost" && conn.status === "connected") {
    if (tunnelAnnounced.has(connectionId)) return;
    toast.error(`Connection to ${conn.info.name} lost`, { id, description: message ?? undefined });
  } else if (status === "connected" && conn.status !== "connected") {
    toast.success(`Reconnected to ${conn.info.name}`, { id });
    refetchFailed(qc, connectionId);
  }
}

/** What a keepalive ping result means for a connection currently in `current`; null = no change. */
export function statusFromPing(current: ConnStatus, result: ConnStatus | null): ConnStatus | null {
  // A failed ping (error) or an id the backend no longer knows both mean the session is gone.
  const next: ConnStatus = result === null || result === "disconnected" ? "lost" : result;
  if (next === current) return null;
  // A reconnect is in flight; let it finish rather than flapping back to "lost".
  if (current === "reconnecting" && next === "lost") return null;
  return next;
}

function onPingResult(
  qc: QueryClient,
  connectionId: string,
  result: ConnStatus | null,
  error?: unknown,
): void {
  const conn = useConnectionsStore.getState().byId[connectionId];
  if (!conn) return;
  const next = statusFromPing(conn.status, result);
  if (!next) return;
  let message: string | null = null;
  if (next === "lost") {
    if (result === null) message = toIpcError(error).message;
    else if (result === "disconnected") message = "The session is no longer open on the server.";
  }
  applyStatus(connectionId, next, message, qc);
}

type VisibilityDoc = Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener">;

export interface KeepaliveOptions {
  ping: (connectionId: string) => Promise<ConnStatus>;
  connectionIds: () => readonly string[];
  onResult: (connectionId: string, status: ConnStatus | null, error?: unknown) => void;
  intervalMs?: number;
  /** A ping still pending after this long is reported as a failure (see `PING_TIMEOUT_MS`). */
  timeoutMs?: number;
  doc?: VisibilityDoc;
  now?: () => number;
}

/**
 * Pings every open connection each `intervalMs` while the document is visible, so a dropped
 * connection surfaces (banner) before the user's next query. Pauses while hidden and catches up
 * on return when a beat was missed. A ping that does not answer within `timeoutMs` is reported as
 * a failure right away; the connection is not pinged again until that ping settles (its late
 * answer is still reported, so a recovery shows up). Returns a stop function.
 */
export function startKeepalive(opts: KeepaliveOptions): () => void {
  const intervalMs = opts.intervalMs ?? KEEPALIVE_INTERVAL_MS;
  const timeoutMs = opts.timeoutMs ?? PING_TIMEOUT_MS;
  const doc = opts.doc ?? document;
  const now = opts.now ?? Date.now;
  const inFlight = new Set<string>();
  const deadlines = new Set<ReturnType<typeof setTimeout>>();
  let timer: ReturnType<typeof setInterval> | null = null;
  let lastBeat = now();
  let stopped = false;

  const beat = () => {
    lastBeat = now();
    for (const id of opts.connectionIds()) {
      if (inFlight.has(id)) continue;
      inFlight.add(id);
      const deadline = setTimeout(() => {
        deadlines.delete(deadline);
        if (!stopped) opts.onResult(id, null, pingTimeoutError(timeoutMs));
      }, timeoutMs);
      deadlines.add(deadline);
      const settle = () => {
        clearTimeout(deadline);
        deadlines.delete(deadline);
      };
      opts
        .ping(id)
        .then(
          (status) => {
            settle();
            if (!stopped) opts.onResult(id, status);
          },
          (err: unknown) => {
            settle();
            if (!stopped) opts.onResult(id, null, err);
          },
        )
        .finally(() => inFlight.delete(id));
    }
  };
  const resume = () => {
    if (timer === null) timer = setInterval(beat, intervalMs);
  };
  const pause = () => {
    if (timer !== null) clearInterval(timer);
    timer = null;
  };
  const onVisibility = () => {
    if (doc.visibilityState === "hidden") {
      pause();
      return;
    }
    if (now() - lastBeat >= intervalMs) beat();
    resume();
  };

  if (doc.visibilityState !== "hidden") resume();
  doc.addEventListener("visibilitychange", onVisibility);
  return () => {
    stopped = true;
    pause();
    for (const d of deadlines) clearTimeout(d);
    deadlines.clear();
    doc.removeEventListener("visibilitychange", onVisibility);
  };
}

// ---- connection-loss banner ------------------------------------------------------------------

export interface BannerState {
  title: string;
  message: string | null;
  /** Show a spinner instead of the warning icon. */
  spinning: boolean;
  canReconnect: boolean;
}

/** What the banner above the tabs shows for the active connection (null = hidden). */
export function bannerState(conn: OpenConnection | null, busy: boolean): BannerState | null {
  if (!conn || (conn.status !== "lost" && conn.status !== "reconnecting")) return null;
  const spinning = busy || conn.status === "reconnecting";
  const name = conn.info.name;
  return {
    title: spinning ? `Reconnecting to ${name}…` : `Connection to ${name} lost`,
    message: conn.message,
    spinning,
    canReconnect: !busy,
  };
}

/**
 * Swaps a dead connection for a freshly opened one: the new connection takes the old one's slot,
 * focus and schema, open tabs follow it, and caches keyed by the old id are dropped. Returns false
 * (and changes nothing) when the old connection is no longer open.
 */
export function migrateConnection(oldId: string, info: ConnectionInfo, qc: QueryClient): boolean {
  const before = useConnectionsStore.getState();
  const old = before.byId[oldId];
  if (!old) return false;
  const position = before.order.indexOf(oldId);
  const nextActive = before.activeId === oldId ? info.connectionId : before.activeId;
  const newId = info.connectionId;

  before.add(info);
  if (old.currentSchema) before.setCurrentSchema(newId, old.currentSchema);
  useTabsStore.getState().retargetConnection(oldId, newId);
  useConnectionsStore.getState().remove(oldId);
  useConnectionsStore.setState((s) => {
    const order = s.order.filter((id) => id !== newId);
    order.splice(position < 0 ? order.length : Math.min(position, order.length), 0, newId);
    return { order, activeId: nextActive };
  });

  qc.removeQueries({ predicate: (q) => q.queryKey.includes(oldId) });
  void qc.invalidateQueries({ queryKey: profileKeys.all });
  // Best effort: release whatever the backend still holds for the dead session.
  ipc.connections.disconnect(oldId).catch(() => undefined);
  return true;
}

export type ReconnectOutcome = "alive" | "reconnected" | "failed";

/** Reconnects in flight, by the id of the connection being recovered. */
const reconnects = new Map<string, Promise<ReconnectOutcome>>();
const reconnectListeners = new Set<() => void>();
const notifyReconnects = () => reconnectListeners.forEach((l) => l());
const subscribeReconnects = (onChange: () => void) => {
  reconnectListeners.add(onChange);
  return () => void reconnectListeners.delete(onChange);
};

/** Whether a `reconnectConnection` for this connection is still running. */
export function isReconnecting(connectionId: string): boolean {
  return reconnects.has(connectionId);
}

/** Reactive `isReconnecting`, shared by every banner instance (survives unmounts). */
export function useReconnectInFlight(connectionId: string | null): boolean {
  return useSyncExternalStore(
    subscribeReconnects,
    () => connectionId !== null && reconnects.has(connectionId),
  );
}

const openConnection = (id: string) => useConnectionsStore.getState().byId[id];

/** Best effort: closes a session we opened but will not use. */
function release(connectionId: string): void {
  ipc.connections.disconnect(connectionId).catch(() => undefined);
}

/**
 * A new session starts on the profile's default database; put it back on the one the user had
 * switched to, so retargeted tabs and staged edits keep pointing at the same tables.
 */
async function restoreDatabase(
  info: ConnectionInfo,
  wanted: string | null,
): Promise<ConnectionInfo> {
  if (info.driver === "sqlite" || !wanted || wanted === info.currentDatabase) return info;
  try {
    return await ipc.connections.switchDatabase(info.connectionId, wanted);
  } catch (err) {
    const e = toIpcError(err);
    throw new IpcError({
      ...e.payload,
      message: `Could not reopen database “${wanted}”: ${e.message}`,
    });
  }
}

async function runReconnect(connectionId: string, qc: QueryClient): Promise<ReconnectOutcome> {
  const conn = openConnection(connectionId);
  if (!conn) return "failed";
  const { name, profileId } = conn.info;
  const id = statusToastId(profileId);
  useConnectionsStore.getState().setStatus(connectionId, "reconnecting", conn.message);
  /** A session opened here that has not been handed to the store yet. */
  let opened: string | null = null;
  try {
    const status = await pingWithDeadline(ipc.connections.ping(connectionId)).catch(
      (): ConnStatus => "lost",
    );
    // Disconnected by the user meanwhile: nothing to recover.
    if (!openConnection(connectionId)) return "failed";
    if (status === "connected") {
      useConnectionsStore.getState().setStatus(connectionId, "connected", null);
      refetchFailed(qc, connectionId);
      toast.success(`Reconnected to ${name}`, { id });
      return "alive";
    }
    const fresh = await ipc.connections.connect(profileId);
    opened = fresh.connectionId;
    const wanted = (openConnection(connectionId) ?? conn).info.currentDatabase;
    const info = await restoreDatabase(fresh, wanted);
    if (!migrateConnection(connectionId, info, qc)) {
      release(fresh.connectionId);
      return "failed";
    }
    opened = null;
    toast.success(`Reconnected to ${name}`, {
      id,
      description: "Opened a new session; your tabs were kept.",
    });
    return "reconnected";
  } catch (err) {
    if (opened) release(opened);
    if (!openConnection(connectionId)) return "failed";
    const message = toIpcError(err).message;
    useConnectionsStore.getState().setStatus(connectionId, "lost", message);
    toast.error(`Could not reconnect to ${name}`, { id, description: message });
    return "failed";
  }
}

/**
 * Banner "Reconnect": a ping first (the server may simply be back), otherwise a fresh connection
 * from the same profile, switched to the database the old one was on, that the open tabs are moved
 * onto. Calls for a connection that is already reconnecting share the attempt in flight.
 */
export function reconnectConnection(
  connectionId: string,
  qc: QueryClient,
): Promise<ReconnectOutcome> {
  const running = reconnects.get(connectionId);
  if (running) return running;
  const attempt = runReconnect(connectionId, qc).finally(() => {
    reconnects.delete(connectionId);
    notifyReconnects();
  });
  reconnects.set(connectionId, attempt);
  notifyReconnects();
  return attempt;
}

/** Keeps the connections store in sync with backend status events and runs the keepalive. Mount once. */
export function useConnectionEvents(): void {
  const qc = useQueryClient();
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenEvent("connection:status", (e) =>
      applyStatus(e.connectionId, e.status, e.message, qc),
    ).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [qc]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenEvent("tunnel:status", applyTunnelStatus).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
      resetTunnelState();
    };
  }, []);

  useEffect(
    () =>
      startKeepalive({
        ping: (id) => ipc.connections.ping(id),
        connectionIds: () => useConnectionsStore.getState().order,
        onResult: (id, status, error) => onPingResult(qc, id, status, error),
      }),
    [qc],
  );
}
