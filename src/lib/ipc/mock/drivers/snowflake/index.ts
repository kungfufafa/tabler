import { split } from "@/features/drivers/portable/scanner";
import {
  baseUrlProblem,
  KEY_PAIR_HOST,
  localFilePath,
  normalizeAccount,
  parameterName,
  snowflakeHost,
} from "@/features/drivers/snowflake/params";
import type { ConnectionProfile } from "../../../bindings/ConnectionProfile";
import type { ProfileInput } from "../../../bindings/ProfileInput";
import { profile } from "../../fixtures";
import { mockError } from "../../transport";
import type { MockDriver, MockLane } from "../types";
import { LEX, setsAutocommit, setsIgnoreCase, writes } from "./classify";
import { type Account, seedAccount, VERSION } from "./data";
import { AUTOCOMMIT_MESSAGE, IGNORE_CASE_MESSAGE, snowflakeSession } from "./session";

/**
 * The mock driver of the snowflake lane (Snowflake), served in `vite --mode mock` with
 * `?mockDrivers=snowflake`: the account `myorg-tabler` with the `TABLER_DEMO` shop signed in with
 * a key pair, `ANALYTICS` opened read-only with a programmatic access token, and the shop signed
 * in with a password and an MFA push (one session shared by the editor and the app). Connecting
 * checks what `entry::validate`, `SnowflakeParams::from_driver_params` and the sign-in check, in
 * the same words; the form's calls are `keyFingerprint`, `listWarehouses`, `listRoles` and
 * `listDatabases`, all reads.
 */

/** The fingerprint `keyFingerprint` answers for the seed's key (`SHA256:` and base64). */
export const KEY_FINGERPRINT = "SHA256:7Kq1n0Vb3c2mXvQ9pL4sT8wY6zR5aE1dF0gH2jK3lM4=";

const str = (v: unknown): string => (typeof v === "string" ? v.trim() : "");

type Params = Record<string, unknown>;

/** What `entry::validate`, the registry and `SnowflakeParams::from_driver_params` refuse. */
function validate(p: ConnectionProfile): Params {
  if ((p.user ?? "").trim() === "") mockError("INVALID_INPUT", "User is required");
  const params = p.options.driverParams;
  if (!params) mockError("INVALID_INPUT", "Snowflake: the account identifier is required");
  if (str(params.account) === "") mockError("INVALID_INPUT", "Snowflake: account is required");
  const auth = params.auth ?? "keyPair";
  if (!["keyPair", "pat", "oauth", "password"].includes(String(auth)))
    mockError(
      "INVALID_INPUT",
      `Snowflake settings: unknown variant \`${String(auth)}\`, expected one of \`keyPair\`, \`pat\`, \`oauth\`, \`password\``,
    );
  if (normalizeAccount(String(params.account)) === null)
    mockError(
      "INVALID_INPUT",
      `“${String(params.account).trim()}” is no Snowflake account identifier (orgname-account_name or locator.region)`,
    );
  if (auth === "keyPair" && str(params.privateKeyPath) === "")
    mockError("INVALID_INPUT", "Snowflake: key pair sign-in needs the private key file");
  if (auth === "keyPair") refuseShare(str(params.privateKeyPath));
  const base = str(params.baseUrl);
  if (base !== "") {
    const problem = baseUrlProblem(base);
    if (problem) mockError("INVALID_INPUT", `Snowflake endpoint “${base}”: ${problem}`);
    // A connections file could pair the user's key with another host.
    if (auth === "keyPair" && !snowflakeHost(base))
      mockError("INVALID_INPUT", `Snowflake endpoint “${base}”: ${KEY_PAIR_HOST}`);
  }
  const session = params.sessionParameters;
  if (session !== undefined && session !== null && typeof session === "object")
    for (const [name, value] of Object.entries(session)) {
      if (parameterName(name) === null)
        mockError("INVALID_INPUT", `“${name}” is no Snowflake session parameter name`);
      if (!["string", "number", "boolean"].includes(typeof value))
        mockError(
          "INVALID_INPUT",
          `Snowflake: session parameter ${name} takes text, a number or a boolean`,
        );
    }
  return params;
}

/** `params::local_key_path`: a share or device path is never opened. */
function refuseShare(path: string): void {
  if (!localFilePath(path))
    mockError(
      "INVALID_INPUT",
      `Snowflake: the key file “${path}” must be a full path on this machine (C:\\keys\\rsa_key.p8 or /keys/rsa_key.p8), not a network share`,
    );
}

/** `auth::auth_config` without a secret: the sign-in's own words. */
function refuseMissingSecret(auth: unknown, hasSecret: boolean): void {
  if (hasSecret) return;
  const what =
    auth === "pat"
      ? "enter the Snowflake programmatic access token"
      : auth === "oauth"
        ? "enter the Snowflake OAuth access token (or its command)"
        : auth === "password"
          ? "enter the Snowflake password"
          : null;
  if (what) mockError("AUTH_FAILED", `authentication failed: ${what}`);
}

/** One account for every session of the mock, so an edit shows in the next connection too. */
let account: Account | null = null;
const theAccount = (): Account => (account ??= seedAccount());

/** Tests: the account as seeded again. */
export function resetSnowflake(): void {
  account = null;
}

let sessions = 18436728000;

const seed = (
  over: Partial<ConnectionProfile> & { id: string; name: string },
  driverParams: Params,
  options: Partial<ConnectionProfile["options"]> = {},
): ConnectionProfile => {
  const base = profile({
    driver: "snowflake",
    host: null,
    port: null,
    ssl: "verify-full",
    ...over,
  });
  return { ...base, options: { ...base.options, ...options, driverParams } };
};

export const snowflakeMock: MockDriver = {
  driver: "snowflake",
  // `server_info`: `Snowflake ` and `CURRENT_VERSION()`.
  serverVersion: () => `Snowflake ${VERSION}`,
  seedProfiles: () => [
    seed(
      {
        id: "p-mock-snowflake",
        name: "Snowflake shop (key pair)",
        color: "blue",
        user: "TABLER_USER",
        database: "TABLER_DEMO",
        sortOrder: 140,
      },
      {
        account: "myorg-tabler",
        auth: "keyPair",
        privateKeyPath: "/Users/demo/.snowflake/rsa_key.p8",
        warehouse: "COMPUTE_WH",
        role: "SYSADMIN",
        schema: "PUBLIC",
        sessionParameters: { QUERY_TAG: "tabler", TIMEZONE: "Asia/Jakarta" },
      },
      { tag: "development" },
    ),
    seed(
      {
        id: "p-mock-snowflake-ro",
        name: "Snowflake analytics (read-only, PAT)",
        color: "red",
        user: "TABLER_USER",
        database: "ANALYTICS",
        sortOrder: 141,
      },
      {
        account: "myorg-tabler",
        auth: "pat",
        warehouse: "REPORTING_WH",
        role: "ANALYST",
        statementTimeoutSecs: 600,
      },
      { tag: "production", readOnly: true, passwordMode: "keychain" },
    ),
    seed(
      {
        id: "p-mock-snowflake-mfa",
        name: "Snowflake shop (password + MFA)",
        color: "orange",
        user: "TABLER_USER",
        database: "TABLER_DEMO",
        sortOrder: 142,
      },
      { account: "myorg-tabler", auth: "password", mfa: true, warehouse: "COMPUTE_WH" },
      { tag: "testing" },
    ),
  ],
  connect: (p) => {
    const params = validate(p);
    const readOnly = p.options.readOnly;
    // `SnowflakeSession::start`: the startup commands, checked before anything signs in.
    const startup = split(LEX.lex, p.options.startupSql ?? "").map((s) => s.sql.trim());
    if (startup.some((s) => s !== "" && setsAutocommit(s)))
      mockError("INVALID_INPUT", AUTOCOMMIT_MESSAGE);
    // They run on the app's session too.
    if (startup.some((s) => s !== "" && setsIgnoreCase(s)))
      mockError("INVALID_INPUT", IGNORE_CASE_MESSAGE);
    if (readOnly && startup.some((s) => s !== "" && writes(s)))
      mockError(
        "INVALID_INPUT",
        "Snowflake: this connection is read-only, and its startup commands write (or are not known to only read)",
      );
    // The mock has no secret to look at: a sign-in that needs one and keeps none fails.
    refuseMissingSecret(params.auth, (p.options.passwordMode ?? "keychain") !== "none");
    const a = theAccount();
    // The user's default namespace when the profile names no database; a database the role
    // cannot see leaves the session without one, as `CURRENT_DATABASE()` reads back.
    const wanted = (p.database ?? "").trim() || "TABLER_DEMO";
    const database = a.databases.some((d) => d.name === wanted) ? wanted : null;
    const schemaWanted = str(params.schema) || "PUBLIC";
    const schema =
      database !== null && a.schemas.some((s) => s.database === database && s.name === schemaWanted)
        ? schemaWanted
        : null;
    const base = str(params.baseUrl);
    return snowflakeSession({
      account: a,
      user: (p.user ?? "").trim().toUpperCase(),
      role: str(params.role) || "PUBLIC",
      warehouse: str(params.warehouse) || null,
      database,
      schema,
      readOnly,
      shared: params.auth === "password" && params.mfa === true,
      tls: base === "" || base.toLowerCase().startsWith("https:"),
      sessionId: String(sessions++),
    });
  },
  util: (method, args, input: ProfileInput) => {
    const p = input.profile;
    const params = (p.options.driverParams ?? {}) as Params;
    if (method === "keyFingerprint") {
      const given = (args as { privateKeyPath?: unknown } | null)?.privateKeyPath;
      let path: string;
      if (typeof given === "string") path = given;
      else {
        validate(p);
        if ((params.auth ?? "keyPair") !== "keyPair")
          mockError("INVALID_INPUT", "Snowflake: the profile does not sign in with a key pair");
        path = str(params.privateKeyPath);
      }
      refuseShare(path.trim());
      if (/encrypted/i.test(path))
        mockError(
          "INVALID_INPUT",
          "Snowflake: the private key is encrypted; its fingerprint shows once it signs in",
        );
      if (!/\.(p8|pem|key)$/i.test(path))
        mockError(
          "INVALID_INPUT",
          `Snowflake: cannot read the key file ${path}: No such file or directory (os error 2)`,
        );
      return { fingerprint: KEY_FINGERPRINT };
    }
    if (method === "listWarehouses" || method === "listRoles" || method === "listDatabases") {
      validate(p);
      refuseMissingSecret(
        params.auth,
        input.password !== null || p.options.passwordMode === "command",
      );
      const a = theAccount();
      if (method === "listWarehouses") return [...a.warehouses];
      if (method === "listRoles") return [...a.roles];
      return a.databases.map((d) => d.name);
    }
    return mockError("UNSUPPORTED", `unsupported: Snowflake: unknown call “${method}”`);
  },
  utilKind: (method) =>
    ["keyFingerprint", "listWarehouses", "listRoles", "listDatabases"].includes(method)
      ? "read"
      : "write",
};

const lane: MockLane = { drivers: [snowflakeMock] };

export default lane;
