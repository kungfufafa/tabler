import { useCallback } from "react";
import { usePaletteStore } from "@/features/palette/store";
import { isWelcomeWindow, routeProfileToWorkspace } from "@/features/tabs/windowHandoff";
import { useConnectionsStore } from "@/stores/connections";
import { useUiStore } from "@/stores/ui";
import { dropHandedOverPassword, hasHandedOverPassword } from "./passwordPromptStore";
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
 *
 * macOS: in the Welcome window with a workspace window open, the profile opens there (a new
 * workspace in that window, like TablePlus) and the Welcome window closes. A password the
 * connection form handed over stays in this window, so that Connect is done here.
 */
export function useOpenProfile(): (profileId: string) => void {
  const { mutateAsync } = useConnect();
  return useCallback(
    (profileId: string) => {
      const open = openConnectionOf(profileId);
      if (open) {
        // No connect: a password the connection form handed over for it must not linger.
        dropHandedOverPassword(profileId);
        useConnectionsStore.getState().setActive(open);
        useUiStore.getState().setView("workspace");
        return;
      }
      if (!usePaletteStore.getState().beginConnect(profileId)) {
        dropHandedOverPassword(profileId);
        return;
      }
      const connectHere = () =>
        mutateAsync(profileId)
          .finally(() => usePaletteStore.getState().endConnect(profileId))
          // useConnect's onError already told the user.
          .catch(() => undefined);
      if (isWelcomeWindow() && !hasHandedOverPassword(profileId)) {
        void routeProfileToWorkspace(profileId).then((routed) => {
          if (routed) usePaletteStore.getState().endConnect(profileId);
          else void connectHere();
        });
        return;
      }
      void connectHere();
    },
    [mutateAsync],
  );
}
