import type { ConnectionProfile, ConnectionTag, Driver, SafeModeLevel, SslMode } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";
import { CONNECTION_TAGS, usesSocket, type ConnectionFormValues } from "./schema";

/**
 * Connection URLs, as TablePlus reads and writes them ("New connection from URL…", "Copy as
 * URL"): `postgres://user:password@host:port/database?name=…&sslmode=…`, `mysql://…`,
 * `mariadb://…`, `sqlite:///absolute/path.sqlite`. User, password and database are
 * percent-decoded. MySQL's "Use socket" is `?socket=/tmp/mysql.sock` (empty: auto-detect); such a
 * URL may leave the host out (`mysql://root@/db?socket=…`).
 */

const SCHEMES: Record<string, Driver> = {
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

export interface ParsedConnectionUrl {
  /** Form fields the URL sets; the rest keep the form's defaults. */
  values: Partial<ConnectionFormValues>;
  /** The password in the URL, if any (it goes to the Keychain, never into the profile). */
  password: string | null;
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** Parses a connection URL. Throws an Error with a user-facing message when it is not one. */
export function parseConnectionUrl(input: string): ParsedConnectionUrl {
  const text = input.trim().replace(/^jdbc:/i, "");
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(text)?.[1]?.toLowerCase();
  const driver = scheme ? SCHEMES[scheme] : undefined;
  if (!scheme || !driver)
    throw new Error("Use a postgres://, mysql://, mariadb:// or sqlite:// URL.");

  if (driver === "sqlite") {
    // sqlite:///abs/path, sqlite:/abs/path or sqlite://~/path; any query string is ignored.
    const rest = decode(text.slice(scheme.length + 1).split("?")[0]!);
    const path = rest.startsWith("//") ? rest.slice(2) : rest;
    if (path === "" || path === "/") throw new Error("The URL has no database file path.");
    if (!path.startsWith("/") && !path.startsWith("~/"))
      throw new Error("Use an absolute path to the database file.");
    const name = path.split("/").filter(Boolean).at(-1) ?? "SQLite";
    return { values: { driver, sqlitePath: path, name }, password: null };
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
  if (sslMode) values.ssl = sslMode;
  const color = (params.get("color") ?? params.get("statusColor"))?.toLowerCase();
  if (color && (ACCENT_PRESETS as readonly string[]).includes(color))
    values.color = color as ConnectionFormValues["color"];
  // `env` (TablePlus; older builds spelled it `enviroment`) or `tag`.
  const tag = (params.get("env") ?? params.get("enviroment") ?? params.get("tag"))?.toLowerCase();
  if (tag && (CONNECTION_TAGS as readonly string[]).includes(tag))
    values.tag = tag as ConnectionTag;
  const safeMode = safeModeFromParams(
    params.get("safeModeLevel"),
    params.get("advancedSafeModeLevel"),
  );
  if (safeMode) values.safeMode = safeMode;
  return { values, password: url.password === "" ? null : decode(url.password) };
}

/** Percent-encodes a URL component, keeping it readable (`@`, `:`, `/` are encoded). */
const enc = (s: string) => encodeURIComponent(s);

/**
 * The profile as a connection URL ("Copy as URL"). The password is only included when given
 * (it is never stored in the profile).
 */
export function profileToUrl(p: ConnectionProfile, password: string | null = null): string {
  if (p.driver === "sqlite") return `sqlite://${p.sqlitePath ? encodeURI(p.sqlitePath) : ""}`;
  const scheme = p.driver === "postgres" ? "postgresql" : "mysql";
  const user = p.user ? enc(p.user) + (password ? `:${enc(password)}` : "") + "@" : "";
  const host = (p.host ?? "").includes(":") ? `[${p.host}]` : (p.host ?? "");
  const port = p.port ? `:${p.port}` : "";
  const db = p.database ? `/${enc(p.database)}` : "";
  const query = new URLSearchParams({ name: p.name });
  if (usesSocket(p)) query.set("socket", p.options.socketPath ?? "");
  if (p.ssl !== "prefer") query.set("sslmode", p.ssl);
  if (p.options.tag) query.set("env", p.options.tag);
  if (p.options.safeMode && p.options.safeMode !== "silent") {
    const [level, advanced] = safeModeParams(p.options.safeMode);
    query.set("safeModeLevel", level);
    if (advanced) query.set("advancedSafeModeLevel", advanced);
  }
  return `${scheme}://${user}${host}${port}${db}?${query.toString()}`;
}
