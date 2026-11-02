import type { ConnectionFormValues } from "@/features/connections/schema";
import { type ParsedConnectionUrl, profileUrlQuery } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { RedisProtocol } from "@/lib/ipc/bindings/RedisProtocol";
import { databaseIndex, formParams, redisForm, type RedisForm, redisFormOf } from "./params";

/**
 * Redis connection URLs, as redis-cli, redis-py and TablePlus write them:
 * `redis://[[user]:password@]host[:port][/db]` (a bare `password@`, as redis-cli reads it) in plain text, `rediss://…` over TLS (Upstash,
 * ElastiCache, Azure's 6380). A password goes to the secret, never into a field.
 *
 * Keys read: `db` (redis-py, in place of the path), `verify=full|ca|none` (the real-server suite's)
 * or redis-py's `ssl_cert_reqs=required|optional|none` for how a `rediss://` certificate is
 * checked (verified unless `none`), `protocol=2|3`, `socket_connect_timeout` (seconds), and Tabler's own `scan_count`,
 * `preview_bytes`, `preview_elements`, `memory_column=false` and `key_separator`. Writing uses the
 * same keys, then the query every connection URL carries (`profileUrlQuery`).
 */

const SCHEME = /^(rediss?):\/\//i;

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** A host that would not read back as itself: a URL delimiter or a space in it. */
const badHost = (host: string) => /[\s@/?#]/.test(host);

/**
 * How a `rediss://` URL checks the certificate; "bad" for a word this does not know. A bare
 * `rediss://` verifies, as redis-cli, redis-py and node-redis do, and so does `optional` (Python's
 * CERT_OPTIONAL checks like CERT_REQUIRED on a client); only an explicit `none` skips the check.
 */
function verifyOf(q: URLSearchParams): SslMode | "bad" {
  const word = (q.get("verify") ?? q.get("ssl_cert_reqs"))?.trim().toLowerCase();
  if (word === undefined) return "verify-full";
  switch (word) {
    case "full":
    case "required":
    case "optional":
      return "verify-full";
    case "ca":
      return "verify-ca";
    case "none":
      return "require";
    default:
      return "bad";
  }
}

const PROTOCOL: Record<string, RedisProtocol> = { "2": "resp2", "3": "resp3" };

/** A `redis://` or `rediss://` URL as form values; null when it is not one. */
export function parseRedisUrl(input: string): ParsedConnectionUrl | null {
  const text = input.trim();
  const scheme = SCHEME.exec(text)?.[1]?.toLowerCase();
  if (!scheme) return null;
  let url: URL;
  try {
    // The WHATWG parser reads the authority of a scheme it does not know too.
    url = new URL(text);
  } catch {
    return null;
  }
  const host = decode(url.hostname.replace(/^\[(.*)\]$/, "$1"));
  if (host === "" || badHost(host)) return null;
  const q = url.searchParams;
  const path = decode(url.pathname.replace(/^\/+|\/+$/g, ""));
  const database = path !== "" ? path : (q.get("db")?.trim() ?? "");
  if (databaseIndex(database) === null) return null;
  const tls = scheme === "rediss" ? verifyOf(q) : "disable";
  if (tls === "bad") return null;
  const protocolWord = q.get("protocol")?.trim();
  if (protocolWord !== undefined && !Object.hasOwn(PROTOCOL, protocolWord)) return null;
  const params: RedisForm = {
    ...redisFormOf(null),
    scanCount: q.get("scan_count")?.trim() ?? "",
    previewBytes: q.get("preview_bytes")?.trim() ?? "",
    previewElements: q.get("preview_elements")?.trim() ?? "",
    memoryColumn: q.get("memory_column")?.trim().toLowerCase() !== "false",
    keySeparator: q.get("key_separator") ?? "",
    protocol: protocolWord !== undefined ? PROTOCOL[protocolWord]! : "auto",
  };
  // redis-cli reads userinfo without a ':' as the password (the legacy `redis://password@host`),
  // so it goes to the secret and never into the plain user field.
  const authority = text.slice(scheme.length + "://".length).split(/[/?#]/, 1)[0] ?? "";
  const userinfo = authority.includes("@") ? authority.slice(0, authority.lastIndexOf("@")) : "";
  const legacy = userinfo !== "" && !userinfo.includes(":");
  const values: Partial<ConnectionFormValues> = {
    host,
    port: url.port || "6379",
    user: legacy ? "" : decode(url.username),
    database,
    ssl: tls,
    driverParams: formParams(params),
  };
  const timeout = q.get("socket_connect_timeout")?.trim();
  if (timeout !== undefined && /^\d+$/.test(timeout) && Number(timeout) > 0)
    values.connectTimeoutSecs = String(Number(timeout));
  const password = legacy ? decode(url.username) : url.password ? decode(url.password) : null;
  return { values, password: password === "" ? null : password };
}

/** The connection form's reader (`KindUi.url.parse`); a pasted URL and a link read alike. */
export const parseRedisConnectionUrl = (input: string): ParsedConnectionUrl | null =>
  parseRedisUrl(input);

const TLS: ReadonlySet<SslMode> = new Set(["require", "verify-ca", "verify-full"]);

/**
 * The URL of a profile: `rediss://` over TLS (`verify=` for every mode, `none` when unchecked), the user, host,
 * port and DB index, the params that differ from the defaults and the common query; never a
 * password. `Prefer` is plain text, as the backend reads it (Redis has no TLS negotiation).
 */
export function formatRedisUrl(p: ConnectionProfile): string | null {
  if (!p.host || badHost(p.host)) return null;
  const tls = TLS.has(p.ssl);
  const f = redisForm(p);
  const query: [string, string][] = [];
  // `none` spelled out: a bare `rediss://` reads back as verified.
  if (p.ssl === "require") query.push(["verify", "none"]);
  if (p.ssl === "verify-full") query.push(["verify", "full"]);
  if (p.ssl === "verify-ca") query.push(["verify", "ca"]);
  if (f.protocol !== "auto") query.push(["protocol", f.protocol === "resp2" ? "2" : "3"]);
  if (f.scanCount) query.push(["scan_count", f.scanCount]);
  if (f.previewBytes) query.push(["preview_bytes", f.previewBytes]);
  if (f.previewElements) query.push(["preview_elements", f.previewElements]);
  if (!f.memoryColumn) query.push(["memory_column", "false"]);
  if (f.keySeparator) query.push(["key_separator", f.keySeparator]);
  if (p.options.connectTimeoutSecs)
    query.push(["socket_connect_timeout", String(p.options.connectTimeoutSecs)]);
  const params = new URLSearchParams(query);
  for (const [k, v] of profileUrlQuery(p)) params.append(k, v);
  const search = params.toString();
  // `user:@`: a bare `user@` reads back as a password (redis-cli), so the colon keeps it a user.
  const user = p.user ? `${encodeURIComponent(p.user)}:@` : "";
  const host = p.host.includes(":") ? `[${p.host}]` : p.host;
  const port = p.port ? `:${p.port}` : "";
  const db = p.database ? `/${encodeURIComponent(p.database)}` : "";
  return `${tls ? "rediss" : "redis"}://${user}${host}${port}${db}${search ? `?${search}` : ""}`;
}
