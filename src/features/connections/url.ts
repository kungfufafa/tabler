import type { CoreDriver } from "@/features/drivers/catalog";
import type { ConnectionProfile, SafeModeLevel, SslMode } from "@/lib/ipc";
import { ACCENT_PRESETS, isHexColor, type AccentPreset } from "@/lib/theme";
import { parseHex } from "./colors";
import {
  CONNECTION_TAGS,
  MYSQL_VERSIONS,
  TAG_NAME_MAX,
  usesSocket,
  type ConnectionFormValues,
} from "./schema";

/**
 * Connection URLs, as TablePlus reads and writes them ("New connection from URL…", "Copy as
 * URL", `open -a Tabler "<url>"`): `postgres://user:password@host:port/database?name=…`,
 * `mysql://…`, `mariadb://…`, `sqlite:///absolute/path.sqlite`. User, password and database are
 * percent-decoded. MySQL's "Use socket" is `?socket=/tmp/mysql.sock` (empty: auto-detect); such a
 * URL may leave the host out (`mysql://root@/db?socket=…`).
 *
 * Over SSH: `postgresql+ssh://sshuser:sshpass@sshhost:22/dbuser:dbpass@127.0.0.1:5432/db?…`
 * (the SSH server first, then the database as the SSH server sees it).
 *
 * TablePlus's query parameters: `name`, `statusColor` (hex without `#`; a colour that is not one of
 * the presets stays as that custom colour), `env` (older builds: `enviroment`), `safeModeLevel` +
 * `advancedSafeModeLevel`, `tLSMode`, `usePrivateKey`,
 * `windowMode` (`tabbed` / `isolated`), `driverVersion` (MySQL's "Ver." menu: 0 Default,
 * 1 MariaDB, 2 Old MySQL), `lazyload`, and where to go once
 * connected: `schema`, `table` or `view`, plus a row filter — `column` + `operation` (or
 * `operator`) + `value`, or a raw condition in `condition` / `raw` / `query`.
 */

const SCHEMES: Record<string, CoreDriver> = {
  postgres: "postgres",
  postgresql: "postgres",
  pgsql: "postgres",
  mysql: "mysql",
  mariadb: "mysql",
  sqlite: "sqlite",
  sqlite3: "sqlite",
};

const SSL_MODES: Record<string, SslMode> = {
  disable: "disable",
  disabled: "disable",
  allow: "prefer",
  prefer: "prefer",
  preferred: "prefer",
  require: "require",
  required: "require",
  "verify-ca": "verify-ca",
  verify_ca: "verify-ca",
  "verify-full": "verify-full",
  verify_identity: "verify-full",
  "verify-identity": "verify-full",
};

/**
 * TablePlus's integer `tLSMode`. Its values are not documented publicly; read in the order of
 * the TLS menu (0 disable … 4 verify full). An explicit `sslmode` wins.
 */
const TLS_MODES: readonly SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

/**
 * TablePlus's `safeModeLevel` (0 silent, 1 alert, 2 safe) with `advancedSafeModeLevel=1`
 * (skip SELECT / EXPLAIN / SHOW), both ints.
 */
function safeModeFromParams(level: string | null, advanced: string | null): SafeModeLevel | null {
  const skipReads = advanced === "1";
  switch (level) {
    case "0":
      return "silent";
    case "1":
      return skipReads ? "alertNonSelect" : "alert";
    case "2":
      return skipReads ? "safeNonSelect" : "safe";
    default:
      return null;
  }
}

function safeModeParams(mode: SafeModeLevel): [level: string, advanced: string | null] {
  switch (mode) {
    case "silent":
      return ["0", null];
    case "alert":
      return ["1", null];
    case "alertNonSelect":
      return ["1", "1"];
    case "safe":
      return ["2", null];
    case "safeNonSelect":
      return ["2", "1"];
  }
}

/**
 * The preset closest to a `statusColor` (hex, `#` optional) or a preset name. Reading a URL goes
 * through `colorOf`, which keeps a custom hex as it is; this is the fallback for names and the
 * nearest-preset lookup for callers that need a preset.
 */
export function presetForColor(value: string): AccentPreset | null {
  const v = value.trim().toLowerCase();
  if ((ACCENT_PRESETS as readonly string[]).includes(v)) return v as AccentPreset;
  const hex = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/.exec(v)?.[1];
  if (!hex) return null;
  const full = hex.length === 3 ? [...hex].map((c) => c + c).join("") : hex;
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16) / 255) as [
    number,
    number,
    number,
  ];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  const light = (max + min) / 2;
  const sat = delta === 0 ? 0 : delta / (1 - Math.abs(2 * light - 1));
  if (sat < 0.18 || light < 0.08 || light > 0.95) return "gray";
  let hue =
    max === r ? ((g - b) / delta) % 6 : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4;
  hue = (hue * 60 + 360) % 360;
  if (hue < 18 || hue >= 340) return "red";
  if (hue < 42) return "orange";
  if (hue < 70) return "yellow";
  if (hue < 165) return "green";
  if (hue < 200) return "teal";
  if (hue < 255) return "blue";
  return "purple";
}

/**
 * The colour a `statusColor` stands for: a preset name, or the preset whose own hex it is; any
 * other valid hex (3 or 6 digits, `#` optional) stays as that custom `#rrggbb` colour (the profile
 * takes both). Null when it is neither.
 */
export function colorOf(value: string): string | null {
  const v = value.trim().toLowerCase();
  if ((ACCENT_PRESETS as readonly string[]).includes(v)) return v;
  const hex = parseHex(v);
  if (!hex) return presetForColor(v);
  const own = ACCENT_PRESETS.find((p) => PRESET_HEX[p].toLowerCase() === hex.slice(1));
  return own ?? hex;
}

/** A row filter of a deep link: a column condition, or a raw WHERE condition. */
export type UrlFilter =
  | { kind: "condition"; column: string; operator: string; value: string }
  | { kind: "raw"; sql: string };

/** Where a deep link goes once connected. */
export interface UrlTarget {
  schema: string | null;
  /** The table or view to open (null: just the connection). */
  table: string | null;
  filter: UrlFilter | null;
}

/** TablePlus options Tabler reads but that are not part of the connection form. */
export interface UrlExtras {
  /** `isolated`: open in its own window; `tabbed` (default): in this one. */
  windowMode: "tabbed" | "isolated" | null;
  driverVersion: number | null;
  lazyload: boolean | null;
}

export interface ParsedConnectionUrl {
  /** Form fields the URL sets; the rest keep the form's defaults. An SSH password is in
   * `values.sshPassword`. */
  values: Partial<ConnectionFormValues>;
  /** The password in the URL, if any (it goes to the Keychain, never into the profile). */
  password: string | null;
  /** Where to go once connected (deep links). */
  target?: UrlTarget | null;
  extras?: UrlExtras;
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

function boolParam(v: string | null): boolean | null {
  if (v === null) return null;
  const t = v.trim().toLowerCase();
  if (["1", "true", "yes", "on"].includes(t)) return true;
  if (["0", "false", "no", "off"].includes(t)) return false;
  return null;
}

function intParam(v: string | null): number | null {
  if (v === null || !/^\s*-?\d+\s*$/.test(v)) return null;
  return Number(v);
}

/** The schema / table / view and row filter of a URL's query. */
export function targetFromParams(params: URLSearchParams): UrlTarget | null {
  const text = (k: string) => {
    const v = params.get(k)?.trim();
    return v ? v : null;
  };
  const schema = text("schema");
  const table = text("table") ?? text("view");
  const raw = text("condition") ?? text("raw") ?? text("query");
  const column = text("column");
  let filter: UrlFilter | null = null;
  if (raw) filter = { kind: "raw", sql: raw };
  else if (column)
    filter = {
      kind: "condition",
      column,
      operator: (params.get("operation") ?? params.get("operator") ?? "=").trim() || "=",
      value: params.get("value") ?? "",
    };
  if (!schema && !table && !filter) return null;
  return { schema, table, filter };
}

/** The "Ver." entry of a link's `driverVersion` (an index into the menu); null: out of range. */
export function mysqlVersionOf(
  driverVersion: number | null,
): (typeof MYSQL_VERSIONS)[number] | null {
  return driverVersion === null ? null : (MYSQL_VERSIONS[driverVersion] ?? null);
}

function extrasFromParams(params: URLSearchParams): UrlExtras {
  const mode = params.get("windowMode")?.trim().toLowerCase();
  return {
    windowMode: mode === "isolated" || mode === "tabbed" ? mode : null,
    driverVersion: intParam(params.get("driverVersion")),
    lazyload: boolParam(params.get("lazyload")),
  };
}

/** `name`, colour, tag and safe mode: shared by every driver. */
function commonValues(params: URLSearchParams, values: Partial<ConnectionFormValues>): void {
  const color = params.get("statusColor") ?? params.get("color");
  const resolved = color ? colorOf(color) : null;
  if (resolved) values.color = resolved;
  // `env` (TablePlus; older builds spelled it `enviroment`) or `tag`: a built-in tag in any case,
  // or a custom one.
  const tag = (params.get("env") ?? params.get("enviroment") ?? params.get("tag"))?.trim();
  if (tag && tag.length <= TAG_NAME_MAX) {
    const builtin = CONNECTION_TAGS.find((t) => t === tag.toLowerCase());
    values.tag = builtin ?? tag;
  }
  const safeMode = safeModeFromParams(
    params.get("safeModeLevel"),
    params.get("advancedSafeModeLevel"),
  );
  if (safeMode) values.safeMode = safeMode;
}

/** `[user[:password]@]host[:port]` of a `+ssh` URL's SSH server. */
function parseSshAuthority(text: string): {
  user: string;
  password: string | null;
  host: string;
  port: string;
} {
  const at = text.lastIndexOf("@");
  const userinfo = at >= 0 ? text.slice(0, at) : "";
  const hostport = at >= 0 ? text.slice(at + 1) : text;
  const colon = userinfo.indexOf(":");
  const user = decode(colon >= 0 ? userinfo.slice(0, colon) : userinfo);
  const password = colon >= 0 ? decode(userinfo.slice(colon + 1)) : null;
  const ipv6 = /^\[([^\]]+)\](?::(\d+))?$/.exec(hostport);
  const plain = /^([^:]*)(?::(\d+))?$/.exec(hostport);
  const host = decode(ipv6?.[1] ?? plain?.[1] ?? "");
  const port = ipv6?.[2] ?? plain?.[2] ?? "";
  if (host === "") throw new Error("The URL has no SSH host.");
  return { user, password, host, port };
}

/**
 * Query keys that a URL means for Tabler itself, never for the server (lower case): what the URL
 * reader consumes, the form-owned libpq keys that the backend refuses in the Parameters field, and
 * the three libpq keys that have a field of their own.
 */
const OWN_QUERY_KEYS: ReadonlySet<string> = new Set([
  // Tabler / TablePlus
  "name",
  "statuscolor",
  "color",
  "env",
  "enviroment",
  "tag",
  "safemodelevel",
  "advancedsafemodelevel",
  "sslmode",
  "ssl-mode",
  "ssl_mode",
  "tlsmode",
  "useprivatekey",
  "socket",
  "driverversion",
  "lazyload",
  "windowmode",
  // Where to go (`targetFromParams`)
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
  // Form fields of their own (`sslnegotiation` etc. fill `pgSslNegotiation`…)
  "sslnegotiation",
  "channel_binding",
  "connect_timeout",
  "host",
  "hostaddr",
  "port",
  "user",
  "password",
  "dbname",
  "sslrootcert",
  "sslcert",
  "sslkey",
  "client_encoding",
]);

/**
 * libpq (and JDBC) connection keys that are not server settings and that Tabler's driver does not
 * read: in the Parameters field they would run as `set_config('keepalives', …)` and make the
 * connect fail ("unrecognized configuration parameter"), so a URL's copy of them is left out, as
 * it was before URLs carried parameters. (`options`, `application_name`,
 * `fallback_application_name` and `target_session_attrs` are read by the driver and stay.)
 */
const UNSUPPORTED_CONNECTION_KEYS: ReadonlySet<string> = new Set([
  "passfile",
  "require_auth",
  "requiressl",
  "sslpassword",
  "sslcertmode",
  "sslcrl",
  "sslcrldir",
  "sslcompression",
  "sslsni",
  "requirepeer",
  "ssl_min_protocol_version",
  "ssl_max_protocol_version",
  "min_protocol_version",
  "max_protocol_version",
  "krbsrvname",
  "gsslib",
  "gssdelegation",
  "gssencmode",
  "service",
  "replication",
  "keepalives",
  "keepalives_idle",
  "keepalives_interval",
  "keepalives_count",
  "tcp_user_timeout",
  "load_balance_hosts",
  "ssl",
  "sslfactory",
]);

/** Whether a URL's query key goes to the Parameters field (and a field's pair back to the URL). */
function isServerParam(key: string): boolean {
  return !OWN_QUERY_KEYS.has(key) && !UNSUPPORTED_CONNECTION_KEYS.has(key) && PARAM_NAME.test(key);
}

/** A setting / libpq key the Parameters field accepts (the backend refuses anything else). */
const PARAM_NAME = /^[A-Za-z_][A-Za-z0-9_.]*$/;

/** A value as the Parameters field reads it back: single-quoted when it needs to be. */
export function quoteParamValue(value: string): string {
  if (value !== "" && !/[\s'"\\%&]/.test(value)) return value;
  return `'${value.replace(/[\\']/g, (c) => `\\${c}`)}'`;
}

/**
 * Splits the Parameters field like the backend does (`split_params` in conn_options.rs):
 * whitespace, newline or `&` separated `key=value` pairs, a value may be single-quoted (`\'` and
 * `\\` escaped), an unquoted one with a `%` is percent-decoded. Keys are lower-cased. A malformed
 * rest ends the list (the form refuses it when the connection is saved).
 */
export function splitParams(text: string): [key: string, value: string][] {
  const out: [string, string][] = [];
  let i = 0;
  const blank = (c: string | undefined) => c !== undefined && /\s/.test(c);
  const skip = (pred: (c: string) => boolean) => {
    while (i < text.length && pred(text[i]!)) i++;
  };
  for (;;) {
    skip((c) => blank(c) || c === "&" || c === "?");
    if (i >= text.length) break;
    let key = "";
    while (i < text.length && !(text[i] === "=" || blank(text[i]) || text[i] === "&"))
      key += text[i++];
    skip((c) => c === " " || c === "\t");
    if (text[i] !== "=") break;
    i++;
    skip((c) => c === " " || c === "\t");
    let value = "";
    if (text[i] === "'") {
      i++;
      let closed = false;
      while (i < text.length) {
        const c = text[i++]!;
        if (c === "\\") {
          if (i < text.length) value += text[i++];
        } else if (c === "'") {
          closed = true;
          break;
        } else value += c;
      }
      if (!closed) break;
    } else {
      while (i < text.length && !(blank(text[i]) || text[i] === "&")) value += text[i++];
      if (value.includes("%")) value = decodeParam(value);
    }
    out.push([key.toLowerCase(), value]);
  }
  return out;
}

/** Percent-decoding of an unquoted Parameters value (`+` is a space), like the backend's. */
function decodeParam(value: string): string {
  const spaced = value.replace(/\+/g, " ");
  try {
    return decodeURIComponent(spaced);
  } catch {
    return spaced;
  }
}

/**
 * A PostgreSQL URL's libpq parameters: `sslnegotiation=direct`, `channel_binding=disable` and
 * `connect_timeout` fill their own fields; every other key Tabler does not read (`jit=off`,
 * `application_name`, `options`…) is appended to `pgParams` as `key=value`, in URL order.
 */
function postgresValues(params: URLSearchParams, values: Partial<ConnectionFormValues>): void {
  const extra: string[] = [];
  for (const [rawKey, value] of params) {
    const key = rawKey.trim().toLowerCase();
    if (key === "sslnegotiation") {
      if (value.trim().toLowerCase() === "direct") values.pgSslNegotiation = "direct";
    } else if (key === "channel_binding") {
      if (value.trim().toLowerCase() === "disable") values.pgDisableChannelBinding = true;
    } else if (key === "connect_timeout") {
      const seconds = intParam(value);
      if (seconds !== null && seconds > 0) values.connectTimeoutSecs = String(seconds);
    } else if (isServerParam(key)) {
      extra.push(`${key}=${quoteParamValue(value)}`);
    }
  }
  if (extra.length > 0) values.pgParams = extra.join(" ");
}

/** Parses a connection URL. Throws an Error with a user-facing message when it is not one. */
export function parseConnectionUrl(input: string): ParsedConnectionUrl {
  const text = input.trim().replace(/^jdbc:/i, "");
  const fullScheme = /^([a-z][a-z0-9+.-]*):/i.exec(text)?.[1]?.toLowerCase();
  const overSsh = fullScheme?.endsWith("+ssh") ?? false;
  const scheme = overSsh ? fullScheme!.slice(0, -4) : fullScheme;
  const driver = scheme ? SCHEMES[scheme] : undefined;
  if (!fullScheme || !scheme || !driver)
    throw new Error("Use a postgres://, mysql://, mariadb:// or sqlite:// URL.");

  if (overSsh) {
    // `<db>+ssh://<ssh authority>/<the database URL's authority, path and query>`
    const rest = text.slice(fullScheme.length + 1).replace(/^\/\//, "");
    const slash = rest.indexOf("/");
    if (slash < 0) throw new Error("After the SSH server, add the database: …/user@host/db");
    const ssh = parseSshAuthority(rest.slice(0, slash));
    const inner = rest.slice(slash + 1);
    const params = new URLSearchParams(inner.split("?")[1] ?? "");
    let parsed: ParsedConnectionUrl;
    if (driver === "sqlite") {
      // The file's path on the SSH server: `~/…` as is, anything else from the root.
      const raw = decode(inner.split("?")[0]!);
      if (raw === "" || raw.endsWith("/")) throw new Error("The URL has no database file path.");
      const path = raw.startsWith("~") ? raw : `/${raw.replace(/^\/+/, "")}`;
      const values: Partial<ConnectionFormValues> = {
        driver,
        sqlitePath: path,
        name: params.get("name")?.trim() || (path.split("/").filter(Boolean).at(-1) ?? "SQLite"),
      };
      commonValues(params, values);
      parsed = {
        values,
        password: null,
        target: targetFromParams(params),
        extras: extrasFromParams(params),
      };
    } else {
      parsed = parseConnectionUrl(`${scheme}://${inner}`);
    }
    const usePrivateKey = boolParam(params.get("usePrivateKey"));
    parsed.values = {
      ...parsed.values,
      sshEnabled: true,
      sshHost: ssh.host,
      sshPort: ssh.port || "22",
      sshUser: ssh.user,
      sshAuth: usePrivateKey === true ? "key" : ssh.password !== null ? "password" : "key",
      ...(ssh.password !== null ? { sshPassword: ssh.password } : {}),
    };
    if (!params.get("name") && driver !== "sqlite")
      parsed.values.name = `${parsed.values.name ?? ""} via ${ssh.host}`.trim();
    return parsed;
  }

  if (driver === "sqlite") {
    // sqlite:///abs/path, sqlite:/abs/path or sqlite://~/path; the query names the target.
    const [pathPart, query = ""] = text.slice(scheme.length + 1).split("?") as [string, string?];
    const rest = decode(pathPart);
    const path = rest.startsWith("//") ? rest.slice(2) : rest;
    if (path === "" || path === "/") throw new Error("The URL has no database file path.");
    if (!path.startsWith("/") && !path.startsWith("~/"))
      throw new Error("Use an absolute path to the database file.");
    const params = new URLSearchParams(query);
    const values: Partial<ConnectionFormValues> = {
      driver,
      sqlitePath: path,
      name: params.get("name")?.trim() || (path.split("/").filter(Boolean).at(-1) ?? "SQLite"),
    };
    commonValues(params, values);
    return {
      values,
      password: null,
      target: targetFromParams(params),
      extras: extrasFromParams(params),
    };
  }

  // `user@` without a host is no URL to the parser; a socket URL needs no host.
  const parseable =
    driver === "mysql" && /[?&]socket=/.test(text)
      ? text.replace(/^([a-z][a-z0-9+.-]*:\/\/[^/?#]*@)(?=[/?#]|$)/i, "$1localhost")
      : text;
  let url: URL;
  try {
    url = new URL(parseable);
  } catch {
    throw new Error("This is not a valid connection URL.");
  }
  const params = url.searchParams;
  const socket = driver === "mysql" ? params.get("socket") : null;
  const host = url.hostname.replace(/^\[(.*)\]$/, "$1") || (socket !== null ? "localhost" : "");
  if (host === "") throw new Error("The URL has no host.");
  const database = decode(url.pathname.replace(/^\//, ""));
  const values: Partial<ConnectionFormValues> = {
    driver,
    host,
    port: url.port || String(driver === "postgres" ? 5432 : 3306),
    user: decode(url.username),
    database,
  };
  if (socket !== null) {
    values.useSocket = true;
    values.socketPath = socket.trim();
  }
  const name = params.get("name");
  values.name = name?.trim() || `${values.user ? `${values.user}@` : ""}${host}`;
  const ssl = params.get("sslmode") ?? params.get("ssl-mode") ?? params.get("ssl_mode");
  const sslMode = ssl ? SSL_MODES[ssl.toLowerCase()] : undefined;
  const tls = intParam(params.get("tLSMode") ?? params.get("tlsMode"));
  if (sslMode) values.ssl = sslMode;
  else if (tls !== null && TLS_MODES[tls]) values.ssl = TLS_MODES[tls];
  commonValues(params, values);
  if (driver === "postgres") postgresValues(params, values);
  const extras = extrasFromParams(params);
  // MySQL's "Ver." menu: the link's index fills the form's dropdown.
  const version = driver === "mysql" ? mysqlVersionOf(extras.driverVersion) : null;
  if (version) values.mysqlVersion = version;
  return {
    values,
    password: url.password === "" ? null : decode(url.password),
    target: targetFromParams(params),
    extras,
  };
}

/** What a link to Tabler asks for. */
export type DeepLink =
  | { kind: "connection"; parsed: ParsedConnectionUrl }
  /** A saved connection by its id (`tabler://connection/<id>`, TablePlus's undocumented id). */
  | { kind: "saved"; profileId: string; target: UrlTarget | null; extras: UrlExtras };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * Parses a link Tabler was opened with: a connection URL (any form above), or a `tabler:` link —
 * `tabler://connection/<id>?schema=…&table=…` (also `tabler://?id=<id>`) for a saved connection,
 * `tabler://open?url=<connection URL>`. Throws an Error with a user-facing message.
 */
export function parseDeepLink(input: string): DeepLink {
  const text = input.trim();
  if (!/^tabler:/i.test(text)) return { kind: "connection", parsed: parseConnectionUrl(text) };
  let url: URL;
  try {
    url = new URL(text.replace(/^tabler:/i, "tabler:"));
  } catch {
    throw new Error("This is not a valid Tabler link.");
  }
  const params = url.searchParams;
  const inner = params.get("url");
  if (inner) return { kind: "connection", parsed: parseConnectionUrl(inner) };
  // `tabler://connection/<id>`: the host is "connection", the id the first path segment.
  const segments = [url.hostname, ...url.pathname.split("/")].filter(Boolean).map(decode);
  const id =
    params.get("id") ??
    (segments[0]?.toLowerCase() === "connection" ? segments[1] : segments[0]) ??
    "";
  if (!UUID.test(id)) throw new Error("The link names no saved connection.");
  return {
    kind: "saved",
    profileId: id.toLowerCase(),
    target: targetFromParams(params),
    extras: extrasFromParams(params),
  };
}

/** Percent-encodes a URL component, keeping it readable (`@`, `:`, `/` are encoded). */
const enc = (s: string) => encodeURIComponent(s);

/** The `statusColor` hex (no `#`, upper case) of each colour preset, as `presetForColor` reads it. */
const PRESET_HEX: Record<AccentPreset, string> = {
  red: "DB4241",
  orange: "DE6F00",
  yellow: "CF9B00",
  green: "25984D",
  teal: "009C9C",
  blue: "2279DC",
  purple: "8B5DCE",
  gray: "606369",
};

/** A connection's colour as TablePlus's `statusColor`: hex without `#`; null without a colour. */
export function statusColorOf(color: string | null): string | null {
  if (!color) return null;
  if (isHexColor(color)) return color.slice(1).toUpperCase();
  return PRESET_HEX[color as AccentPreset] ?? null;
}

/** The query every driver's URL carries: name, colour, tag, safe mode and the "Ver." / lazy load. */
function commonQuery(p: ConnectionProfile): URLSearchParams {
  const query = new URLSearchParams({ name: p.name });
  const color = statusColorOf(p.color);
  if (color) query.set("statusColor", color);
  return query;
}

function tailQuery(p: ConnectionProfile, query: URLSearchParams): void {
  if (p.options.tag) query.set("env", p.options.tag);
  if (p.options.safeMode && p.options.safeMode !== "silent") {
    const [level, advanced] = safeModeParams(p.options.safeMode);
    query.set("safeModeLevel", level);
    if (advanced) query.set("advancedSafeModeLevel", advanced);
  }
  const version =
    p.driver === "mysql" ? MYSQL_VERSIONS.indexOf(p.options.mysqlVersion ?? "default") : 0;
  if (version > 0) query.set("driverVersion", String(version));
  if (typeof p.options.lazyLoad === "boolean")
    query.set("lazyload", p.options.lazyLoad ? "true" : "false");
}

/**
 * A PostgreSQL profile's libpq parameters: `sslnegotiation=direct`, `channel_binding=disable`,
 * `connect_timeout`, then the Parameters field's own pairs (read back by `postgresValues`). A pair
 * whose key means something else in a Tabler URL (`name`, `env`…) is left out: it could not come
 * back (and the form's own libpq keys are refused in the Parameters field anyway).
 */
function postgresQuery(p: ConnectionProfile, query: URLSearchParams): void {
  const o = p.options;
  if (o.pgSslNegotiation === "direct") query.set("sslnegotiation", "direct");
  if (o.pgDisableChannelBinding === true) query.set("channel_binding", "disable");
  if (o.connectTimeoutSecs) query.set("connect_timeout", String(o.connectTimeoutSecs));
  for (const [key, value] of splitParams(o.pgParams ?? ""))
    if (isServerParam(key)) query.append(key, value);
}

/** `user@host[:port]` of the SSH server in a `+ssh` URL (no password: it is never kept). */
function sshAuthority(ssh: NonNullable<ConnectionProfile["ssh"]>): string {
  const user = ssh.user ? `${enc(ssh.user)}@` : "";
  const host = ssh.host.includes(":") ? `[${ssh.host}]` : ssh.host;
  // 0: the port of ~/.ssh/config; 22 is the URL's default.
  const port = ssh.port && ssh.port !== 22 ? `:${ssh.port}` : "";
  return `${user}${host}${port}`;
}

/**
 * The profile as a connection URL ("Copy as URL"). The password is only included when given
 * (it is never stored in the profile). With SSH it is the `+ssh` form TablePlus reads —
 * `postgresql+ssh://sshuser@sshhost:2222/dbuser:dbpass@127.0.0.1:5432/db?…` — whose SSH password is
 * never included; `usePrivateKey=true` for key authentication.
 */
export function profileToUrl(p: ConnectionProfile, password: string | null = null): string {
  const query = commonQuery(p);
  const ssh = p.ssh;
  if (p.driver === "sqlite") {
    tailQuery(p, query);
    const path = p.sqlitePath ? encodeURI(p.sqlitePath) : "";
    if (ssh) {
      if (ssh.auth === "key") query.set("usePrivateKey", "true");
      // The file's path on the SSH server: `~/…` as is, else from the root.
      return `sqlite+ssh://${sshAuthority(ssh)}/${path.replace(/^\/+/, "")}?${query.toString()}`;
    }
    return `sqlite://${path}?${query.toString()}`;
  }
  const scheme = p.driver === "postgres" ? "postgresql" : "mysql";
  const user = p.user ? enc(p.user) + (password ? `:${enc(password)}` : "") + "@" : "";
  const host = (p.host ?? "").includes(":") ? `[${p.host}]` : (p.host ?? "");
  const port = p.port ? `:${p.port}` : "";
  const db = p.database ? `/${enc(p.database)}` : "";
  if (usesSocket(p)) query.set("socket", p.options.socketPath ?? "");
  if (p.ssl !== "prefer") query.set("sslmode", p.ssl);
  if (ssh?.auth === "key") query.set("usePrivateKey", "true");
  if (p.driver === "postgres") postgresQuery(p, query);
  tailQuery(p, query);
  const database = `${user}${host}${port}${db}?${query.toString()}`;
  return ssh ? `${scheme}+ssh://${sshAuthority(ssh)}/${database}` : `${scheme}://${database}`;
}
