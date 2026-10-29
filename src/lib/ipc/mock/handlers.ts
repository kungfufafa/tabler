import {
  CORE_DRIVERS,
  CORE_KINDS,
  extraSecrets as driverExtraSecrets,
  isCoreKind,
  KIND_DESCRIPTORS,
  kindOf,
} from "@/features/drivers/catalog";
import type { ApplyRequest } from "../bindings/ApplyRequest";
import type { ApplyResult } from "../bindings/ApplyResult";
import type { BrowsePage } from "../bindings/BrowsePage";
import type { BrowseRequest } from "../bindings/BrowseRequest";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { ExtraSecret } from "../bindings/ExtraSecret";
import type { FilterGroup } from "../bindings/FilterGroup";
import type { ProfileInput } from "../bindings/ProfileInput";
import type { RowCount } from "../bindings/RowCount";
import type { RowIdentity } from "../bindings/RowIdentity";
import type { RowKey } from "../bindings/RowKey";
import type { RowResult } from "../bindings/RowResult";
import type { StagedChange } from "../bindings/StagedChange";
import type { TableRef } from "../bindings/TableRef";
import type { Value } from "../bindings/Value";
import { mockAdminDatabases, mockAdminSchemas } from "./admin";
import {
  mockBootstrapScript,
  mockCreateSqliteFile,
  mockDriverOptions,
  mockPasswordCommand,
  mockSocketCheck,
  mockSqliteKey,
  mockSshPrompt,
} from "./connect_extras";
import { mockCheckHostKey } from "./export";
import { SEED_PROFILES, tableMetaFor, tablesFor } from "./fixtures";
import { matchesGroup, rowsFor, sortRows } from "./rows";
import { mockEmit, mockError, registerMock } from "./transport";

const profiles = new Map<string, ConnectionProfile>(SEED_PROFILES.map((p) => [p.id, p]));
const secrets = new Map<string, string>([["p-local-pg", "secret"]]);
/** The DB password the mock stores for a profile (safe-mode prompts check it). */
export const mockStoredPassword = (profileId: string): string | null =>
  secrets.get(profileId) ?? null;
/** The second secrets (key passphrase, session token) of the drivers that take them, by profile id. */
const extraSecrets = new Map<string, Partial<Record<ExtraSecret, string>>>();
export { extraSecrets as mockExtraSecrets };

/**
 * Saves the second secrets `input` carries for its driver, like `store_secrets`: absent or null
 * keeps the stored one, "" removes it, anything else replaces it. Other drivers' are not touched.
 */
function storeExtraSecrets(input: ProfileInput): void {
  const id = input.profile.id;
  const typed: Record<ExtraSecret, string | null | undefined> = {
    keyPassphrase: input.keyPassphrase,
    sessionToken: input.sessionToken,
  };
  for (const extra of driverExtraSecrets(input.profile.driver)) {
    const value = typed[extra];
    if (value === null || value === undefined) continue;
    const stored = { ...extraSecrets.get(id) };
    if (value === "") delete stored[extra];
    else stored[extra] = value;
    extraSecrets.set(id, stored);
  }
}
const connections = new Map<string, ConnectionInfo & { profile: ConnectionProfile }>();
/** "Ask every time" passwords typed this session, by profile id (see mock/connections_io.ts). */
const sessionPasswords = new Map<string, string>();
export { profiles as mockProfiles, secrets as mockSecrets };

const uuid = () => crypto.randomUUID();
const now = () => new Date().toISOString();

function serverVersion(p: ConnectionProfile): string {
  return p.driver === "postgres"
    ? "PostgreSQL 17.6 (Homebrew) on aarch64-apple-darwin"
    : p.driver === "mysql"
      ? "MySQL 9.6.0"
      : "SQLite 3.54.0";
}

/** The SSH tunnel settings a profile connects through (SQLite files never use one). */
function sshOf(p: ConnectionProfile) {
  return p.driver === "sqlite" ? null : p.ssh;
}

/**
 * Mirrors `registry::ensure_built`: a database this build lacks (the mock has the core ones) is
 * refused before its script, password command or tunnel would run.
 */
function ensureBuilt(p: ConnectionProfile): void {
  const kind = kindOf(p);
  if (!isCoreKind(kind))
    mockError(
      "UNSUPPORTED",
      `unsupported: ${KIND_DESCRIPTORS[kind].label} is not built into this Tabler`,
    );
}

function conn(connectionId: string) {
  const c = connections.get(connectionId);
  if (!c) mockError("NOT_CONNECTED", "not connected");
  return c;
}

// The mock is a build of the core drivers (their databases in the picker's order).
registerMock("app_info", () => ({
  version: "0.1.0",
  platform: "mock",
  debug: true,
  drivers: [...CORE_DRIVERS],
  kinds: [...CORE_KINDS],
}));
// The browser has no app to quit; `beforeunload` guards the tab instead.
registerMock("quit_app", () => undefined);
registerMock("set_unsaved", () => undefined);

/** SQLite files created with "New…" in this session (paths containing "missing" do not exist). */
const createdSqliteFiles = new Set<string>();

/**
 * Mirrors the backend's `check_database_file` (never connect to, and so create, a missing file):
 * the error message for a missing SQLite file, else null.
 */
function missingSqliteFile(p: ConnectionProfile): string | null {
  if (p.driver !== "sqlite") return null;
  const path = p.sqlitePath ?? "";
  if (!path.includes("missing") || createdSqliteFiles.has(path)) return null;
  return `SQLite database file not found: ${path}. Use “New…” to create one.`;
}

registerMock("create_sqlite_database", ({ path, key }) => {
  if (typeof path !== "string" || path.trim() === "")
    mockError("INVALID_INPUT", "SQLite file path is required");
  createdSqliteFiles.add(path);
  mockCreateSqliteFile(path, typeof key === "string" ? key : null);
});

// --- profiles ---
registerMock("list_profiles", () =>
  [...profiles.values()].sort((a, b) => a.sortOrder - b.sortOrder || a.name.localeCompare(b.name)),
);
registerMock(
  "get_profile",
  ({ id }) => profiles.get(id as string) ?? mockError("NOT_FOUND", `profile ${id}`),
);
registerMock("save_profile", ({ input }) => {
  const { profile, password, clearPassword } = input as ProfileInput;
  if (!profile.name.trim()) mockError("INVALID_INPUT", "Connection name is required");
  const existing = profiles.get(profile.id);
  const stored: ConnectionProfile = {
    ...profile,
    createdAt: existing?.createdAt ?? now(),
    updatedAt: now(),
    sortOrder: existing?.sortOrder ?? profiles.size + 1,
    lastConnectedAt: existing?.lastConnectedAt ?? null,
  };
  profiles.set(stored.id, stored);
  const keychain = (stored.options.passwordMode ?? "keychain") === "keychain";
  if (clearPassword || !keychain) secrets.delete(stored.id);
  else if (password) secrets.set(stored.id, password);
  storeExtraSecrets(input as ProfileInput);
  return stored;
});
registerMock("delete_profile", ({ id }) => {
  profiles.delete(id as string);
  secrets.delete(id as string);
  extraSecrets.delete(id as string);
});
registerMock("duplicate_profile", ({ id }) => {
  const src = profiles.get(id as string) ?? mockError("NOT_FOUND", `profile ${id}`);
  const copy: ConnectionProfile = {
    ...src,
    id: uuid(),
    name: `${src.name} copy`,
    sortOrder: profiles.size + 1,
    lastConnectedAt: null,
    createdAt: now(),
    updatedAt: now(),
  };
  profiles.set(copy.id, copy);
  const pw = secrets.get(src.id);
  if (pw) secrets.set(copy.id, pw);
  const extras = extraSecrets.get(src.id);
  if (extras) extraSecrets.set(copy.id, { ...extras });
  return copy;
});
registerMock("reorder_profiles", ({ ids }) => {
  (ids as string[]).forEach((id, i) => {
    const p = profiles.get(id);
    if (p) profiles.set(id, { ...p, sortOrder: i + 1 });
  });
});
registerMock("get_secret_status", ({ id }) => ({
  hasPassword: secrets.has(id as string),
  hasSshPassword: false,
  hasSshPassphrase: false,
  hasKeyPassphrase: extraSecrets.get(id as string)?.keyPassphrase !== undefined,
  hasSessionToken: extraSecrets.get(id as string)?.sessionToken !== undefined,
}));
registerMock("test_connection", async ({ input }) => {
  const { profile, password } = input as ProfileInput;
  await new Promise((r) => setTimeout(r, 300));
  const failed = (message: string) => ({
    ok: false,
    serverVersion: null,
    latencyMs: 0,
    message,
    tunnelUsed: profile.ssh !== null,
  });
  ensureBuilt(profile);
  try {
    mockBootstrapScript(profile);
  } catch (err) {
    return failed(err instanceof Error ? err.message : String(err));
  }
  mockDriverOptions(profile);
  const missing = missingSqliteFile(profile);
  if (missing)
    return { ok: false, serverVersion: null, latencyMs: 0, message: missing, tunnelUsed: false };
  try {
    mockSqliteKey(profile, password ?? secrets.get(profile.id) ?? sessionPasswords.get(profile.id));
  } catch (err) {
    return failed(err instanceof Error ? err.message : String(err));
  }
  mockSocketCheck(profile);
  mockPasswordCommand(profile);
  // SSH profiles verify the server's host key first (unknown keys are parked under the profile id),
  // then answer the server's 2FA question, if it asks one.
  const ssh = sshOf(profile);
  mockCheckHostKey(profile.id, ssh);
  await mockSshPrompt(profile, "test");
  if ((profile.host ?? "").includes("bad")) {
    return {
      ok: false,
      serverVersion: null,
      latencyMs: 312,
      message: "connection refused (mock)",
      tunnelUsed: ssh !== null,
    };
  }
  return {
    ok: true,
    serverVersion: serverVersion(profile),
    latencyMs: ssh ? 48 : 12,
    message: `Connected to ${serverVersion(profile)}`,
    tunnelUsed: ssh !== null,
  };
});

// --- connections ---
registerMock("connect", async ({ profileId, password, databaseOverride }) => {
  const p = profiles.get(profileId as string) ?? mockError("NOT_FOUND", `profile ${profileId}`);
  await new Promise((r) => setTimeout(r, 250));
  ensureBuilt(p);
  if (p.options.passwordMode === "ask") {
    const typed = typeof password === "string" ? password : sessionPasswords.get(p.id);
    if (typed === undefined)
      mockError("AUTH_FAILED", `authentication failed: enter the password of “${p.name}”`);
    if (typed === "wrong") mockError("AUTH_FAILED", "authentication failed: wrong password");
    sessionPasswords.set(p.id, typed);
  }
  mockBootstrapScript(p);
  mockDriverOptions(p);
  const missing = missingSqliteFile(p);
  if (missing) mockError("NOT_FOUND", `not found: ${missing}`);
  mockSqliteKey(
    p,
    p.options.passwordMode === "ask"
      ? sessionPasswords.get(p.id)
      : p.options.passwordMode === "none"
        ? null
        : ((password as string | null | undefined) ?? secrets.get(p.id)),
  );
  mockSocketCheck(p);
  mockPasswordCommand(p);
  const ssh = sshOf(p);
  mockCheckHostKey(p.id, ssh);
  await mockSshPrompt(p, "connect");
  if ((p.host ?? "").includes("bad"))
    mockError("CONNECT_FAILED", `could not connect to ${p.host}:${p.port}: connection refused`);
  const info: ConnectionInfo = {
    connectionId: uuid(),
    profileId: p.id,
    name: p.name,
    color: p.color,
    driver: p.driver,
    serverVersion: serverVersion(p),
    currentDatabase:
      p.driver === "sqlite"
        ? p.sqlitePath
        : ((databaseOverride as string | null | undefined) ?? p.database ?? "postgres"),
    currentUser: p.user,
    tunnel: ssh ? { localPort: 50000 + (connections.size % 1000) } : null,
    // Another encoding than UTF-8 makes the session read-only (see `set_connection_encoding`).
    readOnly:
      p.options.readOnly ||
      (p.driver !== "sqlite" &&
        !/^(utf8|utf-8|utf8mb3|utf8mb4|unicode)?$/i.test(p.options.encoding?.trim() ?? "")),
  };
  connections.set(info.connectionId, { ...info, profile: p });
  profiles.set(p.id, { ...p, lastConnectedAt: now() });
  queueMicrotask(() =>
    mockEmit("connection:status", {
      connectionId: info.connectionId,
      status: "connected",
      message: null,
    }),
  );
  return info;
});
registerMock("disconnect", ({ connectionId }) => {
  connections.delete(connectionId as string);
  mockEmit("connection:status", { connectionId, status: "disconnected", message: null });
});
registerMock("list_connections", () =>
  [...connections.values()].map(({ profile: _p, ...info }) => info),
);
registerMock("ping", ({ connectionId }) =>
  connections.has(connectionId as string) ? "connected" : "disconnected",
);
registerMock("switch_database", ({ connectionId, database }) => {
  const c = conn(connectionId as string);
  c.currentDatabase = database as string;
  const { profile: _p, ...info } = c;
  return info;
});
/** View Using Encoding…: the live session's encoding, by connection id. */
const sessionEncodings = new Map<string, string>();
const defaultEncoding = (c: ConnectionInfo & { profile: ConnectionProfile }) =>
  c.profile.options.encoding || (c.driver === "mysql" ? "utf8mb4" : "UTF8");
registerMock("get_connection_encoding", ({ connectionId }) => {
  const c = conn(connectionId as string);
  if (c.driver === "sqlite")
    mockError("INVALID_INPUT", "SQLite text is always UTF-8: there is no connection encoding");
  return sessionEncodings.get(c.connectionId) ?? defaultEncoding(c);
});
registerMock("set_connection_encoding", async ({ connectionId, encoding }) => {
  const c = conn(connectionId as string);
  if (c.driver === "sqlite")
    mockError("INVALID_INPUT", "SQLite text is always UTF-8: there is no connection encoding");
  const enc = typeof encoding === "string" ? encoding.trim() : "";
  if (enc !== "" && !/^[A-Za-z0-9_-]{1,40}$/.test(enc))
    mockError("INVALID_INPUT", `“${enc}” is not an encoding name`);
  if (enc.toLowerCase() === "klingon")
    mockError("DB_ERROR", `invalid value for parameter "client_encoding": "${enc}"`);
  await new Promise((r) => setTimeout(r, 200));
  if (enc === "") sessionEncodings.delete(c.connectionId);
  else sessionEncodings.set(c.connectionId, enc);
  // Tabler reads and writes UTF-8 only: another encoding makes the session read-only (the
  // backend's `effective_read_only`).
  const utf8 = /^(utf8|utf-8|utf8mb3|utf8mb4|unicode)$/i.test(
    sessionEncodings.get(c.connectionId) ?? defaultEncoding(c),
  );
  c.readOnly = c.profile.options.readOnly || !utf8;
  const { profile: _p, ...info } = c;
  return info;
});

// --- schema ---
registerMock("list_databases", ({ connectionId }) => {
  const c = conn(connectionId as string);
  const names =
    c.driver === "postgres"
      ? ["tabler_dev", "postgres", "ecommerce_sales"]
      : c.driver === "mysql"
        ? ["tabler_dev", "tabler_inv", "shop", "sys"]
        : [c.profile.sqlitePath ?? "sample.sqlite"];
  // The server always lists the database the session is connected to.
  if (c.currentDatabase && !names.includes(c.currentDatabase)) names.unshift(c.currentDatabase);
  return mockAdminDatabases(c.driver, names).map((name) => ({
    name,
    owner: c.driver === "postgres" ? "tabler" : null,
    encoding: c.driver === "sqlite" ? null : "UTF8",
    isCurrent: name === c.currentDatabase,
  }));
});
registerMock("list_schemas", ({ connectionId }) => {
  const c = conn(connectionId as string);
  if (c.driver === "postgres")
    return mockAdminSchemas(c.connectionId, c.currentDatabase, [
      { name: "public", isDefault: true },
      { name: "inventory", isDefault: false },
    ]);
  if (c.driver === "mysql") return [{ name: c.currentDatabase ?? "tabler_dev", isDefault: true }];
  return [
    { name: "main", isDefault: true },
    { name: "temp", isDefault: false },
  ];
});
registerMock("list_tables", ({ connectionId, schema }) => {
  const c = conn(connectionId as string);
  if (schema === "temp") return [];
  return tablesFor(c.driver, schema as string);
});
registerMock("get_table", ({ connectionId, table }) => {
  const c = conn(connectionId as string);
  const t = table as TableRef;
  return (
    tableMetaFor(c.driver, t.schema ?? "public", t.name) ??
    mockError("NOT_FOUND", `table ${t.name}`)
  );
});
registerMock(
  "get_view_definition",
  ({ table }) => `SELECT * FROM users WHERE is_active -- ${(table as TableRef).name}`,
);
registerMock("get_completion_metadata", ({ connectionId }) => {
  const c = conn(connectionId as string);
  const schemas: Record<string, Record<string, string[]>> = {};
  const names =
    c.driver === "postgres"
      ? ["public", "inventory"]
      : c.driver === "mysql"
        ? [c.currentDatabase ?? "tabler_dev"]
        : ["main"];
  for (const s of names) {
    schemas[s] = Object.fromEntries(
      tablesFor(c.driver, s).map((t) => [
        t.name,
        (tableMetaFor(c.driver, s, t.name)?.columns ?? []).map((col) => col.name),
      ]),
    );
  }
  return { defaultSchema: names[0] ?? null, schemas };
});
registerMock("invalidate_schema_cache", () => undefined);

// --- browse ---
registerMock("browse_table", async ({ connectionId, req }) => {
  const c = conn(connectionId as string);
  const { table, columns: selected, filter, sort, limit, offset } = req as BrowseRequest;
  const schema = table.schema ?? "public";
  const meta =
    tableMetaFor(c.driver, schema, table.name) ?? mockError("NOT_FOUND", `table ${table.name}`);
  const total = meta.rowEstimate ?? 25;
  const all = rowsFor(`${c.driver}:${schema}.${table.name}`, meta.columns, total);
  let rows = filter ? all.filter((r) => matchesGroup(filter, r, meta.columns)) : all;
  rows = sortRows(rows, sort ?? [], meta.columns);
  const page = rows.slice(offset, offset + limit);
  const cols = selected ? meta.columns.filter((col) => selected.includes(col.name)) : meta.columns;
  const idx = cols.map((col) => meta.columns.indexOf(col));
  const identity: RowIdentity =
    meta.primaryKey.length > 0 ? "pk" : c.driver === "sqlite" ? "rowid" : "none";
  // The generator is O(rows); simulate the backend's latency for big tables so loading states show.
  if (total > 50_000) await new Promise((r) => setTimeout(r, 120));
  return {
    columns: cols,
    rows: page.map((r) => idx.map((i) => r[i]!)),
    pkColumns: meta.primaryKey,
    identity,
    rowIds: identity === "rowid" ? page.map((_, i) => offset + i + 1) : null,
    offset,
    limit,
    sql: `SELECT ${cols.map((col) => `"${col.name}"`).join(", ")} FROM "${schema}"."${table.name}"${sort?.length ? ` ORDER BY ${sort.map((s) => `"${s.column}" ${s.dir.toUpperCase()}`).join(", ")}` : ""} LIMIT ${limit} OFFSET ${offset}`,
    durationMs: total > 50_000 ? 118 : 4,
  } satisfies BrowsePage;
});
registerMock("count_table_rows", ({ connectionId, table, filter, exact, estimateAbove }) => {
  const c = conn(connectionId as string);
  const t = table as TableRef;
  const schema = t.schema ?? "public";
  const meta = tableMetaFor(c.driver, schema, t.name) ?? mockError("NOT_FOUND", `table ${t.name}`);
  const total = meta.rowEstimate ?? 25;
  const above = typeof estimateAbove === "number" ? estimateAbove : 500_000;
  if (!filter && !exact && total > above) return { count: total, exact: false };
  const all = rowsFor(`${c.driver}:${schema}.${t.name}`, meta.columns, total);
  const rows = filter
    ? all.filter((r) => matchesGroup(filter as FilterGroup, r, meta.columns))
    : all;
  return { count: rows.length, exact: true } satisfies RowCount;
});
registerMock("fetch_cell", ({ connectionId, table, key, column }) => {
  const c = conn(connectionId as string);
  const t = table as TableRef;
  const schema = t.schema ?? "public";
  const meta = tableMetaFor(c.driver, schema, t.name) ?? mockError("NOT_FOUND", `table ${t.name}`);
  const all = rowsFor(`${c.driver}:${schema}.${t.name}`, meta.columns, meta.rowEstimate ?? 25);
  const colIdx = meta.columns.findIndex((col) => col.name === column);
  const k = key as RowKey;
  const row =
    k.kind === "rowid"
      ? all[k.value - 1]
      : all.find((r) =>
          Object.entries(k.value).every(
            ([name, v]) =>
              JSON.stringify(r[meta.columns.findIndex((col) => col.name === name)]) ===
              JSON.stringify(v),
          ),
        );
  return row?.[colIdx] ?? { t: "null" };
});

// --- mutate ---
function literalOf(v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "default":
      return "DEFAULT";
    case "bool":
      return v.v ? "TRUE" : "FALSE";
    case "int":
    case "float":
      return String(v.v);
    case "bigInt":
    case "decimal":
      return v.v;
    case "bytes":
      return `'\\x…'::bytea`;
    case "other":
      return `'${v.v.text}'`;
    case "array":
      return `'{…}'`;
    default:
      return `'${String(v.v).replace(/'/g, "''")}'`;
  }
}

function whereOf(key: RowKey): string {
  if (key.kind === "rowid") return `rowid = ${key.value}`;
  return Object.entries(key.value)
    .map(([k, v]) => (v.t === "null" ? `"${k}" IS NULL` : `"${k}" = ${literalOf(v)}`))
    .join(" AND ");
}

function statementsOf(req: ApplyRequest): Array<{ change: StagedChange; sql: string }> {
  const t = `"${req.table.schema ?? "public"}"."${req.table.name}"`;
  const order = [
    ...req.changes.filter((c) => c.op === "delete"),
    ...req.changes.filter((c) => c.op === "update"),
    ...req.changes.filter((c) => c.op === "insert"),
  ];
  return order.map((change) => {
    if (change.op === "delete")
      return { change, sql: `DELETE FROM ${t} WHERE ${whereOf(change.key)}` };
    if (change.op === "update") {
      const sets = Object.entries(change.changes)
        .map(([k, v]) => `"${k}" = ${literalOf(v)}`)
        .join(", ");
      return { change, sql: `UPDATE ${t} SET ${sets} WHERE ${whereOf(change.key)} RETURNING *` };
    }
    const cols = Object.entries(change.values).filter(([, v]) => v.t !== "default");
    return {
      change,
      sql: `INSERT INTO ${t} (${cols.map(([k]) => `"${k}"`).join(", ")}) VALUES (${cols.map(([, v]) => literalOf(v)).join(", ")}) RETURNING *`,
    };
  });
}

registerMock("preview_changes", ({ req }) => statementsOf(req as ApplyRequest).map((s) => s.sql));

registerMock("apply_changes", async ({ connectionId, req }) => {
  const c = conn(connectionId as string);
  const r = req as ApplyRequest;
  const schema = r.table.schema ?? "public";
  const meta =
    tableMetaFor(c.driver, schema, r.table.name) ?? mockError("NOT_FOUND", `table ${r.table.name}`);
  const all = rowsFor(
    `${c.driver}:${schema}.${r.table.name}`,
    meta.columns,
    meta.rowEstimate ?? 25,
  );
  const colIdx = (name: string) => meta.columns.findIndex((col) => col.name === name);
  const findRow = (key: RowKey): number => {
    if (key.kind === "rowid") return key.value - 1;
    return all.findIndex((row) =>
      Object.entries(key.value).every(
        ([name, v]) => JSON.stringify(row[colIdx(name)]) === JSON.stringify(v),
      ),
    );
  };
  const started = performance.now();
  const statements = statementsOf(r);
  const sql = statements.map((s) => s.sql);
  const results: RowResult[] = [];
  const snapshot = all.map((row) => row.slice());
  const emailIdx = colIdx("email");
  const emails = new Set(emailIdx >= 0 ? all.map((row) => JSON.stringify(row[emailIdx])) : []);
  for (let i = 0; i < statements.length; i++) {
    const { change } = statements[i]!;
    const fail = (message: string) => {
      all.splice(0, all.length, ...snapshot);
      const error = {
        code: "DB_ERROR",
        message,
        detail: null,
        hint: null,
        sqlstate: "23505",
        vendorCode: null,
        position: null,
        statementIndex: i,
        data: null,
      };
      return {
        success: false,
        results: statements.map((s, j) => ({
          rowId: s.change.rowId,
          op: s.change.op,
          affected: 0,
          returned: null,
          error: j === i ? error : null,
        })),
        sql,
        error,
        durationMs: Math.round(performance.now() - started),
      } satisfies ApplyResult;
    };
    if (change.op === "delete") {
      const idx = findRow(change.key);
      if (idx < 0)
        return fail(
          "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else",
        );
      all.splice(idx, 1);
      results.push({ rowId: change.rowId, op: "delete", affected: 1, returned: null, error: null });
    } else if (change.op === "update") {
      const idx = findRow(change.key);
      if (idx < 0)
        return fail(
          "expected to affect 1 row(s) but matched 0; the row may have been changed or deleted by someone else",
        );
      const row = all[idx]!.slice();
      for (const [name, v] of Object.entries(change.changes)) {
        if (
          name === "email" &&
          v.t === "text" &&
          emails.has(JSON.stringify(v)) &&
          JSON.stringify(row[emailIdx]) !== JSON.stringify(v)
        ) {
          return fail(`duplicate key value violates unique constraint "users_email_key"`);
        }
        row[colIdx(name)] = v.t === "default" ? { t: "null" } : v;
      }
      all[idx] = row;
      results.push({
        rowId: change.rowId,
        op: "update",
        affected: 1,
        returned: Object.fromEntries(meta.columns.map((col, k) => [col.name, row[k]!])),
        error: null,
      });
    } else {
      const row: Value[] = meta.columns.map((col, k) => {
        const v = change.values[col.name] ?? { t: "default" };
        if (v.t !== "default") return v;
        if (col.isAutoIncrement) return { t: "int", v: all.length + 1 };
        if (col.kind === "timestampTz" || col.kind === "timestamp")
          return { t: "timestampTz", v: "2026-10-11 09:00:00+07" };
        if (col.kind === "bool") return { t: "bool", v: true };
        if (col.kind === "uuid")
          return {
            t: "uuid",
            v: `00000000-0000-4000-8000-${String(all.length + 1).padStart(12, "0")}`,
          };
        return k === 0 ? { t: "int", v: all.length + 1 } : { t: "null" };
      });
      if (emailIdx >= 0 && emails.has(JSON.stringify(row[emailIdx])))
        return fail(`duplicate key value violates unique constraint "users_email_key"`);
      all.push(row);
      results.push({
        rowId: change.rowId,
        op: "insert",
        affected: 1,
        returned: Object.fromEntries(meta.columns.map((col, k) => [col.name, row[k]!])),
        error: null,
      });
    }
  }
  return {
    success: true,
    results,
    sql,
    error: null,
    durationMs: Math.round(performance.now() - started),
  } satisfies ApplyResult;
});
