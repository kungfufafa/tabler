import type { ConnectionProfile } from "@/lib/ipc";
import type { BigqueryAuth } from "@/lib/ipc/bindings/BigqueryAuth";
import type { BigqueryParams } from "@/lib/ipc/bindings/BigqueryParams";

/**
 * The lane's driver params (`BigqueryParams` in `src/lib/ipc/bindings`) read leniently from a
 * profile or the form: anything of the wrong type counts as unset, since the backend's
 * `#[serde(default)]` would refuse it with a message of its own. The form keeps the numbers as
 * typed text (the cost guard with a unit, the job timeout in seconds), the extra projects as one
 * comma-separated line and the labels as `key = value` lines. The checks are
 * `BigqueryParams::validate`'s, in its words.
 */

type Params = Record<string, unknown> | null | undefined;

export const AUTHS: readonly (readonly [BigqueryAuth, string])[] = [
  ["serviceAccount", "Service account key file"],
  ["applicationDefault", "Application Default Credentials"],
  ["accessToken", "Access token"],
];

/** `DEFAULT_MAXIMUM_BYTES_BILLED`: 1 GiB, what a new profile starts with. */
export const DEFAULT_MAXIMUM_BYTES_BILLED = 2 ** 30;
/** `DEFAULT_API_ENDPOINT`. */
export const DEFAULT_API_ENDPOINT = "https://bigquery.googleapis.com/bigquery/v2";
/** The backend refuses numbers JavaScript cannot carry exactly. */
const SAFE = Number.MAX_SAFE_INTEGER;

/** The form's view of the params: strings for the text fields ("" when unset). */
export interface BigqueryForm {
  auth: BigqueryAuth;
  keyFilePath: string;
  location: string;
  defaultDataset: string;
  /** Project ids, comma-separated. */
  extraProjects: string;
  /** "1 GB", "500 MB" or bytes; "" for no limit. */
  maximumBytesBilled: string;
  useQueryCache: boolean;
  /** Seconds; "" for BigQuery's own limit. */
  jobTimeoutSecs: string;
  /** One `key = value` per line. */
  labels: string;
  apiEndpoint: string;
}

const UNITS: readonly (readonly [string, number])[] = [
  ["TB", 2 ** 40],
  ["GB", 2 ** 30],
  ["MB", 2 ** 20],
  ["KB", 2 ** 10],
];

/**
 * Bytes as the form shows them: the largest unit that divides them (`1 GB`), as the editor's
 * cost notice counts (1024 per step), else plain bytes.
 */
export function formatBytes(n: number): string {
  for (const [unit, size] of UNITS) if (n >= size && n % size === 0) return `${n / size} ${unit}`;
  return String(n);
}

/**
 * The bytes of a cost-guard field: digits with an optional unit (`B`, `KB`, `MB`, `GB`, `TB`, or
 * `KiB`… for the same 1024 steps); null when it cannot be read. "" is no limit.
 */
export function parseBytes(text: string): number | null {
  const m = /^(\d+(?:\.\d+)?)\s*([kmgt]i?b|b)?$/i.exec(text.trim());
  if (!m) return null;
  const unit = (m[2] ?? "b").toUpperCase().replace("IB", "B");
  const size = unit === "B" ? 1 : (UNITS.find(([u]) => u === unit)?.[1] ?? null);
  if (size === null) return null;
  const n = Math.round(Number(m[1]) * size);
  return Number.isSafeInteger(n) ? n : null;
}

/** The milliseconds of a seconds field (up to 3 decimals); null when it cannot be read. */
export function parseSeconds(text: string): number | null {
  if (!/^\d+(?:\.\d{1,3})?$/.test(text.trim())) return null;
  const ms = Math.round(Number(text.trim()) * 1000);
  return Number.isSafeInteger(ms) ? ms : null;
}

/** A label line that is not `key = value`. */
export interface LabelLineError {
  line: number;
  text: string;
}

/** The labels of the form's text, and the lines that are no label. Blank lines are skipped. */
export function parseLabels(text: string): {
  labels: Record<string, string>;
  bad: LabelLineError[];
} {
  const labels: Record<string, string> = {};
  const bad: LabelLineError[] = [];
  text.split(/\r?\n/).forEach((raw, i) => {
    const line = raw.trim();
    if (line === "") return;
    const eq = line.search(/[=:]/);
    if (eq < 0) bad.push({ line: i + 1, text: line });
    else labels[line.slice(0, eq).trim()] = line.slice(eq + 1).trim();
  });
  return { labels, bad };
}

export const formatLabels = (labels: Record<string, string>): string =>
  Object.entries(labels)
    .map(([k, v]) => `${k} = ${v}`)
    .join("\n");

/** The project ids of the form's line: split at commas, trimmed, blanks dropped. */
export const parseProjects = (text: string): string[] =>
  text
    .split(",")
    .map((p) => p.trim())
    .filter((p) => p !== "");

/** `validate`'s label check: lower-case letters, digits, `_` and `-`, 63 at most. */
function labelPartOk(s: string, key: boolean): boolean {
  return [...s].length <= 63 && /^[\p{Ll}0-9_-]*$/u.test(s) && (!key || /^\p{Ll}/u.test(s));
}

/** The backend's refusal of a label, in its words; null when it is accepted. */
export function labelProblem(key: string, value: string): string | null {
  return labelPartOk(key, true) && labelPartOk(value, false)
    ? null
    : `The label “${key}” must use lower-case letters, digits, _ and - (63 at most, the key starting with a letter)`;
}

/** `validate`'s dataset check: letters, digits and underscores. */
export const datasetOk = (name: string): boolean => /^[\p{L}\p{N}_]+$/u.test(name);

/**
 * `validate`'s base URL check: the client appends the API's path after the endpoint, which after
 * a `?` or `#` would land in the query or fragment and send the request to the endpoint's own
 * path.
 */
export const endpointIsBase = (url: string): boolean => !/[?#]/.test(url);

/**
 * `transport::credentials_may_go_to`: every call carries the bearer token, so the endpoint must be
 * HTTPS, or plain HTTP to this machine (`localhost` or a loopback address, the host compared
 * whole after the URL parser has normalized it).
 */
export function endpointOk(url: string): boolean {
  if (!endpointIsBase(url)) return false;
  let u: URL;
  try {
    u = new URL(url.trim());
  } catch {
    return false;
  }
  const host = u.hostname;
  if (u.protocol === "https:") return host !== "";
  if (u.protocol !== "http:") return false;
  return host === "localhost" || /^127\.\d+\.\d+\.\d+$/.test(host) || host === "[::1]";
}

/**
 * `transport::google_credentials_may_go_to`: a key file's or ADC's token is the user's Google
 * sign-in, so it goes only to an https `googleapis.com` host (private endpoints too) or to this
 * machine. Only a profile's own access token may go to another https endpoint.
 */
export function googleEndpointOk(url: string): boolean {
  if (!endpointIsBase(url)) return false;
  let u: URL;
  try {
    u = new URL(url.trim());
  } catch {
    return false;
  }
  const host = u.hostname;
  const local = host === "localhost" || /^127\.\d+\.\d+\.\d+$/.test(host) || host === "[::1]";
  if (u.protocol === "http:") return local;
  if (u.protocol !== "https:") return false;
  return local || host === "googleapis.com" || host.endsWith(".googleapis.com");
}

/**
 * `params::local_file_path`: `C:\…`, `C:/…` or `/…`. A UNC or device path (`\\host\share`,
 * `//host/share`, `\\?\UNC\…`, `\??\…`) would make Windows sign in to that host, handing it the
 * user's NTLM hash, before the file is read.
 */
export const localFilePath = (path: string): boolean =>
  /^[A-Za-z]:[\\/]/.test(path) || /^[\\/][^\\/?]/.test(path);

const str = (v: unknown): string => (typeof v === "string" ? v.trim() : "");

const AUTH_IDS = AUTHS.map(([id]) => id);

/** The form's values of stored params (or of the form's own, which are already strings). */
export function bigqueryParamsOf(params: Params): BigqueryForm {
  const auth = AUTH_IDS.find((a) => a === params?.auth) ?? "serviceAccount";
  const projects = params?.extraProjects;
  const labels = params?.labels;
  const bytes = params?.maximumBytesBilled;
  const timeout = params?.jobTimeoutMs;
  return {
    auth,
    keyFilePath: str(params?.keyFilePath),
    location: str(params?.location),
    defaultDataset: str(params?.defaultDataset),
    extraProjects: Array.isArray(projects)
      ? projects.filter((p): p is string => typeof p === "string").join(", ")
      : str(projects),
    // A missing guard is the backend's default; `null` is none.
    maximumBytesBilled:
      bytes === undefined
        ? formatBytes(DEFAULT_MAXIMUM_BYTES_BILLED)
        : typeof bytes === "number" && Number.isSafeInteger(bytes)
          ? formatBytes(bytes)
          : str(bytes),
    useQueryCache: params?.useQueryCache !== false,
    jobTimeoutSecs:
      typeof timeout === "number" && Number.isSafeInteger(timeout)
        ? String(timeout / 1000)
        : typeof timeout === "string"
          ? timeout.trim()
          : str(params?.jobTimeoutSecs),
    labels:
      typeof labels === "string"
        ? labels
        : typeof labels === "object" && labels !== null
          ? formatLabels(
              Object.fromEntries(
                Object.entries(labels).filter(
                  (e): e is [string, string] => typeof e[1] === "string",
                ),
              ),
            )
          : "",
    apiEndpoint: str(params?.apiEndpoint),
  };
}

export const bigqueryParams = (p: ConnectionProfile): BigqueryForm =>
  bigqueryParamsOf(p.options.driverParams);

/** A problem of the form's params, on the param it names. */
export interface ParamIssue {
  key: keyof BigqueryForm;
  message: string;
}

/** `BigqueryParams::validate` on the form's values, in its words. */
export function paramIssues(f: BigqueryForm): ParamIssue[] {
  const out: ParamIssue[] = [];
  if (f.auth === "serviceAccount" && f.keyFilePath === "")
    out.push({ key: "keyFilePath", message: "Choose the service account's key file (JSON)" });
  else if (f.auth === "serviceAccount" && !localFilePath(f.keyFilePath))
    out.push({
      key: "keyFilePath",
      message: `The key file “${f.keyFilePath}” must be a full path on this machine (C:\\keys\\sa.json or /keys/sa.json), not a network share`,
    });
  if (f.maximumBytesBilled.trim() !== "") {
    const bytes = parseBytes(f.maximumBytesBilled);
    if (bytes === null)
      out.push({
        key: "maximumBytesBilled",
        message: "Bytes, or a number with B, KB, MB, GB or TB (empty for no limit)",
      });
    else if (bytes === 0)
      out.push({
        key: "maximumBytesBilled",
        message: "Maximum bytes billed must be more than 0 (or empty for no limit)",
      });
    else if (bytes > SAFE)
      out.push({ key: "maximumBytesBilled", message: "The number is too large" });
  }
  if (f.jobTimeoutSecs.trim() !== "") {
    const ms = parseSeconds(f.jobTimeoutSecs);
    if (ms === null) out.push({ key: "jobTimeoutSecs", message: "Seconds (empty for none)" });
    else if (ms > SAFE) out.push({ key: "jobTimeoutSecs", message: "The number is too large" });
  }
  if (f.defaultDataset !== "" && !datasetOk(f.defaultDataset))
    out.push({
      key: "defaultDataset",
      message: `“${f.defaultDataset}” is no dataset name (letters, digits and underscores)`,
    });
  const { labels, bad } = parseLabels(f.labels);
  const labelIssue =
    bad.length > 0
      ? `Line ${bad[0]!.line}: write each label as key = value`
      : Object.entries(labels)
          .map(([k, v]) => labelProblem(k, v))
          .find((m) => m !== null);
  if (labelIssue) out.push({ key: "labels", message: labelIssue });
  if (f.apiEndpoint !== "" && !endpointIsBase(f.apiEndpoint))
    out.push({
      key: "apiEndpoint",
      message: `The API endpoint “${f.apiEndpoint}” must be a base URL, without a query (?) or fragment (#)`,
    });
  else if (f.apiEndpoint !== "" && !endpointOk(f.apiEndpoint))
    out.push({
      key: "apiEndpoint",
      message: `The API endpoint “${f.apiEndpoint}” must be an https URL (plain http only to this machine)`,
    });
  else if (f.apiEndpoint !== "" && f.auth !== "accessToken" && !googleEndpointOk(f.apiEndpoint))
    out.push({
      key: "apiEndpoint",
      message: `With a key file or Application Default Credentials, the API endpoint “${f.apiEndpoint}” must be a googleapis.com host (or this machine); another endpoint needs an access token`,
    });
  return out;
}

/**
 * The params a profile keeps: the sign-in and the cost guard always (`null`: no limit, which
 * differs from a missing guard, the backend's 1 GiB), the rest only when set. Values the form
 * refuses first are dropped.
 */
export function toBigqueryParams(f: BigqueryForm): Partial<BigqueryParams> {
  const out: Partial<BigqueryParams> = { auth: f.auth };
  if (f.auth === "serviceAccount" && f.keyFilePath !== "") out.keyFilePath = f.keyFilePath;
  if (f.location !== "") out.location = f.location;
  if (f.defaultDataset !== "") out.defaultDataset = f.defaultDataset;
  const projects = parseProjects(f.extraProjects);
  if (projects.length > 0) out.extraProjects = projects;
  const bytes = f.maximumBytesBilled.trim() === "" ? null : parseBytes(f.maximumBytesBilled);
  out.maximumBytesBilled =
    f.maximumBytesBilled.trim() === ""
      ? null
      : bytes !== null && bytes > 0 && bytes <= SAFE
        ? bytes
        : DEFAULT_MAXIMUM_BYTES_BILLED;
  if (!f.useQueryCache) out.useQueryCache = false;
  const ms = f.jobTimeoutSecs.trim() === "" ? null : parseSeconds(f.jobTimeoutSecs);
  if (ms !== null && ms <= SAFE) out.jobTimeoutMs = ms;
  const { labels } = parseLabels(f.labels);
  if (Object.keys(labels).length > 0) out.labels = labels;
  if (f.apiEndpoint !== "") out.apiEndpoint = f.apiEndpoint;
  return out;
}
