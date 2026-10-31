import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import { ES_SCHEMES } from "./address";
import {
  AUTH_METHODS,
  AWS_CREDENTIALS,
  AWS_SERVICES,
  elasticForm,
  elasticFormOf,
  elasticParamsOf,
  formParams,
  type ElasticForm,
} from "./params";

/**
 * `elasticsearch://[user[:password]@]host[:port][/path-prefix][?…]` (or `opensearch://`): the
 * host and port of the cluster, the Basic user (a password there goes to the Keychain, never
 * into the profile), the path prefix as the URL's path, and in the query `sslmode` (or
 * `ssl=true|false`, as the real-server suite's variable spells it), the lane's params (`auth`,
 * `apiKeyId`, `cloudId`, `urls` comma-separated, `awsCredentials`, `awsProfile`, `awsRegion`,
 * `awsService`, `caFingerprint`, `showSystemIndices`, `requestTimeoutSecs`) and the shared ones
 * (`name`, `statusColor`, `env`…), which the connection URL reader takes. With a Cloud ID or
 * `urls` the host may be left out (`elasticsearch://elastic@?cloudId=…`).
 */

export const ES_DEFAULT_PORT = 9200;
/** HTTPS with the certificate checked: what a new profile and a URL without `sslmode` get. */
export const ES_DEFAULT_SSL: SslMode = "verify-full";

const SSL_MODES: readonly SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

function decode(s: string): string | null {
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

const flag = (v: string | null): boolean =>
  v !== null && ["1", "true", "yes", "on"].includes(v.toLowerCase());

function sslOf(q: URLSearchParams): SslMode | null {
  const mode = q.get("sslmode");
  if (mode !== null) return SSL_MODES.find((s) => s === mode) ?? null;
  const ssl = q.get("ssl");
  if (ssl === null) return ES_DEFAULT_SSL;
  return flag(ssl) ? "require" : "disable";
}

export function parseElasticUrl(input: string): ParsedConnectionUrl | null {
  const m =
    /^(elasticsearch|opensearch):\/\/(?:([^@/?#]*)@)?([^/?#]*)(\/[^?#]*)?(?:\?([^#]*))?$/i.exec(
      input.trim(),
    );
  if (!m) return null;
  const [, , userinfo, authority = "", path = "", query] = m;
  let user = "";
  let password: string | null = null;
  if (userinfo !== undefined) {
    const colon = userinfo.indexOf(":");
    const name = decode(colon >= 0 ? userinfo.slice(0, colon) : userinfo);
    const secret = colon >= 0 ? decode(userinfo.slice(colon + 1)) : null;
    if (name === null || (colon >= 0 && secret === null)) return null;
    user = name;
    password = secret === "" ? null : secret;
  }
  const hostPort = /^(?:\[([^\]]+)\]|([^:[\]]*))(?::(\d+))?$/.exec(authority);
  if (!hostPort) return null;
  const host = decode(hostPort[1] ?? hostPort[2] ?? "");
  const port = hostPort[3] ?? "";
  if (host === null || (port !== "" && Number(port) > 65535)) return null;
  const q = new URLSearchParams(query ?? "");
  const ssl = sslOf(q);
  if (ssl === null) return null;
  const choice = <T extends string>(
    key: string,
    list: readonly (readonly [T, string])[],
    or: T,
  ): T | null => {
    const v = q.get(key);
    if (v === null) return or;
    return list.find(([id]) => id === v)?.[0] ?? null;
  };
  const auth = choice("auth", AUTH_METHODS, "basic");
  const awsCredentials = choice("awsCredentials", AWS_CREDENTIALS, "accessKey");
  const awsService = choice("awsService", AWS_SERVICES, "");
  // An unknown choice is refused rather than read as the default.
  if (auth === null || awsCredentials === null || awsService === null) return null;
  const prefix = decode(path.replace(/\/+$/, ""));
  if (prefix === null) return null;
  const text = (key: string) => q.get(key)?.trim() ?? "";
  const form: ElasticForm = {
    ...elasticFormOf(null),
    urls: (q.get("urls") ?? "")
      .split(",")
      .map((u) => u.trim())
      .filter((u) => u !== "")
      .join("\n"),
    cloudId: text("cloudId"),
    pathPrefix: prefix,
    auth,
    apiKeyId: text("apiKeyId"),
    awsCredentials,
    awsProfile: text("awsProfile"),
    awsRegion: text("awsRegion"),
    awsService,
    caFingerprint: text("caFingerprint"),
    showSystemIndices: flag(q.get("showSystemIndices")),
    requestTimeoutSecs: text("requestTimeoutSecs"),
  };
  const named = host || form.cloudId.split(":")[0] || "";
  return {
    values: {
      host,
      port: port || (host !== "" ? String(ES_DEFAULT_PORT) : ""),
      user,
      database: "",
      ssl,
      driverParams: formParams(form),
      name: named === "" ? "Elasticsearch" : `Elasticsearch ${named}`,
    },
    password,
  };
}

/** The `elasticsearch://` URL of a profile: no secret, only the params that differ from the defaults. */
export function formatElasticUrl(p: ConnectionProfile): string | null {
  const params = elasticParamsOf(elasticForm(p));
  const q = new URLSearchParams();
  if (p.ssl !== ES_DEFAULT_SSL) q.set("sslmode", p.ssl);
  if (params.auth !== "basic") q.set("auth", params.auth);
  if (params.apiKeyId) q.set("apiKeyId", params.apiKeyId);
  if (params.cloudId) q.set("cloudId", params.cloudId);
  // A Host field that holds a whole URL (`https://es:9200/x`) cannot be an authority: it goes
  // with the URLs, which the backend reads the same way.
  const hostText = p.host ?? "";
  const hostIsUrl = /[/@]/.test(hostText);
  const urls = hostIsUrl && params.urls.length === 0 ? [hostText] : params.urls;
  if (urls.length > 0) q.set("urls", urls.join(","));
  if (params.auth === "awsSigv4" && params.awsCredentials !== "accessKey")
    q.set("awsCredentials", params.awsCredentials);
  if (params.awsProfile) q.set("awsProfile", params.awsProfile);
  if (params.awsRegion) q.set("awsRegion", params.awsRegion);
  if (params.awsService) q.set("awsService", params.awsService);
  if (params.caFingerprint) q.set("caFingerprint", params.caFingerprint);
  if (params.showSystemIndices) q.set("showSystemIndices", "true");
  if (params.requestTimeoutSecs) q.set("requestTimeoutSecs", String(params.requestTimeoutSecs));
  // A user only signs in with Basic (and is the access key id of SigV4's access key).
  const signs =
    params.auth === "basic" ||
    (params.auth === "awsSigv4" && params.awsCredentials === "accessKey");
  const user = signs && p.user ? `${encodeURIComponent(p.user)}@` : "";
  const bare = hostIsUrl ? "" : hostText;
  const host = bare.includes(":") ? `[${bare}]` : encodeURIComponent(bare);
  const port = bare !== "" && p.port ? `:${p.port}` : "";
  const prefix = params.pathPrefix
    ? params.pathPrefix
        .split("/")
        .map((s) => encodeURIComponent(s))
        .join("/")
    : "";
  const query = q.toString();
  return `${ES_SCHEMES[0]}://${user}${host}${port}${prefix}${query ? `?${query}` : ""}`;
}
