import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { BigqueryFields } from "./BigqueryFields";
import { bigqueryParams, bigqueryParamsOf, paramIssues, toBigqueryParams } from "./params";
import { formatBigqueryUrl, parseBigqueryUrl } from "./url";

/**
 * The BigQuery form: the project jobs run and bill in (the descriptor's Project, required), how
 * the connection signs in (a service-account key file read at connect, Application Default
 * Credentials, or an access token kept as the secret or printed by a Command Line such as
 * `gcloud auth print-access-token`), and the lane's settings (`BigqueryParams`). There is no host,
 * port, user, SSH or SSL: every row is the lane's (`Fields`), and the secret shows only for an
 * access token. The checks are `BigqueryParams::validate`'s, in its words, so a profile the form
 * saves is one the backend opens.
 */

function validate(v: ConnectionFormValues): FormIssue[] {
  // "Project is required" is the schema's (the descriptor's required field); the backend takes
  // any project name, an emulator's `test` too.
  const out: FormIssue[] = [];
  const f = bigqueryParamsOf(v.driverParams);
  for (const issue of paramIssues(f))
    out.push({ path: `driverParams.${issue.key}`, message: issue.message });
  if (f.auth === "accessToken" && v.passwordMode === "none")
    out.push({
      path: "passwordMode",
      message: "An access token is the secret: store it, ask for it, or print it with a command",
    });
  return out;
}

/**
 * The profile to save: the project as the database, no host, port, user, SSH or TLS files, and a
 * secret only for an access token (the other sign-ins read none, so none is ever asked for).
 */
function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const f = bigqueryParamsOf(v.driverParams);
  const token = f.auth === "accessToken";
  return {
    ...base,
    host: null,
    port: null,
    user: null,
    ssh: null,
    database: base.database?.trim() || null,
    options: {
      ...base.options,
      passwordMode: token ? base.options.passwordMode : "none",
      passwordCommand: token ? base.options.passwordCommand : null,
      sslKeyPath: null,
      sslCertPath: null,
      driverParams: { ...toBigqueryParams(f) },
    },
  };
}

const AUTH_SHORT = {
  serviceAccount: "key file",
  applicationDefault: "ADC",
  accessToken: "token",
} as const;

/** `project · dataset · location · how it signs in`. */
function summary(p: ConnectionProfile): string {
  const f = bigqueryParams(p);
  return [p.database ?? "", f.defaultDataset, f.location, AUTH_SHORT[f.auth]]
    .filter((s) => s !== "")
    .join(" · ");
}

export const bigqueryUi: KindUi = {
  // Every row is the lane's: the secret shows only for an access token.
  rows: [],
  labels: { database: "Project", secret: "Access token" },
  sslModes: null,
  defaults: () => ({
    host: "",
    port: "",
    user: "",
    ssl: "verify-full",
    passwordMode: "none",
    driverParams: { ...bigqueryParamsOf(null) },
  }),
  Fields: BigqueryFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...bigqueryParams(p) } }),
  url: { schemes: ["bigquery"], parse: parseBigqueryUrl, format: formatBigqueryUrl },
  summary,
};
