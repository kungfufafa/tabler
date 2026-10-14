import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useOpenProfile } from "@/features/connections/openProfile";
import { installMenuAcceleratorSync } from "@/lib/keys/menuSync";
import { loadPrefs } from "@/stores/prefs";
import { installSessionOwnership, openHandoff, restoreAtStartup } from "@/stores/sessionRestore";
import { MAIN_WINDOW, windowLabel } from "./windowHandoff";

/**
 * App-wide session wiring, mounted once by the app shell. The main window reads the saved session
 * and reopens its workspaces (General ▸ "Reopen closed workspaces at startup"), and keeps it saved
 * from then on; a window opened later shows what it was opened with (a connection, a moved tab)
 * and is not part of the saved session until the main window closes: then the oldest window left
 * keeps it saved. Every window keeps the native menu's accelerators on the user's keymap.
 */
export function SessionHost() {
  const qc = useQueryClient();
  const openProfile = useOpenProfile();
  const openRef = useRef(openProfile);
  useEffect(() => {
    openRef.current = openProfile;
  }, [openProfile]);

  useEffect(() => {
    let cancelled = false;
    void windowLabel().then((label) => {
      if (cancelled) return;
      if (label === MAIN_WINDOW)
        void restoreAtStartup({
          qc,
          openProfile: (profileId) => openRef.current(profileId),
          prefsLoaded: loadPrefs,
        });
      else {
        void loadPrefs();
        void openHandoff({ qc, prefsLoaded: loadPrefs });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [qc]);

  useEffect(() => installSessionOwnership(qc), [qc]);
  useEffect(() => installMenuAcceleratorSync(), []);
  return null;
}
