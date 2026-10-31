import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import {
  authOf,
  DEFAULT_PAGE_REQUESTS,
  DEFAULT_SAMPLE_SIZE,
  dynamoForm,
  dynamoFormOf,
  dynamoParamsOf,
  formParams,
  type DynamoForm,
} from "./params";

/**
 * `dynamodb://[accessKeyId[:secret]@]region[?auth=…&profile=…&endpoint=…&…]`: the region as the
 * authority, the access key ID as its user (a secret there goes to the Keychain, never into the
 * profile), and the lane's params in the query: `auth` (accessKey, profile, environment,
 * command, local), `profile`, `endpoint`, `consistentReads`, `sampleSize`, `pageRequests`,
 * `consumedCapacity`, and `sslmode` for an endpoint of its own. Other query parameters are the
 * shared ones (`name`, `statusColor`, `env`…), which the connection URL reader takes.
 */

/**
 * A URL's `sslmode` as one of the form's two (`DYNAMO_SSL_MODES`): an endpoint of its own is
 * always HTTPS, so `disable` and `prefer` are `require` (no certificate check, as the backend
 * reads them anyway) and `verify-ca` is `verify-full`.
 */
const SSL_MODES: ReadonlyMap<string, SslMode> = new Map<string, SslMode>([
  ["disable", "require"],
  ["prefer", "require"],
  ["require", "require"],
  ["verify-ca", "verify-full"],
  ["verify-full", "verify-full"],
]);

function decode(s: string): string | null {
  try {
    return decodeURIComponent(s);
  } catch {
    return null;
  }
}

const flag = (v: string | null): boolean =>
  v !== null && ["1", "true", "yes", "on"].includes(v.toLowerCase());

export function parseDynamoUrl(input: string): ParsedConnectionUrl | null {
  const m = /^dynamodb:\/\/(?:([^@/?#]*)@)?([^/?#]*)\/?(?:\?([^#]*))?$/i.exec(input.trim());
  if (!m) return null;
  const [, userinfo, host, query] = m;
  const region = decode(host ?? "");
  if (region === null) return null;
  let user = "";
  let password: string | null = null;
  if (userinfo !== undefined) {
    const colon = userinfo.indexOf(":");
    const id = decode(colon >= 0 ? userinfo.slice(0, colon) : userinfo);
    const secret = colon >= 0 ? decode(userinfo.slice(colon + 1)) : null;
    if (id === null || (colon >= 0 && secret === null)) return null;
    user = id;
    password = secret === "" ? null : secret;
  }
  const q = new URLSearchParams(query ?? "");
  const form: DynamoForm = {
    ...dynamoFormOf(null),
    auth: q.has("auth") ? authOf(q.get("auth")) : "accessKey",
    profileName: q.get("profile")?.trim() ?? "",
    endpoint: q.get("endpoint")?.trim() ?? "",
    consistentReads: flag(q.get("consistentReads")),
    sampleSize: q.get("sampleSize")?.trim() ?? "",
    pageRequests: q.get("pageRequests")?.trim() ?? "",
    consumedCapacity: flag(q.get("consumedCapacity")),
  };
  // An unknown source is refused rather than read as an access key.
  if (q.has("auth") && authOf(q.get("auth")) !== q.get("auth")) return null;
  const sslmode = q.get("sslmode");
  const ssl = SSL_MODES.get(sslmode ?? "") ?? "verify-full";
  return {
    values: {
      host: "",
      port: "",
      user,
      database: region.toLowerCase(),
      ssl,
      // The Command source reads the secret's Command Line mode.
      ...(form.auth === "command" ? { passwordMode: "command" as const } : {}),
      driverParams: formParams(form),
      name: region === "" ? "DynamoDB" : `DynamoDB ${region.toLowerCase()}`,
    },
    password,
  };
}

/** The `dynamodb://` URL of a profile: no secret, only the params that differ from the defaults. */
export function formatDynamoUrl(p: ConnectionProfile): string | null {
  const f = dynamoForm(p);
  const params = dynamoParamsOf(f);
  const q = new URLSearchParams();
  if (params.auth !== "accessKey") q.set("auth", params.auth);
  if (params.profileName) q.set("profile", params.profileName);
  if (params.endpoint) q.set("endpoint", params.endpoint);
  if (params.consistentReads) q.set("consistentReads", "true");
  if (params.sampleSize !== DEFAULT_SAMPLE_SIZE) q.set("sampleSize", String(params.sampleSize));
  if (params.pageRequests !== DEFAULT_PAGE_REQUESTS)
    q.set("pageRequests", String(params.pageRequests));
  if (params.consumedCapacity) q.set("consumedCapacity", "true");
  if (params.endpoint && p.ssl !== "verify-full") q.set("sslmode", p.ssl);
  const user =
    (params.auth === "accessKey" || params.auth === "command") && p.user
      ? `${encodeURIComponent(p.user)}@`
      : "";
  const query = q.toString();
  return `dynamodb://${user}${encodeURIComponent(p.database ?? "")}${query ? `?${query}` : ""}`;
}
