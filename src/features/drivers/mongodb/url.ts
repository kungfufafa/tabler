import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { MongoAuth } from "@/lib/ipc/bindings/MongoAuth";
import {
  formParams,
  MECHANISM_NAME,
  mongoForm,
  mongoFormOf,
  READ_PREFERENCES,
  type MongoForm,
} from "./params";

/**
 * MongoDB connection strings (`mongodb://` and `mongodb+srv://`), read as the backend's
 * `UriParts` reads them. A URL the form's fields can hold (one host, no SRV, only the options the
 * form has) is spread over them; any other is kept whole as the profile's `uri`, without its
 * password (which goes to the secret: the URL is saved in plain text) and without the keys that
 * are Tabler's own (name, colour, Safe Mode…).
 */

export const MONGO_DEFAULT_PORT = 27017;

export interface MongoUrl {
  srv: boolean;
  /** `host[:port]` as written. */
  hosts: string[];
  user: string | null;
  password: string | null;
  database: string | null;
  /** The options in order, names as written and values decoded. */
  options: [string, string][];
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** Reads a connection string; null when it is not one (`UriParts::parse`'s refusals). */
export function parseMongoUrl(input: string): MongoUrl | null {
  const text = input.trim();
  const sep = text.indexOf("://");
  if (sep < 0) return null;
  const scheme = text.slice(0, sep).toLowerCase();
  if (scheme !== "mongodb" && scheme !== "mongodb+srv") return null;
  const srv = scheme === "mongodb+srv";
  const rest = text.slice(sep + 3);
  const end = rest.search(/[/?]/);
  const authority = end < 0 ? rest : rest.slice(0, end);
  const tail = end < 0 ? "" : rest.slice(end);
  const at = authority.lastIndexOf("@");
  const userinfo = at >= 0 ? authority.slice(0, at) : null;
  const hosts = (at >= 0 ? authority.slice(at + 1) : authority)
    .split(",")
    .map((h) => h.trim())
    .filter((h) => h !== "");
  if (hosts.length === 0) return null;
  if (srv && (hosts.length > 1 || (hosts[0]!.includes(":") && !hosts[0]!.startsWith("["))))
    return null;
  let user: string | null = null;
  let password: string | null = null;
  if (userinfo !== null) {
    const colon = userinfo.indexOf(":");
    user = decode(colon >= 0 ? userinfo.slice(0, colon) : userinfo) || null;
    password = colon >= 0 ? decode(userinfo.slice(colon + 1)) : null;
  }
  const q = tail.indexOf("?");
  const path = q >= 0 ? tail.slice(0, q) : tail;
  const query = q >= 0 ? tail.slice(q + 1) : "";
  // `u:12/x@h`, `me@corp.com:12?x@h`: a raw `/` or `?` in a password ends the authority early;
  // its pieces would land in the host, port, database or option fields. With or without a user
  // part, a raw `@` after the authority is refused (`%40`).
  if (tail.includes("@")) return null;
  const options = query
    .split(/[&;]/)
    .filter((kv) => kv !== "")
    .map((kv): [string, string] => {
      const eq = kv.indexOf("=");
      return eq < 0 ? [decode(kv), ""] : [decode(kv.slice(0, eq)), decode(kv.slice(eq + 1))];
    });
  const database = decode(path.replace(/^\/+/, "")) || null;
  return { srv, hosts, user, password, database, options };
}

/** The value of option `name` (the last one given; names are case-insensitive). */
export function optionOf(url: MongoUrl, name: string): string | null {
  const found = url.options.filter(([k]) => k.toLowerCase() === name.toLowerCase());
  return found.length > 0 ? found[found.length - 1]![1] : null;
}

/** Whether the string turns TLS on (`tls` or `ssl`; on by default for SRV). */
export function urlTls(url: MongoUrl): boolean {
  const v = optionOf(url, "tls") ?? optionOf(url, "ssl");
  return v === null ? url.srv : v.toLowerCase() === "true";
}

/** The SSL mode a URL's TLS options stand for. */
function sslOfUrl(url: MongoUrl): SslMode {
  if (!urlTls(url)) return "disable";
  const insecure = ["tlsAllowInvalidCertificates", "tlsInsecure"].some(
    (k) => optionOf(url, k)?.toLowerCase() === "true",
  );
  return insecure ? "require" : "verify-full";
}

/** `host[:port]` as host and port (null: the default; NaN: not a port). */
function hostPort(item: string): { host: string; port: number | null } {
  const v6 = /^\[([^\]]*)\](?::(.*))?$/.exec(item);
  const [host, port] = v6
    ? [v6[1]!, v6[2]]
    : item.includes(":")
      ? [item.slice(0, item.lastIndexOf(":")), item.slice(item.lastIndexOf(":") + 1)]
      : [item, undefined];
  if (port === undefined) return { host, port: null };
  return { host, port: /^\d{1,5}$/.test(port) ? Number(port) : NaN };
}

/** IPv6 addresses go in brackets. */
const hostForUrl = (host: string) => (host.includes(":") ? `[${host}]` : host);

/** Query keys a URL means for Tabler itself (`connections/url.ts` reads them), lower case. */
const TABLER_KEYS = new Set([
  "name",
  "statuscolor",
  "color",
  "env",
  "enviroment",
  "tag",
  "safemodelevel",
  "advancedsafemodelevel",
  "useprivatekey",
  "lazyload",
  "windowmode",
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

/** The options the form has fields for (lower case). */
const FORM_KEYS = new Set([
  "authsource",
  "authmechanism",
  "replicaset",
  "readpreference",
  "directconnection",
  "appname",
  "serverselectiontimeoutms",
  "connecttimeoutms",
  "tls",
  "ssl",
  "tlsallowinvalidcertificates",
  "tlsinsecure",
  "tlscafile",
  "tlscertificatekeyfile",
]);

const mechanismOf = (name: string): MongoAuth | null =>
  (Object.entries(MECHANISM_NAME).find(([, n]) => n === name.toUpperCase())?.[0] as
    MongoAuth | undefined) ?? null;

/** Milliseconds as whole seconds ("" when they are not). */
const secondsOf = (ms: string | null): string =>
  ms !== null && /^\d+$/.test(ms) && Number(ms) % 1000 === 0 && Number(ms) > 0
    ? String(Number(ms) / 1000)
    : "";

/** Whether every option of the URL has a field it fits in. */
function fitsTheForm(url: MongoUrl): boolean {
  if (url.srv || url.hosts.length !== 1) return false;
  return url.options.every(([k, v]) => {
    switch (k.toLowerCase()) {
      case "authmechanism":
        return mechanismOf(v) !== null;
      case "readpreference":
        return READ_PREFERENCES.some(([id]) => id === v);
      case "directconnection":
      case "tls":
      case "ssl":
      case "tlsallowinvalidcertificates":
      case "tlsinsecure":
        return v === "true" || v === "false";
      case "serverselectiontimeoutms":
      case "connecttimeoutms":
        return secondsOf(v) !== "";
      default:
        return FORM_KEYS.has(k.toLowerCase());
    }
  });
}

/** The URL without its credentials and Tabler's own keys, as the profile keeps it. */
function storedUri(url: MongoUrl): string {
  const scheme = url.srv ? "mongodb+srv" : "mongodb";
  const options = url.options.filter(([k]) => !TABLER_KEYS.has(k.toLowerCase()));
  return buildUrl(scheme, null, url.hosts, url.database, options);
}

function buildUrl(
  scheme: string,
  user: string | null,
  hosts: readonly string[],
  database: string | null,
  options: readonly (readonly [string, string])[],
): string {
  const auth = user ? `${encodeURIComponent(user)}@` : "";
  const path = database ? `/${encodeURIComponent(database)}` : options.length > 0 ? "/" : "";
  const query = options
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join("&");
  return `${scheme}://${auth}${hosts.join(",")}${path}${query ? `?${query}` : ""}`;
}

/** Reads a pasted URL for the connection form (`KindUi.url.parse`); null when it is not one. */
export function parseMongoConnectionUrl(text: string): ParsedConnectionUrl | null {
  const url = parseMongoUrl(text);
  if (!url) return null;
  const own = { ...url, options: url.options.filter(([k]) => !TABLER_KEYS.has(k.toLowerCase())) };
  const first = hostPort(own.hosts[0]!);
  if (Number.isNaN(first.port)) return null;
  const values: Partial<ConnectionFormValues> = {
    host: first.host,
    port: String(first.port ?? MONGO_DEFAULT_PORT),
    user: own.user ?? "",
    database: own.database ?? "",
    ssl: sslOfUrl(own),
  };
  const form: MongoForm = mongoFormOf(null);
  if (!fitsTheForm(own)) {
    form.uri = storedUri(own);
  } else {
    const opt = (k: string) => optionOf(own, k);
    form.authSource = opt("authSource") ?? "";
    form.replicaSet = opt("replicaSet") ?? "";
    form.appName = opt("appName") ?? "";
    form.authMechanism = mechanismOf(opt("authMechanism") ?? "") ?? "default";
    form.readPreference =
      READ_PREFERENCES.find(([id]) => id === opt("readPreference"))?.[0] ?? "primary";
    const direct = opt("directConnection");
    form.directConnection = direct === "true" ? "on" : direct === "false" ? "off" : "";
    form.serverSelectionTimeoutSecs = secondsOf(opt("serverSelectionTimeoutMS"));
    const connect = secondsOf(opt("connectTimeoutMS"));
    if (connect !== "") values.connectTimeoutSecs = connect;
    const ca = opt("tlsCAFile");
    if (ca) values.sslCaPath = ca;
    const pem = opt("tlsCertificateKeyFile");
    // One PEM file holds the certificate and its key.
    if (pem) {
      values.sslCertPath = pem;
      values.sslKeyPath = pem;
    }
  }
  values.driverParams = formParams(form);
  return { values, password: own.password };
}

/** The TLS options that say an SSL mode (`prefer`: none, the URL decides). */
function tlsOptions(ssl: SslMode): [string, string][] {
  switch (ssl) {
    case "disable":
      return [["tls", "false"]];
    case "require":
      return [
        ["tls", "true"],
        ["tlsAllowInvalidCertificates", "true"],
      ];
    case "verify-ca":
    case "verify-full":
      return [["tls", "true"]];
    case "prefer":
      return [];
  }
}

const TLS_KEYS = ["tls", "ssl", "tlsallowinvalidcertificates", "tlsinsecure"];

/** A profile as a connection string, without its password (`KindUi.url.format`). */
export function formatMongoUrl(p: ConnectionProfile): string | null {
  const f = mongoForm(p);
  if (f.uri !== "") {
    const url = parseMongoUrl(f.uri);
    if (!url) return f.uri;
    let options = url.options;
    // The form's SSL mode, where the URL does not already say it.
    if (p.ssl !== "prefer" && sslOfUrl(url) !== (p.ssl === "verify-ca" ? "verify-full" : p.ssl))
      options = [
        ...options.filter(([k]) => !TLS_KEYS.includes(k.toLowerCase())),
        ...tlsOptions(p.ssl),
      ];
    return buildUrl(
      url.srv ? "mongodb+srv" : "mongodb",
      url.user ?? p.user,
      url.hosts,
      url.database ?? p.database,
      options,
    );
  }
  const host = hostForUrl(p.host?.trim() || "127.0.0.1");
  const options: [string, string][] = [];
  const add = (k: string, v: string) => {
    if (v !== "") options.push([k, v]);
  };
  add("authSource", f.authSource);
  add("authMechanism", MECHANISM_NAME[f.authMechanism] ?? "");
  add("replicaSet", f.replicaSet);
  if (f.readPreference !== "primary") add("readPreference", f.readPreference);
  if (f.directConnection !== "")
    add("directConnection", f.directConnection === "on" ? "true" : "false");
  add("appName", f.appName);
  if (/^\d+$/.test(f.serverSelectionTimeoutSecs))
    add("serverSelectionTimeoutMS", String(Number(f.serverSelectionTimeoutSecs) * 1000));
  const connect = p.options.connectTimeoutSecs;
  if (connect) add("connectTimeoutMS", String(connect * 1000));
  if (p.ssl !== "disable" && p.ssl !== "prefer") {
    options.push(...tlsOptions(p.ssl));
    add("tlsCAFile", p.sslCaPath ?? "");
    add("tlsCertificateKeyFile", p.options.sslCertPath ?? p.options.sslKeyPath ?? "");
  }
  return buildUrl(
    "mongodb",
    p.user,
    [`${host}:${p.port ?? MONGO_DEFAULT_PORT}`],
    p.database,
    options,
  );
}
