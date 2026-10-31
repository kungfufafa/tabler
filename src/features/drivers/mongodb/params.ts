import type { ConnectionProfile } from "@/lib/ipc";
import type { MongoAuth } from "@/lib/ipc/bindings/MongoAuth";
import type { MongoParams } from "@/lib/ipc/bindings/MongoParams";
import type { MongoReadPreference } from "@/lib/ipc/bindings/MongoReadPreference";

/**
 * `MongoParams` (`drivers/mongodb/params.rs`) read leniently from a profile or the form, and
 * written back with only what is set: a value of the wrong type counts as unset, as the
 * backend's `#[serde(default)]` refuses it with a message of its own.
 */

type Params = Record<string, unknown> | null | undefined;

export const AUTH_MECHANISMS: readonly (readonly [MongoAuth, string])[] = [
  ["default", "Default (SCRAM, or the URL's)"],
  ["scramSha256", "SCRAM-SHA-256"],
  ["scramSha1", "SCRAM-SHA-1"],
  ["x509", "X.509 certificate"],
  ["plain", "PLAIN (LDAP)"],
  ["oidc", "MONGODB-OIDC (token in the password)"],
];

export const READ_PREFERENCES: readonly (readonly [MongoReadPreference, string])[] = [
  ["primary", "Primary"],
  ["primaryPreferred", "Primary preferred"],
  ["secondary", "Secondary"],
  ["secondaryPreferred", "Secondary preferred"],
  ["nearest", "Nearest"],
];

/** The URL spelling of each mechanism (`authMechanism=`). */
export const MECHANISM_NAME: Record<MongoAuth, string | null> = {
  default: null,
  scramSha256: "SCRAM-SHA-256",
  scramSha1: "SCRAM-SHA-1",
  x509: "MONGODB-X509",
  plain: "PLAIN",
  oidc: "MONGODB-OIDC",
};

/** The form's view of the params: text fields as text ("" when unset). */
export interface MongoForm {
  uri: string;
  authMechanism: MongoAuth;
  authSource: string;
  replicaSet: string;
  /** "" unset (the backend decides), else on or off. */
  directConnection: "" | "on" | "off";
  readPreference: MongoReadPreference;
  appName: string;
  serverSelectionTimeoutSecs: string;
  sampleSize: string;
}

const str = (p: Params, key: string): string => {
  const v = p?.[key];
  return typeof v === "string" ? v.trim() : typeof v === "number" ? String(v) : "";
};

function oneOf<T extends string>(v: unknown, list: readonly (readonly [T, string])[], or: T): T {
  return list.find(([id]) => id === v)?.[0] ?? or;
}

export function mongoFormOf(p: Params): MongoForm {
  const direct = p?.directConnection;
  return {
    uri: str(p, "uri"),
    authMechanism: oneOf(p?.authMechanism, AUTH_MECHANISMS, "default"),
    authSource: str(p, "authSource"),
    replicaSet: str(p, "replicaSet"),
    directConnection:
      direct === true || direct === "on" ? "on" : direct === false || direct === "off" ? "off" : "",
    readPreference: oneOf(p?.readPreference, READ_PREFERENCES, "primary"),
    appName: str(p, "appName"),
    serverSelectionTimeoutSecs: str(p, "serverSelectionTimeoutSecs"),
    sampleSize: str(p, "sampleSize"),
  };
}

export const mongoForm = (p: ConnectionProfile): MongoForm => mongoFormOf(p.options.driverParams);

/** A whole number of the form ("" unset; NaN when it is not one). */
export function wholeNumber(text: string): number | null {
  if (text === "") return null;
  return /^\d+$/.test(text) ? Number(text) : NaN;
}

/** The params to save: the two choices always, the rest only when set. */
export function mongoParamsOf(f: MongoForm): MongoParams {
  const out: MongoParams = { authMechanism: f.authMechanism, readPreference: f.readPreference };
  if (f.uri !== "") out.uri = f.uri;
  if (f.authSource !== "") out.authSource = f.authSource;
  if (f.replicaSet !== "") out.replicaSet = f.replicaSet;
  if (f.directConnection !== "") out.directConnection = f.directConnection === "on";
  if (f.appName !== "") out.appName = f.appName;
  const selection = wholeNumber(f.serverSelectionTimeoutSecs);
  if (selection !== null && !Number.isNaN(selection)) out.serverSelectionTimeoutSecs = selection;
  const sample = wholeNumber(f.sampleSize);
  if (sample !== null && !Number.isNaN(sample)) out.sampleSize = sample;
  return out;
}

/** The form's params as `ConnectionFormValues.driverParams` holds them. */
export function formParams(f: MongoForm): Record<string, unknown> {
  return { ...f };
}
