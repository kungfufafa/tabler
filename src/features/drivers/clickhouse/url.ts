import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import {
  type ClickhouseForm,
  clickhouseParams,
  formatSettings,
  parseRoles,
  sessionTimeoutOf,
  settingProblem,
} from "./params";

/**
 * `clickhouse://user:password@host:8123/database?…`: the HTTP interface's host and port, the user
 * (its password goes to the secret, never into a field), the database, TLS as `sslmode` (or
 * `secure=true` / `ssl=true`, which check the certificate as the backend's Prefer does, and
 * `skip_verify=true`, which does not), roles (`role=a&role=b` or `roles=a,b`), the editor's session
 * timeout, a certificate sign-in, and every other key as a setting sent with each request, as
 * clickhouse-go reads its DSN. `clickhouses://` is HTTPS. A setting the backend refuses makes it
 * no URL: dropping it would connect without what the user asked for, and `readonly=0` would read
 * as a request to write.
 */

/** The SSL modes by name; prefer and allow check fully, as `entry::certificate_check` does. */
const SSL_MODES: Record<string, SslMode> = {
  disable: "disable",
  disabled: "disable",
  prefer: "verify-full",
  allow: "verify-full",
  require: "require",
  required: "require",
  "verify-ca": "verify-ca",
  verify_ca: "verify-ca",
  "verify-full": "verify-full",
  verify_full: "verify-full",
  "verify-identity": "verify-full",
};

const modeOf = (name: string): SslMode | null =>
  Object.hasOwn(SSL_MODES, name) ? SSL_MODES[name]! : null;

/** Query keys the app reads for every URL (`connections/url.ts`), never settings. */
const COMMON_KEYS = new Set([
  "name",
  "statuscolor",
  "env",
  "enviroment",
  "safemodelevel",
  "advancedsafemodelevel",
  "tlsmode",
  "useprivatekey",
  "windowmode",
  "driverversion",
  "lazyload",
  "schema",
  "table",
  "view",
  "column",
  "operation",
  "operator",
  "value",
  "condition",
  "raw",
  "query",
]);

/** The lane's own keys, lower case without `_` and `-`. */
const OWN = {
  sslmode: ["sslmode"],
  secure: ["secure", "ssl", "tls", "https"],
  skipVerify: ["skipverify", "insecureskipverify"],
  role: ["role", "roles"],
  sessionTimeout: ["sessiontimeout", "sessiontimeoutsecs"],
  certificateAuth: ["certificateauth", "sslcertificateauth"],
  connectTimeout: ["connecttimeout", "dialtimeout"],
} as const;

const normKey = (key: string) => key.toLowerCase().replace(/[_-]/g, "");
const OWN_KEYS = new Set<string>(Object.values(OWN).flat());

function decode(s: string): string | null {
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

const flag = (v: string | undefined): boolean | null => {
  const t = v?.trim().toLowerCase();
  if (t === undefined) return null;
  if (["1", "true", "yes", "on"].includes(t)) return true;
  if (["0", "false", "no", "off"].includes(t)) return false;
  return null;
};

/** A host that cannot be written back into a URL, or is no host name. */
const badHost = (host: string) => /[\s@/?#]/.test(host);

/**
 * The SSL mode a URL asks for: `sslmode`, else `secure` as a flag (with `skip_verify`), else the
 * scheme's. A value of neither kind is no URL: reading it as off would send the password in the
 * clear to a server the user meant to reach over HTTPS.
 */
function sslOf(https: boolean, get: (keys: readonly string[]) => string | undefined) {
  const mode = get(OWN.sslmode)?.trim().toLowerCase();
  if (mode !== undefined) return modeOf(mode);
  const secure = get(OWN.secure);
  const on = secure === undefined ? https : flag(secure);
  if (on === null) return null;
  if (!on) return "disable";
  const skip = get(OWN.skipVerify);
  const insecure = skip === undefined ? false : flag(skip);
  if (insecure === null) return null;
  return insecure ? "require" : "verify-full";
}

/** A `clickhouse://` or `clickhouses://` URL as form values; null when it is not one. */
export function parseClickhouseUrl(input: string): ParsedConnectionUrl | null {
  const m = /^(clickhouses?):\/\/([^/?#]*)([^?#]*)(?:\?([^#]*))?/i.exec(input.trim());
  if (!m) return null;
  const https = m[1]!.toLowerCase() === "clickhouses";
  const authority = m[2]!;
  const at = authority.lastIndexOf("@");
  const userinfo = at < 0 ? null : authority.slice(0, at);
  const hostport = authority.slice(at + 1);
  const colon = userinfo?.indexOf(":") ?? -1;
  const user = userinfo === null ? "" : decode(colon < 0 ? userinfo : userinfo.slice(0, colon));
  const rawPassword = userinfo === null || colon < 0 ? null : userinfo.slice(colon + 1);
  const password = rawPassword === null ? null : decode(rawPassword);
  const where =
    /^\[([^\]]+)\](?::(\d*))?$/.exec(hostport) ?? /^([^:[\]]*)(?::(\d*))?$/.exec(hostport);
  const host = where ? decode(where[1]!) : null;
  // A malformed %-escape anywhere makes it no URL rather than a half-read one.
  if (
    user === null ||
    host === null ||
    host === "" ||
    badHost(host) ||
    (rawPassword !== null && password === null)
  )
    return null;
  const database = decode(m[3]!.replace(/^\/+|\/+$/g, ""));
  if (database === null || database.includes("/")) return null;
  const query = new Map<string, string[]>();
  const settings: { name: string; value: string }[] = [];
  for (const [k, v] of new URLSearchParams(m[4] ?? "")) {
    const key = normKey(k);
    if (OWN_KEYS.has(key)) query.set(key, [...(query.get(key) ?? []), v]);
    else if (!COMMON_KEYS.has(k.toLowerCase())) {
      if (settingProblem(k) !== null) return null;
      settings.push({ name: k.trim(), value: v });
    }
  }
  const get = (keys: readonly string[]) => {
    for (const k of keys) {
      const v = query.get(k);
      if (v !== undefined) return v.at(-1);
    }
    return undefined;
  };
  const ssl = sslOf(https, get);
  if (ssl === null) return null;
  const roles = OWN.role.flatMap((k) => query.get(k) ?? []).flatMap(parseRoles);
  const timeout = get(OWN.sessionTimeout)?.trim() ?? "";
  const params: ClickhouseForm = {
    certificateAuth: flag(get(OWN.certificateAuth)) === true,
    roles: roles.join(", "),
    settings: formatSettings(settings),
    sessionTimeoutSecs: sessionTimeoutOf(timeout) === null ? "" : timeout,
  };
  const values: Partial<ConnectionFormValues> = {
    host,
    port: where?.[2] ?? "",
    user,
    database,
    ssl,
    driverParams: { ...params },
  };
  const connect = get(OWN.connectTimeout)?.trim();
  if (connect !== undefined && /^\d+$/.test(connect) && Number(connect) > 0)
    values.connectTimeoutSecs = String(Number(connect));
  return { values, password: password === "" ? null : password };
}

/**
 * The `clickhouse://` URL of a profile: `user@host:port/database` with the SSL mode, the roles,
 * the session timeout, a certificate sign-in and the settings; never a password. A setting whose
 * name the app reads for every URL (`name`, `table`…) cannot come back, so such a profile has no
 * URL (the backend refuses those names anyway, except a few harmless ones).
 */
export function formatClickhouseUrl(p: ConnectionProfile): string | null {
  if (!p.host || badHost(p.host)) return null;
  const f = clickhouseParams(p);
  const query: [string, string][] = [];
  if (p.ssl !== "disable") query.push(["sslmode", p.ssl === "prefer" ? "verify-full" : p.ssl]);
  if (f.certificateAuth) query.push(["certificateAuth", "true"]);
  for (const role of parseRoles(f.roles)) query.push(["role", role]);
  if (f.sessionTimeoutSecs) query.push(["sessionTimeout", f.sessionTimeoutSecs]);
  if (p.options.connectTimeoutSecs)
    query.push(["connectTimeout", String(p.options.connectTimeoutSecs)]);
  for (const line of f.settings.split(/\r?\n/)) {
    const eq = line.indexOf("=");
    const name = eq < 0 ? "" : line.slice(0, eq).trim();
    if (name === "") continue;
    const key = name.toLowerCase();
    if (COMMON_KEYS.has(key) || OWN_KEYS.has(normKey(name))) return null;
    query.push([name, line.slice(eq + 1).trim()]);
  }
  const host = p.host.includes(":") ? `[${p.host}]` : p.host;
  const user = p.user ? `${encodeURIComponent(p.user)}@` : "";
  const port = p.port ? `:${p.port}` : "";
  const database = p.database ? `/${encodeURIComponent(p.database)}` : "";
  const q = new URLSearchParams(query).toString();
  return `clickhouse://${user}${host}${port}${database}${q ? `?${q}` : ""}`;
}
