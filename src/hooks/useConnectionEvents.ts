import { useEffect } from "react";
import { toast } from "sonner";
import { listenEvent } from "@/lib/ipc/events";
import { useConnectionsStore } from "@/stores/connections";

/** Keeps the connections store in sync with backend status events. Mount once. */
export function useConnectionEvents(): void {
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenEvent("connection:status", (e) => {
      const store = useConnectionsStore.getState();
      const conn = store.byId[e.connectionId];
      if (!conn) return;
      store.setStatus(e.connectionId, e.status, e.message);
      if (e.status === "lost") {
        toast.error(`Connection to ${conn.info.name} lost`, {
          description: e.message ?? undefined,
        });
      } else if (e.status === "connected" && conn.status !== "connected") {
        toast.success(`Reconnected to ${conn.info.name}`);
      }
    }).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
