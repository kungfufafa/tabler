import type { ConnectionFormValues } from "@/features/connections/schema";
import { type ParsedConnectionUrl, profileUrlQuery } from "@/features/connections/url";
import type { UrlSource } from "@/features/drivers/types";
import type { ConnectionProfile } from "@/lib/ipc";
import type { SnowflakeAuth } from "@/lib/ipc/bindings/SnowflakeAuth";
import {
  baseUrlProblem,
  formatParameterValue,
  localFilePath,
  normalizeAccount,
  parameterName,
  type SnowflakeForm,
  snowflakeHost,
  snowflakeParams,
  toSnowflakeParams,
  valueOf,
} from "./params";

/**
 * `snowflake://` URLs, as the real-server suite writes them (`tests/snowflake.rs`):
 * `snowflake://USER[:SECRET]@ACCOUNT/DATABASE/SCHEMA?warehouse=W&role=R&auth=…`, with
 * `auth=keyPair&privateKeyPath=<file>[&passphrase=<key passphrase>]` (the default when a key file
 * is given), `auth=pat` (SECRET is the token), `auth=oauth` or `auth=password` (`mfa=true` when
 * the account asks a push). Also `baseUrl`, `statementTimeout` (seconds), `connectTimeout`, and
 * session parameters as upper-case keys (`QUERY_TAG=etl`); JDBC's
 * `jdbc:snowflake://<account>.snowflakecomputing.com/?user=…&db=…&schema=…&warehouse=…&role=…`
 * with its `authenticator` and `private_key_file` reads too. The account may be written as a
 * host or a URL (`normalize_account`). A secret or passphrase comes out of the URL into the
 * secrets and is never written back.
 *
 * A clicked link (`url.parse` with source "link", or none) connects unasked, so it never picks
 * the user's own key: it would sign a JWT for the account and user the link names, with a key
 * file the link chose, and send it where the link says. Such a URL reads as a programmatic
 * access token sign-in instead (the link's own token, or the connect asks for one). Its endpoint
 * must be Snowflake's own (or this machine) unless it carries its own secret: a token the user
 * types would otherwise go to the link's host. A URL the user pastes opens in the form first,
 * where the user sees the sign-in and endpoint, so it keeps them and round-trips a profile. A
 * key file must be a full local path: opening `\\host\share\k.p8` would hand that host the user's
 * NTLM hash. A link's session parameters are dropped too: they would change, unseen, how the
 * session reads what the grid writes (`TIMEZONE` for a time stamp typed without an offset) or
 * what it shows.
 */

/** The `auth` / JDBC `authenticator` words (lower case, without `_` and `-`). */
const AUTH_NAMES = new Map<string, { auth: SnowflakeAuth; mfa?: boolean }>([
  ["keypair", { auth: "keyPair" }],
  ["snowflakejwt", { auth: "keyPair" }],
  ["jwt", { auth: "keyPair" }],
  ["pat", { auth: "pat" }],
  ["programmaticaccesstoken", { auth: "pat" }],
  ["oauth", { auth: "oauth" }],
  ["password", { auth: "password" }],
  ["snowflake", { auth: "password" }],
  ["usernamepasswordmfa", { auth: "password", mfa: true }],
]);

/** The query keys Tabler reads itself (any case); every other upper-case key is a parameter. */
const OWN_KEYS = new Set([
  "auth",
  "authenticator",
  "privatekeypath",
  "private_key_file",
  "private_key_path",
  "passphrase",
  "private_key_file_pwd",
  "mfa",
  "warehouse",
  "role",
  "db",
  "database",
  "schema",
  "user",
  "token",
  "baseurl",
  "statementtimeout",
  "statementtimeoutsecs",
  "connecttimeout",
  "logintimeout",
]);

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** `true` / `yes` / `on` / `1`, `false` / `no` / `off` / `0`; null for anything else. */
function bool(v: string | undefined): boolean | null {
  const t = v?.trim().toLowerCase();
  if (t === undefined || t === "") return null;
  if (["true", "yes", "on", "1"].includes(t)) return true;
  if (["false", "no", "off", "0"].includes(t)) return false;
  return null;
}

/** Whether a URL may pick the user's own key (see the module's comment). */
type Ambient = "keep" | "drop";

function readUrl(input: string, ambient: Ambient): ParsedConnectionUrl | null {
  const m = /^snowflake:\/\/(?:([^@/?#]*)@)?([^/?#]*)(?:\/([^?#]*))?(?:\?([^#]*))?(?:#.*)?$/is.exec(
    input.trim().replace(/^jdbc:/i, ""),
  );
  if (!m) return null;
  const [, userinfo, host = "", path = "", query = ""] = m;
  const account = normalizeAccount(decode(host));
  if (account === null) return null;
  // Keys in lower case for Tabler's own, the last one winning; session parameters as written.
  const own = new Map<string, string>();
  const parameters: Record<string, string | number | boolean> = {};
  for (const [k, v] of new URLSearchParams(query)) {
    const lower = k.toLowerCase();
    if (OWN_KEYS.has(lower)) own.set(lower, v.trim());
    else if (/^[A-Z_][A-Z0-9_]*$/.test(k)) parameters[parameterName(k)!] = valueOf(v);
  }
  const get = (...keys: string[]): string => {
    for (const k of keys) {
      const v = own.get(k);
      if (v !== undefined && v !== "") return v;
    }
    return "";
  };

  const colon = userinfo?.indexOf(":") ?? -1;
  const user =
    decode(userinfo === undefined ? "" : colon < 0 ? userinfo : userinfo.slice(0, colon)).trim() ||
    get("user");
  const secret =
    (userinfo !== undefined && colon >= 0 ? decode(userinfo.slice(colon + 1)) : "") ||
    get("token") ||
    null;

  const keyFile = get("privatekeypath", "private_key_file", "private_key_path");
  // Opening a share would sign in to its host (NTLM on Windows) before any check runs.
  if (keyFile !== "" && !localFilePath(keyFile)) return null;
  let auth: SnowflakeAuth;
  let mfa = false;
  const named = get("auth", "authenticator");
  if (named !== "") {
    const a = AUTH_NAMES.get(named.toLowerCase().replace(/[-_]/g, ""));
    if (!a) return null;
    auth = a.auth;
    mfa = a.mfa ?? false;
  } else auth = keyFile !== "" ? "keyPair" : secret ? "pat" : "keyPair";
  const mfaParam = bool(own.get("mfa"));
  if (own.has("mfa") && mfaParam === null) return null;
  if (mfaParam !== null) mfa = mfaParam;
  // A link signs in with what it brings: the user's key stays the user's choice in the form.
  const keep = ambient === "keep";
  if (!keep && auth === "keyPair") auth = "pat";

  const baseUrl = get("baseurl");
  if (baseUrl !== "" && baseUrlProblem(baseUrl) !== null) return null;
  // A secret the URL brings may go where it says; one the user types only to Snowflake or here.
  if (!keep && baseUrl !== "" && !snowflakeHost(baseUrl) && !secret) return null;
  // The backend's `check`: the key's JWT never goes to another host.
  if (auth === "keyPair" && baseUrl !== "" && !snowflakeHost(baseUrl)) return null;

  const timeout = get("statementtimeout", "statementtimeoutsecs");
  if (timeout !== "" && !/^\d+$/.test(timeout)) return null;
  const [database = "", schema = ""] = path.replace(/\/+$/, "").split("/").map(decode);

  const form: SnowflakeForm = {
    account,
    auth,
    privateKeyPath: auth === "keyPair" ? keyFile : "",
    mfa: auth === "password" && mfa,
    warehouse: get("warehouse"),
    role: get("role"),
    schema: schema.trim() || get("schema"),
    baseUrl,
    // A link's are dropped (see the module's comment).
    sessionParameters: Object.entries(keep ? parameters : {})
      .map(([k, v]) => `${k} = ${formatParameterValue(v)}`)
      .join("\n"),
    statementTimeoutSecs: timeout,
  };
  const values: Partial<ConnectionFormValues> = {
    host: "",
    port: "",
    user,
    database: database.trim() || get("db", "database"),
    driverParams: { ...form },
    name: account,
  };
  const passphrase = get("passphrase", "private_key_file_pwd");
  if (keep && auth === "keyPair" && passphrase !== "") values.keyPassphrase = passphrase;
  const connect = get("connecttimeout", "logintimeout");
  if (/^\d+$/.test(connect) && Number(connect) > 0)
    values.connectTimeoutSecs = String(Number(connect));
  return { values, password: auth === "keyPair" ? null : secret };
}

/**
 * A `snowflake://` URL as form values, for `url.parse`: the user's key only from a paste, which
 * the form shows first; a link (or no source) reads as a token sign-in.
 */
export const parseSnowflakeUrl = (input: string, source?: UrlSource): ParsedConnectionUrl | null =>
  readUrl(input, source === "paste" ? "keep" : "drop");

/**
 * The `snowflake://` URL of a profile: user, account, database and schema, the sign-in and the
 * settings, then the query every connection URL carries (`profileUrlQuery`); never a secret.
 * Pasted, it reads back as the same profile.
 */
export function formatSnowflakeUrl(p: ConnectionProfile): string | null {
  const f = snowflakeParams(p);
  const params = toSnowflakeParams(f);
  const account = normalizeAccount(f.account);
  if (account === null) return null;
  const q = new URLSearchParams();
  q.set("auth", f.auth);
  if (params.privateKeyPath) q.set("privateKeyPath", params.privateKeyPath);
  if (params.mfa) q.set("mfa", "true");
  if (params.warehouse) q.set("warehouse", params.warehouse);
  if (params.role) q.set("role", params.role);
  if (params.baseUrl) q.set("baseUrl", params.baseUrl);
  if (params.statementTimeoutSecs !== undefined && params.statementTimeoutSecs !== null)
    q.set("statementTimeout", String(params.statementTimeoutSecs));
  if (p.options.connectTimeoutSecs) q.set("connectTimeout", String(p.options.connectTimeoutSecs));
  for (const [k, v] of Object.entries(params.sessionParameters ?? {}))
    q.set(k, formatParameterValue(v));
  for (const [k, v] of profileUrlQuery(p)) q.append(k, v);
  const user = p.user ? `${encodeURIComponent(p.user)}@` : "";
  const db = encodeURIComponent(p.database ?? "");
  const schema = params.schema ? `/${encodeURIComponent(params.schema)}` : "";
  const path = db !== "" || schema !== "" ? `/${db}${schema}` : "";
  return `snowflake://${user}${account}${path}?${q.toString()}`;
}
