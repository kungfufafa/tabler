import type { ConnectionProfile, Driver, SslMode } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";
import type { ConnectionFormValues } from "./schema";

/**
 * Connection URLs, as TablePlus reads and writes them ("New connection from URL…", "Copy as
 * URL"): `postgres://user:password@host:port/database?name=…&sslmode=…`, `mysql://…`,
 * `mariadb://…`, `sqlite:///absolute/path.sqlite`. User, password and database are
 * percent-decoded.
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

  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw new Error("This is not a valid connection URL.");
  }
  const host = url.hostname.replace(/^\[(.*)\]$/, "$1");
  if (host === "") throw new Error("The URL has no host.");
  const params = url.searchParams;
  const database = decode(url.pathname.replace(/^\//, ""));
  const values: Partial<ConnectionFormValues> = {
    driver,
    host,
    port: url.port || String(driver === "postgres" ? 5432 : 3306),
    user: decode(url.username),
    database,
  };
  const name = params.get("name");
  values.name = name?.trim() || `${values.user ? `${values.user}@` : ""}${host}`;
  const ssl = params.get("sslmode") ?? params.get("ssl-mode") ?? params.get("ssl_mode");
  const sslMode = ssl ? SSL_MODES[ssl.toLowerCase()] : undefined;
  if (sslMode) values.ssl = sslMode;
  const color = (params.get("color") ?? params.get("statusColor"))?.toLowerCase();
  if (color && (ACCENT_PRESETS as readonly string[]).includes(color))
    values.color = color as ConnectionFormValues["color"];
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
  if (p.ssl !== "prefer") query.set("sslmode", p.ssl);
  return `${scheme}://${user}${host}${port}${db}?${query.toString()}`;
}
