import type { ColumnMeta } from "../bindings/ColumnMeta";
import type { ConnectionProfile } from "../bindings/ConnectionProfile";
import type { ConnectionTag } from "../bindings/ConnectionTag";
import type { Driver } from "../bindings/Driver";
import type { TableMeta } from "../bindings/TableMeta";
import type { TableSummary } from "../bindings/TableSummary";
import type { ValueKind } from "../bindings/ValueKind";

export function profile(
  p: Partial<ConnectionProfile> & { id: string; name: string; driver: Driver },
  tag: ConnectionTag | null = null,
): ConnectionProfile {
  const built: ConnectionProfile = {
    color: "blue",
    group: null,
    host: p.driver === "sqlite" ? null : "127.0.0.1",
    port: p.driver === "postgres" ? 5432 : p.driver === "mysql" ? 3306 : null,
    user: p.driver === "sqlite" ? null : "tabler",
    database: p.driver === "sqlite" ? null : "tabler_dev",
    ssl: "prefer",
    sslCaPath: null,
    sqlitePath: p.driver === "sqlite" ? "/Users/me/dev/fixtures/sample.sqlite" : null,
    ssh: null,
    options: {
      tag: null,
      safeMode: null,
      timezone: null,
      connectTimeoutSecs: null,
      readOnly: false,
      startupSql: null,
      defaultRowLimit: null,
      sqliteForeignKeys: true,
    },
    sortOrder: 0,
    createdAt: "2026-10-01T08:00:00.000Z",
    updatedAt: "2026-10-01T08:00:00.000Z",
    lastConnectedAt: null,
    ...p,
  };
  return tag ? { ...built, options: { ...built.options, tag } } : built;
}

export const SEED_PROFILES: ConnectionProfile[] = [
  profile(
    {
      id: "p-local-pg",
      name: "Local Postgres",
      driver: "postgres",
      color: "blue",
      group: "Local",
      sortOrder: 1,
      lastConnectedAt: "2026-10-10T14:00:00.000Z",
    },
    "local",
  ),
  profile(
    {
      id: "p-shop-mysql",
      name: "Shop MySQL",
      driver: "mysql",
      color: "orange",
      group: "Local",
      sortOrder: 2,
      user: "root",
    },
    "development",
  ),
  profile(
    {
      id: "p-sample-sqlite",
      name: "sample.sqlite",
      driver: "sqlite",
      color: "green",
      group: null,
      sortOrder: 3,
    },
    "local",
  ),
  profile({
    id: "p-prod-pg",
    name: "Production (read-only)",
    driver: "postgres",
    color: "red",
    group: "Production",
    sortOrder: 4,
    host: "db.internal.example.com",
    options: {
      tag: "production",
      safeMode: "alertNonSelect",
      timezone: null,
      connectTimeoutSecs: null,
      readOnly: true,
      startupSql: null,
      defaultRowLimit: null,
      sqliteForeignKeys: true,
    },
  }),
  // SSH host-key flow (see mock/export.ts): the first connect / test asks to trust the bastion's
  // key, later ones succeed; "Rotated host" always presents a key that differs from the trusted one.
  profile(
    {
      id: "p-staging-ssh",
      name: "Staging via SSH",
      driver: "postgres",
      color: "purple",
      group: "Staging",
      sortOrder: 5,
      host: "10.0.1.20",
      user: "app",
      database: "app_staging",
      ssh: {
        host: "bastion.example.com",
        port: 22,
        user: "deploy",
        auth: "agent",
        keyPath: null,
        localPort: null,
        keepaliveSecs: null,
      },
    },
    "staging",
  ),
  profile({
    id: "p-rotated-ssh",
    name: "Rotated host",
    driver: "mysql",
    color: "yellow",
    group: "Staging",
    sortOrder: 6,
    host: "10.0.2.15",
    user: "app",
    ssh: {
      host: "rotated.example.com",
      port: 2222,
      user: "deploy",
      auth: "key",
      keyPath: "~/.ssh/id_ed25519",
      localPort: null,
      keepaliveSecs: null,
    },
  }),
];

type ColSpec = [name: string, dataType: string, kind: ValueKind, opts?: Partial<ColumnMeta>];

function col(ordinal: number, [name, dataType, kind, opts]: ColSpec): ColumnMeta {
  return {
    name,
    ordinal,
    dataType,
    kind,
    nullable: true,
    defaultExpr: null,
    isPk: false,
    pkOrdinal: null,
    isAutoIncrement: false,
    isGenerated: false,
    isUnsigned: false,
    comment: null,
    enumValues: null,
    charMaxLen: null,
    numericPrecision: null,
    numericScale: null,
    collation: null,
    arrayElemType: null,
    extra: null,
    ...opts,
  };
}

const pk = (extra: Partial<ColumnMeta> = {}): Partial<ColumnMeta> => ({
  isPk: true,
  pkOrdinal: 1,
  nullable: false,
  isAutoIncrement: true,
  ...extra,
});

export const TABLE_COLUMNS: Record<string, ColSpec[]> = {
  users: [
    ["id", "bigint", "bigInt", pk({ defaultExpr: "nextval('users_id_seq'::regclass)" })],
    ["email", "text", "text", { nullable: false }],
    ["name", "text", "text", { nullable: false }],
    ["nickname", "text", "text"],
    [
      "role",
      "user_role",
      "enum",
      {
        nullable: false,
        enumValues: ["admin", "editor", "viewer"],
        defaultExpr: "'viewer'::user_role",
      },
    ],
    ["is_active", "boolean", "bool", { nullable: false, defaultExpr: "true" }],
    [
      "balance",
      "numeric(20,6)",
      "decimal",
      { defaultExpr: "0", numericPrecision: 20, numericScale: 6 },
    ],
    ["big_id", "bigint", "bigInt"],
    ["uid", "uuid", "uuid", { defaultExpr: "gen_random_uuid()" }],
    ["settings", "jsonb", "json"],
    ["tags", "text[]", "array", { arrayElemType: "text" }],
    ["avatar", "bytea", "bytes"],
    ["bio", "text", "text"],
    ["score", "double precision", "float"],
    [
      "created_at",
      "timestamp with time zone",
      "timestampTz",
      { nullable: false, defaultExpr: "now()" },
    ],
    ["last_login", "timestamp without time zone", "timestamp"],
  ],
  posts: [
    ["id", "bigint", "bigInt", pk({ extra: "ALWAYS" })],
    ["user_id", "bigint", "bigInt", { nullable: false }],
    ["title", "text", "text", { nullable: false }],
    ["body", "text", "text"],
    [
      "status",
      "post_status",
      "enum",
      { nullable: false, enumValues: ["draft", "published", "archived"] },
    ],
    ["published_at", "timestamp with time zone", "timestampTz"],
    ["views", "integer", "int", { nullable: false, defaultExpr: "0" }],
    [
      "created_at",
      "timestamp with time zone",
      "timestampTz",
      { nullable: false, defaultExpr: "now()" },
    ],
    [
      "updated_at",
      "timestamp with time zone",
      "timestampTz",
      { nullable: false, defaultExpr: "now()" },
    ],
  ],
  tags: [
    ["id", "integer", "int", pk()],
    ["name", "text", "text", { nullable: false }],
  ],
  post_tags: [
    ["post_id", "bigint", "bigInt", { isPk: true, pkOrdinal: 1, nullable: false }],
    ["tag_id", "integer", "int", { isPk: true, pkOrdinal: 2, nullable: false }],
  ],
  audit_log: [
    ["at", "timestamp without time zone", "timestamp"],
    ["actor", "text", "text"],
    ["action", "text", "text"],
    ["payload", "jsonb", "json"],
  ],
  events_big: [
    ["id", "bigint", "bigInt", pk()],
    ["ts", "timestamp without time zone", "timestamp", { nullable: false }],
    ["kind", "text", "text", { nullable: false }],
    ["value", "numeric(12,2)", "decimal"],
    ["payload", "jsonb", "json"],
  ],
  types_zoo: [
    ["id", "integer", "int", pk()],
    ["c_smallint", "smallint", "int"],
    ["c_numeric", "numeric", "decimal"],
    ["c_money", "money", "other"],
    ["c_bytea", "bytea", "bytes"],
    ["c_interval", "interval", "interval"],
    ["c_inet", "inet", "other"],
    ["c_point", "point", "other"],
    ["c_int_array", "integer[]", "array", { arrayElemType: "integer" }],
  ],
  "Order Items": [
    ["order_id", "integer", "int", { isPk: true, pkOrdinal: 1, nullable: false }],
    ["item", "text", "text", { isPk: true, pkOrdinal: 2, nullable: false }],
    ["qty", "integer", "int"],
    ["price", "numeric(10,2)", "decimal"],
  ],
  CamelCase: [
    ["Id", "integer", "int", pk({ isAutoIncrement: false })],
    ["DisplayName", "text", "text"],
    ["select", "text", "text"],
    ["user", "text", "text"],
    ["from", "text", "text"],
  ],
  active_users: [
    ["id", "bigint", "bigInt"],
    ["email", "text", "text"],
    ["name", "text", "text"],
  ],
  post_counts: [
    ["user_id", "bigint", "bigInt"],
    ["name", "text", "text"],
    ["posts", "bigint", "bigInt"],
  ],
  post_counts_mv: [
    ["user_id", "bigint", "bigInt"],
    ["name", "text", "text"],
    ["posts", "bigint", "bigInt"],
  ],
  items: [
    ["id", "integer", "int", pk()],
    ["sku", "text", "text", { nullable: false }],
    ["name", "text", "text"],
    ["stock", "integer", "int"],
  ],
  stock_movements: [
    ["id", "integer", "int", pk()],
    ["item_id", "integer", "int", { nullable: false }],
    ["delta", "integer", "int"],
    ["at", "timestamp with time zone", "timestampTz"],
  ],
};

const PUBLIC_TABLES: Array<[string, TableSummary["kind"], number | null]> = [
  ["users", "table", 50],
  ["posts", "table", 200],
  ["tags", "table", 15],
  ["post_tags", "table", 400],
  ["audit_log", "table", 30],
  ["events_big", "table", 100000],
  ["types_zoo", "table", 3],
  ["Order Items", "table", 10],
  ["CamelCase", "table", 5],
  ["active_users", "view", null],
  ["post_counts", "view", null],
  ["post_counts_mv", "materializedView", 50],
];

const INVENTORY_TABLES: Array<[string, TableSummary["kind"], number | null]> = [
  ["items", "table", 20],
  ["stock_movements", "table", 120],
];

export function tablesFor(driver: Driver, schema: string): TableSummary[] {
  const list =
    driver === "postgres" && schema === "inventory"
      ? INVENTORY_TABLES
      : driver === "mysql" && schema === "tabler_inv"
        ? INVENTORY_TABLES
        : PUBLIC_TABLES.filter(([, kind]) => driver === "postgres" || kind !== "materializedView");
  return list.map(([name, kind, rows]) => ({
    name,
    schema,
    kind,
    rowEstimate: rows,
    sizeBytes: rows ? rows * 180 : null,
    comment: name === "users" ? "Registered accounts" : null,
  }));
}

export function tableMetaFor(driver: Driver, schema: string, name: string): TableMeta | null {
  const specs = TABLE_COLUMNS[name];
  if (!specs) return null;
  const columns = specs.map((spec, i) => col(i + 1, spec));
  const kind = name.endsWith("_mv")
    ? "materializedView"
    : name === "active_users" || name === "post_counts"
      ? "view"
      : "table";
  const primaryKey = columns
    .filter((c) => c.isPk)
    .sort((a, b) => (a.pkOrdinal ?? 0) - (b.pkOrdinal ?? 0))
    .map((c) => c.name);
  return {
    table: { schema, name },
    kind,
    columns,
    primaryKey,
    indexes:
      primaryKey.length > 0
        ? [
            {
              name: `${name}_pkey`,
              columns: primaryKey,
              unique: true,
              primary: true,
              method: "btree",
              predicate: null,
              definition: null,
              comment: null,
              include: [],
            },
          ]
        : [],
    foreignKeys:
      name === "posts"
        ? [
            {
              name: "posts_user_id_fkey",
              columns: ["user_id"],
              refTable: { schema, name: "users" },
              refColumns: ["id"],
              onUpdate: "NO ACTION",
              onDelete: "CASCADE",
              definition: null,
            },
          ]
        : name === "post_tags"
          ? [
              {
                name: "post_tags_post_id_fkey",
                columns: ["post_id"],
                refTable: { schema, name: "posts" },
                refColumns: ["id"],
                onUpdate: "NO ACTION",
                onDelete: "CASCADE",
                definition: null,
              },
              {
                name: "post_tags_tag_id_fkey",
                columns: ["tag_id"],
                refTable: { schema, name: "tags" },
                refColumns: ["id"],
                onUpdate: "NO ACTION",
                onDelete: "CASCADE",
                definition: null,
              },
            ]
          : [],
    triggers:
      name === "posts"
        ? [
            {
              name: "posts_set_updated_at",
              timing: "BEFORE",
              events: ["UPDATE"],
              enabled: true,
              definition:
                "CREATE TRIGGER posts_set_updated_at BEFORE UPDATE ON posts FOR EACH ROW EXECUTE FUNCTION set_updated_at()",
            },
          ]
        : [],
    comment: null,
    rowEstimate: PUBLIC_TABLES.find(([n]) => n === name)?.[2] ?? null,
    ddl: driver === "postgres" ? null : `CREATE TABLE "${name}" (...)`,
    checks: [],
    options: null,
  };
}
