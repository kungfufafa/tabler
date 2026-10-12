import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useOpenProfile } from "@/features/connections/openProfile";
import { installMenuAcceleratorSync } from "@/lib/keys/menuSync";
import { loadPrefs } from "@/stores/prefs";
import { restoreAtStartup } from "@/stores/sessionRestore";

/**
 * App-wide session wiring, mounted once by the app shell: reads the saved session and reopens its
 * workspaces (General ▸ "Reopen closed workspaces at startup"), keeps it saved from then on, and
 * keeps the native menu's accelerators on the user's keymap.
 */
export function SessionHost() {
  const qc = useQueryClient();
  const openProfile = useOpenProfile();
  const openRef = useRef(openProfile);
  useEffect(() => {
    openRef.current = openProfile;
  }, [openProfile]);

  useEffect(() => {
    void restoreAtStartup({
      qc,
      openProfile: (profileId) => openRef.current(profileId),
      prefsLoaded: loadPrefs,
    });
  }, [qc]);

  useEffect(() => installMenuAcceleratorSync(), []);
  return null;
}
