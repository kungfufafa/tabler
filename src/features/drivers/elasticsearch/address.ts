/**
 * Where a profile's cluster is, read as the backend reads it (`transport.rs` `BaseUrl::parse`,
 * `params.rs` `check_prefix` and `check_region`, `auth.rs` `cloud_id_url` and
 * `region_for_host`, `tls_client::parse_pin`), so the form says before connecting what the
 * connect would refuse. Each check returns the reason, or null when it passes.
 */

export interface BaseUrl {
  https: boolean;
  host: string;
  port: number;
  /** The path prefix without a trailing `/` ("" when none). */
  prefix: string;
}

/** The connection URL schemes (`URL_SCHEMES`): TLS then comes from the SSL mode. */
export const ES_SCHEMES = ["elasticsearch", "opensearch"] as const;

const isLocal = (host: string) => host === "localhost" || host === "127.0.0.1" || host === "::1";

/** Why a path prefix is refused (`check_prefix`), or null. */
export function prefixProblem(prefix: string): string | null {
  const bad = (why: string) => `The path prefix “${prefix}” ${why}`;
  if (!prefix.startsWith("/")) return bad("must start with /");
  if ([...prefix].some((c) => /[\s?#\\%]/u.test(c) || /\p{Cc}/u.test(c)))
    return bad("may hold no spaces, ?, #, % or \\");
  if (prefix.split("/").some((s) => s === "." || s === ".." || s.startsWith("_")))
    return bad("may hold no . or .. segment, nor one starting with _");
  return null;
}

/** A node URL or host as the backend reads it, or the reason it is refused. */
export function parseBaseUrl(text: string, https: boolean): BaseUrl | { error: string } {
  const t = text.trim();
  const bad = (why: string) => ({ error: `${t}: ${why}` });
  let rest = t;
  let tls = https;
  let explicitHttps = false;
  const sep = t.indexOf("://");
  if (sep >= 0) {
    const scheme = t.slice(0, sep).toLowerCase();
    rest = t.slice(sep + 3);
    if (scheme === "https") [tls, explicitHttps] = [true, true];
    else if (scheme === "http") tls = false;
    else if (!(ES_SCHEMES as readonly string[]).includes(scheme))
      return bad("use http://, https://, elasticsearch:// or opensearch://");
  }
  if (/[?#]/.test(rest)) return bad("a query or fragment is not allowed");
  const slash = rest.indexOf("/");
  const authority = slash < 0 ? rest : rest.slice(0, slash);
  const path = slash < 0 ? "" : rest.slice(slash);
  if (authority.includes("@")) return bad("put the user and password in their own fields");
  let host: string;
  let port: string | null;
  if (authority.startsWith("[")) {
    const close = authority.indexOf("]");
    if (close < 0) return bad("unclosed IPv6 bracket");
    host = authority.slice(1, close);
    const after = authority.slice(close + 1);
    if (after === "") port = null;
    else if (after.startsWith(":")) port = after.slice(1);
    else return bad("the port is not a number");
  } else {
    const colon = authority.lastIndexOf(":");
    host = colon < 0 ? authority : authority.slice(0, colon);
    port = colon < 0 ? null : authority.slice(colon + 1);
  }
  if (host === "") return bad("no host");
  if (/[\s\\%?#@]/u.test(host) || /\p{Cc}/u.test(host))
    return bad("the host holds characters a host name cannot");
  let portNumber: number;
  if (port !== null) {
    if (!/^\+?\d+$/.test(port) || Number(port) > 65535) return bad("the port is not a number");
    portNumber = Number(port);
  } else portNumber = explicitHttps && !isLocal(host) ? 443 : 9200;
  const prefix = path.replace(/\/+$/, "");
  if (prefix !== "") {
    const problem = prefixProblem(prefix);
    if (problem) return { error: problem };
  }
  return { https: tls, host, port: portNumber, prefix };
}

/** The cluster an Elastic Cloud ID names (`cloud_id_url`), or null when it is not one. */
export function cloudIdUrl(cloudId: string): BaseUrl | null {
  const t = cloudId.trim();
  const colon = t.indexOf(":");
  const encoded = colon < 0 ? t : t.slice(colon + 1);
  let decoded: string;
  try {
    if (!/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)) return null;
    const bytes = Uint8Array.from(atob(encoded), (c) => c.charCodeAt(0));
    decoded = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
  const parts = decoded.split("$");
  if (parts.length < 2) return null;
  const split = (s: string): [string, string | null] => {
    const i = s.lastIndexOf(":");
    return i < 0 ? [s, null] : [s.slice(0, i), s.slice(i + 1)];
  };
  const [host, hostPort] = split(parts[0]!);
  const [es, esPort] = split(parts[1]!);
  const labelOk = (s: string) => /^[A-Za-z0-9.-]+$/.test(s);
  if (!labelOk(host) || !labelOk(es)) return null;
  const port = esPort ?? hostPort;
  if (port !== null && !/^\+?\d+$/.test(port)) return null;
  if (port !== null && Number(port) > 65535) return null;
  return {
    https: true,
    host: `${es}.${host}`,
    port: port === null ? 443 : Number(port),
    prefix: "",
  };
}

/** The base URL as text (`BaseUrl::root`). */
export function rootOf(b: BaseUrl): string {
  const host = b.host.includes(":") ? `[${b.host}]` : b.host;
  return `${b.https ? "https" : "http"}://${host}:${b.port}${b.prefix}`;
}

/** The region of an Amazon OpenSearch endpoint host (`region_for_host`). */
export function regionForHost(host: string): string | null {
  const parts = host.split(".");
  const i = parts.findIndex((p) => p === "es" || p === "aoss");
  if (i < 1 || parts[i + 1] !== "amazonaws") return null;
  return parts[i - 1] ?? null;
}

/** OpenSearch Serverless's hosts (`AwsService::for_host`). */
export const isServerlessHost = (host: string): boolean => host.includes(".aoss.");

/** Why a region name is refused (`check_region`), or null. */
export function regionProblem(region: string): string | null {
  const r = region.trim();
  return r !== "" && r.length <= 64 && /^[a-z0-9-]+$/.test(r) && !/^-|-$/.test(r)
    ? null
    : `“${region}” is not an AWS region name (like us-east-1)`;
}

/** Why a CA fingerprint is refused (`parse_pin`: 64 hex digits, `:` and spaces ignored). */
export function pinProblem(text: string): string | null {
  return /^[0-9A-Fa-f]{64}$/.test(text.replace(/[: \t]/g, ""))
    ? null
    : "The certificate fingerprint must be a SHA-256 in hex (64 digits)";
}
