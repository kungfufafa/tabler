import { toast } from "sonner";
import { confirmsWithPasscode, coreDialect } from "@/features/drivers/catalog";
import { ipc, type ConnectionProfile, type Driver, type SafeModeLevel } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { effectiveLevel } from "./levels";
import { decide, requestWrites } from "./policy";
import { requestSafetyPrompt, type PromptTarget, type SafeSecret } from "./store";

/** What is about to be sent to the server, for the connection's safe-mode check. */
export type GuardRequest =
  /** Statements typed in the SQL editor (one entry per statement about to run). */
  | { kind: "query"; statements: string[] }
  /** Generated SQL of staged data / structure changes about to be committed. */
  | { kind: "commit"; statements: string[] }
  /**
   * Any other operation that sends SQL or changes the server: import, restore, backup, kill a
   * process, create / drop a database, user management. `write: false` = read-only (e.g. backup).
   */
  | { kind: "operation"; label: string; write: boolean };

/** Title of the Alert Mode dialog for queries and commits (TablePlus's wording). */
export const SEND_QUERIES_TITLE = "Do you want to send these queries to the server?";
export const READ_ONLY_MESSAGE = "This connection is read-only";

/**
 * The last safe-mode level loaded (or saved) per profile id. When a connection's profile cannot be
 * loaded (deleted while connected, the app database failing) the guard keeps asking what it asked
 * last instead of going silent: it fails closed.
 */
const knownLevels = new Map<string, SafeModeLevel>();

/** Records the level a profile was loaded or saved with (see `fallbackLevel`). */
export function rememberSafeModeLevel(
  profileId: string,
  level: SafeModeLevel | null | undefined,
): void {
  knownLevels.set(profileId, effectiveLevel(level));
}

/** The level of a profile that could not be loaded: the last one known, else Alert Mode 1. */
export function fallbackLevel(profileId: string): SafeModeLevel {
  return knownLevels.get(profileId) ?? "alert";
}

async function loadProfile(profileId: string): Promise<ConnectionProfile | null> {
  try {
    const profile = await ipc.profiles.get(profileId);
    rememberSafeModeLevel(profileId, profile.options.safeMode);
    return profile;
  } catch (err) {
    // A connection whose profile went away: it keeps asking what it asked last (`fallbackLevel`).
    console.warn("[safety] could not load the connection's profile", err);
    return null;
  }
}

/**
 * What a Safe Mode prompt asks for: the DB password when the backend has one to check (stored in
 * the Keychain, or typed at connect for "Ask every time"), else the connection's name. A
 * connection whose secret nobody types (a token, or a command's output on a driver beyond the
 * core three, see `confirmsWithPasscode`) asks for the app passcode when one is set, else for its
 * name; the backend's `verify_safe_mode` makes the same choice.
 */
export async function passwordSecret(
  profile: Pick<ConnectionProfile, "id" | "driver" | "options"> | null,
): Promise<SafeSecret> {
  if (!profile) return "name";
  if (confirmsWithPasscode(profile)) {
    try {
      return (await ipc.security.passcodeStatus()).enabled ? "passcode" : "name";
    } catch {
      return "name";
    }
  }
  if (profile.options.passwordMode === "ask") return "password";
  if (profile.options.passwordMode === "none") return "name";
  try {
    return (await ipc.profiles.secretStatus(profile.id)).hasPassword ? "password" : "name";
  } catch {
    return "name";
  }
}

/** Title of a Safe Mode prompt. */
export function passwordTitle(secret: SafeSecret, name: string): string {
  switch (secret) {
    case "password":
      return `Enter the password of ${name} to continue`;
    case "passcode":
      return `Enter the app passcode to continue on ${name}`;
    case "name":
      return `Type the name of ${name} to continue`;
  }
}

/**
 * Why a Safe Mode prompt asks for the name (null when it asks for the password or the passcode):
 * there is no saved password to check, or the connection signs in with a token and no app
 * passcode is set.
 */
export function nameReason(
  secret: SafeSecret,
  profile?: Pick<ConnectionProfile, "driver" | "options"> | null,
): string | null {
  if (secret !== "name") return null;
  return profile && confirmsWithPasscode(profile)
    ? "This connection signs in with a token, which Safe Mode does not ask for, so it asks for its name instead. Set a passcode in Settings ▸ Security to be asked for that."
    : "This connection has no saved password to check, so Safe Mode asks for its name instead.";
}

/** Joins the non-empty lines of a prompt description. */
export function lines(...parts: (string | null | undefined)[]): string | null {
  const kept = parts.filter((p): p is string => !!p);
  return kept.length > 0 ? kept.join("\n") : null;
}

export function promptTarget(
  info: { profileId: string; name: string; driver: Driver },
  profile: ConnectionProfile | null,
): PromptTarget {
  return {
    profileId: info.profileId,
    name: profile?.name ?? info.name,
    driver: info.driver,
    tag: profile?.options.tag ?? null,
    level: profile ? effectiveLevel(profile.options.safeMode) : fallbackLevel(info.profileId),
  };
}

const CONFIRM_LABEL: Record<GuardRequest["kind"], string> = {
  query: "Send",
  commit: "Commit",
  operation: "Continue",
};

/**
 * Asks what the connection's safe-mode level requires (nothing, a confirmation alert, or the
 * password) before `request` is sent; resolves true to go ahead, false when the user declined.
 * Every path that sends user-visible SQL calls this right before the IPC call.
 *
 * A write on a read-only connection is refused outright (toast, false). Prompts are queued: one
 * dialog at a time, in call order.
 */
export async function guardExecution(
  connectionId: string,
  request: GuardRequest,
): Promise<boolean> {
  const conn = useConnectionsStore.getState().byId[connectionId];
  // Not an open connection: the backend refuses the call anyway.
  if (!conn) return true;
  const { info } = conn;
  // Another driver's statements all count as writes: the core classifier cannot read them.
  const writes = requestWrites(request, coreDialect(info.driver));
  const profile = await loadProfile(info.profileId);
  const target = promptTarget(info, profile);
  const decision = decide(target.level, writes, info.readOnly);
  if (decision === "go") return true;
  if (decision === "blocked") {
    toast.error(READ_ONLY_MESSAGE, {
      id: `safe-mode-read-only:${connectionId}`,
      description: "Nothing was sent. Turn off Read-only in Edit Connection… to change data.",
    });
    return false;
  }
  const statements = request.kind === "operation" ? [] : request.statements;
  const confirmLabel = CONFIRM_LABEL[request.kind];
  if (decision === "alert") {
    return requestSafetyPrompt({
      kind: "alert",
      target,
      title: request.kind === "operation" ? request.label : SEND_QUERIES_TITLE,
      description: null,
      statements,
      confirmLabel,
    });
  }
  const secret = await passwordSecret(profile);
  return requestSafetyPrompt({
    kind: "password",
    secret,
    target,
    title: passwordTitle(secret, target.name),
    description: lines(
      request.kind === "operation" ? request.label : null,
      nameReason(secret, profile),
    ),
    statements,
    confirmLabel,
  });
}
