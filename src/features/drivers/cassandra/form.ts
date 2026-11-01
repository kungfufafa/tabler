import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { CassandraFields } from "./CassandraFields";
import {
  cassandraParams,
  cassandraParamsOf,
  inRange,
  PAGE_SIZE,
  REQUEST_TIMEOUT,
  toCassandraParams,
} from "./params";
import { formatCassandraUrl, parseCassandraUrl } from "./url";

/**
 * The Cassandra / ScyllaDB form: the contact point and port (9042), a user and password for
 * PasswordAuthenticator (none: no authentication), the keyspace as the database, TLS by the SSL
 * mode as `entry::dial` maps it, and the lane's own settings (`CassandraParams`): consistency,
 * serial consistency, local datacenter, request timeout, the editor's page size, compression and
 * single node.
 */

/**
 * The SSL modes, in CQL's words. A node takes TLS on its port or not at all (no negotiation), so
 * there is no Prefer: the backend reads it as Disable, and `toProfile` saves it so.
 */
export const SSL_MODES = [
  ["disable", "Off"],
  ["require", "On, any certificate"],
  ["verify-ca", "On, verify the certificate"],
  ["verify-full", "On, verify the certificate and host"],
] as const;

function validate(v: ConnectionFormValues): FormIssue[] {
  const p = cassandraParamsOf(v.driverParams);
  const out: FormIssue[] = [];
  if (p.requestTimeoutSecs !== "" && inRange(p.requestTimeoutSecs, REQUEST_TIMEOUT) === null)
    out.push({
      path: "driverParams.requestTimeoutSecs",
      message: `Seconds, ${REQUEST_TIMEOUT.min} to ${REQUEST_TIMEOUT.max}`,
    });
  if (p.pageSize !== "" && inRange(p.pageSize, PAGE_SIZE) === null)
    out.push({
      path: "driverParams.pageSize",
      message: `Rows, ${PAGE_SIZE.min} to ${PAGE_SIZE.max.toLocaleString("en-US")}`,
    });
  // A client certificate needs both files (`entry::validate`): the shared check asks for them
  // while TLS is on, and the saved profile drops them while it is off.
  return out;
}

function toProfile(v: ConnectionFormValues, base: ConnectionProfile): ConnectionProfile {
  const driverParams = toCassandraParams(cassandraParamsOf(v.driverParams));
  if (base.ssl !== "prefer") return { ...base, options: { ...base.options, driverParams } };
  // Prefer is Disable to CQL: saved as what it means, without client files (as Disable is).
  return {
    ...base,
    ssl: "disable",
    options: { ...base.options, driverParams, sslKeyPath: null, sslCertPath: null },
  };
}

/** `host : port : user : keyspace`, and the consistency when it is not the default. */
function summary(p: ConnectionProfile): string {
  const { consistency } = cassandraParams(p);
  return [
    p.host,
    p.port === null ? null : String(p.port),
    p.user,
    p.database,
    consistency === "LOCAL_ONE" ? null : consistency,
  ]
    .filter((s): s is string => s !== null && s !== "")
    .join(" : ");
}

export const cassandraUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  sslModes: SSL_MODES,
  defaults: () => ({
    port: "9042",
    user: "",
    ssl: "disable",
    driverParams: { ...cassandraParamsOf(null) },
  }),
  Fields: CassandraFields,
  validate,
  toProfile,
  fromProfile: (p) => ({ driverParams: { ...cassandraParams(p) } }),
  url: {
    schemes: ["cassandra"],
    parse: parseCassandraUrl,
    format: formatCassandraUrl,
  },
  summary,
};
