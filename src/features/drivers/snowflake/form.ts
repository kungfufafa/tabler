import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import { isWindows } from "@/lib/platform";
import type { FormIssue, KindUi } from "../types";
import {
  CREDENTIAL_MANAGER_MAX_CHARS,
  paramIssues,
  SECRET_LABEL,
  snowflakeParams,
  snowflakeParamsOf,
  toSnowflakeParams,
  usesSecret,
} from "./params";
import { SnowflakeFields } from "./SnowflakeFields";
import { formatSnowflakeUrl, parseSnowflakeUrl } from "./url";

/**
 * Snowflake's form: the account identifier (pasted in any form: a URL, a host, the identifier),
 * the user, how the login proves who it is (a key pair: the private key file with its passphrase
 * as the second secret; a programmatic access token; an OAuth token, usually from Command Line;
 * the password, with an MFA push when the account asks one), then the warehouse, role, database
 * and schema the session starts in, an endpoint of its own (PrivateLink, a proxy), session
 * parameters and the statement timeout. There is no host, port, SSH or SSL row: Snowflake is
 * HTTPS to the account. Every row is the lane's (`Fields`), since which secret shows depends on
 * the sign-in. The checks are `SnowflakeParams::from_driver_params`'s, so a profile the form saves
 * is one the backend opens.
 */

/** Whether a PAT kept in the Keychain would pass Windows Credential Manager's 1,280 characters. */
export function patTooLong(v: ConnectionFormValues, windows = isWindows): boolean {
  const { auth } = snowflakeParamsOf(v.driverParams);
  return (
    windows &&
    auth === "pat" &&
    v.passwordMode === "keychain" &&
    [...v.password].length > CREDENTIAL_MANAGER_MAX_CHARS
  );
}

function validate(v: ConnectionFormValues): FormIssue[] {
  // "User is required" and the account's presence are the schema's (the descriptor's required
  // field and param); the lane says what its values must look like.
  const f = snowflakeParamsOf(v.driverParams);
  const out: FormIssue[] = paramIssues(f)
    .filter((i) => !(i.key === "account" && f.account === ""))
    .map((i) => ({ path: `driverParams.${i.key}`, message: i.message }));
  if (usesSecret(f.auth) && v.passwordMode === "none")
    out.push({
      path: "passwordMode",
      message: `The ${SECRET_LABEL[f.auth].toLowerCase()} is the sign-in: store it, ask for it or run a command`,
    });
  if (patTooLong(v))
    out.push({
      path: "password",
      message: `Windows Credential Manager keeps up to ${CREDENTIAL_MANAGER_MAX_CHARS.toLocaleString("en-US")} characters: choose Command Line or Ask every time for this token`,
    });
  return out;
}

/**
 * The profile to save: no host, port, SSH or TLS files; the secret's storage only for a sign-in
 * that reads it. A key pair keeps the Keychain mode, which holds the key's passphrase (the second
 * secret), and never a password or command.
 */
function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const f = snowflakeParamsOf(v.driverParams);
  const secret = usesSecret(f.auth);
  return {
    ...base,
    host: null,
    port: null,
    ssh: null,
    ssl: "verify-full",
    sslCaPath: null,
    database: base.database?.trim() || null,
    options: {
      ...base.options,
      passwordMode: secret ? base.options.passwordMode : "keychain",
      passwordCommand: secret ? base.options.passwordCommand : null,
      sslKeyPath: null,
      sslCertPath: null,
      driverParams: { ...toSnowflakeParams(f) },
    },
  };
}

const AUTH_SHORT: Record<string, string> = {
  keyPair: "key pair",
  pat: "PAT",
  oauth: "OAuth",
  password: "password",
};

/** `user@account · warehouse · database.schema · sign-in`. */
function summary(p: ConnectionProfile): string {
  const f = snowflakeParams(p);
  const who = `${p.user ? `${p.user}@` : ""}${f.account}`;
  const where = [p.database ?? "", f.schema].filter((s) => s !== "").join(".");
  const auth = `${AUTH_SHORT[f.auth] ?? f.auth}${f.auth === "password" && f.mfa ? " + MFA" : ""}`;
  return [who, f.warehouse, where, auth].filter((s) => s !== "").join(" · ");
}

export const snowflakeUi: KindUi = {
  // Every row is the lane's: the secret or the key file shows by the sign-in.
  rows: [],
  sslModes: null,
  defaults: () => ({
    host: "",
    port: "",
    ssl: "verify-full",
    passwordMode: "keychain",
    driverParams: { ...snowflakeParamsOf(null) },
  }),
  Fields: SnowflakeFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...snowflakeParams(p) } }),
  url: { schemes: ["snowflake"], parse: parseSnowflakeUrl, format: formatSnowflakeUrl },
  summary,
};
