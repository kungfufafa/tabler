import type { QueryClient } from "@tanstack/react-query";
import { ipc, type ConnectionProfile } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { getPrefs, loadPrefs } from "@/stores/prefs";
import type { Session } from "@/stores/session";
import { loadSession } from "@/stores/sessionPersist";
import { MAIN_WINDOW, windowLabel } from "@/features/tabs/windowHandoff";
import { openConnectionDialog } from "./store";
import { profileKeys } from "./useConnections";

/**
 * General ▸ Application ▸ "At launch" (TablePlus's "Connection startup", §10.1): the welcome
 * screen, or a chosen connection opened at startup, or its connection form ("Prompt the input
 * form"). The startup session restore wins: when "Reopen closed workspaces at startup" reopens
 * workspaces, the default connection is not opened as well. Only the main window runs it: a
 * window opened later (File ▸ New Window, "Open in New Window") shows what it was opened with.
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
  /** Defaults: the app's preferences and saved session. */
  prefsLoaded?: () => Promise<void>;
  session?: () => Promise<Session>;
  /** Default: this window's label (`windowLabel`). */
  windowLabel?: () => Promise<string>;
}

/** Whether the startup restore reopens workspaces (the same test `restoreAtStartup` makes). */
export function restoreReopens(
  session: Session,
  profiles: readonly ConnectionProfile[],
  reopenWorkspaces: boolean,
): boolean {
  if (!reopenWorkspaces) return false;
  return session.workspaces.some((w) => w.open && profiles.some((p) => p.id === w.profileId));
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
  const [session, profiles] = await Promise.all([
    (deps.session ?? loadSession)(),
    deps.qc
      .fetchQuery({ queryKey: profileKeys.all, queryFn: ipc.profiles.list, staleTime: 60_000 })
      .catch(() => [] as ConnectionProfile[]),
  ]);
  if (restoreReopens(session, profiles, prefs.reopenWorkspaces)) return "restored";
  // Something was opened already (a deep link, the user was faster).
  if (useConnectionsStore.getState().order.length > 0) return "restored";
  const profile = profiles.find((p) => p.id === prefs.startupProfileId);
  if (!profile) return "missing";
  if (prefs.startupPromptForm) {
    openConnectionDialog(profile);
    return "form";
  }
  deps.openProfile(profile.id);
  return "opened";
}

/** Test helper: lets `openStartupConnection` run again. */
export function resetStartupConnection(): void {
  started = false;
}
