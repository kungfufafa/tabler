import { useCallback } from "react";
import { usePaletteStore } from "@/features/palette/store";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { useConnect } from "./useConnections";

/** The newest open connection of a profile, if any. */
export function openConnectionOf(profileId: string): string | null {
  const s = useConnectionsStore.getState();
  for (let i = s.order.length - 1; i >= 0; i--) {
    const id = s.order[i]!;
    if (s.byId[id]?.info.profileId === profileId) return id;
  }
  return null;
}

/**
 * Opens a saved profile: switches to its open connection when there is one (no second session,
 * pool and tunnel), otherwise connects, ignoring requests while that profile's connect is still in
 * flight (shared with the palette: one connect per profile at a time, whoever started it).
 */
export function useOpenProfile(): (profileId: string) => void {
  const { mutateAsync } = useConnect();
  return useCallback(
    (profileId: string) => {
      const open = openConnectionOf(profileId);
      if (open) {
        useConnectionsStore.getState().setActive(open);
        useUiStore.getState().setView("workspace");
        return;
      }
      if (!usePaletteStore.getState().beginConnect(profileId)) return;
      mutateAsync(profileId)
        .finally(() => usePaletteStore.getState().endConnect(profileId))
        // useConnect's onError already told the user.
        .catch(() => undefined);
    },
    [mutateAsync],
  );
}
