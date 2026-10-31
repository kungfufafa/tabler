import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import {
  cloudIdUrl,
  ES_SCHEMES,
  isServerlessHost,
  parseBaseUrl,
  pinProblem,
  prefixProblem,
  regionForHost,
  regionProblem,
  rootOf,
  type BaseUrl,
} from "./address";
import { ElasticFields } from "./ElasticFields";
import {
  elasticForm,
  elasticFormOf,
  elasticParamsOf,
  formParams,
  urlList,
  wholeNumber,
} from "./params";
import { ES_DEFAULT_PORT, ES_DEFAULT_SSL, formatElasticUrl, parseElasticUrl } from "./url";

/**
 * Elasticsearch's and OpenSearch's form: the host and port (or node URLs, or a Cloud ID), the
 * user and password (or an API key, a bearer token, SigV4 keys), TLS, and the lane's own rows.
 * `validate` says before connecting what the backend's `ElasticParams::check`, `entry::bases`,
 * `validate` and `check_tunnel` refuse.
 */

/** As the backend's `MAX_REQUEST_TIMEOUT_SECS` (a day). */
const MAX_REQUEST_TIMEOUT_SECS = 86_400;

export const ES_SSL_MODES: ReadonlyArray<readonly [SslMode, string]> = [
  ["verify-full", "HTTPS, verify the certificate and host"],
  ["verify-ca", "HTTPS, verify the certificate"],
  ["require", "HTTPS, no certificate check"],
  ["prefer", "HTTPS, or as each URL says"],
  ["disable", "HTTP (no TLS)"],
];

/** The addresses the backend would try, or the first reason one is refused. */
export function addressesOf(
  v: Pick<ConnectionFormValues, "host" | "port" | "ssl" | "driverParams">,
): { bases: BaseUrl[] } | { issue: FormIssue } {
  const f = elasticFormOf(v.driverParams);
  const https = v.ssl !== "disable";
  const urls = urlList(f.urls);
  const bases: BaseUrl[] = [];
  if (f.cloudId !== "") {
    const base = cloudIdUrl(f.cloudId);
    if (!base)
      return { issue: { path: "driverParams.cloudId", message: "The Cloud ID is not valid" } };
    bases.push(base);
  } else if (urls.length > 0) {
    for (const u of urls) {
      const base = parseBaseUrl(u, https);
      if ("error" in base) return { issue: { path: "driverParams.urls", message: base.error } };
      bases.push(base);
    }
  } else {
    const host = v.host.trim();
    if (host === "")
      return { issue: { path: "host", message: "Enter the host, the URLs or a Cloud ID" } };
    const base = parseBaseUrl(host, https);
    if ("error" in base) return { issue: { path: "host", message: base.error } };
    // A port of its own wins over the host's default.
    if (v.port !== "" && !host.includes("://")) base.port = Number(v.port);
    bases.push(base);
  }
  if (f.pathPrefix !== "")
    for (const b of bases) if (b.prefix === "") b.prefix = f.pathPrefix.replace(/\/+$/, "");
  return { bases };
}

function validate(v: ConnectionFormValues): FormIssue[] {
  const f = elasticFormOf(v.driverParams);
  const out: FormIssue[] = [];
  const issue = (path: FormIssue["path"], message: string) => out.push({ path, message });
  if (f.cloudId !== "" && urlList(f.urls).length > 0)
    issue("driverParams.cloudId", "Give either a Cloud ID or URLs, not both");
  const prefix = f.pathPrefix.replace(/\/+$/, "");
  const prefixIssue = prefix === "" ? null : prefixProblem(prefix);
  if (prefixIssue) issue("driverParams.pathPrefix", prefixIssue);
  if (f.caFingerprint !== "") {
    const pin = pinProblem(f.caFingerprint);
    if (pin) issue("driverParams.caFingerprint", pin);
  }
  if (f.awsRegion !== "") {
    const region = regionProblem(f.awsRegion);
    if (region) issue("driverParams.awsRegion", region);
  }
  const timeout = wholeNumber(f.requestTimeoutSecs);
  if (
    timeout !== null &&
    (Number.isNaN(timeout) || timeout < 1 || timeout > MAX_REQUEST_TIMEOUT_SECS)
  )
    issue("driverParams.requestTimeoutSecs", `1 to ${MAX_REQUEST_TIMEOUT_SECS} seconds`);
  if (out.length > 0) return out;

  const found = addressesOf(v);
  if ("issue" in found) return [found.issue];
  const { bases } = found;
  // The field that names the cluster.
  const where: FormIssue["path"] =
    f.cloudId !== ""
      ? "driverParams.cloudId"
      : urlList(f.urls).length > 0
        ? "driverParams.urls"
        : "host";
  // An `http://` URL never wins over a profile that asks for TLS.
  const wantsTls =
    ["require", "verify-ca", "verify-full"].includes(v.ssl) || f.caFingerprint !== "";
  const plain = bases.find((b) => !b.https);
  if (wantsTls && plain)
    issue(
      where,
      `${rootOf(plain)} is plain HTTP, but the SSL mode or the CA fingerprint asks for TLS: use https://, or set SSL to “HTTP” or “as each URL says”`,
    );
  if (
    (f.auth === "awsSigv4" && f.awsService === "aoss") ||
    bases.some((b) => isServerlessHost(b.host))
  )
    issue(
      f.auth === "awsSigv4" && f.awsService === "aoss" ? "driverParams.awsService" : where,
      "OpenSearch Serverless (aoss) collections are not supported yet; Amazon OpenSearch Service domains (es) are",
    );
  if (v.sshEnabled) {
    const host = v.host.trim() || "127.0.0.1";
    const port = Number(v.port || ES_DEFAULT_PORT);
    const [b] = bases;
    if (bases.length !== 1 || !b || b.host.toLowerCase() !== host.toLowerCase() || b.port !== port)
      issue(
        where,
        `Over SSH the tunnel reaches ${host}:${port} only: name the cluster by the host and port (not URLs or a Cloud ID)`,
      );
  }
  if (f.auth === "awsSigv4" && f.awsRegion === "" && bases.some((b) => !regionForHost(b.host)))
    issue(
      "driverParams.awsRegion",
      "AWS SigV4 needs the region (the endpoint's host name does not tell it)",
    );
  return out;
}

export const elasticUi: KindUi = {
  rows: ["hostPort", "user", "secret", "ssl"],
  sslModes: ES_SSL_MODES,
  defaults: () => ({
    host: "127.0.0.1",
    port: String(ES_DEFAULT_PORT),
    ssl: ES_DEFAULT_SSL,
    user: "elastic",
    driverParams: formParams(elasticFormOf(null)),
  }),
  Fields: ElasticFields,
  validate,
  toProfile: (v, base): ConnectionProfile => ({
    ...base,
    // The cluster has no database to choose: the session names it.
    database: null,
    options: {
      ...base.options,
      driverParams: { ...elasticParamsOf(elasticFormOf(v.driverParams)) },
    },
  }),
  fromProfile: (p) => ({ driverParams: formParams(elasticForm(p)) }),
  url: { schemes: ES_SCHEMES, parse: parseElasticUrl, format: formatElasticUrl },
  summary: (p) => {
    const f = elasticForm(p);
    const urls = urlList(f.urls);
    const where =
      f.cloudId !== ""
        ? `Cloud ${f.cloudId.split(":")[0] || "ID"}`
        : urls.length > 0
          ? urls.join(", ")
          : `${p.host || "127.0.0.1"} : ${p.port ?? ES_DEFAULT_PORT}`;
    return `${where}${f.pathPrefix ? ` ${f.pathPrefix}` : ""}`;
  },
};
