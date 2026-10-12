import { z } from "zod";
import type { ConnectionProfile, PasswordMode, ProfileInput, SafeModeLevel } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";

const port = z
  .string()
  .trim()
  .refine(
    (v) => v === "" || (/^\d+$/.test(v) && Number(v) > 0 && Number(v) < 65536),
    "Port must be 1–65535",
  );

const seconds = z
  .string()
  .trim()
  .refine((v) => v === "" || /^\d+$/.test(v), "Must be a number");

/** Password storage modes, in the order of TablePlus's menu next to Password. */
export const PASSWORD_MODES = [
  "keychain",
  "ask",
  "none",
] as const satisfies readonly PasswordMode[];
export const PASSWORD_MODE_LABEL: Record<PasswordMode, string> = {
  keychain: "Store in keychain",
  ask: "Ask every time",
  none: "No password",
};

/** TablePlus's safe-mode levels, in its order, with its exact labels. */
export const SAFE_MODES = [
  "silent",
  "alert",
  "alertNonSelect",
  "safe",
  "safeNonSelect",
] as const satisfies readonly SafeModeLevel[];
export const SAFE_MODE_LABEL: Record<SafeModeLevel, string> = {
  silent: "Silent Mode",
  alert: "Alert Mode 1",
  alertNonSelect: "Alert Mode 2",
  safe: "Safe Mode 1",
  safeNonSelect: "Safe Mode 2",
};

/** TablePlus's environment tags, in its order. */
export const CONNECTION_TAGS = [
  "local",
  "testing",
  "development",
  "staging",
  "production",
] as const;

export const connectionFormSchema = z
  .object({
    name: z.string().trim().min(1, "Name is required").max(80),
    color: z.enum(ACCENT_PRESETS),
    /** Null only for profiles saved before tags existed (until they are edited). */
    tag: z.enum(CONNECTION_TAGS).nullable(),
    group: z.string().trim().max(60),
    driver: z.enum(["postgres", "mysql", "sqlite"]),
    host: z.string().trim(),
    port,
    user: z.string().trim(),
    database: z.string().trim(),
    ssl: z.enum(["disable", "prefer", "require", "verify-ca", "verify-full"]),
    sslCaPath: z.string().trim(),
    /** TLS client certificate: private key and certificate (PEM), set together. */
    sslKeyPath: z.string().trim(),
    sslCertPath: z.string().trim(),
    sqlitePath: z.string().trim(),
    password: z.string(),
    passwordMode: z.enum(PASSWORD_MODES),
    clearPassword: z.boolean(),
    sshEnabled: z.boolean(),
    sshHost: z.string().trim(),
    /** Empty: the `Port` of ~/.ssh/config, else 22. */
    sshPort: port,
    /** Empty: the `User` of ~/.ssh/config, else the local user. */
    sshUser: z.string().trim(),
    sshAuth: z.enum(["password", "key", "agent"]),
    /** Empty: the `IdentityFile` of ~/.ssh/config, else ~/.ssh/id_ed25519 / id_ecdsa / id_rsa. */
    sshKeyPath: z.string().trim(),
    sshPassword: z.string(),
    sshPassphrase: z.string(),
    /** Empty: any free port. */
    sshLocalPort: port,
    /** Empty: every 30 s. */
    sshKeepaliveSecs: seconds,
    readOnly: z.boolean(),
    /** SQLite only: `PRAGMA foreign_keys` on every session connection. */
    sqliteForeignKeys: z.boolean(),
    timezone: z.string().trim(),
    connectTimeoutSecs: seconds,
    safeMode: z.enum(SAFE_MODES),
    /** Bootstrap commands: SQL run right after login on every connect. */
    startupSql: z.string(),
  })
  .superRefine((v, ctx) => {
    if (v.driver === "sqlite") {
      if (!v.sqlitePath)
        ctx.addIssue({ code: "custom", path: ["sqlitePath"], message: "Choose a database file" });
      return;
    }
    if (!v.host) ctx.addIssue({ code: "custom", path: ["host"], message: "Host is required" });
    if (!v.user) ctx.addIssue({ code: "custom", path: ["user"], message: "User is required" });
    if (v.ssl !== "disable") {
      if (v.sslKeyPath && !v.sslCertPath)
        ctx.addIssue({
          code: "custom",
          path: ["sslCertPath"],
          message: "Choose the certificate that goes with the key (Cert…)",
        });
      if (v.sslCertPath && !v.sslKeyPath)
        ctx.addIssue({
          code: "custom",
          path: ["sslKeyPath"],
          message: "Choose the private key of the certificate (Key…)",
        });
    }
    if (v.sshEnabled && !v.sshHost)
      ctx.addIssue({ code: "custom", path: ["sshHost"], message: "SSH host is required" });
  });

export type ConnectionFormValues = z.infer<typeof connectionFormSchema>;

export const DEFAULT_FORM: ConnectionFormValues = {
  name: "",
  color: "blue",
  tag: "local",
  group: "",
  driver: "postgres",
  host: "127.0.0.1",
  port: "",
  user: "",
  database: "",
  ssl: "prefer",
  sslCaPath: "",
  sslKeyPath: "",
  sslCertPath: "",
  sqlitePath: "",
  password: "",
  passwordMode: "keychain",
  clearPassword: false,
  sshEnabled: false,
  sshHost: "",
  sshPort: "22",
  sshUser: "",
  sshAuth: "key",
  sshKeyPath: "",
  sshPassword: "",
  sshPassphrase: "",
  sshLocalPort: "",
  sshKeepaliveSecs: "",
  readOnly: false,
  sqliteForeignKeys: true,
  timezone: "",
  connectTimeoutSecs: "",
  safeMode: "silent",
  startupSql: "",
};

/**
 * Form values for a new profile. `connectTimeout` is the "Connect timeout" preference (seconds)
 * and `safeMode` the "Default Safe Mode for new connection" preference.
 */
export function newProfileForm(
  connectTimeout: number,
  safeMode: SafeModeLevel = "silent",
): ConnectionFormValues {
  return { ...DEFAULT_FORM, port: "5432", connectTimeoutSecs: String(connectTimeout), safeMode };
}

export function profileToForm(p: ConnectionProfile): ConnectionFormValues {
  return {
    ...DEFAULT_FORM,
    name: p.name,
    color: (ACCENT_PRESETS as readonly string[]).includes(p.color ?? "")
      ? (p.color as ConnectionFormValues["color"])
      : "blue",
    tag: p.options.tag,
    group: p.group ?? "",
    driver: p.driver,
    host: p.host ?? "",
    port: p.port ? String(p.port) : "",
    user: p.user ?? "",
    database: p.database ?? "",
    ssl: p.ssl,
    sslCaPath: p.sslCaPath ?? "",
    sslKeyPath: p.options.sslKeyPath ?? "",
    sslCertPath: p.options.sslCertPath ?? "",
    sqlitePath: p.sqlitePath ?? "",
    passwordMode: p.options.passwordMode ?? "keychain",
    sshEnabled: p.ssh !== null,
    sshHost: p.ssh?.host ?? "",
    sshPort: p.ssh ? (p.ssh.port ? String(p.ssh.port) : "") : "22",
    sshUser: p.ssh?.user ?? "",
    sshAuth: p.ssh?.auth ?? "key",
    sshKeyPath: p.ssh?.keyPath ?? "",
    sshLocalPort: p.ssh?.localPort ? String(p.ssh.localPort) : "",
    sshKeepaliveSecs: p.ssh?.keepaliveSecs ? String(p.ssh.keepaliveSecs) : "",
    readOnly: p.options.readOnly,
    sqliteForeignKeys: p.options.sqliteForeignKeys,
    timezone: p.options.timezone ?? "",
    connectTimeoutSecs: p.options.connectTimeoutSecs ? String(p.options.connectTimeoutSecs) : "",
    safeMode: p.options.safeMode ?? "silent",
    startupSql: p.options.startupSql ?? "",
  };
}

const orNull = (s: string) => (s === "" ? null : s);
const numOrNull = (s: string) => (s === "" ? null : Number(s));

/**
 * Builds the backend payload. `existing` keeps ids/timestamps when editing. The typed password
 * only goes to the Keychain for "Store in keychain"; the other modes send none (an "Ask every
 * time" password is passed to the connect or test call instead).
 */
export function formToProfileInput(
  v: ConnectionFormValues,
  existing: ConnectionProfile | null,
  id: string,
): ProfileInput {
  const sqlite = v.driver === "sqlite";
  const keychain = v.passwordMode === "keychain";
  const sslFiles = !sqlite && v.ssl !== "disable";
  const profile: ConnectionProfile = {
    id,
    name: v.name,
    color: v.color,
    group: orNull(v.group),
    driver: v.driver,
    host: sqlite ? null : orNull(v.host),
    port: sqlite ? null : numOrNull(v.port),
    user: sqlite ? null : orNull(v.user),
    database: sqlite ? null : orNull(v.database),
    ssl: v.ssl,
    sslCaPath: orNull(v.sslCaPath),
    sqlitePath: sqlite ? orNull(v.sqlitePath) : null,
    ssh:
      !sqlite && v.sshEnabled
        ? {
            host: v.sshHost,
            // 0: the Port of ~/.ssh/config, else 22.
            port: v.sshPort === "" ? 0 : Number(v.sshPort),
            user: v.sshUser,
            auth: v.sshAuth,
            keyPath: v.sshAuth === "key" ? orNull(v.sshKeyPath) : null,
            localPort: numOrNull(v.sshLocalPort),
            keepaliveSecs: numOrNull(v.sshKeepaliveSecs) || null,
          }
        : null,
    options: {
      tag: v.tag,
      safeMode: v.safeMode,
      timezone: orNull(v.timezone),
      connectTimeoutSecs: numOrNull(v.connectTimeoutSecs),
      readOnly: v.readOnly,
      startupSql: v.startupSql.trim() === "" ? null : v.startupSql,
      defaultRowLimit: existing?.options.defaultRowLimit ?? null,
      sqliteForeignKeys: v.sqliteForeignKeys,
      passwordMode: v.passwordMode,
      sslKeyPath: sslFiles ? orNull(v.sslKeyPath) : null,
      sslCertPath: sslFiles ? orNull(v.sslCertPath) : null,
    },
    sortOrder: existing?.sortOrder ?? 0,
    createdAt: existing?.createdAt ?? "",
    updatedAt: existing?.updatedAt ?? "",
    lastConnectedAt: existing?.lastConnectedAt ?? null,
  };
  return {
    profile,
    password: keychain && v.password !== "" ? v.password : null,
    sshPassword: v.sshPassword === "" ? null : v.sshPassword,
    sshPassphrase: v.sshPassphrase === "" ? null : v.sshPassphrase,
    clearPassword: keychain && v.clearPassword,
  };
}

/**
 * A new connection's form prefilled from a parsed connection URL ("New connection from URL…"):
 * the URL's fields over the defaults, its password in the password field.
 */
export function urlToForm(
  parsed: { values: Partial<ConnectionFormValues>; password: string | null },
  connectTimeout: number,
  safeMode: SafeModeLevel = "silent",
): ConnectionFormValues {
  const base = newProfileForm(connectTimeout, safeMode);
  return { ...base, ...parsed.values, password: parsed.password ?? "" };
}

/**
 * The launcher row's second line, TablePlus style: `host : port : user : database` (empty parts
 * left out), or the SQLite file path.
 */
export function connectionDetail(p: ConnectionProfile): string {
  if (p.driver === "sqlite") return p.sqlitePath ?? "";
  return [p.host, p.port === null ? null : String(p.port), p.user, p.database]
    .filter((s): s is string => s !== null && s !== "")
    .join(" : ");
}

/** One-line summary for cards: `user@host:port/db` or the SQLite path. */
export function connectionSummary(p: ConnectionProfile): string {
  if (p.driver === "sqlite") return p.sqlitePath ?? "";
  const hostPort = `${p.host ?? ""}${p.port ? `:${p.port}` : ""}`;
  const db = p.database ? `/${p.database}` : "";
  return `${p.user ? `${p.user}@` : ""}${hostPort}${db}`;
}
