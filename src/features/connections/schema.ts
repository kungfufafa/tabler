import { z } from "zod";
import type { ConnectionProfile, ProfileInput } from "@/lib/ipc";
import { ACCENT_PRESETS } from "@/lib/theme";

const port = z
  .string()
  .trim()
  .refine(
    (v) => v === "" || (/^\d+$/.test(v) && Number(v) > 0 && Number(v) < 65536),
    "Port must be 1–65535",
  );

export const connectionFormSchema = z
  .object({
    name: z.string().trim().min(1, "Name is required").max(80),
    color: z.enum(ACCENT_PRESETS),
    group: z.string().trim().max(60),
    driver: z.enum(["postgres", "mysql", "sqlite"]),
    host: z.string().trim(),
    port,
    user: z.string().trim(),
    database: z.string().trim(),
    ssl: z.enum(["disable", "prefer", "require", "verify-ca", "verify-full"]),
    sslCaPath: z.string().trim(),
    sqlitePath: z.string().trim(),
    password: z.string(),
    clearPassword: z.boolean(),
    sshEnabled: z.boolean(),
    sshHost: z.string().trim(),
    sshPort: port,
    sshUser: z.string().trim(),
    sshAuth: z.enum(["password", "key", "agent"]),
    sshKeyPath: z.string().trim(),
    sshPassword: z.string(),
    sshPassphrase: z.string(),
    readOnly: z.boolean(),
    timezone: z.string().trim(),
    connectTimeoutSecs: z
      .string()
      .trim()
      .refine((v) => v === "" || /^\d+$/.test(v), "Must be a number"),
  })
  .superRefine((v, ctx) => {
    if (v.driver === "sqlite") {
      if (!v.sqlitePath)
        ctx.addIssue({ code: "custom", path: ["sqlitePath"], message: "Choose a database file" });
      return;
    }
    if (!v.host) ctx.addIssue({ code: "custom", path: ["host"], message: "Host is required" });
    if (!v.user) ctx.addIssue({ code: "custom", path: ["user"], message: "User is required" });
    if (v.sshEnabled) {
      if (!v.sshHost)
        ctx.addIssue({ code: "custom", path: ["sshHost"], message: "SSH host is required" });
      if (!v.sshUser)
        ctx.addIssue({ code: "custom", path: ["sshUser"], message: "SSH user is required" });
      if (v.sshAuth === "key" && !v.sshKeyPath)
        ctx.addIssue({
          code: "custom",
          path: ["sshKeyPath"],
          message: "Private key path is required",
        });
    }
  });

export type ConnectionFormValues = z.infer<typeof connectionFormSchema>;

export const DEFAULT_FORM: ConnectionFormValues = {
  name: "",
  color: "blue",
  group: "",
  driver: "postgres",
  host: "127.0.0.1",
  port: "",
  user: "",
  database: "",
  ssl: "prefer",
  sslCaPath: "",
  sqlitePath: "",
  password: "",
  clearPassword: false,
  sshEnabled: false,
  sshHost: "",
  sshPort: "22",
  sshUser: "",
  sshAuth: "key",
  sshKeyPath: "",
  sshPassword: "",
  sshPassphrase: "",
  readOnly: false,
  timezone: "",
  connectTimeoutSecs: "",
};

/**
 * Form values for a new profile. `connectTimeout` is the "Connect timeout" preference (seconds),
 * the default for new connections.
 */
export function newProfileForm(connectTimeout: number): ConnectionFormValues {
  return { ...DEFAULT_FORM, port: "5432", connectTimeoutSecs: String(connectTimeout) };
}

export function profileToForm(p: ConnectionProfile): ConnectionFormValues {
  return {
    ...DEFAULT_FORM,
    name: p.name,
    color: (ACCENT_PRESETS as readonly string[]).includes(p.color ?? "")
      ? (p.color as ConnectionFormValues["color"])
      : "blue",
    group: p.group ?? "",
    driver: p.driver,
    host: p.host ?? "",
    port: p.port ? String(p.port) : "",
    user: p.user ?? "",
    database: p.database ?? "",
    ssl: p.ssl,
    sslCaPath: p.sslCaPath ?? "",
    sqlitePath: p.sqlitePath ?? "",
    sshEnabled: p.ssh !== null,
    sshHost: p.ssh?.host ?? "",
    sshPort: p.ssh ? String(p.ssh.port) : "22",
    sshUser: p.ssh?.user ?? "",
    sshAuth: p.ssh?.auth ?? "key",
    sshKeyPath: p.ssh?.keyPath ?? "",
    readOnly: p.options.readOnly,
    timezone: p.options.timezone ?? "",
    connectTimeoutSecs: p.options.connectTimeoutSecs ? String(p.options.connectTimeoutSecs) : "",
  };
}

const orNull = (s: string) => (s === "" ? null : s);
const numOrNull = (s: string) => (s === "" ? null : Number(s));

/** Builds the backend payload. `existing` keeps ids/timestamps when editing. */
export function formToProfileInput(
  v: ConnectionFormValues,
  existing: ConnectionProfile | null,
  id: string,
): ProfileInput {
  const sqlite = v.driver === "sqlite";
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
            port: v.sshPort === "" ? 22 : Number(v.sshPort),
            user: v.sshUser,
            auth: v.sshAuth,
            keyPath: v.sshAuth === "key" ? orNull(v.sshKeyPath) : null,
            localPort: null,
            keepaliveSecs: null,
          }
        : null,
    options: {
      timezone: orNull(v.timezone),
      connectTimeoutSecs: numOrNull(v.connectTimeoutSecs),
      readOnly: v.readOnly,
      startupSql: existing?.options.startupSql ?? null,
      defaultRowLimit: existing?.options.defaultRowLimit ?? null,
      sqliteForeignKeys: existing?.options.sqliteForeignKeys ?? true,
    },
    sortOrder: existing?.sortOrder ?? 0,
    createdAt: existing?.createdAt ?? "",
    updatedAt: existing?.updatedAt ?? "",
    lastConnectedAt: existing?.lastConnectedAt ?? null,
  };
  return {
    profile,
    password: v.password === "" ? null : v.password,
    sshPassword: v.sshPassword === "" ? null : v.sshPassword,
    sshPassphrase: v.sshPassphrase === "" ? null : v.sshPassphrase,
    clearPassword: v.clearPassword,
  };
}

/** One-line summary for cards: `user@host:port/db` or the SQLite path. */
export function connectionSummary(p: ConnectionProfile): string {
  if (p.driver === "sqlite") return p.sqlitePath ?? "";
  const hostPort = `${p.host ?? ""}${p.port ? `:${p.port}` : ""}`;
  const db = p.database ? `/${p.database}` : "";
  return `${p.user ? `${p.user}@` : ""}${hostPort}${db}`;
}
