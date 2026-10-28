import type { AdminDdl } from "../bindings/AdminDdl";
import type { AdminResult } from "../bindings/AdminResult";
import type { ConnectionInfo } from "../bindings/ConnectionInfo";
import type { DatabaseOptions } from "../bindings/DatabaseOptions";
import type { Driver } from "../bindings/Driver";
import type { KillOutcome } from "../bindings/KillOutcome";
import type { KillScope } from "../bindings/KillScope";
import type { ObjectPrivileges } from "../bindings/ObjectPrivileges";
import type { ProcessRow } from "../bindings/ProcessRow";
import type { ResourceLimits } from "../bindings/ResourceLimits";
import type { RoleAttributes } from "../bindings/RoleAttributes";
import type { SchemaInfo } from "../bindings/SchemaInfo";
import type { UserChange } from "../bindings/UserChange";
import type { UserDetail } from "../bindings/UserDetail";
import type { UserRef } from "../bindings/UserRef";
import type { UserState } from "../bindings/UserState";
import type { UserSummary } from "../bindings/UserSummary";
import { mockError, mockInvoke, registerMock } from "./transport";

/**
 * In-memory server administration for `pnpm dev:mock`: databases and schemas created / renamed /
 * dropped from ⌘K show up in `list_databases` / `list_schemas` (handlers.ts applies the overlay),
 * a process list whose sessions age and can be killed, and accounts with privileges whose edits
 * go through a TypeScript copy of the backend's diff (sqlgen/admin.rs), passwords masked.
 */

const MASK = "'********'";

async function connection(connectionId: unknown): Promise<ConnectionInfo> {
  const all = await mockInvoke<ConnectionInfo[]>("list_connections", {});
  const c = all.find((x) => x.connectionId === connectionId);
  if (!c) mockError("NOT_CONNECTED", "not connected");
  return c;
}

const quoteIdent = (driver: Driver, s: string) =>
  driver === "mysql" ? `\`${s.replace(/`/g, "``")}\`` : `"${s.replace(/"/g, '""')}"`;
const quoteLit = (driver: Driver, s: string) =>
  driver === "mysql"
    ? `'${s.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}'`
    : s.includes("\\")
      ? `E'${s.replace(/'/g, "''").replace(/\\/g, "\\\\")}'`
      : `'${s.replace(/'/g, "''")}'`;

// --- databases and schemas -------------------------------------------------------------------

interface Overlay {
  created: string[];
  dropped: Set<string>;
  renamed: Map<string, string>;
}

/** Per driver (the mock's servers are shared by every connection of a driver). */
const databaseOverlay = new Map<Driver, Overlay>();
/** Per connection + database. */
const schemaOverlay = new Map<string, Overlay>();

function overlayOf<K>(map: Map<K, Overlay>, key: K): Overlay {
  let o = map.get(key);
  if (!o) {
    o = { created: [], dropped: new Set(), renamed: new Map() };
    map.set(key, o);
  }
  return o;
}

function applyOverlay(names: readonly string[], o: Overlay | undefined): string[] {
  if (!o) return [...names];
  const out = names
    .map((n) => o.renamed.get(n) ?? n)
    .filter((n) => !o.dropped.has(n))
    .concat(o.created.filter((n) => !o.dropped.has(n)));
  return [...new Set(out)];
}

/** `list_databases` after the admin edits of this session (called by handlers.ts). */
export function mockAdminDatabases(driver: Driver, names: readonly string[]): string[] {
  return applyOverlay(names, databaseOverlay.get(driver));
}

/** `list_schemas` (PostgreSQL) after the admin edits of this session (called by handlers.ts). */
export function mockAdminSchemas(
  connectionId: string,
  database: string | null,
  schemas: readonly SchemaInfo[],
): SchemaInfo[] {
  const o = schemaOverlay.get(`${connectionId}\u0000${database ?? ""}`);
  const names = applyOverlay(
    schemas.map((s) => s.name),
    o,
  );
  return names.map((name) => ({
    name,
    isDefault: schemas.find((s) => s.name === name)?.isDefault ?? false,
  }));
}

async function currentDatabases(c: ConnectionInfo): Promise<string[]> {
  const list = await mockInvoke<Array<{ name: string }>>("list_databases", {
    connectionId: c.connectionId,
  });
  return list.map((d) => d.name);
}

async function currentSchemas(c: ConnectionInfo): Promise<string[]> {
  const list = await mockInvoke<SchemaInfo[]>("list_schemas", { connectionId: c.connectionId });
  return list.map((s) => s.name);
}

const PG_OPTIONS: DatabaseOptions = {
  encodings: ["UTF8", "SQL_ASCII", "LATIN1", "LATIN2", "LATIN9", "WIN1252", "EUC_JP", "KOI8R"].map(
    (name) => ({ name, description: null, defaultCollation: null }),
  ),
  collations: [
    { name: "C", encoding: null },
    { name: "POSIX", encoding: null },
    { name: "de_DE.UTF-8", encoding: "UTF8" },
    { name: "en_GB.UTF-8", encoding: "UTF8" },
    { name: "en_US.UTF-8", encoding: "UTF8" },
    { name: "id_ID.UTF-8", encoding: "UTF8" },
    { name: "en_US.ISO8859-1", encoding: "LATIN1" },
    { name: "de_DE.ISO8859-15", encoding: "LATIN9" },
  ],
  templates: ["template0", "template1", "postgres", "tabler_dev"],
  defaultEncoding: "UTF8",
  defaultCollation: "en_US.UTF-8",
};

const MYSQL_CHARSETS: Array<[string, string, string]> = [
  ["ascii", "US ASCII", "ascii_general_ci"],
  ["binary", "Binary pseudo charset", "binary"],
  ["latin1", "cp1252 West European", "latin1_swedish_ci"],
  ["utf8mb3", "UTF-8 Unicode", "utf8mb3_general_ci"],
  ["utf8mb4", "UTF-8 Unicode", "utf8mb4_0900_ai_ci"],
];

const MYSQL_OPTIONS: DatabaseOptions = {
  encodings: MYSQL_CHARSETS.map(([name, description, defaultCollation]) => ({
    name,
    description,
    defaultCollation,
  })),
  collations: [
    ["ascii_bin", "ascii"],
    ["ascii_general_ci", "ascii"],
    ["binary", "binary"],
    ["latin1_bin", "latin1"],
    ["latin1_general_ci", "latin1"],
    ["latin1_swedish_ci", "latin1"],
    ["utf8mb3_bin", "utf8mb3"],
    ["utf8mb3_general_ci", "utf8mb3"],
    ["utf8mb3_unicode_ci", "utf8mb3"],
    ["utf8mb4_0900_ai_ci", "utf8mb4"],
    ["utf8mb4_0900_as_cs", "utf8mb4"],
    ["utf8mb4_0900_bin", "utf8mb4"],
    ["utf8mb4_bin", "utf8mb4"],
    ["utf8mb4_general_ci", "utf8mb4"],
    ["utf8mb4_unicode_520_ci", "utf8mb4"],
    ["utf8mb4_unicode_ci", "utf8mb4"],
  ].map(([name, encoding]) => ({ name: name!, encoding: encoding! })),
  templates: [],
  defaultEncoding: "utf8mb4",
  defaultCollation: null,
};

function ddlSql(driver: Driver, op: AdminDdl): string {
  if (driver === "sqlite")
    mockError(
      "UNSUPPORTED",
      "unsupported: SQLite databases are files: create or remove them in the connection form",
    );
  const q = (s: string) => quoteIdent(driver, s);
  if (op.name.trim() === "") mockError("INVALID_INPUT", "Name is required");
  switch (op.op) {
    case "createDatabase": {
      let sql = `CREATE DATABASE ${q(op.name)}`;
      if (driver === "postgres") {
        if (op.template) sql += ` TEMPLATE ${q(op.template)}`;
        if (op.encoding) sql += ` ENCODING ${quoteLit(driver, op.encoding)}`;
        if (op.collation) {
          const lit = quoteLit(driver, op.collation);
          sql += ` LC_COLLATE ${lit} LC_CTYPE ${lit}`;
        }
      } else {
        if (op.encoding) sql += ` CHARACTER SET ${op.encoding}`;
        if (op.collation) sql += ` COLLATE ${op.collation}`;
      }
      return sql;
    }
    case "renameDatabase":
      if (driver !== "postgres")
        mockError("UNSUPPORTED", "unsupported: MySQL does not support renaming a database");
      return `ALTER DATABASE ${q(op.name)} RENAME TO ${q(op.newName)}`;
    case "dropDatabase":
      return `DROP DATABASE ${q(op.name)}`;
    case "createSchema":
      return `CREATE SCHEMA ${q(op.name)}${op.owner ? ` AUTHORIZATION ${q(op.owner)}` : ""}`;
    case "dropSchema":
      return `DROP SCHEMA ${q(op.name)}${op.cascade ? " CASCADE" : ""}`;
  }
}

const result = (executed: string[], t0: number): AdminResult => ({
  executed,
  failedIndex: null,
  error: null,
  durationMs: Math.round(performance.now() - t0) + 3,
});

const failed = (message: string, detail: string | null, t0: number): AdminResult => ({
  executed: [],
  failedIndex: 0,
  error: {
    code: "DB_ERROR",
    message,
    detail,
    hint: null,
    sqlstate: null,
    vendorCode: null,
    position: null,
    statementIndex: 0,
    data: null,
  },
  durationMs: Math.round(performance.now() - t0) + 2,
});

registerMock("admin_database_options", async ({ connectionId }) => {
  const c = await connection(connectionId);
  if (c.driver === "sqlite")
    mockError("UNSUPPORTED", "unsupported: SQLite has no server databases");
  return c.driver === "mysql" ? MYSQL_OPTIONS : PG_OPTIONS;
});

registerMock("admin_preview_ddl", async ({ connectionId, op }) => {
  const c = await connection(connectionId);
  return [ddlSql(c.driver, op as AdminDdl)];
});

registerMock("admin_apply_ddl", async ({ connectionId, op: raw }) => {
  const t0 = performance.now();
  const c = await connection(connectionId);
  const op = raw as AdminDdl;
  const sql = ddlSql(c.driver, op);
  if (c.readOnly) mockError("UNSUPPORTED", "unsupported: this connection is read-only");
  if ((op.op === "renameDatabase" || op.op === "dropDatabase") && op.name === c.currentDatabase)
    mockError(
      "INVALID_INPUT",
      `${op.name} is the database this connection is using: switch to another database first`,
    );
  const isDb = op.op === "createDatabase" || op.op === "renameDatabase" || op.op === "dropDatabase";
  const existing = isDb ? await currentDatabases(c) : await currentSchemas(c);
  const what = isDb ? "database" : "schema";
  const overlay = isDb
    ? overlayOf(databaseOverlay, c.driver)
    : overlayOf(schemaOverlay, `${c.connectionId}\u0000${c.currentDatabase ?? ""}`);
  switch (op.op) {
    case "createDatabase":
    case "createSchema":
      if (existing.includes(op.name))
        return failed(`${what} "${op.name}" already exists`, null, t0);
      overlay.dropped.delete(op.name);
      overlay.created.push(op.name);
      break;
    case "renameDatabase":
      if (!existing.includes(op.name))
        return failed(`database "${op.name}" does not exist`, null, t0);
      if (existing.includes(op.newName))
        return failed(`database "${op.newName}" already exists`, null, t0);
      overlay.renamed.set(op.name, op.newName);
      overlay.created = overlay.created.map((n) => (n === op.name ? op.newName : n));
      break;
    case "dropDatabase":
    case "dropSchema":
      if (!existing.includes(op.name))
        return failed(`${what} "${op.name}" does not exist`, null, t0);
      if (op.op === "dropDatabase" && op.name === "tabler_dev")
        return failed(
          `database "${op.name}" is being accessed by other users`,
          "There are 2 other sessions using the database.",
          t0,
        );
      if (op.op === "dropSchema" && !op.cascade && op.name === "public")
        return failed(
          `cannot drop schema ${op.name} because other objects depend on it`,
          "table public.users depends on schema public",
          t0,
        );
      overlay.dropped.add(op.name);
      break;
  }
  return result([sql], t0);
});

// --- process list ----------------------------------------------------------------------------

interface MockProcess extends Omit<ProcessRow, "durationMs" | "startedAt"> {
  /** When the current state started (ms epoch). */
  since: number;
  superuser?: boolean;
}

const processes = new Map<Driver, MockProcess[]>();

function seedProcesses(driver: Driver, me: ConnectionInfo): MockProcess[] {
  const now = Date.now();
  const ago = (s: number) => now - s * 1000;
  const db = me.currentDatabase ?? "tabler_dev";
  if (driver === "mysql") {
    const row = (
      id: number,
      user: string,
      host: string,
      database: string | null,
      command: string,
      secs: number,
      state: string | null,
      query: string | null,
      own = false,
    ): MockProcess => ({
      id: String(id),
      user,
      client: host,
      database,
      application: null,
      command,
      state,
      waitEvent: null,
      query,
      current: false,
      own,
      since: ago(secs),
    });
    return [
      row(
        5,
        "event_scheduler",
        "localhost",
        null,
        "Daemon",
        86_400,
        "Waiting on empty queue",
        null,
      ),
      row(18, me.currentUser ?? "root", "127.0.0.1:50112", db, "Sleep", 4, null, null, true),
      row(19, me.currentUser ?? "root", "127.0.0.1:50113", db, "Sleep", 37, null, null, true),
      row(31, "app", "10.0.4.12:41822", "shop", "Query", 42, "User sleep", "SELECT SLEEP(120)"),
      row(
        32,
        "app",
        "10.0.4.12:41830",
        "shop",
        "Query",
        7,
        "Waiting for table metadata lock",
        "ALTER TABLE orders ADD COLUMN note TEXT",
      ),
      row(40, "report", "10.0.4.20:52001", "tabler_dev", "Sleep", 912, null, null),
      row(
        44,
        "report",
        "10.0.4.20:52002",
        "tabler_dev",
        "Query",
        3,
        "Sending data",
        "SELECT customer_id, SUM(total) FROM orders GROUP BY customer_id ORDER BY 2 DESC",
      ),
    ];
  }
  const row = (
    id: number,
    user: string,
    database: string,
    application: string,
    client: string | null,
    state: string,
    secs: number,
    query: string,
    extra: Partial<MockProcess> = {},
  ): MockProcess => ({
    id: String(id),
    user,
    database,
    application,
    client,
    state,
    command: null,
    waitEvent: null,
    query,
    current: false,
    own: application === "tabler",
    since: ago(secs),
    ...extra,
  });
  return [
    row(48101, me.currentUser ?? "tabler", db, "tabler", "127.0.0.1:50211", "idle", 3, "SELECT 1"),
    row(
      48102,
      me.currentUser ?? "tabler",
      db,
      "tabler",
      "127.0.0.1:50212",
      "idle",
      61,
      "SELECT * FROM users LIMIT 300",
    ),
    row(
      48155,
      "app_rw",
      "shop",
      "rails",
      "10.0.4.12:41822",
      "active",
      42,
      "UPDATE orders SET status = 'shipped' WHERE created_at < now() - interval '30 days'",
      { waitEvent: "Lock: transactionid" },
    ),
    row(
      48160,
      "app_rw",
      "shop",
      "rails",
      "10.0.4.12:41830",
      "idle in transaction",
      185,
      "SELECT * FROM orders WHERE id = 1042 FOR UPDATE",
    ),
    row(
      48170,
      "analyst",
      "tabler_dev",
      "psql",
      "10.0.4.20:52001",
      "active",
      8,
      "SELECT pg_sleep(300)",
      {
        waitEvent: "Timeout: PgSleep",
      },
    ),
    row(48171, "postgres", "postgres", "pg_cron scheduler", null, "idle", 3600, "", {
      superuser: true,
    }),
    row(
      48180,
      "report",
      "tabler_dev",
      "Metabase",
      "10.0.4.30:60311",
      "idle",
      930,
      "SELECT count(*) FROM order_items",
    ),
  ];
}

registerMock("admin_list_processes", async ({ connectionId }) => {
  const c = await connection(connectionId);
  if (c.driver === "sqlite")
    mockError(
      "UNSUPPORTED",
      "unsupported: SQLite has no server processes: the process list is not available",
    );
  let list = processes.get(c.driver);
  if (!list) {
    list = seedProcesses(c.driver, c);
    processes.set(c.driver, list);
  }
  const now = Date.now();
  // The session that lists them (a pool connection of this app) shows its own query.
  const listing: ProcessRow =
    c.driver === "mysql"
      ? {
          id: "17",
          user: c.currentUser,
          database: c.currentDatabase,
          application: null,
          client: "127.0.0.1:50110",
          state: "executing",
          command: "Query",
          startedAt: null,
          durationMs: 0,
          waitEvent: null,
          query:
            "SELECT ID, USER, HOST, DB, COMMAND, TIME, STATE, INFO, ID = CONNECTION_ID() FROM information_schema.PROCESSLIST ORDER BY ID",
          current: true,
          own: true,
        }
      : {
          id: "48100",
          user: c.currentUser,
          database: c.currentDatabase,
          application: "tabler",
          client: "127.0.0.1:50210",
          state: "active",
          command: null,
          startedAt: new Date(now).toISOString(),
          durationMs: 1,
          waitEvent: null,
          query:
            "SELECT pid::text, usename::text, datname::text, … FROM pg_stat_activity WHERE backend_type = 'client backend' ORDER BY pid",
          current: true,
          own: true,
        };
  const rows = list.map(({ since, superuser: _s, ...p }): ProcessRow => {
    const ms = now - since;
    return {
      ...p,
      // MySQL reports whole seconds.
      durationMs: c.driver === "mysql" ? Math.floor(ms / 1000) * 1000 : ms,
      startedAt: c.driver === "mysql" ? null : new Date(since).toISOString(),
    };
  });
  return [listing, ...rows].sort((a, b) => Number(a.id) - Number(b.id));
});

registerMock("admin_kill_processes", async ({ connectionId, ids, scope }) => {
  const c = await connection(connectionId);
  if (c.readOnly) mockError("UNSUPPORTED", "unsupported: this connection is read-only");
  const list = processes.get(c.driver) ?? [];
  const outcomes: KillOutcome[] = [];
  for (const id of ids as string[]) {
    const i = list.findIndex((p) => p.id === id);
    const p = list[i];
    if (!p) {
      outcomes.push({ id, ok: false, message: "No such process: it may have ended already" });
      continue;
    }
    if (p.superuser && c.driver === "postgres") {
      outcomes.push({
        id,
        ok: false,
        message: `permission denied to ${scope === "query" ? "cancel query" : "terminate process"}: only superusers can ${scope === "query" ? "cancel queries of" : "terminate"} superuser processes`,
      });
      continue;
    }
    if ((scope as KillScope) === "connection") list.splice(i, 1);
    else if (c.driver === "mysql") {
      list[i] = { ...p, command: "Sleep", state: null, query: null, since: Date.now() };
    } else {
      list[i] = { ...p, state: "idle", waitEvent: null, since: Date.now() };
    }
    outcomes.push({ id, ok: true, message: null });
  }
  return outcomes;
});

// --- users -----------------------------------------------------------------------------------

const PG_ROLE: RoleAttributes = {
  superuser: false,
  createDb: false,
  createRole: false,
  login: false,
  replication: false,
  inherit: true,
  connectionLimit: -1,
  validUntil: null,
};
const NO_LIMITS: ResourceLimits = {
  maxQueriesPerHour: 0,
  maxUpdatesPerHour: 0,
  maxConnectionsPerHour: 0,
  maxUserConnections: 0,
};

const privs = (object: string, privileges: string[]): ObjectPrivileges => ({ object, privileges });

function pgRole(
  name: string,
  attrs: Partial<RoleAttributes>,
  extra: Partial<UserState> = {},
): UserState {
  return {
    name,
    host: null,
    globalPrivileges: [],
    limits: null,
    attributes: { ...PG_ROLE, ...attrs },
    databasePrivileges: [],
    schemaPrivileges: [],
    tablePrivileges: [],
    partialTablePrivileges: [],
    ...extra,
  };
}

function myUser(name: string, host: string, extra: Partial<UserState> = {}): UserState {
  return {
    name,
    host,
    globalPrivileges: [],
    limits: NO_LIMITS,
    attributes: null,
    databasePrivileges: [],
    schemaPrivileges: [],
    tablePrivileges: [],
    partialTablePrivileges: [],
    ...extra,
  };
}

const ALL_MYSQL = [
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "CREATE",
  "DROP",
  "RELOAD",
  "PROCESS",
  "FILE",
  "REFERENCES",
  "INDEX",
  "ALTER",
  "SHOW DATABASES",
  "SUPER",
  "CREATE TEMPORARY TABLES",
  "LOCK TABLES",
  "EXECUTE",
  "REPLICATION SLAVE",
  "REPLICATION CLIENT",
  "CREATE VIEW",
  "SHOW VIEW",
  "CREATE ROUTINE",
  "ALTER ROUTINE",
  "CREATE USER",
  "EVENT",
  "TRIGGER",
];

const users = new Map<Driver, UserState[]>([
  [
    "postgres",
    [
      pgRole(
        "tabler",
        { superuser: true, login: true, createDb: true, createRole: true, replication: true },
        {
          databasePrivileges: [privs("tabler_dev", ["CONNECT", "CREATE", "TEMPORARY"])],
          schemaPrivileges: [
            privs("public", ["USAGE", "CREATE"]),
            privs("inventory", ["USAGE", "CREATE"]),
          ],
          tablePrivileges: [privs("public", ["SELECT", "INSERT", "UPDATE", "DELETE"])],
        },
      ),
      pgRole(
        "app_rw",
        { login: true, connectionLimit: 20 },
        {
          databasePrivileges: [
            privs("tabler_dev", ["CONNECT"]),
            privs("ecommerce_sales", ["CONNECT", "TEMPORARY"]),
          ],
          schemaPrivileges: [privs("public", ["USAGE"])],
          tablePrivileges: [privs("public", ["SELECT", "INSERT", "UPDATE", "DELETE"])],
        },
      ),
      pgRole(
        "app_ro",
        { login: true },
        {
          databasePrivileges: [privs("tabler_dev", ["CONNECT"])],
          schemaPrivileges: [privs("public", ["USAGE"])],
          tablePrivileges: [privs("public", ["SELECT"])],
          partialTablePrivileges: [privs("inventory", ["SELECT"])],
        },
      ),
      pgRole(
        "analyst",
        { login: true, validUntil: "2027-03-31 23:59:59+07" },
        {
          databasePrivileges: [privs("tabler_dev", ["CONNECT"])],
          schemaPrivileges: [privs("public", ["USAGE"])],
          partialTablePrivileges: [privs("public", ["SELECT"])],
        },
      ),
      pgRole(
        "reporting",
        {},
        {
          schemaPrivileges: [privs("public", ["USAGE"])],
          tablePrivileges: [privs("public", ["SELECT"])],
        },
      ),
      pgRole("replicator", { login: true, replication: true, connectionLimit: 3 }),
    ],
  ],
  [
    "mysql",
    [
      myUser("root", "localhost", { globalPrivileges: ALL_MYSQL }),
      myUser("root", "%", { globalPrivileges: ALL_MYSQL }),
      myUser("app", "%", {
        databasePrivileges: [
          privs("shop", ["SELECT", "INSERT", "UPDATE", "DELETE"]),
          privs("tabler_dev", ["SELECT", "INSERT", "UPDATE", "DELETE", "CREATE TEMPORARY TABLES"]),
        ],
        limits: { ...NO_LIMITS, maxUserConnections: 50 },
      }),
      myUser("readonly", "10.0.%", {
        globalPrivileges: ["SHOW DATABASES"],
        databasePrivileges: [privs("tabler_dev", ["SELECT", "SHOW VIEW"])],
        limits: { ...NO_LIMITS, maxQueriesPerHour: 5000 },
      }),
      myUser("report", "10.0.4.20", {
        globalPrivileges: ["PROCESS"],
        databasePrivileges: [privs("tabler_dev", ["SELECT"])],
      }),
      myUser("mysql.sys", "localhost"),
    ],
  ],
]);

const key = (u: { name: string; host: string | null }) => `${u.name}\u0000${u.host ?? ""}`;

function accounts(driver: Driver): UserState[] {
  if (driver === "sqlite") mockError("UNSUPPORTED", "unsupported: SQLite has no users");
  return users.get(driver) ?? [];
}

registerMock("admin_list_users", async ({ connectionId, includeNologin }) => {
  const c = await connection(connectionId);
  return accounts(c.driver)
    .map((u): UserSummary => ({
      name: u.name,
      host: u.host,
      canLogin: u.attributes ? u.attributes.login : true,
      superuser: u.attributes ? u.attributes.superuser : u.globalPrivileges.includes("SUPER"),
    }))
    .filter((u) => c.driver !== "postgres" || includeNologin === true || u.canLogin)
    .sort((a, b) => a.name.localeCompare(b.name) || (a.host ?? "").localeCompare(b.host ?? ""));
});

registerMock("admin_get_user", async ({ connectionId, user }) => {
  const c = await connection(connectionId);
  const ref = user as UserRef;
  const found = accounts(c.driver).find((u) => key(u) === key(ref));
  if (!found) mockError("NOT_FOUND", `not found: user ${ref.name}`);
  const detail: UserDetail = {
    state: structuredClone(found),
    schemas:
      c.driver === "postgres"
        ? mockAdminSchemas(c.connectionId, c.currentDatabase, [
            { name: "public", isDefault: true },
            { name: "inventory", isDefault: false },
          ]).map((s) => s.name)
        : [],
  };
  return detail;
});

// A TypeScript copy of sqlgen/admin.rs's diff, enough for Preview SQL in the mock.
const MYSQL_DB_ORDER = [
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "CREATE",
  "DROP",
  "REFERENCES",
  "INDEX",
  "ALTER",
  "CREATE TEMPORARY TABLES",
  "LOCK TABLES",
  "EXECUTE",
  "CREATE VIEW",
  "SHOW VIEW",
  "CREATE ROUTINE",
  "ALTER ROUTINE",
  "EVENT",
  "TRIGGER",
];
const PG_TABLE_ORDER = ["SELECT", "INSERT", "UPDATE", "DELETE"];
const PG_DB_ORDER = ["CONNECT", "CREATE", "TEMPORARY"];
const PG_SCHEMA_ORDER = ["USAGE", "CREATE"];

const on = (list: readonly ObjectPrivileges[], object: string) =>
  new Set(list.find((p) => p.object === object)?.privileges ?? []);
const objectsOf = (...lists: ReadonlyArray<readonly ObjectPrivileges[]>) =>
  [...new Set(lists.flatMap((l) => l.map((p) => p.object)))].sort();

function grantRevoke(
  out: Array<{ sql: string; display: string }>,
  before: Set<string>,
  after: Set<string>,
  order: readonly string[],
  target: string,
  grantee: string,
) {
  const added = order.filter((p) => after.has(p) && !before.has(p));
  const removed = order.filter((p) => before.has(p) && !after.has(p));
  if (added.length) {
    const sql = `GRANT ${added.join(", ")} ON ${target} TO ${grantee}`;
    out.push({ sql, display: sql });
  }
  if (removed.length) {
    const sql = `REVOKE ${removed.join(", ")} ON ${target} FROM ${grantee}`;
    out.push({ sql, display: sql });
  }
}

function userStatements(
  driver: Driver,
  change: UserChange,
): Array<{ sql: string; display: string }> {
  const out: Array<{ sql: string; display: string }> = [];
  const { original, edited, password } = change;
  const plain = (sql: string) => out.push({ sql, display: sql });
  const secret = (head: string, tail = "") =>
    out.push({
      sql: `${head}${quoteLit(driver, password ?? "")}${tail}`,
      display: `${head}${MASK}${tail}`,
    });
  if (!edited) {
    if (original)
      plain(
        driver === "mysql"
          ? `DROP USER ${quoteLit(driver, original.name)}@${quoteLit(driver, original.host ?? "%")}`
          : `DROP ROLE ${quoteIdent(driver, original.name)}`,
      );
    return out;
  }
  if (edited.name.trim() === "") mockError("INVALID_INPUT", "User name is required");
  const base: UserState = original ?? {
    ...myUser("", "%"),
    host: null,
    limits: null,
  };
  if (driver === "mysql") {
    const acct = `${quoteLit(driver, edited.name)}@${quoteLit(driver, edited.host ?? "%")}`;
    const limits = edited.limits ?? NO_LIMITS;
    const before = original?.limits ?? NO_LIMITS;
    const fields: Array<[string, keyof ResourceLimits]> = [
      ["MAX_QUERIES_PER_HOUR", "maxQueriesPerHour"],
      ["MAX_UPDATES_PER_HOUR", "maxUpdatesPerHour"],
      ["MAX_CONNECTIONS_PER_HOUR", "maxConnectionsPerHour"],
      ["MAX_USER_CONNECTIONS", "maxUserConnections"],
    ];
    const withClause = (create: boolean) =>
      fields
        .filter(([, k]) => (create ? limits[k] !== 0 : limits[k] !== before[k]))
        .map(([n, k]) => `${n} ${limits[k]}`)
        .join(" ");
    if (!original) {
      const w = withClause(true);
      const tail = w ? ` WITH ${w}` : "";
      if (password !== null) secret(`CREATE USER ${acct} IDENTIFIED BY `, tail);
      else plain(`CREATE USER ${acct}${tail}`);
    } else {
      const old = `${quoteLit(driver, original.name)}@${quoteLit(driver, original.host ?? "%")}`;
      if (old !== acct) plain(`RENAME USER ${old} TO ${acct}`);
      if (password !== null) secret(`ALTER USER ${acct} IDENTIFIED BY `);
      const w = withClause(false);
      if (w) plain(`ALTER USER ${acct} WITH ${w}`);
    }
    grantRevoke(
      out,
      new Set(base.globalPrivileges),
      new Set(edited.globalPrivileges),
      ALL_MYSQL,
      "*.*",
      acct,
    );
    for (const db of objectsOf(base.databasePrivileges, edited.databasePrivileges))
      grantRevoke(
        out,
        on(base.databasePrivileges, db),
        on(edited.databasePrivileges, db),
        MYSQL_DB_ORDER,
        `${quoteIdent(driver, db)}.*`,
        acct,
      );
    return out;
  }
  const role = quoteIdent(driver, edited.name);
  const attrs = edited.attributes ?? PG_ROLE;
  const prev = original?.attributes ?? PG_ROLE;
  const flags: Array<[keyof RoleAttributes, string]> = [
    ["superuser", "SUPERUSER"],
    ["createDb", "CREATEDB"],
    ["createRole", "CREATEROLE"],
    ["login", "LOGIN"],
    ["replication", "REPLICATION"],
    ["inherit", "INHERIT"],
  ];
  const opts = flags
    .filter(([k]) => attrs[k] !== prev[k])
    .map(([k, w]) => (attrs[k] ? w : `NO${w}`));
  if (attrs.connectionLimit !== prev.connectionLimit)
    opts.push(`CONNECTION LIMIT ${attrs.connectionLimit}`);
  if ((attrs.validUntil ?? "") !== (prev.validUntil ?? ""))
    opts.push(
      `VALID UNTIL ${attrs.validUntil ? quoteLit(driver, attrs.validUntil) : "'infinity'"}`,
    );
  if (!original) {
    const head = opts.length ? `CREATE ROLE ${role} WITH ${opts.join(" ")}` : `CREATE ROLE ${role}`;
    if (password !== null) secret(`${head}${opts.length ? " " : " WITH "}PASSWORD `);
    else plain(head);
  } else {
    if (original.name !== edited.name)
      plain(`ALTER ROLE ${quoteIdent(driver, original.name)} RENAME TO ${role}`);
    if (opts.length) plain(`ALTER ROLE ${role} WITH ${opts.join(" ")}`);
    if (password !== null) secret(`ALTER ROLE ${role} WITH PASSWORD `);
  }
  for (const db of objectsOf(base.databasePrivileges, edited.databasePrivileges))
    grantRevoke(
      out,
      on(base.databasePrivileges, db),
      on(edited.databasePrivileges, db),
      PG_DB_ORDER,
      `DATABASE ${quoteIdent(driver, db)}`,
      role,
    );
  for (const s of objectsOf(base.schemaPrivileges, edited.schemaPrivileges))
    grantRevoke(
      out,
      on(base.schemaPrivileges, s),
      on(edited.schemaPrivileges, s),
      PG_SCHEMA_ORDER,
      `SCHEMA ${quoteIdent(driver, s)}`,
      role,
    );
  for (const s of objectsOf(
    base.tablePrivileges,
    base.partialTablePrivileges,
    edited.tablePrivileges,
    edited.partialTablePrivileges,
  )) {
    const allBefore = on(base.tablePrivileges, s);
    const someBefore = on(base.partialTablePrivileges, s);
    const allAfter = on(edited.tablePrivileges, s);
    const someAfter = on(edited.partialTablePrivileges, s);
    const grant = PG_TABLE_ORDER.filter((p) => allAfter.has(p) && !allBefore.has(p));
    const revoke = PG_TABLE_ORDER.filter(
      (p) => (allBefore.has(p) || someBefore.has(p)) && !allAfter.has(p) && !someAfter.has(p),
    );
    const target = `ALL TABLES IN SCHEMA ${quoteIdent(driver, s)}`;
    if (grant.length) plain(`GRANT ${grant.join(", ")} ON ${target} TO ${role}`);
    if (revoke.length) plain(`REVOKE ${revoke.join(", ")} ON ${target} FROM ${role}`);
  }
  return out;
}

registerMock("admin_preview_user", async ({ connectionId, change }) => {
  const c = await connection(connectionId);
  accounts(c.driver);
  return userStatements(c.driver, change as UserChange).map((s) => s.display);
});

registerMock("admin_apply_user", async ({ connectionId, change: raw }) => {
  const t0 = performance.now();
  const c = await connection(connectionId);
  if (c.readOnly) mockError("UNSUPPORTED", "unsupported: this connection is read-only");
  const list = accounts(c.driver);
  const change = raw as UserChange;
  const statements = userStatements(c.driver, change);
  const { original, edited } = change;
  const target = edited ?? original;
  if (!target) return result([], t0);
  const existing = list.findIndex((u) => original !== null && key(u) === key(original));
  if (!edited) {
    if (original?.name === "postgres" || original?.name === "tabler")
      return failed(
        `role "${original.name}" cannot be dropped because some objects depend on it`,
        "owner of database tabler_dev\n12 objects in database tabler_dev",
        t0,
      );
    if (existing >= 0) list.splice(existing, 1);
    return result(
      statements.map((s) => s.display),
      t0,
    );
  }
  const clash = list.find(
    (u) => key(u) === key(edited) && (original === null || key(u) !== key(original)),
  );
  if (clash)
    return failed(
      c.driver === "mysql"
        ? `Operation CREATE USER failed for '${edited.name}'@'${edited.host ?? "%"}'`
        : `role "${edited.name}" already exists`,
      null,
      t0,
    );
  if (change.password !== null && change.password.length < 4 && c.driver === "mysql")
    return failed("Your password does not satisfy the current policy requirements", null, t0);
  const saved = structuredClone(edited);
  if (existing >= 0) list[existing] = saved;
  else list.push(saved);
  return result(
    statements.map((s) => s.display),
    t0,
  );
});
