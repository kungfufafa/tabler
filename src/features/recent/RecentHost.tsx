import { useEffect, useRef } from "react";
import { useOpenProfile } from "@/features/connections/openProfile";
import { useProfiles } from "@/features/connections/useConnections";
import { useLockStore } from "@/features/lock/store";
import { ipc } from "@/lib/ipc";
import { listenWindowEvent } from "@/lib/ipc/events";
import { parseRecentMenuId } from "./menuIds";
import { openRecent } from "./openRecent";
import { installRecentTracking } from "./track";

/**
 * File ▸ Open Recent (mount once): records what this window opens, opens the entries the menu
 * sends to it, and keeps the submenu's connection names in step with the saved connections.
 * While the app is locked an entry does nothing: this listener is not behind the action
 * registry's lock gate (`LockHost`), and the native menu is not covered by the lock screen.
 */
export function RecentHost() {
  const openProfile = useOpenProfile();
  const openProfileRef = useRef(openProfile);
  openProfileRef.current = openProfile;

  useEffect(() => installRecentTracking(), []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenWindowEvent("menu:action", (id) => {
      const pick = parseRecentMenuId(id);
      if (!pick || useLockStore.getState().status === "locked") return;
      openRecent(pick, openProfileRef.current);
    }).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // A connection renamed or deleted: the submenu shows the current names only.
  const profiles = useProfiles().data;
  const seen = useRef<string | null>(null);
  useEffect(() => {
    if (!profiles) return;
    const names = JSON.stringify(profiles.map((p) => [p.id, p.name]));
    if (seen.current === null) {
      seen.current = names;
      return;
    }
    if (seen.current === names) return;
    seen.current = names;
    ipc.recent
      .refreshMenu()
      .catch((e: unknown) => console.warn("[recent] could not refresh the menu", e));
  }, [profiles]);

  return null;
}
