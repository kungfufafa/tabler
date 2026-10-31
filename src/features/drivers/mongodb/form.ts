import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { MongoFields } from "./MongoFields";
import { formParams, mongoForm, mongoFormOf, mongoParamsOf, wholeNumber } from "./params";
import {
  formatMongoUrl,
  MONGO_DEFAULT_PORT,
  optionOf,
  parseMongoConnectionUrl,
  parseMongoUrl,
  urlTls,
} from "./url";

/**
 * MongoDB's form: host and port (or a connection string for a replica set, SRV or options the
 * form lacks), user and password, the default database, TLS, and the lane's own rows. `validate`
 * says before connecting what the backend's `MongoParams::check` and `connect.rs` refuse.
 */

/** As the backend's `schema::MAX_SAMPLE_SIZE`. */
const MAX_SAMPLE_SIZE = 10_000;

/** Off is first: servers listen in plain text unless set up for TLS. */
export const MONGO_SSL_MODES: ReadonlyArray<readonly [SslMode, string]> = [
  ["disable", "Off"],
  ["prefer", "As the URL says"],
  ["require", "On"],
  ["verify-full", "On, verify the certificate"],
];

/** Options of a URL that would put a secret in plain text with the profile (lower case). */
function secretInUrl(name: string, value: string): string | null {
  switch (name) {
    case "tlscertificatekeyfilepassword":
      return "Give the client key's password as the key passphrase, not in the URL";
    case "proxypassword":
      return "Take proxyPassword out of the URL: it would be saved in plain text";
    case "authmechanismproperties":
      return value
        .split(",")
        .some((p) => p.split(":")[0]?.trim().toUpperCase() === "AWS_SESSION_TOKEN")
        ? "Take AWS_SESSION_TOKEN out of the URL: it would be saved in plain text"
        : null;
    default:
      return null;
  }
}

function validate(v: ConnectionFormValues): FormIssue[] {
  const f = mongoFormOf(v.driverParams);
  const out: FormIssue[] = [];
  const url = f.uri !== "" ? parseMongoUrl(f.uri) : null;
  if (f.uri !== "") {
    if (!url) {
      out.push({
        path: "driverParams.uri",
        message:
          "Use a mongodb:// or mongodb+srv:// URL with a host, and write a `/`, `?` or `@` in the user name, password, database or options as %2F, %3F, %40",
      });
    } else {
      const issue = (message: string) => out.push({ path: "driverParams.uri", message });
      if (url.password !== null)
        issue("Put the password in Password, not in the URL (it is saved in plain text)");
      for (const [k, value] of url.options) {
        const secret = secretInUrl(k.toLowerCase(), value);
        if (secret) issue(secret);
      }
      const mechanism = optionOf(url, "authMechanism")?.toUpperCase();
      if (mechanism === "MONGODB-AWS" || mechanism === "GSSAPI")
        issue(`${mechanism} sign-in is not built into Tabler: use SCRAM, X.509, PLAIN or OIDC`);
      if (v.sshEnabled) {
        const one = url.hosts.length === 1 && !url.srv;
        const [node] = url.hosts;
        const host = (v.host || "127.0.0.1").replace(/^\[(.*)\]$/, "$1").toLowerCase();
        const port = v.port || String(MONGO_DEFAULT_PORT);
        const m = node ? /^\[?([^\]]*?)\]?(?::(\d+))?$/.exec(node) : null;
        if (!one)
          issue("Over SSH, give one host (mongodb://host:port), not SRV or a list of hosts");
        else if (
          !m ||
          m[1]!.toLowerCase() !== host ||
          (m[2] ?? String(MONGO_DEFAULT_PORT)) !== port
        )
          issue("Over SSH, the URL's host and port must be the Host and Port fields'");
      }
      if (v.ssl === "disable" && urlTls(url))
        out.push({
          path: "ssl",
          message: "The URL asks for TLS: turn SSL on, or set tls=false in the URL",
        });
    }
  }
  const sample = wholeNumber(f.sampleSize);
  if (sample !== null && (Number.isNaN(sample) || sample < 1 || sample > MAX_SAMPLE_SIZE))
    out.push({ path: "driverParams.sampleSize", message: `Between 1 and ${MAX_SAMPLE_SIZE}` });
  const selection = wholeNumber(f.serverSelectionTimeoutSecs);
  if (selection !== null && (Number.isNaN(selection) || selection < 1))
    out.push({ path: "driverParams.serverSelectionTimeoutSecs", message: "At least 1 second" });
  const tls = v.ssl !== "disable" && (v.ssl !== "prefer" || (url !== null && urlTls(url)));
  if (f.authMechanism === "x509") {
    if (!tls) out.push({ path: "ssl", message: "X.509 sign-in needs TLS: turn SSL on" });
    else if (v.sslCertPath === "" && v.sslKeyPath === "")
      out.push({ path: "sslCertPath", message: "X.509 sign-in needs a client certificate" });
  }
  if (f.authMechanism === "plain" && v.user === "" && !url?.user)
    out.push({ path: "user", message: "PLAIN (LDAP) sign-in needs a user" });
  // MongoDB reads the certificate and its key from one PEM file (`tlsCertificateKeyFile`).
  if (tls && v.sslCertPath !== "" && v.sslKeyPath !== "" && v.sslCertPath !== v.sslKeyPath)
    out.push({
      path: "sslKeyPath",
      message: "MongoDB reads the certificate and its key from one PEM file: choose it for both",
    });
  // Through the tunnel the driver would check the certificate against 127.0.0.1.
  if (v.sshEnabled && (v.ssl === "verify-ca" || v.ssl === "verify-full"))
    out.push({
      path: "ssl",
      message: "Over SSH the certificate cannot be verified: use SSL “On”",
    });
  return out;
}

export const mongoUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  labels: { database: "Database" },
  sslModes: MONGO_SSL_MODES,
  defaults: () => ({
    host: "127.0.0.1",
    port: String(MONGO_DEFAULT_PORT),
    ssl: "disable",
    driverParams: formParams(mongoFormOf(null)),
  }),
  Fields: MongoFields,
  validate,
  toProfile: (v, base): ConnectionProfile => ({
    ...base,
    options: { ...base.options, driverParams: { ...mongoParamsOf(mongoFormOf(v.driverParams)) } },
  }),
  fromProfile: (p) => ({ driverParams: formParams(mongoForm(p)) }),
  url: {
    schemes: ["mongodb", "mongodb+srv"],
    parse: parseMongoConnectionUrl,
    format: formatMongoUrl,
  },
  summary: (p) => {
    const f = mongoForm(p);
    const db = p.database ? ` / ${p.database}` : "";
    if (f.uri !== "") {
      const url = parseMongoUrl(f.uri);
      return url ? `${url.srv ? "srv: " : ""}${url.hosts.join(", ")}${db}` : f.uri;
    }
    return `${p.host || "127.0.0.1"} : ${p.port ?? MONGO_DEFAULT_PORT}${db}`;
  },
};
