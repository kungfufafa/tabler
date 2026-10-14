import { useEffect } from "react";
import { createPortal } from "react-dom";
import { actions } from "@/features/palette/actions";
import { ipc } from "@/lib/ipc";
import { usePrefs } from "@/stores/prefs";
import { startIdleTimer } from "./idle";
import { lockTabler } from "./lockTabler";
import { LockScreen } from "./LockScreen";
import { installLockSync, loadLockStatus, lockNow, useLockStore, type LockReason } from "./store";

/** What the system's `lock:system` reasons mean here. */
function reasonOf(event: string): LockReason {
  return event === "user" ? "user" : "system";
}

/**
 * The app lock: reads the passcode status at startup (locked from the first paint when one was
 * set), locks after "Auto lock if away for" without activity, when the Mac sleeps or its screen
 * locks, and on Lock Tabler; while locked only Lock Tabler runs (every other action, shortcut and
 * menu item is refused) and the lock screen covers the window. Mount once.
 */
export function LockHost() {
  const status = useLockStore((s) => s.status);
  const enabled = useLockStore((s) => s.enabled);
  const minutes = usePrefs((p) => p.autoLockMinutes);

  useEffect(() => {
    void loadLockStatus();
  }, []);

  useEffect(() => installLockSync(), []);

  useEffect(() => actions.register("app.lock", lockTabler), []);

  useEffect(() => {
    if (status !== "locked") return;
    return actions.setGate((id) => id === "app.lock");
  }, [status]);

  useEffect(() => {
    if (!enabled || status !== "unlocked" || minutes === 0) return;
    const timer = startIdleTimer({ idleMs: minutes * 60_000, onIdle: () => lockNow("idle") });
    return timer.stop;
  }, [enabled, status, minutes]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void ipc.security
      .onLock((reason) => lockNow(reasonOf(reason)))
      .then((u) => {
        if (cancelled) u();
        else unlisten = u;
      })
      .catch((err: unknown) => console.warn("[lock] cannot listen for system locks", err));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (status !== "locked") return null;
  return createPortal(<LockScreen />, document.body);
}
