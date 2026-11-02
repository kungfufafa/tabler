import type { ParsedConnectionUrl } from "@/features/connections/url";
import type { UrlSource } from "@/features/drivers/types";
import type { ConnectionProfile } from "@/lib/ipc";
import type { BigqueryAuth } from "@/lib/ipc/bindings/BigqueryAuth";
import {
  type BigqueryForm,
  bigqueryParams,
  DEFAULT_MAXIMUM_BYTES_BILLED,
  endpointOk,
  formatBytes,
  formatLabels,
  googleEndpointOk,
  localFilePath,
  parseBytes,
  parseLabels,
  parseProjects,
  toBigqueryParams,
} from "./params";

/**
 * `bigquery://` URLs, as the real-server suite writes them (`tests/bigquery.rs`):
 * `bigquery://<project>?keyFile=<path>&dataset=<dataset>` for a service-account key, or
 * `bigquery://:<access token>@<project>` for a token, with `location`, `endpoint`, `auth`,
 * `extraProjects`, `maximumBytesBilled` (bytes, or `none`), `useQueryCache`, `jobTimeoutMs` and
 * `labels` (`team:data,env:dev`). The project is read as written, not as a host name: domain-scoped
 * ids hold a colon (`example.com:analytics`). A token comes out of the URL into the secret and is
 * never written back.
 *
 * A clicked link (`url.parse` with source "link", or none) connects unasked, so it never picks
 * the user's own Google sign-in (Application Default Credentials or a key file on this machine):
 * it would sign that identity in to a project the link names, and read a key file the link
 * chose. Such a URL reads as an access token sign-in instead (the link's own token, or the
 * connect asks for one). A URL the user pastes (source "paste": New connection from URL, or the
 * form's Import box) opens in the form first, where the user sees the sign-in it picks, so it
 * keeps ADC and the key file, and a profile's URL round-trips through it. A URL also cannot
 * loosen the cost guard (`maximumBytesBilled` above the default, or `none`, reads as the default;
 * the form raises it, where the user sees it), and `endpoint` must be a `googleapis.com` host or
 * loopback unless the URL carries its own token (the link's own, harmless to send). `keyFile`
 * must be a full local path: reading `\\host\share\k.json` would hand that host the user's NTLM
 * hash.
 */

/** Whether a URL may pick the user's own Google sign-in (see the module's comment). */
type Ambient = "keep" | "drop";

const AUTH_NAMES: Record<string, BigqueryAuth> = {
  serviceaccount: "serviceAccount",
  applicationdefault: "applicationDefault",
  adc: "applicationDefault",
  accesstoken: "accessToken",
  token: "accessToken",
};

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

function boolParam(v: string | null): boolean | null | undefined {
  if (v === null) return undefined;
  const t = v.trim().toLowerCase();
  if (["1", "true", "yes", "on"].includes(t)) return true;
  if (["0", "false", "no", "off"].includes(t)) return false;
  return null;
}

/** The form values of a `bigquery://` URL; null when it is not one, or a value cannot be read. */
function readUrl(input: string, ambient: Ambient): ParsedConnectionUrl | null {
  const m = /^bigquery:\/\/(?:([^@/?#]*)@)?([^/?#]*)\/?([^?#]*)(?:\?([^#]*))?/i.exec(input.trim());
  if (!m) return null;
  const [, userinfo, host = "", path = "", query = ""] = m;
  const project = decode(host).trim();
  if (project === "") return null;
  const params = new URLSearchParams(query);
  const get = (k: string) => params.get(k)?.trim() ?? "";
  const colon = userinfo?.indexOf(":") ?? -1;
  const token = userinfo ? decode(colon < 0 ? userinfo : userinfo.slice(colon + 1)) || null : null;

  const keyFile = get("keyFile") || get("keyFilePath");
  // Opening a share would sign in to its host (NTLM on Windows) before any check runs.
  if (keyFile !== "" && !localFilePath(keyFile)) return null;
  let auth: BigqueryAuth;
  const named = get("auth");
  if (named !== "") {
    const a = AUTH_NAMES[named.toLowerCase().replace(/[-_]/g, "")];
    if (!a) return null;
    auth = a;
  } else auth = keyFile ? "serviceAccount" : token ? "accessToken" : "applicationDefault";
  // A link signs in with what it brings: ADC and the key file stay the user's choice in the form.
  if (ambient === "drop") auth = "accessToken";

  const cache = boolParam(params.get("useQueryCache"));
  if (cache === null) return null;
  const guardText = get("maximumBytesBilled");
  let maximumBytesBilled = formatBytes(DEFAULT_MAXIMUM_BYTES_BILLED);
  // A link may tighten the guard, never loosen it.
  if (guardText !== "" && guardText.toLowerCase() !== "none") {
    const bytes = parseBytes(guardText);
    if (bytes === null || bytes === 0) return null;
    if (bytes < DEFAULT_MAXIMUM_BYTES_BILLED) maximumBytesBilled = formatBytes(bytes);
  }
  const timeout = get("jobTimeoutMs");
  if (timeout !== "" && !/^\d+$/.test(timeout)) return null;
  const labels: Record<string, string> = {};
  for (const pair of get("labels").split(",")) {
    if (pair.trim() === "") continue;
    const at = pair.indexOf(":");
    if (at < 0) return null;
    labels[pair.slice(0, at).trim()] = pair.slice(at + 1).trim();
  }
  // A token the URL brings may go anywhere it says; any other sign-in only to Google or here.
  const endpoint = get("endpoint") || get("apiEndpoint");
  if (
    endpoint !== "" &&
    (!endpointOk(endpoint) || (!googleEndpointOk(endpoint) && !(auth === "accessToken" && token)))
  )
    return null;
  const dataset = get("dataset") || decode(path.replace(/\/+$/, "")).trim();

  const form: BigqueryForm = {
    auth,
    keyFilePath: auth === "serviceAccount" ? keyFile : "",
    location: get("location"),
    defaultDataset: dataset,
    extraProjects: parseProjects(get("extraProjects")).join(", "),
    maximumBytesBilled,
    useQueryCache: cache ?? true,
    jobTimeoutSecs: timeout === "" ? "" : String(Number(timeout) / 1000),
    labels: formatLabels(labels),
    apiEndpoint: endpoint,
  };
  return {
    values: {
      host: "",
      port: "",
      user: "",
      database: project,
      driverParams: { ...form },
      name: project,
    },
    password: token,
  };
}

/**
 * A `bigquery://` URL as form values, for `url.parse`: ADC and key files only from a paste, which
 * the form shows first; a link (or no source) reads as an access token sign-in.
 */
export const parseBigqueryUrl = (input: string, source?: UrlSource): ParsedConnectionUrl | null =>
  readUrl(input, source === "paste" ? "keep" : "drop");

/** The same for the form's Import box, where the user sees the sign-in: a paste. */
export const importBigqueryUrl = (input: string): ParsedConnectionUrl | null =>
  parseBigqueryUrl(input, "paste");

/**
 * The `bigquery://` URL of a profile: its project, sign-in and params, never its token. What the
 * Import box would not take back is left out (a looser guard, an endpoint that needs the token),
 * so the URL reads as the same project with the default guard and Google's endpoint. Pasted, it
 * reads back as the same profile; a link reads an ADC or key file URL as an access token sign-in.
 */
export function formatBigqueryUrl(p: ConnectionProfile): string | null {
  const project = (p.database ?? "").trim();
  if (project === "") return null;
  const params = toBigqueryParams(bigqueryParams(p));
  const q = new URLSearchParams();
  q.set("auth", params.auth ?? "serviceAccount");
  if (params.keyFilePath) q.set("keyFile", params.keyFilePath);
  if (params.defaultDataset) q.set("dataset", params.defaultDataset);
  if (params.location) q.set("location", params.location);
  if (params.extraProjects) q.set("extraProjects", params.extraProjects.join(","));
  const guard = params.maximumBytesBilled;
  if (typeof guard === "number" && guard < DEFAULT_MAXIMUM_BYTES_BILLED)
    q.set("maximumBytesBilled", String(guard));
  if (params.useQueryCache === false) q.set("useQueryCache", "false");
  if (params.jobTimeoutMs !== undefined && params.jobTimeoutMs !== null)
    q.set("jobTimeoutMs", String(params.jobTimeoutMs));
  const labels = parseLabels(formatLabels(params.labels ?? {})).labels;
  if (Object.keys(labels).length > 0)
    q.set(
      "labels",
      Object.entries(labels)
        .map(([k, v]) => `${k}:${v}`)
        .join(","),
    );
  if (params.apiEndpoint && googleEndpointOk(params.apiEndpoint))
    q.set("endpoint", params.apiEndpoint);
  return `bigquery://${encodeURIComponent(project)}?${q.toString()}`;
}
