import { split } from "@/features/drivers/portable/scanner";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { LEX, refusedReadOnly } from "./classify";
import { type Cluster, seedCluster, VERSION_TEXT } from "./data";
import { redact } from "./redact";
import { verticaSession } from "./session";

/**
 * The mock driver of the vertica lane (Vertica), served in `vite --mode mock` with
 * `?mockDrivers=vertica`: a Vertica 24.1 cluster with the `VMart` sample database, reached as
 * `dbadmin` with a password, and the same cluster opened read-only over TLS with an OAuth token
 * from a command. Connecting checks what the backend's `validate`, `Dial::new` and
 * `setup_statements` check, in the same words; there are no calls of the form (`util`) and no
 * session calls.
 */

const str = (p: ConnectionProfile, key: string): string => {
  const v = p.options.driverParams?.[key];
  return typeof v === "string" ? v.trim() : "";
};

/** What `entry::validate`, `VerticaParams::check` and `Dial::new` refuse, with their words. */
function validate(p: ConnectionProfile): void {
  const params = p.options.driverParams ?? {};
  const auth = params.auth ?? "password";
  if (auth !== "password" && auth !== "oauth")
    mockError(
      "INVALID_INPUT",
      `Vertica settings: unknown variant \`${String(auth)}\`, expected \`password\` or \`oauth\``,
    );
  for (const [what, key] of [
    ["client label", "clientLabel"],
    ["workload", "workload"],
  ] as const) {
    const v = str(p, key);
    if (v !== "" && ([...v].length > 128 || /\p{Cc}/u.test(v)))
      mockError("INVALID_INPUT", `Vertica ${what} “${v}”: up to 128 characters, no line breaks`);
  }
  const cert = (p.options.sslCertPath ?? "").trim() !== "";
  const key = (p.options.sslKeyPath ?? "").trim() !== "";
  if (cert !== key)
    mockError(
      "INVALID_INPUT",
      "Vertica: a client certificate needs both its certificate and its key file",
    );
  if (auth === "oauth") {
    if (p.options.passwordMode === "none")
      mockError(
        "INVALID_INPUT",
        "Vertica: an OAuth token comes from the password field or its Command",
      );
    if (p.ssl === "disable" || p.ssl === "prefer")
      mockError(
        "INVALID_INPUT",
        "Vertica: an OAuth token is sent only over TLS (set SSL mode to Require or stricter)",
      );
  }
  if ((p.user ?? "").trim() === "") mockError("INVALID_INPUT", "User is required");
  if ((p.database ?? "").trim() === "") mockError("INVALID_INPUT", "Database is required");
}

/** One cluster for every session of the mock, so an edit shows in the next connection too. */
let cluster: Cluster | null = null;
const theCluster = (): Cluster => (cluster ??= seedCluster());

/** Tests: the cluster as seeded again. */
export function resetVertica(): void {
  cluster = null;
}

let sessions = 0x1b2;

export const verticaMock: MockDriver = {
  driver: "vertica",
  // `ServerInfo::label`: the product and release of `version()`.
  serverVersion: () => VERSION_TEXT.replace(" v", " "),
  seedProfiles: () => [
    ((): ConnectionProfile => {
      const base = profile(
        {
          id: "p-mock-vertica",
          name: "Vertica VMart",
          driver: "vertica",
          color: "purple",
          host: "127.0.0.1",
          port: 5433,
          user: "dbadmin",
          database: "VMart",
          ssl: "prefer",
          sortOrder: 130,
        },
        "local",
      );
      return { ...base, options: { ...base.options, timezone: "UTC" } };
    })(),
    ((): ConnectionProfile => {
      const base = profile({
        id: "p-mock-vertica-oauth",
        name: "Vertica analytics (read-only, OAuth)",
        driver: "vertica",
        host: "vertica.analytics.example.com",
        port: 5433,
        user: "analyst",
        database: "VMart",
        ssl: "verify-full",
        sortOrder: 131,
      });
      return {
        ...base,
        options: {
          ...base.options,
          tag: "production",
          readOnly: true,
          passwordMode: "command",
          passwordCommand: "oidc-token vertica-analytics",
          driverParams: { auth: "oauth", clientLabel: "Tabler analytics", workload: "reporting" },
        },
      };
    })(),
  ],
  connect: (p) => {
    validate(p);
    const readOnly = p.options.readOnly;
    // `setup_statements`: a read-only session refuses a startup command that writes, before
    // anything connects. The message reaches the UI: credentials in it are masked.
    for (const st of split(LEX.lex, p.options.startupSql ?? "")) {
      const sql = st.sql.trim();
      if (readOnly && sql !== "" && refusedReadOnly(sql))
        mockError(
          "INVALID_INPUT",
          `Startup command “${redact(sql)}”: it writes (or is not known to only read), and the connection is read-only`,
        );
    }
    const c = theCluster();
    const database = (p.database ?? "").trim();
    // One database per cluster: the server refuses any other name at login.
    if (database.toLowerCase() !== c.database.toLowerCase())
      mockError("DB_ERROR", `Database "${database}" does not exist`);
    return verticaSession({
      cluster: c,
      user: (p.user ?? "").trim(),
      readOnly,
      commandPassword: p.options.passwordMode === "command",
      tls: p.ssl !== "disable",
      clientLabel: str(p, "clientLabel") || "Tabler",
      sessionId: `v_vmart_node0001-3104:0x${(sessions++).toString(16)}`,
    });
  },
};

const lane: MockLane = { drivers: [verticaMock] };

export default lane;
