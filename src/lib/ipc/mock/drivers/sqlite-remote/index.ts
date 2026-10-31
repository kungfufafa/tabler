import { splitStatements } from "@/features/editor/scanner";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { D1_DATABASES, database } from "./data";
import { READ_ONLY_MESSAGE, mayWrite, remoteSession } from "./session";

/**
 * The mock drivers of the sqlite-remote lane (LibSQL and Cloudflare D1), served in
 * `vite --mode mock` with `?mockDrivers=libsql,d1` (or `sqlite-remote`): a Turso database and a
 * local `sqld`, and a D1 account with two databases, one of them opened read-only. Connecting
 * checks what the backend's `validate` and `open` check, with the same words.
 */

const str = (p: ConnectionProfile, key: string): string => {
  const v = p.options.driverParams?.[key];
  return typeof v === "string" ? v.trim() : "";
};

/**
 * `libsql/url.rs::parse_url`'s `has_secret`: a token key in any casing (even empty or repeated),
 * or a user or a password, alone or together.
 */
function urlHoldsSecret(url: string): boolean {
  let u: URL;
  try {
    u = new URL(url.includes("://") ? url.replace(/^[a-z]+:\/\//i, "https://") : `https://${url}`);
  } catch {
    return false;
  }
  if (u.username !== "" || u.password !== "") return true;
  return [...u.searchParams.keys()].some((k) =>
    ["authtoken", "auth_token"].includes(k.toLowerCase()),
  );
}

/** `libsql/entry.rs::validate`: a host or a URL, no secret in the URL, Basic with a user. */
function validateLibsql(p: ConnectionProfile): void {
  const url = str(p, "url");
  if (url !== "") {
    if (!/^(libsql|https?|wss?):\/\//i.test(url) && url.includes("://"))
      mockError(
        "INVALID_INPUT",
        "a LibSQL URL starts with libsql://, https://, http://, wss:// or ws://",
      );
    if (urlHoldsSecret(url))
      mockError(
        "INVALID_INPUT",
        "LibSQL: the URL holds a token or a password: put it in the Auth Token field (and the user in User)",
      );
    if (p.ssh) mockError("INVALID_INPUT", "LibSQL over SSH takes the host and port, not a URL");
  } else if ((p.host ?? "").trim() === "") mockError("INVALID_INPUT", "Host is required");
  if (p.options.driverParams?.basicAuth === true && (p.user ?? "").trim() === "")
    mockError("INVALID_INPUT", "LibSQL: Basic authentication needs a user");
}

/** The host the sidebar names the database after: the URL's, or the profile's. */
function libsqlHost(p: ConnectionProfile): string {
  const url = str(p, "url");
  if (url === "") return p.host ?? "";
  try {
    return new URL(url.replace(/^[a-z]+:\/\//i, "https://")).hostname;
  } catch {
    return url;
  }
}

/**
 * `parse_url`'s `tls`: the scheme says it, and only `libsql://` (or no scheme) reads `?tls=`, its
 * key in any casing and its value `0` or `false` exactly.
 */
export const libsqlTls = (p: ConnectionProfile) => {
  const url = str(p, "url");
  if (url === "") return p.ssl !== "disable";
  const at = url.indexOf("://");
  const scheme = at < 0 ? "libsql" : url.slice(0, at).toLowerCase();
  if (scheme !== "libsql") return !["http", "ws"].includes(scheme);
  const q = url.indexOf("?");
  let tls = true;
  if (q < 0) return tls;
  for (const [key, value] of new URLSearchParams(url.slice(q + 1).split("#")[0])) {
    if (key.toLowerCase() === "tls") tls = !["0", "false"].includes(value);
  }
  return tls;
};

export const libsqlMock: MockDriver = {
  driver: "libsql",
  serverVersion: () => "LibSQL (SQLite 3.45.1)",
  seedProfiles: () => [
    profile(
      {
        id: "p-mock-libsql",
        name: "Turso shop",
        driver: "libsql",
        color: "green",
        host: "shop-acme.aws-eu-west-1.turso.io",
        port: null,
        user: null,
        database: null,
        ssl: "verify-full",
        sortOrder: 110,
      },
      "production",
    ),
    profile(
      {
        id: "p-mock-sqld",
        name: "sqld (local)",
        driver: "libsql",
        host: "127.0.0.1",
        port: 8080,
        user: null,
        database: null,
        ssl: "disable",
        sortOrder: 111,
      },
      "local",
    ),
  ],
  connect: (p) => {
    validateLibsql(p);
    const readOnly = p.options.readOnly;
    // `LibsqlSession::connect`: a startup command that writes fails a read-only connect.
    const startup = p.options.startupSql ?? "";
    if (
      readOnly &&
      splitStatements(startup, "sqlite").some(({ start, end }) =>
        mayWrite(startup.slice(start, end)),
      )
    )
      mockError("INVALID_INPUT", `${READ_ONLY_MESSAGE}: a startup command writes`);
    const host = libsqlHost(p);
    return remoteSession("libsql", database("libsql", "shop"), {
      readOnly,
      tls: libsqlTls(p),
      // The mock's servers are `sqld` and Turso, which refuse `PRAGMA query_only = 1`.
      queryOnly: false,
      databases: () => [{ name: host, isCurrent: true }],
    });
  },
};

const ACCOUNT = /^[0-9a-f]{32}$/i;

/** `d1/api.rs::validate_account_id`. */
function checkAccount(p: ConnectionProfile): void {
  if (!ACCOUNT.test(str(p, "accountId")))
    mockError(
      "INVALID_INPUT",
      "the Cloudflare account ID is 32 hexadecimal characters (Workers & Pages ▸ Overview)",
    );
}

/**
 * `D1Params::api_base`: no user, password, query or fragment (any `?` or `#`, even empty), then
 * HTTPS, or HTTP to this machine only. The base is never echoed.
 */
function checkApiBase(p: ConnectionProfile): boolean {
  const base = str(p, "apiBase");
  if (base === "") return true;
  let url: URL;
  try {
    url = new URL(base);
  } catch {
    return mockError("INVALID_INPUT", `Cloudflare D1: the API base: invalid URL`);
  }
  if (url.username !== "" || url.password !== "" || /[?#]/.test(base))
    return mockError(
      "INVALID_INPUT",
      "Cloudflare D1: the API base takes no user, password, query or fragment: put the API token in the API Token field",
    );
  const loopback = /^(localhost|127(?:\.\d+){3}|\[::1\])$/i.test(url.hostname);
  if (url.protocol === "https:") return true;
  if (url.protocol === "http:" && loopback) return false;
  return mockError(
    "INVALID_INPUT",
    "Cloudflare D1: the API base must be an https:// URL (http:// only to this machine)",
  );
}

export const D1_ACCOUNT = "0123456789abcdef0123456789abcdef";

/** A D1 profile of the mock's account. */
function d1Profile(
  p: Partial<ConnectionProfile> & { id: string; name: string },
  readOnly: boolean,
) {
  const base = profile({
    ...p,
    driver: "d1",
    host: null,
    port: null,
    user: null,
    ssl: "verify-full",
  });
  return {
    ...base,
    options: {
      ...base.options,
      tag: "production",
      readOnly,
      driverParams: { accountId: D1_ACCOUNT },
    },
  };
}

export const d1Mock: MockDriver = {
  driver: "d1",
  serverVersion: () => "Cloudflare D1 (SQLite 3.45.0)",
  seedProfiles: () => [
    d1Profile(
      { id: "p-mock-d1", name: "D1 shop", color: "orange", database: "shop", sortOrder: 112 },
      false,
    ),
    d1Profile(
      {
        id: "p-mock-d1-ro",
        name: "D1 analytics (read-only)",
        database: "analytics",
        sortOrder: 113,
      },
      true,
    ),
  ],
  connect: (p) => {
    checkAccount(p);
    const tls = checkApiBase(p);
    const wanted = (p.database ?? "").trim();
    // A name is looked up once; the session keeps the UUID.
    const db =
      D1_DATABASES.find((d) => d.uuid === wanted.toLowerCase() || d.name === wanted) ??
      mockError("NOT_FOUND", `not found: Cloudflare D1 database “${wanted}”`);
    return remoteSession("d1", database("d1", db.name), {
      readOnly: p.options.readOnly,
      tls,
      databases: () => D1_DATABASES.map((d) => ({ name: d.name, isCurrent: d.uuid === db.uuid })),
    });
  },
  // The backend also checks the account here; the mock's calls get no profile.
  util: (method) => {
    if (method !== "listDatabases")
      return mockError("UNSUPPORTED", `unsupported: Cloudflare D1: unknown call “${method}”`);
    return D1_DATABASES.map(({ uuid, name, createdAt, fileSize }) => ({
      uuid,
      name,
      createdAt,
      fileSize,
    }));
  },
};

const lane: MockLane = { drivers: [libsqlMock, d1Mock] };

export default lane;
