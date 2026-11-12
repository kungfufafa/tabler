import type { QueryClient } from "@tanstack/react-query";
import { MAIN_WINDOW, windowLabel } from "@/features/tabs/windowHandoff";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs, loadPrefs } from "@/stores/prefs";
import { startupOpening } from "@/stores/windowShape";
import { openConnectionDialog } from "./store";
import { profileKeys } from "./useConnections";

/**
 * Opens the explicitly chosen startup connection, optionally showing its connection form first.
 * The session host has already loaded remembered tabs when this runs.
 */

export type StartupOutcome =
  | "welcome"
  | "restored"
  | "missing"
  | "opened"
  | "form"
  /** Not the main window: nothing opens by itself. */
  | "other-window";

export interface StartupDeps {
  qc: QueryClient;
  openProfile: (profileId: string) => void;
  /** Default: the app's preferences. */
  prefsLoaded?: () => Promise<void>;
  /** Default: this window's label (`windowLabel`). */
  windowLabel?: () => Promise<string>;
}

let started = false;

/** Runs the "At launch" preference once per app start. */
export async function openStartupConnection(deps: StartupDeps): Promise<StartupOutcome> {
  if (started) return "welcome";
  started = true;
  if ((await (deps.windowLabel ?? windowLabel)()) !== MAIN_WINDOW) return "other-window";
  await (deps.prefsLoaded ?? loadPrefs)().catch(() => undefined);
  const prefs = getPrefs();
  if (prefs.startupAction !== "connection" || prefs.startupProfileId === "") return "welcome";
  const profiles = await deps.qc
    .fetchQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 })
    .catch(() => [] as ConnectionProfile[]);
  // Something was opened already (a deep link, the user was faster).
  if (useConnectionsStore.getState().order.length > 0) return "restored";
  const profile = profiles.find((p) => p.id === prefs.startupProfileId);
  if (!profile) return "missing";
  if (prefs.startupPromptForm) {
    openConnectionDialog(profile);
    return "form";
  }
  // A workspace window while it connects, not a Welcome window that grows a moment later.
  startupOpening();
  deps.openProfile(profile.id);
  return "opened";
}

/** Test helper: lets `openStartupConnection` run again. */
export function resetStartupConnection(): void {
  started = false;
}
