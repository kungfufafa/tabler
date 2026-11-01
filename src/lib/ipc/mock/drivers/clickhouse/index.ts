import { settingProblem } from "@/features/drivers/clickhouse/params";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { serverDatabases } from "./data";
import { clickhouseSession } from "./session";

/**
 * The mock driver of the clickhouse lane (ClickHouse), served in `vite --mode mock` with
 * `?mockDrivers=clickhouse`: a local server over HTTP with `shop`, `analytics` and `system`, and
 * a ClickHouse Cloud service opened read-only over HTTPS with a role and a setting. Connecting
 * checks what the backend's `validate` and `open` check, in the same words; there are no calls of
 * the form (`util`) and no session calls.
 */

/** The server's `version()` by where it runs. */
const isCloud = (p: ConnectionProfile) => /\.clickhouse\.cloud$/i.test(p.host ?? "");
const versionOf = (p: ConnectionProfile) => (isCloud(p) ? "25.10.1.3832" : "25.8.2.29");

/** What `entry::validate` refuses, with its words. */
function validate(p: ConnectionProfile): void {
  const params = p.options.driverParams ?? {};
  if ((p.host ?? "").trim() === "") mockError("INVALID_INPUT", "Host is required");
  const settings = Array.isArray(params.settings) ? params.settings : [];
  for (const s of settings) {
    const name = typeof s === "object" && s !== null ? (s as { name?: unknown }).name : null;
    const problem = settingProblem(typeof name === "string" ? name : "");
    if (problem) mockError("INVALID_INPUT", problem);
  }
  const roles = Array.isArray(params.roles) ? params.roles : [];
  for (const r of roles)
    // eslint-disable-next-line no-control-regex
    if (typeof r === "string" && /[\u0000-\u001f\u007f-\u009f]/.test(r.trim()))
      mockError("INVALID_INPUT", `ClickHouse: “${r.trim()}” is not a role name`);
  const secs = params.sessionTimeoutSecs;
  if (typeof secs === "number" && (secs < 1 || secs > 3600))
    mockError("INVALID_INPUT", "ClickHouse: the session timeout is 1 to 3600 seconds");
  const cert = (p.options.sslCertPath ?? "").trim() !== "";
  const key = (p.options.sslKeyPath ?? "").trim() !== "";
  if (cert !== key)
    mockError(
      "INVALID_INPUT",
      "ClickHouse: a client certificate needs both its certificate and its key file",
    );
  if (params.certificateAuth === true) {
    if (p.ssl === "disable")
      mockError(
        "INVALID_INPUT",
        "ClickHouse: signing in with a certificate needs HTTPS (turn SSL on)",
      );
    if (!cert)
      mockError(
        "INVALID_INPUT",
        "ClickHouse: signing in with a certificate needs the client certificate and key files",
      );
  }
}

export const clickhouseMock: MockDriver = {
  driver: "clickhouse",
  serverVersion: (p) => `ClickHouse ${versionOf(p)}`,
  seedProfiles: () => [
    profile(
      {
        id: "p-mock-clickhouse",
        name: "ClickHouse local",
        driver: "clickhouse",
        color: "yellow",
        host: "127.0.0.1",
        port: 8123,
        user: "default",
        database: "shop",
        ssl: "disable",
        sortOrder: 130,
      },
      "local",
    ),
    ((): ConnectionProfile => {
      const base = profile({
        id: "p-mock-clickhouse-cloud",
        name: "ClickHouse Cloud analytics (read-only)",
        driver: "clickhouse",
        host: "abc123xyz.eu-central-1.aws.clickhouse.cloud",
        port: 8443,
        user: "analyst",
        database: "analytics",
        ssl: "verify-full",
        sortOrder: 131,
      });
      return {
        ...base,
        options: {
          ...base.options,
          tag: "production",
          readOnly: true,
          driverParams: {
            roles: ["reporting"],
            settings: [{ name: "max_execution_time", value: "60" }],
          },
        },
      };
    })(),
  ],
  connect: (p) => {
    validate(p);
    const databases = serverDatabases();
    const database = (p.database ?? "").trim() || "default";
    // `SELECT currentDatabase()` of the first request fails for a database that is not there.
    if (!databases.has(database))
      mockError("DB_ERROR", `Database ${database} does not exist. (UNKNOWN_DATABASE)`);
    return clickhouseSession({
      databases,
      host: (p.host ?? "").trim(),
      database,
      user: (p.user ?? "").trim() || "default",
      version: versionOf(p),
      readOnly: p.options.readOnly,
      confirm: p.options.passwordMode === "command" ? "passcode" : "password",
      tls: p.ssl !== "disable",
    });
  },
};

const lane: MockLane = { drivers: [clickhouseMock] };

export default lane;
