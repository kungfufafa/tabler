import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { profileKeys } from "@/features/connections/useConnections";
import { useLockStore } from "@/features/lock/store";
import { usePaletteStore } from "@/features/palette/store";
import { ipc, type OpenRequest } from "@/lib/ipc";
import { listenWindowEvent, type Unlisten } from "@/lib/ipc/events";
import { useConnectionsStore } from "@/stores/connections";
import { FilterConfirmDialog } from "./FilterConfirmDialog";
import { dropTemporary, openSqlFile, PENDING_FILE_TTL_MS, runPlan } from "./open";
import { planOpen } from "./route";
import { setPendingFile, useDeepLinkStore } from "./store";

/** How long the palette may stay closed with no connect running before a waiting file is
 * dropped (the user did not pick a connection). */
const PICK_GRACE_MS = 2000;

/**
 * Opens what Tabler was asked to open from outside (connection URLs, `tabler://` links, SQLite
 * and SQL files): takes the backend's queue on start and on every `deeplink:open`, routes each
 * request (`route.ts`) and carries it out (`open.ts`). Also removes a temporary connection's
 * profile once its last connection closes, and opens a waiting `.sql` file in the connection
 * the user picks, and asks before applying a link's raw SQL condition. Mount once per window.
 *
 * While the app is locked nothing is taken from the backend's queue (the requests wait there)
 * and nothing is opened: connecting with stored credentials is what the lock refuses. The
 * unlock drains the queue.
 */
export function DeepLinkHost() {
  const qc = useQueryClient();

  useEffect(() => {
    let gone = false;
    let unlisten: Unlisten | null = null;
    let running: Promise<void> | null = null;
    // Requests taken from the queue just before the lock engaged: opened after the unlock.
    let held: OpenRequest[] = [];
    const locked = () => useLockStore.getState().status === "locked";
    const drain = () => {
      if (locked()) return;
      running ??= (async () => {
        try {
          while (!locked()) {
            const requests = held.length > 0 ? held.splice(0) : await ipc.deeplink.takePending();
            if (requests.length === 0 || gone) break;
            for (const [i, request] of requests.entries()) {
              if (locked()) {
                held = requests.slice(i);
                break;
              }
              const [profiles, temporary] = await Promise.all([
                qc.ensureQueryData({ queryKey: profileKeys.all, queryFn: ipc.profiles.list }),
                ipc.deeplink.temporaryProfiles(),
              ]);
              const plan = planOpen(request, {
                profiles,
                temporary: new Set(temporary),
                activeConnectionId: useConnectionsStore.getState().activeId,
              });
              await runPlan(qc, plan);
            }
          }
        } catch (e) {
          console.warn("[deeplink] could not open a request", e);
        } finally {
          running = null;
        }
      })();
    };
    drain();
    void listenWindowEvent("deeplink:open", drain).then((fn) => {
      if (gone) fn();
      else unlisten = fn;
    });
    const unsubscribeLock = useLockStore.subscribe((s, prev) => {
      if (prev.status === "locked" && s.status === "unlocked") drain();
    });
    return () => {
      gone = true;
      unlisten?.();
      unsubscribeLock();
    };
  }, [qc]);

  // A temporary connection's profile goes away with its last connection (in any window).
  useEffect(
    () =>
      useConnectionsStore.subscribe((s, prev) => {
        const closed = prev.order
          .filter((id) => !s.byId[id])
          .map((id) => prev.byId[id]?.info.profileId)
          .filter((p): p is string => p !== undefined)
          .filter((p) => !Object.values(s.byId).some((c) => c.info.profileId === p));
        if (closed.length === 0) return;
        void (async () => {
          try {
            const [temporary, open] = await Promise.all([
              ipc.deeplink.temporaryProfiles(),
              ipc.connections.list(),
            ]);
            for (const profileId of new Set(closed)) {
              if (!temporary.includes(profileId)) continue;
              if (open.some((c) => c.profileId === profileId)) continue;
              await dropTemporary(qc, profileId);
            }
          } catch (e) {
            console.warn("[deeplink] could not check temporary connections", e);
          }
        })();
      }),
    [qc],
  );

  // A waiting `.sql` file opens in the connection the user picks next (connects or switches to).
  useEffect(
    () =>
      useConnectionsStore.subscribe((s, prev) => {
        const pending = useDeepLinkStore.getState().pendingFile;
        if (!pending || !s.activeId || s.activeId === prev.activeId) return;
        if (s.order.length < prev.order.length) return; // a connection closed: not a choice
        setPendingFile(null);
        if (Date.now() - pending.since > PENDING_FILE_TTL_MS) return;
        void openSqlFile(s.activeId, pending.path);
      }),
    [],
  );

  // ...unless the palette is closed without a connection being picked.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const check = () => {
      if (timer) clearTimeout(timer);
      timer = null;
      const { open, connecting } = usePaletteStore.getState();
      if (!useDeepLinkStore.getState().pendingFile || open || connecting.length > 0) return;
      timer = setTimeout(() => {
        const now = usePaletteStore.getState();
        if (!now.open && now.connecting.length === 0) setPendingFile(null);
      }, PICK_GRACE_MS);
    };
    const unsubscribe = usePaletteStore.subscribe(check);
    return () => {
      unsubscribe();
      if (timer) clearTimeout(timer);
    };
  }, []);

  // A link's raw SQL condition asks before it applies.
  return <FilterConfirmDialog />;
}
