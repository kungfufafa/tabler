import { formToProfileInput, type ConnectionFormValues } from "@/features/connections/schema";
import type { ProfileInput } from "@/lib/ipc";
import type { KindFieldsProps } from "../types";
import { snowflakeParamsOf, toSnowflakeParams, usesSecret } from "./params";

/**
 * What the form's calls send (`keyFingerprint`, `listWarehouses`, `listRoles`, `listDatabases`:
 * all reads, `entry::util_kind`), and how the secret's storage follows the sign-in.
 */

/** The form's calls (`entry.rs`). */
export type FormCall = "keyFingerprint" | "listWarehouses" | "listRoles" | "listDatabases";

/** No saved profile's id reaches the form: a stored secret is not used for the lists. */
const UNSAVED = "00000000-0000-0000-0000-000000000000";

/**
 * The profile and secrets the form would connect with: the key file and its typed passphrase,
 * or the password, PAT or token typed (whatever its storage) or printed by the command.
 */
export function utilInput(v: ConnectionFormValues): ProfileInput {
  const input = formToProfileInput(v, null, UNSAVED);
  const f = snowflakeParamsOf(v.driverParams);
  const secret = usesSecret(f.auth);
  input.profile = {
    ...input.profile,
    host: null,
    port: null,
    ssh: null,
    database: v.database.trim() || null,
    options: {
      ...input.profile.options,
      passwordMode: secret ? v.passwordMode : "keychain",
      passwordCommand: secret && v.passwordMode === "command" ? v.passwordCommand.trim() : null,
      driverParams: { ...toSnowflakeParams(f) },
    },
  };
  input.password = secret && v.password !== "" ? v.password : null;
  input.keyPassphrase = !secret && v.keyPassphrase !== "" ? v.keyPassphrase : null;
  input.sessionToken = null;
  return input;
}

/**
 * The secret's storage after the sign-in became `auth`: a key pair keeps the Keychain mode (its
 * passphrase) and no password; an OAuth token usually comes from a command; a PAT or password is
 * typed.
 */
export function followAuth(form: KindFieldsProps["form"], auth: string): void {
  const mode = form.getValues("passwordMode");
  const opts = { shouldDirty: true };
  if (auth === "keyPair") {
    form.setValue("password", "", opts);
    if (mode !== "keychain") form.setValue("passwordMode", "keychain", opts);
  } else {
    form.setValue("keyPassphrase", "", opts);
    if (auth === "oauth" && mode !== "command") form.setValue("passwordMode", "command", opts);
    else if (auth !== "oauth" && mode === "none") form.setValue("passwordMode", "keychain", opts);
  }
}

export type Listing =
  | { state: "idle" | "loading" }
  | { state: "done"; items: string[] }
  | { state: "failed"; message: string };

/** What the lists need before they can sign in; null when they can. */
export function missingForList(v: ConnectionFormValues, hasStoredPassword: boolean): string | null {
  const f = snowflakeParamsOf(v.driverParams);
  if (f.account === "") return "Enter the account first";
  if (v.user.trim() === "") return "Enter the user first";
  if (f.auth === "keyPair" && f.privateKeyPath === "") return "Choose the private key file first";
  if (usesSecret(f.auth) && v.passwordMode !== "command" && v.password === "")
    return hasStoredPassword
      ? "Type the secret again to list (the stored one is used to connect)"
      : "Enter the secret first";
  return null;
}
