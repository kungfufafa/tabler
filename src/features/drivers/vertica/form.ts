import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { nameProblem, toVerticaParams, verticaParams, verticaParamsOf } from "./params";
import { formatVerticaUrl, parseVerticaUrl } from "./url";
import { VerticaFields } from "./VerticaFields";

/**
 * Vertica's form: host and port (5433), the user, the password (hashed as the server asks, SHA-512
 * or MD5) or an OAuth access token in its place (usually from the Command mode), the database
 * (one per cluster), and TLS by the SSL mode as libpq names them, which `connect::Dial` follows:
 * Prefer goes on in plain text when the server refuses TLS, Require takes any certificate, and the
 * verify modes check it against the CA file (and the host name). The shared SSL rows carry the
 * client certificate of Vertica's `tls` authentication.
 */

/** What `entry::validate` and `VerticaParams::check` refuse, on the fields they name. */
function validate(v: ConnectionFormValues): FormIssue[] {
  const p = verticaParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  for (const key of ["clientLabel", "workload"] as const) {
    const problem = nameProblem(p[key]);
    if (problem) out.push({ path: `driverParams.${key}`, message: problem });
  }
  if (p.auth === "oauth") {
    if (v.passwordMode === "none")
      out.push({
        path: "passwordMode",
        message: "An OAuth token is the password: store it, ask for it or run a command",
      });
    // Prefer goes on without TLS when the server (or anyone between) says no.
    if (v.ssl === "disable" || v.ssl === "prefer")
      out.push({
        path: "ssl",
        message: "An OAuth token is sent only over TLS: choose Require or stricter",
      });
  }
  return out;
}

function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  return {
    ...base,
    options: { ...base.options, driverParams: toVerticaParams(verticaParamsOf(v.driverParams)) },
  };
}

/** `user@host:port/database`, and what is not the default login. */
function summary(p: ConnectionProfile): string {
  const { auth } = verticaParams(p);
  const where = `${p.host ?? ""}${p.port === null ? "" : `:${p.port}`}`;
  const who = p.user ? `${p.user}@` : "";
  const db = p.database ? `/${p.database}` : "";
  return `${who}${where}${db}${auth === "oauth" ? " (OAuth)" : ""}`;
}

export const verticaUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  defaults: () => ({
    port: "5433",
    driverParams: { ...verticaParamsOf(null) },
  }),
  Fields: VerticaFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...verticaParams(p) } }),
  url: {
    schemes: ["vertica"],
    parse: parseVerticaUrl,
    format: formatVerticaUrl,
  },
  summary,
};
