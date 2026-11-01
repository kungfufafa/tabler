import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { ClickhouseFields, HTTP_PORT } from "./ClickhouseFields";
import {
  clickhouseParams,
  clickhouseParamsOf,
  parseRoles,
  parseSettings,
  SESSION_TIMEOUT,
  sessionTimeoutOf,
  settingProblem,
  toClickhouseParams,
} from "./params";
import { formatClickhouseUrl, parseClickhouseUrl } from "./url";

/**
 * The ClickHouse form: the HTTP interface's host and port (8123, or 8443 over HTTPS), the user
 * (`default` when empty) and its password, or the client certificate of an X.509 user, the
 * database the requests start in, TLS by the SSL mode as `entry::certificate_check` maps it, and
 * the lane's own settings (`ClickhouseParams`). The checks are `entry::validate`'s, in its words
 * where it has them, so a profile the form saves is one the backend opens.
 */

/**
 * The SSL modes, in HTTP's words. There is no plain HTTP to fall back to, so there is no Prefer:
 * the backend checks it fully, and `toProfile` saves it as verify-full.
 */
export const SSL_MODES = [
  ["disable", "HTTP"],
  ["require", "HTTPS, any certificate"],
  ["verify-ca", "HTTPS, verify the certificate"],
  ["verify-full", "HTTPS, verify the certificate and host"],
] as const;

function validate(v: ConnectionFormValues): FormIssue[] {
  const p = clickhouseParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  const { settings, bad } = parseSettings(p.settings);
  const problem =
    bad.length > 0
      ? `Line ${bad[0]!.line}: write each setting as name = value`
      : settings.map((s) => settingProblem(s.name)).find((m) => m !== null);
  if (problem) out.push({ path: "driverParams.settings", message: problem });
  // eslint-disable-next-line no-control-regex
  const badRole = parseRoles(p.roles).find((r) => /[\u0000-\u001f\u007f-\u009f]/.test(r));
  if (badRole !== undefined)
    out.push({ path: "driverParams.roles", message: `“${badRole}” is not a role name` });
  if (p.sessionTimeoutSecs !== "" && sessionTimeoutOf(p.sessionTimeoutSecs) === null)
    out.push({
      path: "driverParams.sessionTimeoutSecs",
      message: `Seconds, ${SESSION_TIMEOUT.min} to ${SESSION_TIMEOUT.max}`,
    });
  if (p.certificateAuth) {
    if (v.ssl === "disable")
      out.push({
        path: "driverParams.certificateAuth",
        message: "Signing in with a certificate needs HTTPS (turn SSL on)",
      });
    else if (v.sslCertPath === "" || v.sslKeyPath === "")
      out.push({
        path: "driverParams.certificateAuth",
        message: "Signing in with a certificate needs the client certificate and key files",
      });
  }
  // A certificate and key that come in pairs: the shared check asks for both while TLS is on.
  return out;
}

function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const driverParams = toClickhouseParams(clickhouseParamsOf(v.driverParams));
  // Prefer has no HTTP fallback: saved as what the backend makes of it.
  const ssl = base.ssl === "prefer" ? "verify-full" : base.ssl;
  return { ...base, ssl, options: { ...base.options, driverParams } };
}

/** `host : port : user : database`, and the roles when there are any. */
function summary(p: ConnectionProfile): string {
  const { roles, certificateAuth } = clickhouseParams(p);
  return [
    p.host,
    p.port === null ? null : String(p.port),
    certificateAuth ? `${p.user ?? "default"} (certificate)` : p.user,
    p.database,
    roles === "" ? null : roles,
  ]
    .filter((s): s is string => s !== null && s !== "")
    .join(" : ");
}

export const clickhouseUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  sslModes: SSL_MODES,
  defaults: () => ({
    port: HTTP_PORT,
    user: "default",
    ssl: "disable",
    driverParams: { ...clickhouseParamsOf(null) },
  }),
  Fields: ClickhouseFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...clickhouseParams(p) } }),
  url: {
    schemes: ["clickhouse", "clickhouses"],
    parse: parseClickhouseUrl,
    format: formatClickhouseUrl,
  },
  summary,
};
