import type { ConnectionProfile } from "@/lib/ipc";
import type { ElasticAuth } from "@/lib/ipc/bindings/ElasticAuth";
import type { ElasticAwsCredentials } from "@/lib/ipc/bindings/ElasticAwsCredentials";
import type { ElasticAwsService } from "@/lib/ipc/bindings/ElasticAwsService";
import type { ElasticParams } from "@/lib/ipc/bindings/ElasticParams";

/**
 * `ElasticParams` (`drivers/elasticsearch/params.rs`) read leniently from a profile or the form,
 * and written back with only what is set: a value of the wrong type counts as unset, as the
 * backend's `#[serde(default)]` refuses it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

export const AUTH_METHODS: readonly (readonly [ElasticAuth, string])[] = [
  ["basic", "User and password"],
  ["apiKey", "API key"],
  ["bearer", "Bearer token"],
  ["awsSigv4", "AWS SigV4 (Amazon OpenSearch Service)"],
  ["none", "None"],
];

export const AWS_CREDENTIALS: readonly (readonly [ElasticAwsCredentials, string])[] = [
  ["accessKey", "Access key (User and Password)"],
  ["profile", "Named profile (~/.aws)"],
  ["environment", "Environment (AWS_ACCESS_KEY_ID…)"],
];

/** "" reads the service from the endpoint's host. */
export const AWS_SERVICES: readonly (readonly ["" | ElasticAwsService, string])[] = [
  ["", "From the host"],
  ["es", "es (OpenSearch Service domain)"],
  ["aoss", "aoss (OpenSearch Serverless)"],
];

/** The form's view of the params: text fields as text ("" when unset). */
export interface ElasticForm {
  /** Node URLs, one per line (or comma-separated). */
  urls: string;
  cloudId: string;
  pathPrefix: string;
  auth: ElasticAuth;
  apiKeyId: string;
  awsCredentials: ElasticAwsCredentials;
  awsProfile: string;
  awsRegion: string;
  awsService: "" | ElasticAwsService;
  caFingerprint: string;
  showSystemIndices: boolean;
  requestTimeoutSecs: string;
}

const str = (p: Params, key: string): string => {
  const v = p?.[key];
  return typeof v === "string" ? v.trim() : typeof v === "number" ? String(v) : "";
};

function oneOf<T extends string>(v: unknown, list: readonly (readonly [T, string])[], or: T): T {
  return list.find(([id]) => id === v)?.[0] ?? or;
}

/** The node URLs of the form's text (lines or commas), trimmed, without empty ones. */
export function urlList(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((u) => u.trim())
    .filter((u) => u !== "");
}

export function elasticFormOf(p: Params): ElasticForm {
  const urls = p?.urls;
  return {
    urls: Array.isArray(urls)
      ? urls.filter((u): u is string => typeof u === "string").join("\n")
      : typeof urls === "string"
        ? urls
        : "",
    cloudId: str(p, "cloudId"),
    pathPrefix: str(p, "pathPrefix"),
    auth: oneOf(p?.auth, AUTH_METHODS, "basic"),
    apiKeyId: str(p, "apiKeyId"),
    awsCredentials: oneOf(p?.awsCredentials, AWS_CREDENTIALS, "accessKey"),
    awsProfile: str(p, "awsProfile"),
    awsRegion: str(p, "awsRegion"),
    awsService: oneOf(p?.awsService, AWS_SERVICES, ""),
    caFingerprint: str(p, "caFingerprint"),
    showSystemIndices: p?.showSystemIndices === true,
    requestTimeoutSecs: str(p, "requestTimeoutSecs"),
  };
}

export const elasticForm = (p: ConnectionProfile): ElasticForm =>
  elasticFormOf(p.options.driverParams);

/** A whole number of the form ("" unset; NaN when it is not one). */
export function wholeNumber(text: string): number | null {
  if (text === "") return null;
  return /^\d+$/.test(text) ? Number(text) : NaN;
}

/** The params to save: the choices always, the rest only when set. */
export function elasticParamsOf(f: ElasticForm): ElasticParams {
  const out: ElasticParams = {
    urls: urlList(f.urls),
    auth: f.auth,
    awsCredentials: f.awsCredentials,
    showSystemIndices: f.showSystemIndices,
  };
  if (f.cloudId !== "") out.cloudId = f.cloudId;
  if (f.pathPrefix !== "") out.pathPrefix = f.pathPrefix;
  // The settings of another sign-in are not kept.
  if (f.auth === "apiKey" && f.apiKeyId !== "") out.apiKeyId = f.apiKeyId;
  if (f.auth === "awsSigv4") {
    if (f.awsCredentials === "profile" && f.awsProfile !== "") out.awsProfile = f.awsProfile;
    if (f.awsRegion !== "") out.awsRegion = f.awsRegion;
    if (f.awsService !== "") out.awsService = f.awsService;
  }
  if (f.caFingerprint !== "") out.caFingerprint = f.caFingerprint;
  const timeout = wholeNumber(f.requestTimeoutSecs);
  if (timeout !== null && !Number.isNaN(timeout)) out.requestTimeoutSecs = timeout;
  return out;
}

/** The form's params as `ConnectionFormValues.driverParams` holds them. */
export function formParams(f: ElasticForm): Record<string, unknown> {
  return { ...f };
}
