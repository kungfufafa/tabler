import { isWindows } from "@/lib/platform";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { serverDatabases } from "./data";
import { refusedReadOnly, sqlserverSession } from "./session";

/**
 * The mock driver of the sqlserver lane (SQL Server), served in `vite --mode mock` with
 * `?mockDrivers=sqlserver`: a SQL Server 2022 with `shop`, `reporting` and `master`, reached with
 * a SQL Server login, and an Azure SQL Database opened read-only with an Entra ID token from a
 * command. Connecting checks what the backend's `validate` and `open` check, in the same words;
 * there are no calls of the form (`util`) and no session calls.
 */

const str = (p: ConnectionProfile, key: string): string => {
  const v = p.options.driverParams?.[key];
  return typeof v === "string" ? v.trim() : "";
};

/** What `entry::validate` and `MssqlParams::check` refuse, with their words. */
function validate(p: ConnectionProfile): void {
  const params = p.options.driverParams ?? {};
  const auth = typeof params.auth === "string" ? params.auth : "sqlServer";
  if (!["sqlServer", "windows", "integrated", "entraToken"].includes(auth))
    mockError("INVALID_INPUT", `SQL Server settings: unknown variant \`${auth}\``);
  const size = params.packetSize;
  if (typeof size === "number" && (size < 512 || size > 32767))
    mockError("INVALID_INPUT", `SQL Server packet size: ${size} is not between 512 and 32767`);
  const instance = str(p, "instance");
  // Rust's `str::len`: 16 UTF-8 bytes, not 16 characters.
  const bytes = new TextEncoder().encode(instance).length;
  if (instance !== "" && (bytes > 16 || !/^[\p{L}\p{N}_$#]+$/u.test(instance)))
    mockError(
      "INVALID_INPUT",
      `SQL Server instance name “${instance}”: up to 16 letters, digits, _, $ or #`,
    );
  if ((auth === "windows" || auth === "integrated") && !isWindows)
    mockError(
      "UNSUPPORTED",
      "unsupported: Windows and integrated authentication work in the Windows build of Tabler only",
    );
  if ((auth === "sqlServer" || auth === "windows") && (p.user ?? "").trim() === "")
    mockError("INVALID_INPUT", "User is required for this authentication");
  const cert = (p.options.sslCertPath ?? "").trim() !== "";
  const key = (p.options.sslKeyPath ?? "").trim() !== "";
  if (cert !== key)
    mockError(
      "INVALID_INPUT",
      "SQL Server: a client certificate needs both its certificate and its key file",
    );
}

/** An Azure SQL host: the engine edition the backend reads (5) names it. */
const isAzure = (p: ConnectionProfile) => /\.database\.windows\.net$/i.test(p.host ?? "");

/** `ServerInfo::label`. */
const version = (p: ConnectionProfile) =>
  isAzure(p) ? "Azure SQL Database (12.0.2000.8)" : "SQL Server 2022 (16.0.4135.4)";

// Above the other sessions' ids (`session.ts`), so a kill never meets the caller's own.
let spids = 80;

export const sqlserverMock: MockDriver = {
  driver: "sqlserver",
  serverVersion: version,
  seedProfiles: () => [
    profile(
      {
        id: "p-mock-sqlserver",
        name: "SQL Server shop",
        driver: "sqlserver",
        color: "red",
        host: "127.0.0.1",
        port: 1433,
        user: "sa",
        database: "shop",
        ssl: "require",
        sortOrder: 120,
      },
      "local",
    ),
    ((): ConnectionProfile => {
      const base = profile({
        id: "p-mock-azure-sql",
        name: "Azure SQL reporting (read-only)",
        driver: "sqlserver",
        host: "contoso-reports.database.windows.net",
        port: 1433,
        user: null,
        database: "reporting",
        ssl: "verify-full",
        sortOrder: 121,
      });
      return {
        ...base,
        options: {
          ...base.options,
          tag: "production",
          readOnly: true,
          passwordMode: "command",
          passwordCommand:
            "az account get-access-token --resource https://database.windows.net/ --query accessToken -o tsv",
          driverParams: { auth: "entraToken" },
        },
      };
    })(),
  ],
  connect: (p) => {
    validate(p);
    const readOnly = p.options.readOnly;
    // A read-only profile whose startup commands write is refused before anything connects.
    if (readOnly && p.options.startupSql && refusedReadOnly(p.options.startupSql))
      mockError(
        "INVALID_INPUT",
        "SQL Server: this connection is read-only, and its startup commands write (or are not known to only read)",
      );
    const databases = serverDatabases();
    const database = (p.database ?? "").trim() || "master";
    if (!databases.has(database))
      // Msg 4060, which is no login error to `errors::AUTH_CODES`.
      mockError(
        "DB_ERROR",
        `Cannot open database "${database}" requested by the login. The login failed.`,
      );
    const auth = p.options.driverParams?.auth;
    return sqlserverSession({
      databases,
      database,
      readOnly,
      commandPassword: p.options.passwordMode === "command",
      tls: p.ssl !== "disable" || p.options.driverParams?.strict === true,
      version: version(p),
      login:
        auth === "entraToken"
          ? "reports@contoso.com"
          : auth === "integrated"
            ? "CORP\\me"
            : (p.user ?? "").trim(),
      application: str(p, "applicationName") || "Tabler",
      spid: spids++,
    });
  },
};

const lane: MockLane = { drivers: [sqlserverMock] };

export default lane;
