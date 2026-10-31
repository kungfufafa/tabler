import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { MssqlAuth } from "@/lib/ipc/bindings/MssqlAuth";
import { AUTH_METHODS, type MssqlForm, mssqlParams, mssqlParamsOf } from "./params";

/**
 * SQL Server connection strings: `sqlserver://` / `mssql://` URLs (also with `jdbc:` before
 * them), JDBC's `sqlserver://host\instance:port;key=value;…`, and ADO.NET's
 * `Server=tcp:host,1433;Database=…;…`. A password or token in one goes to the secret, never into
 * a field; what Tabler cannot connect with (named pipes, shared memory) is not read.
 *
 * Encryption: `encrypt=false` is the SSL mode Disable (SQL Server still encrypts the login),
 * `optional` Prefer, `true` / `mandatory` Require when `trustServerCertificate=true` and
 * verify-full otherwise, and `strict` TDS 8.0 (the `strict` param) with the same certificate rule.
 */

/** Read from a connection string, before it becomes form values. */
interface Parts {
  host: string;
  port: string;
  instance: string;
  user: string | null;
  password: string | null;
  database: string;
  /** Properties by normalized key (`normKey`), the last one winning. */
  props: Map<string, string>;
}

/** A property's key as compared: lower case, without spaces and underscores. */
const normKey = (key: string) => key.toLowerCase().replace(/[\s_]+/g, "");

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** `true` / `yes` / `1` / `sspi`, `false` / `no` / `0`; null for anything else. */
function bool(v: string | undefined): boolean | null {
  const t = v?.trim().toLowerCase();
  if (t === undefined) return null;
  if (["true", "yes", "1", "sspi"].includes(t)) return true;
  if (["false", "no", "0"].includes(t)) return false;
  return null;
}

const first = (props: Map<string, string>, ...keys: string[]): string | undefined => {
  for (const k of keys) {
    const v = props.get(k);
    if (v !== undefined) return v;
  }
  return undefined;
};

/**
 * `key=value` pairs separated by `;`: a value in `{…}` (JDBC, `}}` for `}`) or quotes (ADO.NET,
 * the quote doubled inside) may hold a `;`.
 */
function properties(text: string): Map<string, string> {
  const out = new Map<string, string>();
  let i = 0;
  while (i < text.length) {
    const eq = text.indexOf("=", i);
    const semi = text.indexOf(";", i);
    if (eq < 0 || (semi >= 0 && semi < eq)) {
      if (semi < 0) break;
      i = semi + 1;
      continue;
    }
    const key = normKey(text.slice(i, eq));
    let j = eq + 1;
    while (text[j] === " ") j++;
    let value = "";
    const open = text[j];
    const close = open === "{" ? "}" : open === '"' || open === "'" ? open : null;
    if (close) {
      j++;
      for (; j < text.length; j++) {
        if (text[j] === close) {
          if (text[j + 1] === close) {
            value += close;
            j++;
            continue;
          }
          j++;
          break;
        }
        value += text[j];
      }
      const end = text.indexOf(";", j);
      i = end < 0 ? text.length : end + 1;
    } else {
      const end = text.indexOf(";", j);
      value = text.slice(j, end < 0 ? text.length : end).trim();
      i = end < 0 ? text.length : end + 1;
    }
    if (key !== "") out.set(key, value);
  }
  return out;
}

/**
 * `host`, `host:port`, `host\instance`, `host\instance:port` or `[v6]:port` (an ADO.NET server
 * also `host,port`); null when it names no host.
 */
function hostPart(text: string): { host: string; port: string; instance: string } | null {
  const t = text.trim();
  const v6 = /^\[([^\]]+)\](?:[:,](\d*))?$/.exec(t);
  if (v6) return { host: v6[1]!, port: v6[2] ?? "", instance: "" };
  const m = /^([^\\:,]*)(?:\\([^:,]*))?(?:[:,](\d*))?$/.exec(t);
  if (!m) return null;
  const host = decode(m[1]!.trim());
  if (host === "") return null;
  return { host, port: m[3] ?? "", instance: decode(m[2]?.trim() ?? "") };
}

/** `sqlserver://…` or `mssql://…`, URL or JDBC form; null when it is not one. */
function urlParts(input: string): Parts | null {
  const text = input.trim().replace(/^jdbc:/i, "");
  const m = /^(?:sqlserver|mssql):\/\/(.*)$/is.exec(text);
  if (!m) return null;
  const rest = m[1]!;
  // The authority ends at the first `/`, `?`, `;` or `#` (outside an IPv6 address).
  const end = rest.search(/[/?;#]/);
  const authority = end < 0 ? rest : rest.slice(0, end);
  const tail = end < 0 ? "" : rest.slice(end);
  const at = authority.lastIndexOf("@");
  const userinfo = at < 0 ? null : authority.slice(0, at);
  const colon = userinfo?.indexOf(":") ?? -1;
  const user = userinfo === null ? null : decode(colon < 0 ? userinfo : userinfo.slice(0, colon));
  const password = userinfo === null || colon < 0 ? null : decode(userinfo.slice(colon + 1));
  const props = new Map<string, string>();
  let database = "";
  if (tail.startsWith(";")) {
    for (const [k, v] of properties(tail.slice(1))) props.set(k, v);
  } else {
    const q = tail.indexOf("?");
    const path = (q < 0 ? tail : tail.slice(0, q)).split("#")[0]!;
    database = decode(path.replace(/^\/+|\/+$/g, ""));
    if (q >= 0)
      for (const [k, v] of new URLSearchParams(tail.slice(q + 1).split("#")[0]))
        props.set(normKey(k), v);
  }
  const server = authority.slice(at + 1);
  // JDBC may name the server in a property instead: `sqlserver://;serverName=db;…`.
  const named = server === "" ? first(props, "servername", "server") : undefined;
  const where = hostPart(named ?? server);
  if (!where) return null;
  return { ...where, user, password, database, props };
}

/** ADO.NET's `Server=…;Database=…;…`; null when it names no TCP server. */
function adoParts(input: string): Parts | null {
  const props = properties(input.trim());
  let server = first(props, "server", "datasource", "address", "addr", "networkaddress");
  if (server === undefined) return null;
  // `tcp:` is the only protocol Tabler speaks; `np:` and `lpc:` (pipes, shared memory) are not.
  const proto = /^\s*(\w+):(?!\d)/.exec(server);
  if (proto) {
    if (proto[1]!.toLowerCase() !== "tcp") return null;
    server = server.slice(proto[0].length);
  }
  // `.` and `(local)` are this machine.
  server = server.replace(/^\s*(\.|\(local\))(?=\s*(\\|,|$))/i, "localhost");
  const where = hostPart(server);
  if (!where) return null;
  return {
    ...where,
    user: null,
    password: null,
    database: "",
    props,
  };
}

/** The SSL mode and strict flag of `encrypt` and `trustServerCertificate`. */
function encryptionOf(props: Map<string, string>): { ssl: SslMode; strict: boolean } {
  const encrypt = first(props, "encrypt")?.trim().toLowerCase();
  const trust = bool(first(props, "trustservercertificate")) === true;
  const checked: SslMode = trust ? "require" : "verify-full";
  if (encrypt === "strict") return { ssl: checked, strict: true };
  if (encrypt === "optional") return { ssl: "prefer", strict: false };
  if (encrypt === "mandatory") return { ssl: checked, strict: false };
  return { ssl: bool(encrypt) === false ? "disable" : checked, strict: false };
}

/** The login a connection string asks for. */
function authOf(props: Map<string, string>, user: string): MssqlAuth {
  const own = first(props, "auth")?.trim();
  const known = AUTH_METHODS.find(([a]) => a.toLowerCase() === own?.toLowerCase());
  if (known) return known[0];
  if (first(props, "accesstoken") !== undefined) return "entraToken";
  const integrated = bool(first(props, "integratedsecurity", "trustedconnection")) === true;
  // JDBC's NTLM: integrated security with a user and password of the domain.
  if (first(props, "authenticationscheme")?.trim().toLowerCase() === "ntlm")
    return user !== "" ? "windows" : "integrated";
  return integrated ? "integrated" : "sqlServer";
}

/** Whether a string may choose the Windows user's own login (see `parseSqlServerUrl`). */
type Integrated = "keep" | "drop";

/** The form values of what a connection string says. */
function toForm(parts: Parts, integrated: Integrated): ParsedConnectionUrl {
  const read = formOf(parts);
  const params = mssqlParamsOf(read.values.driverParams);
  if (integrated === "keep" || params.auth !== "integrated") return read;
  // A SQL Server login instead: the form then asks for the user, and Integrated stays the
  // user's own choice in it.
  return { ...read, values: { ...read.values, driverParams: { ...params, auth: "sqlServer" } } };
}

/** The form values a connection string names, its login as written. */
function formOf(parts: Parts): ParsedConnectionUrl {
  const { props } = parts;
  const user = (parts.user ?? first(props, "user", "username", "userid", "uid") ?? "").trim();
  const password =
    first(props, "accesstoken") ?? parts.password ?? first(props, "password", "pwd") ?? null;
  const auth = authOf(props, user);
  const { ssl, strict } = encryptionOf(props);
  const port = parts.port || first(props, "portnumber", "port")?.trim() || "";
  const database =
    parts.database || first(props, "databasename", "database", "initialcatalog")?.trim() || "";
  const size = first(props, "packetsize")?.trim() ?? "";
  const timeout = first(props, "logintimeout", "connecttimeout", "connectiontimeout", "timeout");
  const intent = first(props, "applicationintent")?.trim().toLowerCase();
  const params: MssqlForm = {
    auth,
    instance: (parts.instance || first(props, "instancename", "instance")?.trim()) ?? "",
    strict,
    hostnameInCertificate: first(props, "hostnameincertificate")?.trim() ?? "",
    readOnlyIntent: intent === "readonly",
    applicationName: first(props, "applicationname", "app")?.trim() ?? "",
    multiSubnetFailover: bool(first(props, "multisubnetfailover")) === true,
    packetSize: /^\d+$/.test(size) ? size : "",
  };
  const values: Partial<ConnectionFormValues> = {
    host: parts.host,
    port: /^\d+$/.test(port) ? port : "",
    user: auth === "integrated" || auth === "entraToken" ? "" : user,
    database,
    ssl,
    driverParams: { ...params },
  };
  if (timeout !== undefined && /^\s*\d+\s*$/.test(timeout) && Number(timeout) > 0)
    values.connectTimeoutSecs = String(Number(timeout));
  // A token comes from a command usually; a URL's is kept as the secret like a password.
  return { values, password: password === "" ? null : password };
}

/**
 * A `sqlserver://` / `mssql://` URL or JDBC string as form values; null when it is not one.
 * This is the kind's `url.parse`, which a deep link reaches and connects with unasked: a URL
 * never picks Integrated (the Windows user running Tabler), or a clicked link would sign that
 * user in to a server the link names. It reads as a SQL Server login instead.
 */
export function parseSqlServerUrl(input: string): ParsedConnectionUrl | null {
  const parts = urlParts(input);
  return parts ? toForm(parts, "drop") : null;
}

/** An ADO.NET connection string as form values; null when it names no TCP server. */
export function parseAdoString(input: string): ParsedConnectionUrl | null {
  const parts = adoParts(input);
  return parts ? toForm(parts, "keep") : null;
}

/**
 * Any of the three forms, for the Import box of the open form, where the user sees the login it
 * picks: a URL or JDBC string by its scheme (Integrated kept), else ADO.NET.
 */
export function parseConnectionString(input: string): ParsedConnectionUrl | null {
  if (!/^\s*(jdbc:)?(sqlserver|mssql):/i.test(input)) return parseAdoString(input);
  const parts = urlParts(input);
  return parts ? toForm(parts, "keep") : null;
}

/** IPv6 addresses go in brackets. */
const hostForUrl = (host: string) => (host.includes(":") ? `[${host}]` : host);

/** `encrypt` and `trustServerCertificate` of an SSL mode (see the module's comment). */
function encryptionParams(ssl: SslMode, strict: boolean): [string, string][] {
  const trust: [string, string][] =
    ssl === "verify-ca" || ssl === "verify-full" ? [] : [["trustServerCertificate", "true"]];
  if (strict) return [["encrypt", "strict"], ...trust];
  if (ssl === "disable") return [["encrypt", "false"]];
  if (ssl === "prefer") return [["encrypt", "optional"]];
  return [["encrypt", "true"], ...trust];
}

/**
 * The `sqlserver://` URL of a profile: `user@host:port/database` with the encryption and the
 * params that differ from the defaults; never a password.
 */
export function formatSqlServerUrl(p: ConnectionProfile): string | null {
  if (!p.host) return null;
  const f = mssqlParams(p);
  const user = p.user && (f.auth === "sqlServer" || f.auth === "windows") ? p.user : "";
  const query: [string, string][] = encryptionParams(p.ssl, f.strict);
  if (f.auth !== "sqlServer") query.push(["auth", f.auth]);
  if (f.instance) query.push(["instance", f.instance]);
  if (f.hostnameInCertificate) query.push(["hostNameInCertificate", f.hostnameInCertificate]);
  if (f.readOnlyIntent) query.push(["applicationIntent", "ReadOnly"]);
  if (f.applicationName) query.push(["applicationName", f.applicationName]);
  if (f.multiSubnetFailover) query.push(["multiSubnetFailover", "true"]);
  if (f.packetSize) query.push(["packetSize", f.packetSize]);
  if (p.options.connectTimeoutSecs)
    query.push(["loginTimeout", String(p.options.connectTimeoutSecs)]);
  const authority = `${user ? `${encodeURIComponent(user)}@` : ""}${hostForUrl(p.host)}${p.port ? `:${p.port}` : ""}`;
  const db = p.database ? `/${encodeURIComponent(p.database)}` : "";
  return `sqlserver://${authority}${db}?${new URLSearchParams(query).toString()}`;
}
