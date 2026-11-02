import type { ConnectionProfile } from "@/lib/ipc";
import type { SnowflakeAuth } from "@/lib/ipc/bindings/SnowflakeAuth";
import type { SnowflakeParams } from "@/lib/ipc/bindings/SnowflakeParams";

/**
 * The lane's driver params (`SnowflakeParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own. The form keeps the session
 * parameters as `NAME = value` lines and the statement timeout as typed text. The checks are
 * `SnowflakeParams::from_driver_params`'s (`normalize_account`, `check`, `base_url`), in its words.
 */

type Params = Record<string, unknown> | null | undefined;

/** How the login proves who it is, in the order the form offers them (key pair first: Snowflake
 * is phasing single-factor passwords out). */
export const AUTH_METHODS: readonly (readonly [SnowflakeAuth, string])[] = [
  ["keyPair", "Key pair"],
  ["pat", "Programmatic access token"],
  ["oauth", "OAuth access token"],
  ["password", "Password"],
];

const AUTHS = new Set<string>(AUTH_METHODS.map(([a]) => a));

/** Snowflake's longest statement timeout: 7 days. */
export const MAX_STATEMENT_TIMEOUT_SECS = 604_800;

/**
 * Windows Credential Manager keeps a password as UTF-16 in 2,560 bytes: 1,280 characters
 * (`secrets.rs`). A PAT can be longer.
 */
export const CREDENTIAL_MANAGER_MAX_CHARS = 1280;

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface SnowflakeForm {
  account: string;
  auth: SnowflakeAuth;
  privateKeyPath: string;
  mfa: boolean;
  warehouse: string;
  role: string;
  schema: string;
  baseUrl: string;
  /** One `NAME = value` per line; a quoted value stays text. */
  sessionParameters: string;
  /** Seconds; "" for the account's own limit. */
  statementTimeoutSecs: string;
}

/** Whether the auth uses the profile's secret (password, PAT or token): all but the key pair. */
export const usesSecret = (auth: SnowflakeAuth): boolean => auth !== "keyPair";

/** What the secret is in each sign-in. */
export const SECRET_LABEL: Record<SnowflakeAuth, string> = {
  keyPair: "Password",
  pat: "Access token",
  oauth: "OAuth token",
  password: "Password",
};

const str = (params: Params, key: string): string => {
  const v = params?.[key];
  return typeof v === "string" ? v.trim() : "";
};

/** A session parameter's value as the form writes it: text quoted when it would read otherwise. */
export function formatParameterValue(v: string | number | boolean): string {
  if (typeof v !== "string") return String(v).toUpperCase();
  return valueOf(v) === v && v.trim() === v && v !== "" ? v : `'${v.replaceAll("'", "''")}'`;
}

/**
 * A value as typed: `'text'` is text (`''` an apostrophe), digits a number, TRUE / FALSE a
 * boolean, anything else text as written.
 */
export function valueOf(raw: string): string | number | boolean {
  const t = raw.trim();
  const quoted = /^'((?:[^']|'')*)'$/s.exec(t);
  if (quoted) return quoted[1]!.replaceAll("''", "'");
  if (/^-?\d+(\.\d+)?$/.test(t) && Number.isFinite(Number(t))) return Number(t);
  if (/^(true|false)$/i.test(t)) return t.toLowerCase() === "true";
  return t;
}

/** The session parameters' lines as text. */
export const formatSessionParameters = (params: Record<string, unknown>): string =>
  Object.entries(params)
    .filter(
      (e): e is [string, string | number | boolean] =>
        typeof e[1] === "string" || typeof e[1] === "number" || typeof e[1] === "boolean",
    )
    .map(([k, v]) => `${k} = ${formatParameterValue(v)}`)
    .join("\n");

/** A line that is no `NAME = value`, or whose name Snowflake would not take. */
export interface ParameterLineError {
  line: number;
  text: string;
}

/** `snowflake::parameter_name`: a bare word (letters, digits, `_`), upper-cased. */
export function parameterName(name: string): string | null {
  const upper = name.trim().toUpperCase();
  return /^[A-Z_][A-Z0-9_]*$/.test(upper) ? upper : null;
}

/** The session parameters of the form's text, and the lines that are none. Blank lines skip. */
export function parseSessionParameters(text: string): {
  params: Record<string, string | number | boolean>;
  bad: ParameterLineError[];
} {
  const params: Record<string, string | number | boolean> = {};
  const bad: ParameterLineError[] = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (line === "") return;
    const eq = line.indexOf("=");
    const name = eq < 0 ? null : parameterName(line.slice(0, eq));
    if (eq < 0 || name === null) bad.push({ line: i + 1, text: line });
    else params[name] = valueOf(line.slice(eq + 1));
  });
  return { params, bad };
}

/** The form's values of stored params (or of the form's own, which are already strings). */
export function snowflakeParamsOf(params: Params): SnowflakeForm {
  const auth = params?.auth;
  const session = params?.sessionParameters;
  const timeout = params?.statementTimeoutSecs;
  return {
    account: str(params, "account"),
    auth: typeof auth === "string" && AUTHS.has(auth) ? (auth as SnowflakeAuth) : "keyPair",
    privateKeyPath: str(params, "privateKeyPath"),
    mfa: params?.mfa === true,
    warehouse: str(params, "warehouse"),
    role: str(params, "role"),
    schema: str(params, "schema"),
    baseUrl: str(params, "baseUrl"),
    sessionParameters:
      typeof session === "string"
        ? session
        : typeof session === "object" && session !== null && !Array.isArray(session)
          ? formatSessionParameters(session as Record<string, unknown>)
          : "",
    statementTimeoutSecs:
      typeof timeout === "number" ? String(timeout) : typeof timeout === "string" ? timeout : "",
  };
}

export const snowflakeParams = (p: ConnectionProfile): SnowflakeForm =>
  snowflakeParamsOf(p.options.driverParams);

/**
 * The params a profile keeps: the account and the sign-in always, the rest when set. The key file
 * belongs to the key pair, MFA to the password; the account as `normalize_account` writes it when
 * it reads, else as typed (the backend says why).
 */
export function toSnowflakeParams(f: SnowflakeForm): Partial<SnowflakeParams> {
  const out: Partial<SnowflakeParams> = {
    account: normalizeAccount(f.account) ?? f.account.trim(),
    auth: f.auth,
  };
  if (f.auth === "keyPair" && f.privateKeyPath !== "") out.privateKeyPath = f.privateKeyPath;
  if (f.auth === "password" && f.mfa) out.mfa = true;
  if (f.warehouse !== "") out.warehouse = f.warehouse;
  if (f.role !== "") out.role = f.role;
  if (f.schema !== "") out.schema = f.schema;
  if (f.baseUrl !== "") out.baseUrl = f.baseUrl;
  const { params } = parseSessionParameters(f.sessionParameters);
  if (Object.keys(params).length > 0) out.sessionParameters = params;
  const secs = f.statementTimeoutSecs.trim();
  if (/^\d+$/.test(secs)) out.statementTimeoutSecs = Number(secs);
  return out;
}

const accountChar = /^[A-Za-z0-9_-]+$/;

/**
 * `params::normalize_account`: the account identifier of what a user pastes (a URL, a host, the
 * identifier in any case), lower case, without scheme, domain, port or path; null when it is
 * none (an empty part, a character other than a letter, digit, `_` or `-`, a leading `-`).
 */
export function normalizeAccount(input: string): string | null {
  let s = input.trim();
  for (const scheme of ["https://", "http://"])
    if (s.slice(0, scheme.length).toLowerCase() === scheme) s = s.slice(scheme.length);
  s = s.split(/[/?#]/)[0] ?? "";
  s = s.split(":")[0] ?? "";
  // Rust's `to_ascii_lowercase`: only ASCII letters change.
  let account = s.replace(/[A-Z]/g, (c) => c.toLowerCase());
  for (const domain of [".snowflakecomputing.com", ".snowflakecomputing.cn"])
    if (account.endsWith(domain)) account = account.slice(0, -domain.length);
  const valid =
    account !== "" &&
    account.split(".").every((part) => part !== "" && accountChar.test(part)) &&
    !account.startsWith("-");
  return valid ? account : null;
}

/** Whether a host is this machine (`params::loopback`, after the URL parser wrote it). */
function loopback(host: string): boolean {
  if (host === "localhost" || host === "[::1]") return true;
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/**
 * `params::base_url`: why an endpoint is refused, in the backend's words after its prefix; null
 * when it is fine. `https://host[:port]` only (plain http to this machine only), nothing after
 * the host, no credentials.
 */
export function baseUrlProblem(input: string): string | null {
  let url: URL;
  try {
    url = new URL(input.trim());
  } catch {
    return "not a URL";
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return "use https://";
  if (url.hostname === "") return "no host";
  if (url.protocol === "http:" && !loopback(url.hostname))
    return "use https:// (plain http only reaches a test server on this machine)";
  if (url.username !== "" || url.password !== "") return "credentials do not belong in the URL";
  // `new URL` drops an empty `?` or `#`: the text decides.
  if (
    url.search !== "" ||
    url.hash !== "" ||
    /[?#]/.test(input) ||
    !["", "/"].includes(url.pathname)
  )
    return "give the scheme and host only";
  return null;
}

/**
 * Whether an endpoint is Snowflake's own (`*.snowflakecomputing.com` / `.cn`, PrivateLink
 * included) or this machine: where a link may send a sign-in the user types.
 */
export function snowflakeHost(input: string): boolean {
  if (baseUrlProblem(input) !== null) return false;
  const host = new URL(input.trim()).hostname;
  return /\.snowflakecomputing\.(com|cn)$/.test(host) || loopback(host);
}

/**
 * `params::local_key_path`: a full path on this machine (`C:\…`, `/…`), not a share. Two leading
 * separators in any mix (`\\host`, `/\host`, `\\?\UNC\…`) name a share or a device, since Windows
 * reads `/` as `\`, and opening a share signs in to its host. A `?` is in no Windows file name,
 * only in the object manager's prefixes (`\??\UNC\…`).
 */
export function localFilePath(input: string): boolean {
  const path = input.trim();
  if (/^[\\/]{2}/.test(path) || path.includes("?")) return false;
  return /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("/");
}

/** Why a key pair's endpoint is refused (`params::KEY_PAIR_HOST`). */
export const KEY_PAIR_HOST =
  "a key pair signs in only at Snowflake's own endpoint (*.snowflakecomputing.com) or a test server on this machine (another host could sign in as you with the token)";

/** A problem of the params on the field it names, in the backend's words where it has some. */
export interface ParamIssue {
  key: keyof SnowflakeForm;
  message: string;
}

/** What `SnowflakeParams::from_driver_params` refuses, and what the form checks beyond it. */
export function paramIssues(f: SnowflakeForm): ParamIssue[] {
  const out: ParamIssue[] = [];
  if (f.account === "") out.push({ key: "account", message: "Enter the account identifier" });
  else if (normalizeAccount(f.account) === null)
    out.push({
      key: "account",
      message: `“${f.account}” is no Snowflake account identifier (orgname-account_name or locator.region)`,
    });
  if (f.auth === "keyPair") {
    if (f.privateKeyPath === "")
      out.push({ key: "privateKeyPath", message: "Key pair sign-in needs the private key file" });
    else if (!localFilePath(f.privateKeyPath))
      out.push({
        key: "privateKeyPath",
        message: `The key file “${f.privateKeyPath}” must be a full path on this machine (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share`,
      });
  }
  if (f.baseUrl !== "") {
    const problem = baseUrlProblem(f.baseUrl);
    if (problem)
      out.push({ key: "baseUrl", message: `Snowflake endpoint “${f.baseUrl}”: ${problem}` });
    else if (f.auth === "keyPair" && !snowflakeHost(f.baseUrl))
      out.push({ key: "baseUrl", message: `Snowflake endpoint “${f.baseUrl}”: ${KEY_PAIR_HOST}` });
  }
  const { bad } = parseSessionParameters(f.sessionParameters);
  if (bad.length > 0)
    out.push({
      key: "sessionParameters",
      message: `Line ${bad[0]!.line} is not NAME = value (a name of letters, digits and _)`,
    });
  const secs = f.statementTimeoutSecs.trim();
  if (secs !== "" && (!/^\d+$/.test(secs) || Number(secs) > MAX_STATEMENT_TIMEOUT_SECS))
    out.push({
      key: "statementTimeoutSecs",
      message: `Seconds, up to ${MAX_STATEMENT_TIMEOUT_SECS} (7 days); empty for the account's`,
    });
  return out;
}
