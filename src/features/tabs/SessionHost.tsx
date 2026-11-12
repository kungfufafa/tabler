import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useOpenProfile } from "@/features/connections/openProfile";
import { openStartupConnection } from "@/features/connections/startup";
import { listenWindowEvent } from "@/lib/ipc/events";
import { installMenuAcceleratorSync } from "@/lib/keys/menuSync";
import { loadPrefs } from "@/stores/prefs";
import { installSessionOwnership, openHandoff, restoreAtStartup } from "@/stores/sessionRestore";
import { startupDone } from "@/stores/windowShape";
import { MAIN_WINDOW, windowLabel } from "./windowHandoff";

/**
 * The main window loads the saved session before applying "At launch". A later window shows
 * its handoff and takes over saving the session if it becomes the oldest window left.
 * Every window keeps the native menu's accelerators on the user's keymap.
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
      // Either way the window knows its shape once this is over: a workspace window while
      // something opens, else the Welcome window (`stores/windowShape`).
      if (label === MAIN_WINDOW) {
        const startupDeps = {
          qc,
          openProfile: (profileId: string) => openRef.current(profileId),
          prefsLoaded: loadPrefs,
        };
        void restoreAtStartup(startupDeps)
          .then(() => {
            if (!cancelled) return openStartupConnection(startupDeps);
          })
          .finally(startupDone);
      } else {
        void loadPrefs();
        void openHandoff({ qc, prefsLoaded: loadPrefs }).finally(startupDone);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [qc]);

  // A connection picked in the Welcome window while this (workspace) window is open opens here
  // (macOS, `routeProfileToWorkspace`).
  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | null = null;
    void listenWindowEvent("window:open-profile", (profileId) => openRef.current(profileId)).then(
      (u) => {
        if (gone) u();
        else unlisten = u;
      },
    );
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => installSessionOwnership(qc), [qc]);
  useEffect(() => installMenuAcceleratorSync(), []);
  return null;
}
