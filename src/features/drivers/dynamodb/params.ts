import type { ConnectionProfile } from "@/lib/ipc";
import type { DynamoAuth } from "@/lib/ipc/bindings/DynamoAuth";
import type { DynamoParams } from "@/lib/ipc/bindings/DynamoParams";

/**
 * `DynamoParams` (`drivers/dynamodb/params.rs`) read leniently from a profile or the form, and
 * written back: a value of the wrong type counts as unset, as the backend's `#[serde(default)]`
 * would refuse it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

/** Where the credentials come from, in the form's words. */
export const AUTH_SOURCES: readonly (readonly [DynamoAuth, string])[] = [
  ["accessKey", "Access key"],
  ["profile", "AWS profile (~/.aws)"],
  ["environment", "Environment variables"],
  ["command", "Command (credential_process JSON)"],
  ["local", "DynamoDB Local (no credentials)"],
];

/** As the backend's `DEFAULT_SAMPLE_SIZE` and `DEFAULT_PAGE_REQUESTS`. */
export const DEFAULT_SAMPLE_SIZE = 100;
export const DEFAULT_PAGE_REQUESTS = 5;
/** As the backend's `DynamoParams::check`. */
export const MAX_SAMPLE_SIZE = 1000;
export const MAX_PAGE_REQUESTS = 50;
/** DynamoDB Local's own endpoint (`LOCAL_ENDPOINT`). */
export const LOCAL_ENDPOINT = "http://localhost:8000/";

/** The form's view of the params: numbers as the text typed ("" unset). */
export interface DynamoForm {
  auth: DynamoAuth;
  profileName: string;
  endpoint: string;
  consistentReads: boolean;
  sampleSize: string;
  pageRequests: string;
  consumedCapacity: boolean;
}

const str = (p: Params, key: string): string => {
  const v = p?.[key];
  return typeof v === "string" ? v.trim() : typeof v === "number" ? String(v) : "";
};

export function authOf(v: unknown): DynamoAuth {
  return AUTH_SOURCES.find(([id]) => id === v)?.[0] ?? "accessKey";
}

export function dynamoFormOf(p: Params): DynamoForm {
  return {
    auth: authOf(p?.auth),
    profileName: str(p, "profileName"),
    endpoint: str(p, "endpoint"),
    consistentReads: p?.consistentReads === true,
    sampleSize: str(p, "sampleSize"),
    pageRequests: str(p, "pageRequests"),
    consumedCapacity: p?.consumedCapacity === true,
  };
}

export const dynamoForm = (p: ConnectionProfile): DynamoForm =>
  dynamoFormOf(p.options.driverParams);

/** A whole number of the form ("" unset: null; NaN when it is not one). */
export function wholeNumber(text: string): number | null {
  if (text === "") return null;
  return /^\d+$/.test(text) ? Number(text) : NaN;
}

const numberOr = (text: string, fallback: number): number => {
  const n = wholeNumber(text);
  return n === null || Number.isNaN(n) ? fallback : n;
};

/** The params to save: every field the backend reads, the profile's name and endpoint when set. */
export function dynamoParamsOf(f: DynamoForm): DynamoParams {
  const out: DynamoParams = {
    auth: f.auth,
    consistentReads: f.consistentReads,
    sampleSize: numberOr(f.sampleSize, DEFAULT_SAMPLE_SIZE),
    pageRequests: numberOr(f.pageRequests, DEFAULT_PAGE_REQUESTS),
    consumedCapacity: f.consumedCapacity,
  };
  if (f.auth === "profile" && f.profileName !== "") out.profileName = f.profileName;
  if (f.endpoint !== "") out.endpoint = f.endpoint;
  return out;
}

/** The form's params as `ConnectionFormValues.driverParams` holds them. */
export function formParams(f: DynamoForm): Record<string, unknown> {
  return { ...f };
}

/** Whether the source signs with the profile's access key ID (the form's User). */
export const usesKeyId = (auth: DynamoAuth): boolean => auth === "accessKey" || auth === "command";

/** Whether the source reads the profile's secret (the password, or its Command Line). */
export const usesSecret = (auth: DynamoAuth): boolean => auth === "accessKey" || auth === "command";

/** AWS's spelling of a region name (`check_region`): emulators take any such name too. */
export function isRegionName(region: string): boolean {
  return (
    region.length > 0 &&
    region.length <= 64 &&
    /^[a-z0-9-]+$/.test(region) &&
    !region.startsWith("-") &&
    !region.endsWith("-")
  );
}

/**
 * An endpoint as the backend normalizes it (`normalize_endpoint`): http or https (https when no
 * scheme is given), a host, no credentials, query or fragment, a trailing `/`. A reason when it
 * is not one.
 */
export function normalizeEndpoint(text: string): { url: string } | { error: string } {
  const t = text.trim();
  const withScheme = t.includes("://") ? t : `https://${t}`;
  let url: URL;
  try {
    url = new URL(withScheme);
  } catch {
    return { error: "is not a URL" };
  }
  if (url.protocol !== "http:" && url.protocol !== "https:")
    return { error: "must start with http:// or https://" };
  if (url.hostname === "") return { error: "has no host" };
  if (url.username !== "" || url.password !== "") return { error: "must not hold credentials" };
  if (url.search !== "" || url.hash !== "" || /[?#]/.test(withScheme))
    return { error: "must not have a query or fragment" };
  const out = url.toString();
  return { url: out.endsWith("/") ? out : `${out}/` };
}

/** The domains of AWS's own endpoints, which are always verified (`is_aws_host`). */
const AWS_DOMAINS = ["amazonaws.com", "amazonaws.com.cn", "api.aws"];

export function isAwsHost(endpoint: string): boolean {
  const n = normalizeEndpoint(endpoint);
  if ("error" in n) return false;
  const host = new URL(n.url).hostname.replace(/\.$/, "").toLowerCase();
  return AWS_DOMAINS.some((d) => host === d || host.endsWith(`.${d}`));
}

/**
 * Whether the form's SSL mode applies (`session::endpoint`): only to an https endpoint the form
 * names off AWS's domains, an emulator or a private host. AWS's own are verified in full.
 */
export function formTlsApplies(f: DynamoForm): boolean {
  const endpoint = f.endpoint !== "" ? f.endpoint : f.auth === "local" ? LOCAL_ENDPOINT : "";
  if (endpoint === "") return false;
  const n = normalizeEndpoint(endpoint);
  return "url" in n && n.url.startsWith("https:") && !isAwsHost(n.url);
}
