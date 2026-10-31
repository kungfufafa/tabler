import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { DynamoFields } from "./DynamoFields";
import {
  DEFAULT_PAGE_REQUESTS,
  DEFAULT_SAMPLE_SIZE,
  dynamoForm,
  dynamoFormOf,
  dynamoParamsOf,
  formParams,
  isRegionName,
  LOCAL_ENDPOINT,
  MAX_PAGE_REQUESTS,
  MAX_SAMPLE_SIZE,
  normalizeEndpoint,
  usesKeyId,
  usesSecret,
  wholeNumber,
} from "./params";
import { formatDynamoUrl, parseDynamoUrl } from "./url";

/**
 * DynamoDB's form: where the credentials come from (an access key, an AWS profile, the
 * environment, a command printing `credential_process` JSON, or DynamoDB Local's dummy keys), the
 * region, an endpoint of its own, and the grid's settings. Every row is the lane's (`Fields`):
 * which ones show depends on the source. `validate` says before connecting what the backend's
 * `entry::validate` and `DynamoParams::check` refuse.
 */

/**
 * The SSL modes of an https endpoint the form names off AWS (an emulator, a private host): the
 * endpoint's scheme says whether TLS is used, AWS's own hosts are always verified.
 */
export const DYNAMO_SSL_MODES: ReadonlyArray<readonly [SslMode, string]> = [
  ["require", "HTTPS"],
  ["verify-full", "HTTPS, verify the certificate"],
];

function validate(v: ConnectionFormValues): FormIssue[] {
  const f = dynamoFormOf(v.driverParams);
  const out: FormIssue[] = [];
  const region = v.database.trim();
  if (region === "") out.push({ path: "database", message: "Enter the region (like us-east-1)" });
  else if (!isRegionName(region))
    out.push({ path: "database", message: `“${region}” is not a region name (like us-east-1)` });
  if (f.endpoint !== "") {
    const n = normalizeEndpoint(f.endpoint);
    if ("error" in n)
      out.push({ path: "driverParams.endpoint", message: `The endpoint ${n.error}` });
  }
  if (f.auth === "accessKey" && v.user.trim() === "")
    out.push({ path: "user", message: "Enter the access key ID" });
  if (f.auth === "command" && v.passwordMode !== "command")
    out.push({
      path: "passwordMode",
      message: "The Command source reads the secret's Command Line: choose Command Line",
    });
  if (f.auth === "profile" && /\s/.test(f.profileName))
    out.push({ path: "driverParams.profileName", message: "A profile name has no spaces" });
  const sample = wholeNumber(f.sampleSize);
  if (sample !== null && (Number.isNaN(sample) || sample < 1 || sample > MAX_SAMPLE_SIZE))
    out.push({ path: "driverParams.sampleSize", message: `Between 1 and ${MAX_SAMPLE_SIZE}` });
  const requests = wholeNumber(f.pageRequests);
  if (requests !== null && (Number.isNaN(requests) || requests < 1 || requests > MAX_PAGE_REQUESTS))
    out.push({ path: "driverParams.pageRequests", message: `Between 1 and ${MAX_PAGE_REQUESTS}` });
  return out;
}

/** The profile to save: no host or port, the key ID and secret only for the sources that sign with them. */
function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const f = dynamoFormOf(v.driverParams);
  const keyed = usesKeyId(f.auth);
  return {
    ...base,
    host: null,
    port: null,
    ssh: null,
    user: keyed ? base.user : null,
    database: base.database?.trim() || null,
    options: {
      ...base.options,
      // A source that reads no secret never asks for one.
      passwordMode: usesSecret(f.auth) ? base.options.passwordMode : "none",
      passwordCommand: usesSecret(f.auth) ? base.options.passwordCommand : null,
      driverParams: { ...dynamoParamsOf(f) },
    },
  };
}

function summary(p: ConnectionProfile): string {
  const f = dynamoForm(p);
  const region = p.database ?? "";
  const where = f.endpoint !== "" ? f.endpoint : f.auth === "local" ? LOCAL_ENDPOINT : "";
  const who =
    f.auth === "accessKey"
      ? (p.user ?? "")
      : f.auth === "profile"
        ? `profile ${f.profileName || "default"}`
        : f.auth === "environment"
          ? "environment"
          : f.auth === "command"
            ? "command"
            : "local";
  return [region, who, where].filter((s) => s !== "").join(" · ");
}

export const dynamoUi: KindUi = {
  // Every row is the lane's: which ones show depends on the credential source.
  rows: [],
  labels: { user: "Access key ID", secret: "Secret Access Key", database: "Region" },
  sslModes: DYNAMO_SSL_MODES,
  defaults: () => ({
    host: "",
    port: "",
    database: "us-east-1",
    ssl: "verify-full",
    driverParams: formParams({
      ...dynamoFormOf(null),
      sampleSize: String(DEFAULT_SAMPLE_SIZE),
      pageRequests: String(DEFAULT_PAGE_REQUESTS),
    }),
  }),
  Fields: DynamoFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: formParams(dynamoForm(p)) }),
  url: { schemes: ["dynamodb"], parse: parseDynamoUrl, format: formatDynamoUrl },
  summary,
};
