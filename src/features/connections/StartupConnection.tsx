import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useOpenProfile } from "./openProfile";
import { openStartupConnection } from "./startup";

/** Mounted once by the app shell, next to the session restore. */
export function StartupConnectionHost() {
  const qc = useQueryClient();
  const openProfile = useOpenProfile();
  const openRef = useRef(openProfile);
  useEffect(() => {
    openRef.current = openProfile;
  }, [openProfile]);
  useEffect(() => {
    void openStartupConnection({ qc, openProfile: (id) => openRef.current(id) });
  }, [qc]);
  return null;
}
