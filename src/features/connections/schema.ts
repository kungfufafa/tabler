import { z } from "zod";
import type {
  ConnectionProfile,
  MysqlVersion,
  PasswordMode,
  ProfileInput,
  SafeModeLevel,
  SshPasswordMode,
  SslNegotiation,
} from "@/lib/ipc";
import { ACCENT_PRESETS, HEX_COLOR, isHexColor } from "@/lib/theme";
import { normalizePath } from "./groups";

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
  "command",
] as const satisfies readonly PasswordMode[];
export const PASSWORD_MODE_LABEL: Record<PasswordMode, string> = {
  keychain: "Store in keychain",
  ask: "Ask every time",
  none: "No password",
  command: "Command Line (IAM authentication)",
};

/** The key storage modes of a SQLite database's encryption key (no command: a key is typed). */
export const SQLITE_KEY_MODES = [
  "keychain",
  "ask",
  "none",
] as const satisfies readonly PasswordMode[];

/** SSH password storage, like the database password's menu (no command). */
export const SSH_PASSWORD_MODES = [
  "keychain",
  "ask",
  "none",
] as const satisfies readonly SshPasswordMode[];

/** "SQLCipher compatibility": "" is the default (SQLCipher 4's format). */
export const SQLCIPHER_COMPATIBILITY = ["", "3", "2", "1"] as const;

/**
 * MySQL "Ver." dropdown, in TablePlus's order. Append only: a link's `driverVersion=` is an index
 * into this list (url.ts), so 0 default, 1 mariadb, 2 old, 3 singlestore.
 */
export const MYSQL_VERSIONS = [
  "default",
  "mariadb",
  "old",
  "singlestore",
] as const satisfies readonly MysqlVersion[];
export const MYSQL_VERSION_LABEL: Record<MysqlVersion, string> = {
  default: "Default",
  mariadb: "MariaDB",
  old: "Old MySQL (5.x)",
  singlestore: "SingleStore DB",
};

export const SSL_NEGOTIATIONS = ["postgres", "direct"] as const satisfies readonly SslNegotiation[];

/** Suggested connection encodings (any name the server knows works). */
export const ENCODING_CHOICES: Record<"mysql" | "postgres", readonly string[]> = {
  mysql: [
    "utf8mb4",
    "utf8mb3",
    "latin1",
    "latin2",
    "ascii",
    "cp1250",
    "cp1251",
    "cp1256",
    "greek",
    "hebrew",
    "sjis",
    "ujis",
    "euckr",
    "gbk",
    "gb2312",
    "big5",
    "binary",
  ],
  postgres: [
    "UTF8",
    "LATIN1",
    "LATIN2",
    "LATIN9",
    "WIN1250",
    "WIN1251",
    "WIN1252",
    "SQL_ASCII",
    "EUC_JP",
    "EUC_KR",
    "EUC_CN",
    "SJIS",
    "BIG5",
    "GBK",
    "KOI8R",
  ],
};

/** Longest custom tag name (the backend's `TAG_NAME_MAX`). */
export const TAG_NAME_MAX = 40;

/** The connect timeout (seconds) the backend uses when the field is left empty. */
export const DEFAULT_CONNECT_TIMEOUT_SECS = 60;

/** Where MySQL's "Use socket" looks when the socket path is left empty (the backend's order). */
export const MYSQL_SOCKET_PLACEHOLDER = "/tmp/mysql.sock";
/** The same on the SSH server ("Use socket" with "Over SSH": the path is the server's own). */
export const MYSQL_REMOTE_SOCKET_PLACEHOLDER = "/var/run/mysqld/mysqld.sock";

/** TablePlus's safe-mode levels, in its order, with its exact labels. */
export const SAFE_MODES = [
  "silent",
  "alert",
  "alertNonSelect",
  "safe",
  "safeNonSelect",
] as const satisfies readonly SafeModeLevel[];
export { SAFE_MODE_LABEL } from "@/features/safety/levels";

/** Longest group path (`Clients / Acme / Prod`) the Group field takes. */
export const GROUP_PATH_MAX = 200;

/** TablePlus's built-in environment tags, in its order (custom tags come after them). */
export const CONNECTION_TAGS = [
  "local",
  "testing",
  "development",
  "staging",
  "production",
] as const;
export type BuiltinTag = (typeof CONNECTION_TAGS)[number];

export const connectionFormSchema = z
  .object({
    name: z.string().trim().min(1, "Name is required").max(80),
    /** A preset name, or a `#rrggbb` custom colour ("Custom…": colour picker / Hex). */
    color: z.union([z.enum(ACCENT_PRESETS), z.string().regex(HEX_COLOR)]),
    /**
     * A built-in tag or a custom tag's name. Null only for profiles saved before tags existed
     * (until they are edited).
     */
    tag: z.string().trim().min(1).max(TAG_NAME_MAX).nullable(),
    /** A group path, levels separated by " / " (nested groups). */
    group: z
      .string()
      .trim()
      .refine(
        (g) => normalizePath(g).length <= GROUP_PATH_MAX,
        `Use at most ${GROUP_PATH_MAX} characters for the group path`,
      ),
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
    /** "Command Line (IAM authentication)": the shell command that prints the password. */
    passwordCommand: z.string(),
    clearPassword: z.boolean(),
    /** MySQL "Use socket": connect through a Unix socket (host and port are not used). */
    useSocket: z.boolean(),
    /** Empty: auto-detected (/tmp/mysql.sock, /var/run/mysqld/mysqld.sock, Homebrew's). */
    socketPath: z.string().trim(),
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
    /** Bootstrap CLI script: a shell script run before every connect and test. */
    bootstrapScript: z.string(),
    /** MySQL `SET NAMES` / PostgreSQL `client_encoding`; empty: utf8mb4 / UTF8. */
    encoding: z
      .string()
      .trim()
      .refine((v) => v === "" || /^[A-Za-z0-9_-]{1,40}$/.test(v), "Letters, digits, _ and - only"),
    /** SSH password storage (password authentication). */
    sshPasswordMode: z.enum(SSH_PASSWORD_MODES),
    /** SQLite: "SQLCipher compatibility" (the key is the password field). */
    sqliteCipherCompatibility: z.enum(SQLCIPHER_COMPATIBILITY),
    /** SQLite: the `.sqliterc` resource file run after open. */
    sqliteResourceFile: z.string().trim(),
    /** SQLite over SSH: upload the downloaded copy back on Commit / Disconnect. */
    sqliteSshUpload: z.boolean(),
    mysqlVersion: z.enum(MYSQL_VERSIONS),
    mysqlCleartextPlugin: z.boolean(),
    mysqlLocalInfile: z.boolean(),
    pgSslNegotiation: z.enum(SSL_NEGOTIATIONS),
    pgDisableChannelBinding: z.boolean(),
    /** PostgreSQL extra libpq-style parameters (`key=value …`). */
    pgParams: z.string(),
  })
  .superRefine((v, ctx) => {
    if (v.sshEnabled && !v.sshHost)
      ctx.addIssue({ code: "custom", path: ["sshHost"], message: "SSH host is required" });
    if (v.driver === "sqlite") {
      if (!v.sqlitePath)
        ctx.addIssue({
          code: "custom",
          path: ["sqlitePath"],
          message: v.sshEnabled
            ? "Enter the path of the database file on the SSH server"
            : "Choose a database file",
        });
      return;
    }
    if (
      v.driver === "postgres" &&
      v.pgSslNegotiation === "direct" &&
      /^(disable|prefer)$/.test(v.ssl)
    )
      ctx.addIssue({
        code: "custom",
        path: ["pgSslNegotiation"],
        message: "DIRECT needs the SSL mode require, verify-ca or verify-full",
      });
    const socket = v.driver === "mysql" && v.useSocket;
    if (!v.host && !socket)
      ctx.addIssue({ code: "custom", path: ["host"], message: "Host is required" });
    if (!v.user) ctx.addIssue({ code: "custom", path: ["user"], message: "User is required" });
    // Over SSH the tunnel forwards the socket on the SSH server (TablePlus b228): its path must
    // be a full path there (`~` and relative paths mean nothing to the forwarding).
    if (socket && v.sshEnabled && v.socketPath !== "" && !v.socketPath.startsWith("/"))
      ctx.addIssue({
        code: "custom",
        path: ["socketPath"],
        message: `Over SSH the socket is the one on the SSH server: enter its full path (e.g. ${MYSQL_REMOTE_SOCKET_PLACEHOLDER})`,
      });
    if (v.passwordMode === "command" && v.passwordCommand.trim() === "")
      ctx.addIssue({
        code: "custom",
        path: ["passwordCommand"],
        message: "Enter the command that prints the password",
      });
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
  passwordCommand: "",
  clearPassword: false,
  useSocket: false,
  socketPath: "",
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
  bootstrapScript: "",
  encoding: "",
  sshPasswordMode: "keychain",
  sqliteCipherCompatibility: "",
  sqliteResourceFile: "",
  sqliteSshUpload: false,
  mysqlVersion: "default",
  mysqlCleartextPlugin: false,
  mysqlLocalInfile: false,
  pgSslNegotiation: "postgres",
  pgDisableChannelBinding: false,
  pgParams: "",
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
    color:
      (ACCENT_PRESETS as readonly string[]).includes(p.color ?? "") || isHexColor(p.color)
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
    passwordCommand: p.options.passwordCommand ?? "",
    useSocket: p.driver === "mysql" && p.options.useSocket === true,
    socketPath: p.options.socketPath ?? "",
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
    bootstrapScript: p.options.bootstrapScript ?? "",
    encoding: p.options.encoding ?? "",
    sshPasswordMode: p.ssh?.passwordMode ?? "keychain",
    sqliteCipherCompatibility: compatibilityToForm(p.options.sqliteCipherCompatibility),
    sqliteResourceFile: p.options.sqliteResourceFile ?? "",
    sqliteSshUpload: p.options.sqliteSshUpload === true,
    mysqlVersion: p.options.mysqlVersion ?? "default",
    mysqlCleartextPlugin: p.options.mysqlCleartextPlugin === true,
    mysqlLocalInfile: p.options.mysqlLocalInfile === true,
    pgSslNegotiation: p.options.pgSslNegotiation ?? "postgres",
    pgDisableChannelBinding: p.options.pgDisableChannelBinding === true,
    pgParams: p.options.pgParams ?? "",
  };
}

function compatibilityToForm(
  v: number | null | undefined,
): ConnectionFormValues["sqliteCipherCompatibility"] {
  const s = v === null || v === undefined || v === 4 ? "" : String(v);
  return (SQLCIPHER_COMPATIBILITY as readonly string[]).includes(s)
    ? (s as ConnectionFormValues["sqliteCipherCompatibility"])
    : "";
}

/** The password modes the form offers for a driver (a SQLite key is never a command's output). */
export function passwordModesFor(driver: ConnectionFormValues["driver"]): readonly PasswordMode[] {
  return driver === "sqlite" ? SQLITE_KEY_MODES : PASSWORD_MODES;
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
  const mysql = v.driver === "mysql";
  const postgres = v.driver === "postgres";
  const keychain = v.passwordMode === "keychain";
  const sslFiles = !sqlite && v.ssl !== "disable";
  const socket = mysql && v.useSocket;
  const flag = (on: boolean) => (on ? true : null);
  const profile: ConnectionProfile = {
    id,
    name: v.name,
    color: v.color,
    group: orNull(normalizePath(v.group)),
    driver: v.driver,
    host: sqlite ? null : orNull(v.host),
    port: sqlite ? null : numOrNull(v.port),
    user: sqlite ? null : orNull(v.user),
    database: sqlite ? null : orNull(v.database),
    ssl: v.ssl,
    sslCaPath: orNull(v.sslCaPath),
    sqlitePath: sqlite ? orNull(v.sqlitePath) : null,
    ssh: v.sshEnabled
      ? {
          host: v.sshHost,
          // 0: the Port of ~/.ssh/config, else 22.
          port: v.sshPort === "" ? 0 : Number(v.sshPort),
          user: v.sshUser,
          auth: v.sshAuth,
          keyPath: v.sshAuth === "key" ? orNull(v.sshKeyPath) : null,
          // SQLite over SSH downloads the file: there is no tunnel port.
          localPort: sqlite ? null : numOrNull(v.sshLocalPort),
          keepaliveSecs: numOrNull(v.sshKeepaliveSecs) || null,
          passwordMode: v.sshAuth === "password" ? v.sshPasswordMode : null,
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
      passwordCommand:
        !sqlite && v.passwordMode === "command" ? orNull(v.passwordCommand.trim()) : null,
      useSocket: socket ? true : null,
      socketPath: socket ? orNull(v.socketPath) : null,
      bootstrapScript: v.bootstrapScript.trim() === "" ? null : v.bootstrapScript,
      encoding: sqlite ? null : orNull(v.encoding),
      sqliteCipherCompatibility:
        sqlite && v.sqliteCipherCompatibility !== "" ? Number(v.sqliteCipherCompatibility) : null,
      sqliteResourceFile: sqlite ? orNull(v.sqliteResourceFile) : null,
      sqliteSshUpload: flag(sqlite && v.sshEnabled && v.sqliteSshUpload),
      mysqlVersion: mysql && v.mysqlVersion !== "default" ? v.mysqlVersion : null,
      mysqlCleartextPlugin: flag(mysql && v.mysqlCleartextPlugin),
      mysqlLocalInfile: flag(mysql && v.mysqlLocalInfile),
      pgSslNegotiation: postgres && v.pgSslNegotiation === "direct" ? "direct" : null,
      pgDisableChannelBinding: flag(postgres && v.pgDisableChannelBinding),
      pgParams: postgres && v.pgParams.trim() !== "" ? v.pgParams.trim() : null,
      // Not on the form (a link's `lazyload`, the sidebar's own toggle): editing keeps it.
      lazyLoad: existing?.options.lazyLoad ?? null,
    },
    sortOrder: existing?.sortOrder ?? 0,
    createdAt: existing?.createdAt ?? "",
    updatedAt: existing?.updatedAt ?? "",
    lastConnectedAt: existing?.lastConnectedAt ?? null,
  };
  const sshPasswordKept = v.sshAuth !== "password" || v.sshPasswordMode === "keychain";
  return {
    profile,
    password: keychain && v.password !== "" ? v.password : null,
    sshPassword: v.sshPassword === "" || !sshPasswordKept ? null : v.sshPassword,
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
  const where = usesSocket(p)
    ? [p.options.socketPath || "socket"]
    : [p.host, p.port === null ? null : String(p.port)];
  return [...where, p.user, p.database]
    .filter((s): s is string => s !== null && s !== "")
    .join(" : ");
}

/** One-line summary for cards: `user@host:port/db` or the SQLite path. */
export function connectionSummary(p: ConnectionProfile): string {
  if (p.driver === "sqlite") return p.sqlitePath ?? "";
  const hostPort = usesSocket(p)
    ? (p.options.socketPath ?? "") || "socket"
    : `${p.host ?? ""}${p.port ? `:${p.port}` : ""}`;
  const db = p.database ? `/${p.database}` : "";
  return `${p.user ? `${p.user}@` : ""}${hostPort}${db}`;
}

/** A MySQL profile that connects through a Unix socket ("Use socket"). */
export function usesSocket(p: Pick<ConnectionProfile, "driver" | "options">): boolean {
  return p.driver === "mysql" && p.options.useSocket === true;
}
