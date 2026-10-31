import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile } from "@/lib/ipc";
import { d1Params, libsqlParams } from "./params";

/**
 * The URLs of LibSQL and Cloudflare D1 profiles, as the backend reads them
 * (`sqlite_http/libsql/url.rs`, `d1/entry.rs`). A token or a password in a pasted URL comes out
 * of it into the secret, never into a field or a link that is written back.
 */

/** A pasted LibSQL URL, read as `libsql/url.rs::parse_url` reads it. */
export interface LibsqlUrl {
  host: string;
  /** Null: the scheme's own (443 for HTTPS, 80 for HTTP). */
  port: number | null;
  tls: boolean;
  /** The path below the host, no trailing slash ("" for none). */
  path: string;
  /** `?authToken=`. */
  token: string | null;
  /** `user:password@` (a `sqld` behind basic auth), also with only one of the two. */
  basic: { user: string; password: string } | null;
  /**
   * A token key (any casing, even empty or repeated) or a user or password was in the URL: a
   * stored URL that says so is refused, since the params are kept as plain text.
   */
  hasSecret: boolean;
}

const LIBSQL_SCHEMES: Record<string, boolean> = {
  libsql: true,
  https: true,
  wss: true,
  http: false,
  ws: false,
};

/** Compared in lower case: `?AuthToken=` is the token too, never a parameter sent on. */
const TOKEN_KEYS = new Set(["authtoken", "auth_token"]);

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** IPv6 addresses go in brackets. */
const hostForUrl = (host: string) => (host.includes(":") ? `[${host}]` : host);

/**
 * Reads a LibSQL URL (`libsql://`, `https://`, `http://`, `wss://`, `ws://`, or none, which is
 * HTTPS like Turso's own clients); null when it is not one. `?tls=0` makes a `libsql://` URL plain
 * HTTP.
 */
export function parseLibsqlUrl(input: string): LibsqlUrl | null {
  const text = input.trim();
  if (text === "") return null;
  const at = text.indexOf("://");
  const scheme = at < 0 ? "libsql" : text.slice(0, at).toLowerCase();
  const rest = at < 0 ? text : text.slice(at + 3);
  let tls = LIBSQL_SCHEMES[scheme];
  if (tls === undefined) return null;
  let probe: URL;
  try {
    probe = new URL(`https://${rest}`);
  } catch {
    return null;
  }
  let token: string | null = null;
  let hasSecret = false;
  for (const [key, value] of probe.searchParams) {
    if (TOKEN_KEYS.has(key.toLowerCase())) {
      hasSecret = true;
      // A later empty one does not erase the token.
      if (value !== "") token = value;
    }
    // Only `libsql://` reads `tls` (any casing, like the backend); the scheme says it for others.
    else if (key.toLowerCase() === "tls" && scheme === "libsql")
      tls = !["0", "false"].includes(value);
  }
  // Parsed again with the real scheme, so its default port is the one dropped.
  let url: URL;
  try {
    url = new URL(`${tls ? "https" : "http"}://${rest}`);
  } catch {
    return null;
  }
  const host = url.hostname.replace(/^\[(.*)\]$/, "$1");
  if (host === "") return null;
  const basic =
    url.username === "" && url.password === ""
      ? null
      : { user: decode(url.username), password: decode(url.password) };
  return {
    host,
    port: url.port === "" ? null : Number(url.port),
    tls,
    path: url.pathname.replace(/\/+$/, ""),
    token,
    basic,
    hasSecret: hasSecret || basic !== null,
  };
}

/** `http(s)://host[:port][/path]` of a parsed URL: what the profile's `url` param keeps. */
export function libsqlBase(u: LibsqlUrl): string {
  const port = u.port === null ? "" : `:${u.port}`;
  return `${u.tls ? "https" : "http"}://${hostForUrl(u.host)}${port}${u.path}`;
}

/**
 * The form values of a `libsql://` URL: host and port (a server under a path also as the `url`
 * param, which the backend connects to in their place), HTTPS checking the certificate unless
 * `?tls=0`, Basic credentials as user and password, and `?authToken=` as the secret.
 */
export function parseLibsqlConnectionUrl(input: string): ParsedConnectionUrl | null {
  const u = parseLibsqlUrl(input);
  if (!u) return null;
  // A URL without a port means its scheme's, which the form's empty port does not (8080 when
  // plain): written out unless it is the HTTPS port the form defaults to.
  const port = u.port ?? (u.tls ? null : 80);
  return {
    values: {
      host: u.host,
      port: port === null ? "" : String(port),
      ssl: u.tls ? "verify-full" : "disable",
      user: u.basic?.user ?? "",
      driverParams: { url: u.path === "" ? "" : libsqlBase(u), basicAuth: u.basic !== null },
    },
    password: u.basic ? u.basic.password || null : u.token,
  };
}

/** The `libsql://` URL of a profile: its host or `url` param, the user of Basic auth, no secret. */
export function formatLibsqlUrl(p: ConnectionProfile): string | null {
  const params = libsqlParams(p);
  const user = params.basicAuth && p.user ? `${encodeURIComponent(p.user)}@` : "";
  let host: string;
  let port: number | null;
  let tls: boolean;
  let path = "";
  if (params.url) {
    const u = parseLibsqlUrl(params.url);
    if (!u) return null;
    ({ host, port, tls, path } = u);
    port ??= tls ? null : 80;
  } else {
    if (!p.host) return null;
    host = p.host;
    tls = p.ssl !== "disable";
    // The profile's empty port is the descriptor's: 443 with HTTPS, 8080 without.
    port = p.port ?? (tls ? null : 8080);
  }
  if (tls && port === 443) port = null;
  const authority = `${user}${hostForUrl(host)}${port === null ? "" : `:${port}`}`;
  return `libsql://${authority}${path}${tls ? "" : "?tls=0"}`;
}

/**
 * The form values of `d1://<account>/<database>` (TablePlus also writes `cloudflared1://`): the
 * account ID as the driver param, the database by name or UUID, `?apiBase=` for another API
 * base, and a token given as the URL's password or `?apiToken=` as the secret.
 */
export function parseD1ConnectionUrl(input: string): ParsedConnectionUrl | null {
  const m = /^(?:d1|cloudflared1):\/\/(?:([^@/?#]*)@)?([^/?#]+)\/?([^?#]*)(?:\?([^#]*))?/i.exec(
    input.trim(),
  );
  if (!m) return null;
  const [, userinfo, account = "", path = "", query = ""] = m;
  const accountId = decode(account).trim();
  if (accountId === "") return null;
  const params = new URLSearchParams(query);
  const colon = userinfo?.indexOf(":") ?? -1;
  const fromUserinfo = userinfo
    ? decode(colon < 0 ? userinfo : userinfo.slice(colon + 1)) || null
    : null;
  const database = decode(path.replace(/\/+$/, ""));
  return {
    values: {
      host: "",
      port: "",
      user: "",
      database,
      driverParams: { accountId, apiBase: params.get("apiBase")?.trim() ?? "" },
      name: database || "Cloudflare D1",
    },
    password: params.get("apiToken") || fromUserinfo,
  };
}

/**
 * The API base as a link may carry it: a profile stored before the base was checked keeps only
 * its scheme, host, port and path, since a user, a password or a query could be a credential.
 */
function apiBaseForLink(base: string): string {
  let url: URL;
  try {
    url = new URL(base);
  } catch {
    return /[@?#]/.test(base) ? "" : base;
  }
  if (url.username === "" && url.password === "" && !/[?#]/.test(base)) return base;
  return `${url.protocol}//${url.host}${url.pathname}`.replace(/\/+$/, "");
}

/** The `d1://` URL of a profile (no token, no credential of the API base). */
export function formatD1Url(p: ConnectionProfile): string | null {
  const { accountId, apiBase: stored } = d1Params(p);
  if (accountId === "") return null;
  const apiBase = apiBaseForLink(stored);
  const query = apiBase ? `?apiBase=${encodeURIComponent(apiBase)}` : "";
  return `d1://${encodeURIComponent(accountId)}/${encodeURIComponent(p.database ?? "")}${query}`;
}
