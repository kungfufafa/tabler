import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { MssqlFields } from "./MssqlFields";
import {
  instanceNameOk,
  mssqlParams,
  mssqlParamsOf,
  needsUser,
  PACKET_SIZE,
  toMssqlParams,
} from "./params";
import { formatSqlServerUrl, parseSqlServerUrl } from "./url";

/**
 * SQL Server's form: host and port (or a named instance found through SQL Browser), the login
 * (a SQL Server login, Windows NTLM, the Windows user, or an Entra ID token as the password,
 * usually from its Command mode), the database, and encryption by the SSL mode as the backend
 * maps it (`connect::encryption`): Disable still encrypts the login, Prefer encrypts when the
 * server can, Require takes any certificate (a default SQL Server one is self-signed), and
 * verify-full checks it under the server's name or Hostname in certificate.
 */

/** The SSL modes, in SQL Server's words. verify-ca is left out: it checks the name as well. */
export const SSL_MODES = [
  ["disable", "Login only"],
  ["prefer", "If the server offers it"],
  ["require", "Required, any certificate"],
  ["verify-full", "Required, verify the certificate"],
] as const;

function validate(v: ConnectionFormValues): FormIssue[] {
  const p = mssqlParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  if (needsUser(p.auth) && v.user === "")
    out.push({
      path: "user",
      message:
        p.auth === "windows"
          ? "Enter the Windows user as DOMAIN\\user"
          : "Enter the user of the SQL Server login",
    });
  if (p.auth === "entraToken" && v.passwordMode === "none")
    out.push({
      path: "passwordMode",
      message: "An Entra ID token is the password: store it, ask for it or run a command",
    });
  // With a port or over SSH the backend ignores the instance (`connect::named_instance`), so the
  // field says so (`instanceHint`) rather than refuse a profile the backend takes.
  if (p.instance !== "" && !instanceNameOk(p.instance))
    out.push({
      path: "driverParams.instance",
      message: "Up to 16 bytes of letters, digits, _, $ or #",
    });
  if (p.packetSize !== "") {
    const n = Number(p.packetSize);
    if (!/^\d+$/.test(p.packetSize) || n < PACKET_SIZE.min || n > PACKET_SIZE.max)
      out.push({
        path: "driverParams.packetSize",
        message: `Bytes, ${PACKET_SIZE.min} to ${PACKET_SIZE.max}`,
      });
  }
  return out;
}

function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const p = mssqlParamsOf(v.driverParams);
  return {
    ...base,
    // Integrated and token logins send no user name.
    user: needsUser(p.auth) ? base.user : null,
    options: { ...base.options, driverParams: toMssqlParams(p) },
  };
}

function summary(p: ConnectionProfile): string {
  const { auth, instance } = mssqlParams(p);
  const host = `${p.host ?? ""}${instance ? `\\${instance}` : ""}`;
  const who = auth === "integrated" ? "Windows user" : auth === "entraToken" ? "Entra ID" : p.user;
  return [host, p.port === null ? null : String(p.port), who, p.database]
    .filter((s): s is string => s !== null && s !== "")
    .join(" : ");
}

export const sqlserverUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  sslModes: SSL_MODES,
  defaults: () => ({
    port: "1433",
    ssl: "verify-full",
    driverParams: { ...mssqlParamsOf(null) },
  }),
  Fields: MssqlFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...mssqlParams(p) } }),
  url: {
    schemes: ["sqlserver", "mssql"],
    parse: parseSqlServerUrl,
    format: formatSqlServerUrl,
  },
  summary,
};
