import { toast } from "sonner";
import { profileToUrl } from "@/features/connections/url";
import { writeClipboard } from "@/features/datagrid/clipboard";
import { ipc, toIpcError, type ConnectionProfile } from "@/lib/ipc";
import { getPrefs, setPref } from "@/stores/prefs";
import { passwordSecret, passwordTitle, promptTarget } from "./guard";
import { effectiveLevel, isPasswordLevel } from "./levels";
import { requestSafetyPrompt } from "./store";

/** Title of the one-time confirmation before a URL with the password is copied. */
export const COPY_WITH_PASSWORD_TITLE = "Copy the URL with the password?";

/**
 * Whether the profile has a password the URL could carry: a stored one, or the one typed at
 * connect ("Ask every time", remembered while connected). Command-line (IAM) and no-password
 * profiles have none.
 */
export async function hasRevealablePassword(profile: ConnectionProfile): Promise<boolean> {
  if (profile.driver === "sqlite") return false;
  const mode = profile.options.passwordMode ?? "keychain";
  if (mode === "none" || mode === "command") return false;
  if (mode === "ask") return true;
  try {
    return (await ipc.profiles.secretStatus(profile.id)).hasPassword;
  } catch {
    return false;
  }
}

/**
 * What Copy as URL asks before it reveals the password, TablePlus's rules: a one-time
 * confirmation (the password is copied in plain text), then, when the connection is at Safe Mode
 * 1 / 2 or "Unlock Safe Mode with Touch ID" is on, the connection's password or Touch ID. True =
 * go ahead.
 */
export async function confirmRevealPassword(profile: ConnectionProfile): Promise<boolean> {
  const info = { profileId: profile.id, name: profile.name, driver: profile.driver };
  const target = promptTarget(info, profile);
  if (!getPrefs().copyUrlPasswordConfirmed) {
    const ok = await requestSafetyPrompt({
      kind: "alert",
      target,
      title: COPY_WITH_PASSWORD_TITLE,
      description:
        "The URL carries the connection's password in plain text: anyone who can read the clipboard, or where you paste it, can connect. Copy as URL (without password) leaves it out. You will not be asked again.",
      statements: [],
      confirmLabel: "Copy",
    });
    if (!ok) return false;
    setPref("copyUrlPasswordConfirmed", true);
  }
  const level = effectiveLevel(profile.options.safeMode);
  if (!isPasswordLevel(level) && !getPrefs().safeModeTouchId) return true;
  const secret = await passwordSecret(profile);
  return requestSafetyPrompt({
    kind: "password",
    secret,
    target,
    title: passwordTitle(secret, target.name),
    description: "Copy as URL includes the connection's password.",
    statements: [],
    confirmLabel: "Copy",
  });
}

async function copy(url: string, description: string): Promise<boolean> {
  try {
    await writeClipboard(url);
    toast.success("Copied", { description });
    return true;
  } catch {
    toast.error("Could not copy the URL");
    return false;
  }
}

/**
 * Connection ▸ Copy as URL (and ⌘C on the welcome screen). `withPassword`: the URL carries the
 * password after `confirmRevealPassword`; without one to carry it is copied as is. The toast never
 * shows the password. Resolves true when something was copied.
 */
export async function copyConnectionUrl(
  profile: ConnectionProfile,
  withPassword: boolean,
): Promise<boolean> {
  const plain = profileToUrl(profile);
  if (!withPassword || !(await hasRevealablePassword(profile))) return copy(plain, plain);
  if (!(await confirmRevealPassword(profile))) return false;
  let password: string | null;
  try {
    password = await ipc.security.revealPassword(profile.id);
  } catch (err) {
    toast.error("Could not read the password", { description: toIpcError(err).message });
    return false;
  }
  if (!password) return copy(plain, plain);
  return copy(profileToUrl(profile, password), `${plain} (with the password)`);
}
