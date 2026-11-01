import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import {
  type CassandraForm,
  cassandraParams,
  cassandraParamsOf,
  inRange,
  PAGE_SIZE,
  REQUEST_TIMEOUT,
} from "./params";

/**
 * `cassandra://user:password@host:9042/keyspace?…`, TablePlus's scheme: the contact point, the
 * user of PasswordAuthenticator (its password goes to the secret, never into a field), the
 * keyspace, TLS as `sslmode` (or `ssl=true`, which is Require), and the lane's settings by their
 * `CassandraParams` names (`consistency=LOCAL_QUORUM`, `localDatacenter=dc1`, …), also in the
 * spellings other tools use (`localDC`, `fetchSize`, `requestTimeout`).
 */

const SSL_MODES: Record<string, SslMode> = {
  disable: "disable",
  disabled: "disable",
  // CQL has no negotiation: a node takes TLS on its port or not at all.
  prefer: "disable",
  allow: "disable",
  require: "require",
  required: "require",
  "verify-ca": "verify-ca",
  verify_ca: "verify-ca",
  "verify-full": "verify-full",
  verify_full: "verify-full",
  "verify-identity": "verify-full",
};

/** A mode's name as its SSL mode; null for any other word (`constructor` included). */
const modeOf = (name: string): SslMode | null =>
  Object.hasOwn(SSL_MODES, name) ? SSL_MODES[name]! : null;

/** A query key as compared: lower case, without `_` and `-`. */
const normKey = (key: string) => key.toLowerCase().replace(/[_-]/g, "");

/** Each setting's keys, normalized. */
const KEYS = {
  consistency: ["consistency", "consistencylevel"],
  serialConsistency: ["serialconsistency", "serialconsistencylevel"],
  localDatacenter: ["localdatacenter", "localdc", "datacenter", "dc"],
  requestTimeoutSecs: ["requesttimeoutsecs", "requesttimeout", "readtimeout"],
  pageSize: ["pagesize", "fetchsize"],
  compression: ["compression"],
  singleNode: ["singlenode"],
  connectTimeout: ["connecttimeout", "connecttimeoutsecs"],
  sslmode: ["sslmode", "tlsmode"],
  ssl: ["ssl", "tls"],
} as const;

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

/**
 * The SSL mode a URL asks for: `sslmode`, else `ssl` as a flag (on is Require) or a mode's
 * name, else none. A value of neither kind is no URL: reading it as off would send the password
 * in the clear to a node the user meant to reach over TLS.
 */
function sslOf(get: (keys: readonly string[]) => string | undefined): SslMode | null {
  const mode = get(KEYS.sslmode)?.trim().toLowerCase();
  if (mode !== undefined) return modeOf(mode);
  const ssl = get(KEYS.ssl);
  if (ssl === undefined) return "disable";
  const on = flag(ssl);
  if (on !== null) return on ? "require" : "disable";
  return modeOf(ssl.trim().toLowerCase());
}

/** A contact point that cannot be written back into a URL, or is no host name. */
const badHost = (host: string) => /[\s@/?#]/.test(host);

/** A `cassandra://` URL as form values; null when it is not one. */
export function parseCassandraUrl(input: string): ParsedConnectionUrl | null {
  const m = /^cassandra:\/\/([^/?#]*)([^?#]*)(?:\?([^#]*))?/i.exec(input.trim());
  if (!m) return null;
  const authority = m[1]!;
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
  const keyspace = decode(m[2]!.replace(/^\/+|\/+$/g, ""));
  if (keyspace === null || keyspace.includes("/")) return null;
  const query = new Map<string, string>();
  for (const [k, v] of new URLSearchParams(m[3] ?? "")) query.set(normKey(k), v);
  const get = (keys: readonly string[]) => {
    for (const k of keys) {
      const v = query.get(k);
      if (v !== undefined) return v;
    }
    return undefined;
  };
  const ssl = sslOf(get);
  if (ssl === null) return null;
  const read = cassandraParamsOf({
    consistency: get(KEYS.consistency)?.trim().toUpperCase(),
    serialConsistency: get(KEYS.serialConsistency)?.trim().toUpperCase(),
    localDatacenter: get(KEYS.localDatacenter),
    compression: get(KEYS.compression)?.trim().toLowerCase(),
  });
  const timeout = get(KEYS.requestTimeoutSecs)?.trim() ?? "";
  const size = get(KEYS.pageSize)?.trim() ?? "";
  const params: CassandraForm = {
    ...read,
    requestTimeoutSecs: inRange(timeout, REQUEST_TIMEOUT) === null ? "" : timeout,
    pageSize: inRange(size, PAGE_SIZE) === null ? "" : size,
    singleNode: flag(get(KEYS.singleNode)) === true,
  };
  const values: Partial<ConnectionFormValues> = {
    host,
    port: where?.[2] ?? "",
    user,
    database: keyspace,
    ssl,
    driverParams: { ...params },
  };
  const connect = get(KEYS.connectTimeout)?.trim();
  if (connect !== undefined && /^\d+$/.test(connect) && Number(connect) > 0)
    values.connectTimeoutSecs = String(Number(connect));
  return { values, password: password === "" ? null : password };
}

/**
 * The `cassandra://` URL of a profile: `user@host:port/keyspace` with the SSL mode and the
 * settings that differ from the defaults; never a password.
 */
export function formatCassandraUrl(p: ConnectionProfile): string | null {
  if (!p.host || badHost(p.host)) return null;
  const f = cassandraParams(p);
  const query: [string, string][] = [];
  if (p.ssl !== "disable" && p.ssl !== "prefer") query.push(["sslmode", p.ssl]);
  if (f.consistency !== "LOCAL_ONE") query.push(["consistency", f.consistency]);
  if (f.serialConsistency !== "LOCAL_SERIAL")
    query.push(["serialConsistency", f.serialConsistency]);
  if (f.localDatacenter) query.push(["localDatacenter", f.localDatacenter]);
  if (f.requestTimeoutSecs) query.push(["requestTimeout", f.requestTimeoutSecs]);
  if (f.pageSize) query.push(["pageSize", f.pageSize]);
  if (f.compression !== "none") query.push(["compression", f.compression]);
  if (f.singleNode) query.push(["singleNode", "true"]);
  if (p.options.connectTimeoutSecs)
    query.push(["connectTimeout", String(p.options.connectTimeoutSecs)]);
  const host = p.host.includes(":") ? `[${p.host}]` : p.host;
  const user = p.user ? `${encodeURIComponent(p.user)}@` : "";
  const port = p.port ? `:${p.port}` : "";
  const keyspace = p.database ? `/${encodeURIComponent(p.database)}` : "";
  const q = new URLSearchParams(query).toString();
  return `cassandra://${user}${host}${port}${keyspace}${q ? `?${q}` : ""}`;
}
