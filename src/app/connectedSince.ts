import { useConnectionsStore } from "@/stores/connections";

/**
 * When each open connection's session started, as first seen in the connections store (a
 * reconnect opens a new session under a new id, so it starts over). For the status bar's details.
 */
const since = new Map<string, number>();

function record(ids: readonly string[], now: number): void {
  for (const id of ids) if (!since.has(id)) since.set(id, now);
  for (const id of since.keys()) if (!ids.includes(id)) since.delete(id);
}

record(useConnectionsStore.getState().order, Date.now());
useConnectionsStore.subscribe((s, prev) => {
  if (s.order !== prev.order) record(s.order, Date.now());
});

/** Epoch ms the connection was opened at (null when it is not open). */
export function connectedSince(connectionId: string): number | null {
  return since.get(connectionId) ?? null;
}

/** "09:41 (12 min ago)". */
export function sinceLabel(at: number, now: number = Date.now()): string {
  const d = new Date(at);
  const clock = [d.getHours(), d.getMinutes()].map((n) => String(n).padStart(2, "0")).join(":");
  const minutes = Math.floor(Math.max(0, now - at) / 60_000);
  const ago =
    minutes < 1
      ? "just now"
      : minutes < 60
        ? `${minutes} min ago`
        : minutes < 48 * 60
          ? `${Math.floor(minutes / 60)} h ago`
          : `${Math.floor(minutes / (24 * 60))} days ago`;
  return `${clock} (${ago})`;
}
