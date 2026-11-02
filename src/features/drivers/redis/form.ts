import type { ConnectionFormValues } from "@/features/connections/schema";
import type { ConnectionProfile, SslMode } from "@/lib/ipc";
import type { FormIssue, KindUi } from "../types";
import { splitConsole, tokenize, commandName, CLI_ERROR_TEXT } from "./cli";
import { classifyArgv, refusedInConsole } from "./classify";
import {
  databaseIndex,
  formParams,
  MAX_DATABASES,
  MAX_PREVIEW_BYTES,
  MAX_PREVIEW_ELEMENTS,
  MAX_SCAN_COUNT,
  redisForm,
  redisFormOf,
  toRedisParams,
  wholeNumber,
} from "./params";
import { RedisFields } from "./RedisFields";
import { formatRedisUrl, parseRedisConnectionUrl } from "./url";

/**
 * Redis's form: host and port (6379), an optional ACL user (Redis 6+) and the password (none,
 * stored, asked, or a Command Line token such as an ElastiCache IAM token), the DB index, TLS
 * (off, on, or on with the certificate checked) with its CA and client certificate, and the
 * lane's own rows. `validate` says before connecting what the backend's `entry::validate`
 * refuses: the settings' bounds, the DB index and the bootstrap commands.
 */

/** No `Prefer`: Redis has no TLS negotiation, so the backend reads it as plain text. */
export const REDIS_SSL_MODES: ReadonlyArray<readonly [SslMode, string]> = [
  ["disable", "Off"],
  ["require", "On"],
  ["verify-ca", "On, verify the CA"],
  ["verify-full", "On, verify the certificate and host"],
];

/** Bootstrap commands that would change what the session relies on (`conn::startup_commands`). */
const CONNECTION_STATE = new Set([
  "SELECT",
  "SWAPDB",
  "MULTI",
  "EXEC",
  "DISCARD",
  "WATCH",
  "UNWATCH",
]);

/** The first bootstrap command the backend refuses, in its words; null when it takes them all. */
export function bootstrapProblem(text: string, readOnly: boolean): string | null {
  for (const st of splitConsole(text)) {
    const refused = (why: string) => `Bootstrap command “${st.sql}”: ${why}`;
    const t = tokenize(st.sql);
    if (!t.ok) return refused(CLI_ERROR_TEXT[t.error]);
    const why = refusedInConsole(t.argv);
    if (why) return refused(why);
    if (CONNECTION_STATE.has(commandName(t.argv) ?? ""))
      return refused(
        "the DB index field picks the database, and a transaction cannot span connections",
      );
    if (readOnly && classifyArgv(t.argv) === "write")
      return refused("it writes, and the connection is read-only");
  }
  return null;
}

function validate(v: ConnectionFormValues): FormIssue[] {
  const f = redisFormOf(v.driverParams);
  const out: FormIssue[] = [];
  const within = (key: "scanCount" | "previewBytes" | "previewElements", max: number) => {
    const n = wholeNumber(f[key]);
    if (n !== null && (Number.isNaN(n) || n < 1 || n > max))
      out.push({ path: `driverParams.${key}`, message: `Between 1 and ${max.toLocaleString()}` });
  };
  within("scanCount", MAX_SCAN_COUNT);
  within("previewBytes", MAX_PREVIEW_BYTES);
  within("previewElements", MAX_PREVIEW_ELEMENTS);
  if (f.keySeparator !== "" && [...f.keySeparator].length > 8)
    out.push({ path: "driverParams.keySeparator", message: "1 to 8 characters" });
  if (databaseIndex(v.database) === null)
    out.push({
      path: "database",
      message: `A number from 0 to ${MAX_DATABASES - 1} (or db0, db1, …)`,
    });
  const bootstrap = bootstrapProblem(v.startupSql, v.readOnly);
  if (bootstrap) out.push({ path: "startupSql", message: bootstrap });
  return out;
}

/** `user@host:port/db`, and TLS when it is on. */
function summary(p: ConnectionProfile): string {
  const who = p.user ? `${p.user}@` : "";
  const db = databaseIndex(p.database) ?? 0;
  const tls = p.ssl === "require" || p.ssl === "verify-ca" || p.ssl === "verify-full";
  return `${who}${p.host || "127.0.0.1"}:${p.port ?? 6379}/db${db}${tls ? " (TLS)" : ""}`;
}

export const redisUi: KindUi = {
  rows: ["hostPort", "user", "secret", "database", "ssl"],
  labels: { user: "ACL user", database: "DB index" },
  sslModes: REDIS_SSL_MODES,
  defaults: () => ({
    host: "127.0.0.1",
    port: "6379",
    ssl: "disable",
    driverParams: formParams(redisFormOf(null)),
  }),
  Fields: RedisFields,
  validate,
  toProfile: (v, base): ConnectionProfile => ({
    ...base,
    options: { ...base.options, driverParams: toRedisParams(redisFormOf(v.driverParams)) },
  }),
  fromProfile: (p) => ({ driverParams: formParams(redisForm(p)) }),
  url: {
    schemes: ["redis", "rediss"],
    parse: parseRedisConnectionUrl,
    format: formatRedisUrl,
  },
  summary,
};
