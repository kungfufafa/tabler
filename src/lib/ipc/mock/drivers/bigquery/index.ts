import {
  datasetOk,
  DEFAULT_MAXIMUM_BYTES_BILLED,
  endpointIsBase,
  endpointOk,
  googleEndpointOk,
  labelProblem,
  localFilePath,
} from "@/features/drivers/bigquery/params";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { LISTED_PROJECTS, seedDatasets } from "./data";
import { bigquerySession } from "./session";

/**
 * The mock driver of the bigquery lane (BigQuery), served in `vite --mode mock` with
 * `?mockDrivers=bigquery`: the project `demo-proj` signed in with a service-account key file, the
 * same project opened read-only with Application Default Credentials, and a sandbox signed in
 * with a `gcloud` access token and no cost guard. Connecting checks what
 * `BigqueryParams::validate` and `entry::login` check, in the same words; the form's calls are
 * `listProjects` and `listDatasets`.
 */

/** `BigqueryParams::of` read leniently: a value of the wrong type is the default. */
interface Params {
  auth: string;
  keyFilePath: string;
  location: string;
  defaultDataset: string;
  extraProjects: string[];
  maximumBytesBilled: number | null;
  useQueryCache: boolean;
  labels: Record<string, string>;
  apiEndpoint: string;
}

function paramsOf(p: ConnectionProfile): Params {
  const raw = p.options.driverParams ?? {};
  const str = (v: unknown) => (typeof v === "string" ? v.trim() : "");
  const bytes = raw.maximumBytesBilled;
  return {
    auth: str(raw.auth) || "serviceAccount",
    keyFilePath: str(raw.keyFilePath),
    location: str(raw.location),
    defaultDataset: str(raw.defaultDataset),
    extraProjects: Array.isArray(raw.extraProjects)
      ? raw.extraProjects.filter((x): x is string => typeof x === "string")
      : [],
    maximumBytesBilled:
      bytes === undefined ? DEFAULT_MAXIMUM_BYTES_BILLED : typeof bytes === "number" ? bytes : null,
    useQueryCache: raw.useQueryCache !== false,
    labels:
      typeof raw.labels === "object" && raw.labels !== null && !Array.isArray(raw.labels)
        ? (Object.fromEntries(
            Object.entries(raw.labels).filter(([, v]) => typeof v === "string"),
          ) as Record<string, string>)
        : {},
    apiEndpoint: str(raw.apiEndpoint),
  };
}

/** What `BigqueryParams::validate` and the sign-in refuse, with their words. */
function validate(p: ConnectionProfile, params: Params): void {
  if ((p.database ?? "").trim() === "") mockError("INVALID_INPUT", "Project is required");
  if (params.auth === "serviceAccount" && params.keyFilePath === "")
    mockError("INVALID_INPUT", "BigQuery: choose the service account's key file (JSON).");
  if (params.keyFilePath !== "" && !localFilePath(params.keyFilePath))
    mockError(
      "INVALID_INPUT",
      `BigQuery: the key file “${params.keyFilePath}” must be a full path on this machine (C:\\keys\\sa.json or /keys/sa.json), not a network share.`,
    );
  if (params.maximumBytesBilled === 0)
    mockError(
      "INVALID_INPUT",
      "BigQuery: Maximum bytes billed must be more than 0 (or empty for no limit).",
    );
  if (params.defaultDataset !== "" && !datasetOk(params.defaultDataset))
    mockError(
      "INVALID_INPUT",
      `BigQuery: “${params.defaultDataset}” is no dataset name (letters, digits and underscores).`,
    );
  for (const [k, v] of Object.entries(params.labels))
    if (labelProblem(k, v))
      mockError(
        "INVALID_INPUT",
        `BigQuery: the label “${k}” must use lower-case letters, digits, _ and - (63 at most, the key starting with a letter).`,
      );
  if (params.apiEndpoint !== "" && !endpointIsBase(params.apiEndpoint))
    mockError(
      "INVALID_INPUT",
      `BigQuery: the API endpoint “${params.apiEndpoint}” must be a base URL, without a query (?) or fragment (#).`,
    );
  if (params.apiEndpoint !== "" && !endpointOk(params.apiEndpoint))
    mockError(
      "INVALID_INPUT",
      `BigQuery: the API endpoint “${params.apiEndpoint}” must be an https URL (plain http only to this machine).`,
    );
  if (
    params.apiEndpoint !== "" &&
    params.auth !== "accessToken" &&
    !googleEndpointOk(params.apiEndpoint)
  )
    mockError(
      "INVALID_INPUT",
      `BigQuery: with a key file or Application Default Credentials, the API endpoint “${params.apiEndpoint}” must be a googleapis.com host (or this machine); another endpoint needs an access token.`,
    );
  // `entry::login`: a token profile with nothing to read the token from.
  if (params.auth === "accessToken" && (p.options.passwordMode ?? "keychain") === "none")
    mockError(
      "AUTH_FAILED",
      "authentication failed: BigQuery: enter the access token, or a command that prints one (gcloud auth print-access-token)",
    );
}

/** Who the session says it is (`server_info.current_user`): a key file's service account. */
const userOf = (p: ConnectionProfile, params: Params): string | null =>
  params.auth === "serviceAccount"
    ? `tabler@${(p.database ?? "").trim()}.iam.gserviceaccount.com`
    : null;

const seed = (
  over: Partial<ConnectionProfile> & { id: string; name: string },
  driverParams: Record<string, unknown>,
  options: Partial<ConnectionProfile["options"]> = {},
): ConnectionProfile => {
  const base = profile({
    driver: "bigquery",
    host: null,
    port: null,
    user: null,
    ssl: "verify-full",
    ...over,
  });
  return { ...base, options: { ...base.options, ...options, driverParams } };
};

export const bigqueryMock: MockDriver = {
  driver: "bigquery",
  serverVersion: () => "BigQuery",
  seedProfiles: () => [
    seed(
      {
        id: "p-mock-bigquery",
        name: "BigQuery demo-proj",
        color: "blue",
        database: "demo-proj",
        sortOrder: 160,
      },
      {
        auth: "serviceAccount",
        keyFilePath: "/Users/demo/keys/demo-proj-tabler.json",
        defaultDataset: "shop",
        location: "US",
        extraProjects: ["bigquery-public-data"],
        maximumBytesBilled: DEFAULT_MAXIMUM_BYTES_BILLED,
        labels: { team: "data" },
      },
      { passwordMode: "none" },
    ),
    seed(
      {
        id: "p-mock-bigquery-ro",
        name: "BigQuery analytics (read-only, ADC)",
        color: "red",
        database: "demo-proj",
        sortOrder: 161,
      },
      { auth: "applicationDefault", defaultDataset: "analytics", location: "US" },
      { passwordMode: "none", readOnly: true, tag: "production" },
    ),
    seed(
      {
        id: "p-mock-bigquery-token",
        name: "BigQuery sandbox (gcloud token)",
        color: "green",
        database: "acme-analytics",
        sortOrder: 162,
      },
      { auth: "accessToken", maximumBytesBilled: null, useQueryCache: false },
      { passwordMode: "command", passwordCommand: "gcloud auth print-access-token" },
    ),
  ],
  connect: (p) => {
    const params = paramsOf(p);
    validate(p, params);
    return bigquerySession({
      project: (p.database ?? "").trim(),
      defaultDataset: params.defaultDataset || null,
      location: params.location || "US",
      extraProjects: params.extraProjects,
      maximumBytesBilled: params.maximumBytesBilled,
      useQueryCache: params.useQueryCache,
      readOnly: p.options.readOnly,
      user: userOf(p, params),
      datasets: seedDatasets(),
    });
  },
  util: (method) => {
    if (method === "listProjects") return LISTED_PROJECTS.map((p) => ({ ...p }));
    if (method === "listDatasets")
      return [...seedDatasets().values()]
        .map((d) => ({ name: d.name, location: d.location }))
        .sort((a, b) => a.name.localeCompare(b.name));
    return mockError("UNSUPPORTED", `unsupported: BigQuery: unknown call “${method}”`);
  },
};

const lane: MockLane = { drivers: [bigqueryMock] };

export default lane;
