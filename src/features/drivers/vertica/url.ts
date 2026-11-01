import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import { type VerticaForm, verticaParams } from "./params";

/**
 * Vertica connection URLs: `vertica://user@host:5433/database?…` (also JDBC's
 * `jdbc:vertica://host:5433/database?user=…&TLSmode=require`, whose keys are case-insensitive).
 * A password or OAuth token in one goes to the secret, never into a field.
 *
 * Keys read: the TLS mode as `tlsmode` (vertica-python, JDBC's `TLSmode`), `sslmode`,
 * TablePlus's numeric `tLSMode` or `ssl=true|false`; the client label as `session_label`
 * (vertica-python), `label` (JDBC) or `client_label`; `workload`; `autocommit=false` (Manual
 * commit); `oauth_access_token` / JDBC's `oauthaccesstoken` (an OAuth login whose token is the
 * password) or `auth=oauth`; the connect timeout as `connection_timeout`, `connect_timeout` or
 * JDBC's `LoginTimeout`; `user` and `password` (JDBC puts them in the query). Writing uses
 * vertica-python's keys, so the URL also works with `vertica+vertica_python://` in SQLAlchemy.
 */

// A Map, so a query word such as `constructor` or `__proto__` is no mode.
const TLS_WORDS = new Map<string, SslMode>([
  ["disable", "disable"],
  ["disabled", "disable"],
  ["allow", "prefer"],
  ["prefer", "prefer"],
  ["preferred", "prefer"],
  ["require", "require"],
  ["required", "require"],
  ["verify-ca", "verify-ca"],
  ["verify_ca", "verify-ca"],
  ["verify-full", "verify-full"],
  ["verify_full", "verify-full"],
]);

/** TablePlus's `tLSMode` index. */
const TLS_INDEX: readonly SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

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
  if (t === undefined) return null;
  if (["true", "yes", "on", "1"].includes(t)) return true;
  if (["false", "no", "off", "0"].includes(t)) return false;
  return null;
}

/** The query's keys in lower case, the last one winning. */
function queryOf(search: URLSearchParams): Map<string, string> {
  const out = new Map<string, string>();
  for (const [k, v] of search) out.set(k.toLowerCase(), v);
  return out;
}

const first = (q: Map<string, string>, ...keys: string[]): string | undefined => {
  for (const k of keys) {
    const v = q.get(k);
    if (v !== undefined) return v;
  }
  return undefined;
};

/**
 * The URL's TLS mode: null when it names none, "bad" when it names one this does not know. A
 * mode of neither kind is no URL: keeping the form's default could be weaker than it asked for.
 */
function tlsOf(q: Map<string, string>): SslMode | "bad" | null {
  const word = first(q, "tlsmode", "sslmode", "ssl_mode")?.trim().toLowerCase();
  if (word !== undefined) {
    // TablePlus's `tLSMode` is an index, which lower-cased reads as `tlsmode` too.
    const mode = /^\d$/.test(word) ? TLS_INDEX[Number(word)] : TLS_WORDS.get(word);
    return mode ?? "bad";
  }
  const raw = q.get("ssl");
  if (raw === undefined) return null;
  const ssl = bool(raw);
  return ssl === null ? "bad" : ssl ? "require" : "disable";
}

/** A host that would not read back as itself: a URL delimiter or a space in it. */
const badHost = (host: string) => /[\s@/?#]/.test(host);

/** A `vertica://` URL (or JDBC string) as form values; null when it is not one. */
export function parseVerticaUrl(input: string): ParsedConnectionUrl | null {
  const text = input.trim().replace(/^jdbc:/i, "");
  if (!/^vertica:\/\//i.test(text)) return null;
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  const host = decode(url.hostname.replace(/^\[(.*)\]$/, "$1"));
  if (host === "" || badHost(host)) return null;
  const q = queryOf(url.searchParams);
  const tls = tlsOf(q);
  if (tls === "bad") return null;
  const user = decode(url.username) || first(q, "user", "username")?.trim() || "";
  const token = first(q, "oauth_access_token", "oauthaccesstoken");
  const password =
    (url.password ? decode(url.password) : null) ?? token ?? q.get("password") ?? null;
  const oauth = token !== undefined || first(q, "auth")?.trim().toLowerCase() === "oauth";
  const autocommit = bool(q.get("autocommit"));
  const params: VerticaForm = {
    auth: oauth ? "oauth" : "password",
    clientLabel: first(q, "session_label", "label", "client_label", "clientlabel")?.trim() ?? "",
    workload: q.get("workload")?.trim() ?? "",
    manualCommit: autocommit === false,
  };
  // An OAuth token travels over TLS only (`entry::validate`): without a mode, ask for it.
  const ssl = tls ?? (oauth ? "require" : null);
  const values: Partial<ConnectionFormValues> = {
    host,
    port: url.port || "5433",
    user,
    database: decode(url.pathname.replace(/^\/+|\/+$/g, "")),
    driverParams: { ...params },
  };
  if (ssl) values.ssl = ssl;
  const timeout = first(q, "connection_timeout", "connect_timeout", "logintimeout")?.trim();
  if (timeout !== undefined && /^\d+$/.test(timeout) && Number(timeout) > 0)
    values.connectTimeoutSecs = String(Number(timeout));
  return { values, password: password === "" ? null : password };
}

/** IPv6 addresses go in brackets. */
const hostForUrl = (host: string) => (host.includes(":") ? `[${host}]` : host);

/**
 * The `vertica://` URL of a profile: `user@host:port/database` with the TLS mode and the params
 * that differ from the defaults; never a password or token.
 */
export function formatVerticaUrl(p: ConnectionProfile): string | null {
  // A host holding a delimiter would read back as another server or drop the query.
  if (!p.host || badHost(p.host)) return null;
  const f = verticaParams(p);
  const query: [string, string][] = [["tlsmode", p.ssl]];
  if (f.auth === "oauth") query.push(["auth", "oauth"]);
  if (f.clientLabel) query.push(["session_label", f.clientLabel]);
  if (f.workload) query.push(["workload", f.workload]);
  if (f.manualCommit) query.push(["autocommit", "false"]);
  if (p.options.connectTimeoutSecs)
    query.push(["connection_timeout", String(p.options.connectTimeoutSecs)]);
  const user = p.user ? `${encodeURIComponent(p.user)}@` : "";
  const port = p.port ? `:${p.port}` : "";
  const db = p.database ? `/${encodeURIComponent(p.database)}` : "";
  return `vertica://${user}${hostForUrl(p.host)}${port}${db}?${new URLSearchParams(query).toString()}`;
}
