import { PASSWORD_MODE_LABEL } from "@/features/connections/schema";
import {
  EXTRA_SECRET_LABEL,
  extraSecrets,
  inSentence,
  secretLabel,
} from "@/features/drivers/catalog";
import {
  ipc,
  type ConnectionInfo,
  type ConnectionProfile,
  type ExtraSecret,
  type ProfileInput,
  type SafeModeLevel,
} from "@/lib/ipc";
import {
  lines,
  nameReason,
  passwordSecret,
  passwordTitle,
  promptTarget,
  rememberSafeModeLevel,
} from "./guard";
import { effectiveLevel, isPasswordLevel, lowersSafeLevel, SAFE_MODE_LABEL } from "./levels";
import { requestSafetyPrompt } from "./store";

type PromptInfo = Pick<ConnectionInfo, "profileId" | "name" | "driver">;

const profileInfo = (profile: ConnectionProfile): PromptInfo => ({
  profileId: profile.id,
  name: profile.name,
  driver: profile.driver,
});

const levelChange = (from: SafeModeLevel, to: SafeModeLevel) =>
  `Switching from ${SAFE_MODE_LABEL[from]} to ${SAFE_MODE_LABEL[to]}.`;

/**
 * Asks for the password of a Safe Mode 1 / 2 `profile` before a change that weakens what protects
 * it (`description` says which); profiles at other levels go ahead without asking. True = go ahead.
 */
export async function confirmProtectedChange(
  profile: ConnectionProfile,
  description: string,
  info: PromptInfo = profileInfo(profile),
): Promise<boolean> {
  if (!isPasswordLevel(effectiveLevel(profile.options.safeMode))) return true;
  const secret = await passwordSecret(profile);
  const target = promptTarget(info, profile);
  return requestSafetyPrompt({
    kind: "password",
    secret,
    target,
    title: passwordTitle(secret, target.name),
    description: lines(description, nameReason(secret, profile)),
    statements: [],
    confirmLabel: "Continue",
  });
}

/**
 * Asks for the password when `profile` would leave a Safe level for one that asks less (the
 * toolbar lock goes through here). True = go ahead.
 */
export async function confirmSafeModeChange(
  profile: ConnectionProfile,
  level: SafeModeLevel,
  info: PromptInfo = profileInfo(profile),
): Promise<boolean> {
  const current = effectiveLevel(profile.options.safeMode);
  if (!lowersSafeLevel(current, level)) return true;
  return confirmProtectedChange(profile, levelChange(current, level), info);
}

/**
 * What saving `next` over the Safe Mode 1 / 2 profile `current` changes that the password
 * protects: a lower level, Read-only turned off, another password storage, a new or removed
 * password (an API token… on the drivers that call it so) or second secret (a key passphrase, a
 * session token). Empty below Safe Mode, or when nothing protected changes.
 */
export function protectedChanges(current: ConnectionProfile, next: ProfileInput): string[] {
  const from = effectiveLevel(current.options.safeMode);
  if (!isPasswordLevel(from)) return [];
  const options = next.profile.options;
  const out: string[] = [];
  const to = effectiveLevel(options.safeMode);
  if (lowersSafeLevel(from, to)) out.push(levelChange(from, to));
  if (current.options.readOnly && !options.readOnly) out.push("Turning off Read-only.");
  const mode = current.options.passwordMode ?? "keychain";
  const nextMode = options.passwordMode ?? "keychain";
  if (nextMode !== mode)
    out.push(
      `Changing the password storage from ${PASSWORD_MODE_LABEL[mode]} to ${PASSWORD_MODE_LABEL[nextMode]}.`,
    );
  const secret = inSentence(secretLabel(next.profile.driver));
  if (next.password !== null) out.push(`Saving a new ${secret}.`);
  if (next.clearPassword) out.push(`Removing the saved ${secret}.`);
  // Absent or null: untouched; "" removes the stored one.
  const typed: [ExtraSecret, string | null | undefined][] = [
    ["keyPassphrase", next.keyPassphrase],
    ["sessionToken", next.sessionToken],
  ];
  const takes = extraSecrets(next.profile.driver);
  for (const [extra, value] of typed) {
    if (value === null || value === undefined || !takes.includes(extra)) continue;
    const noun = inSentence(EXTRA_SECRET_LABEL[extra]);
    out.push(value === "" ? `Removing the saved ${noun}.` : `Saving a new ${noun}.`);
  }
  return out;
}

/**
 * Switches the connection's profile to `level` and saves it (the guard reads the profile on every
 * check, so it applies to the next query). Leaving a Safe level for one that asks less first asks
 * for the password, like TablePlus. Resolves with the saved profile, or null when the user
 * cancelled; rejects when loading or saving the profile fails.
 */
export async function changeSafeModeLevel(
  info: Pick<ConnectionInfo, "profileId" | "name" | "driver">,
  level: SafeModeLevel,
): Promise<ConnectionProfile | null> {
  const profile = await ipc.profiles.get(info.profileId);
  const current = effectiveLevel(profile.options.safeMode);
  if (current === level && profile.options.safeMode !== null) return profile;
  if (!(await confirmSafeModeChange(profile, level, info))) return null;
  const saved = await ipc.profiles.save({
    profile: { ...profile, options: { ...profile.options, safeMode: level } },
    password: null,
    sshPassword: null,
    sshPassphrase: null,
    clearPassword: false,
  });
  rememberSafeModeLevel(saved.id, saved.options.safeMode);
  return saved;
}
