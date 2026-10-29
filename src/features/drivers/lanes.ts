import type { DatabaseKind, Driver, SqlDialectId } from "@/lib/ipc";

/**
 * The driver lanes of the renderer: each lane's folder (`src/features/drivers/<lane>/`, and its
 * mock's `src/lib/ipc/mock/drivers/<lane>/`) with the databases, drivers and SQL dialects it
 * brings. `dev/lanes.json` is the authoritative table (`registry.test.ts` checks this copy
 * against it); the lead changes both.
 */
export const LANES = {
  "pg-flavors": {
    kinds: ["cockroachdb", "greenplum", "redshift"],
    drivers: [],
    dialects: ["redshift"],
  },
  "sqlite-remote": { kinds: ["libsql", "d1"], drivers: ["libsql", "d1"], dialects: [] },
  duckdb: { kinds: ["duckdb"], drivers: ["duckdb"], dialects: ["duckdb"] },
  clickhouse: { kinds: ["clickhouse"], drivers: ["clickhouse"], dialects: ["clickhouse"] },
  sqlserver: { kinds: ["sqlserver"], drivers: ["sqlserver"], dialects: ["tsql"] },
  snowflake: { kinds: ["snowflake"], drivers: ["snowflake"], dialects: ["snowflake"] },
  bigquery: { kinds: ["bigquery"], drivers: ["bigquery"], dialects: ["bigquery"] },
  vertica: { kinds: ["vertica"], drivers: ["vertica"], dialects: ["vertica"] },
  dynamodb: { kinds: ["dynamodb"], drivers: ["dynamodb"], dialects: [] },
  cassandra: { kinds: ["cassandra"], drivers: ["cassandra"], dialects: ["cql"] },
  redis: { kinds: ["redis"], drivers: ["redis"], dialects: [] },
  mongodb: { kinds: ["mongodb"], drivers: ["mongodb"], dialects: [] },
  elasticsearch: { kinds: ["elasticsearch"], drivers: ["elasticsearch"], dialects: [] },
} as const satisfies Record<
  string,
  {
    kinds: readonly DatabaseKind[];
    drivers: readonly Driver[];
    dialects: readonly SqlDialectId[];
  }
>;

export type LaneId = keyof typeof LANES;

export const LANE_IDS = Object.keys(LANES) as LaneId[];

const owner = <T extends string>(pick: (l: (typeof LANES)[LaneId]) => readonly string[]) => {
  const out = new Map<T, LaneId>();
  for (const lane of LANE_IDS) for (const id of pick(LANES[lane])) out.set(id as T, lane);
  return out;
};

const KIND_LANE = owner<DatabaseKind>((l) => l.kinds);
const DRIVER_LANE = owner<Driver>((l) => l.drivers);
const DIALECT_LANE = owner<SqlDialectId>((l) => l.dialects);

/** The lane of a database of the picker; null for the core ones. */
export const laneOfKind = (kind: DatabaseKind): LaneId | null => KIND_LANE.get(kind) ?? null;

/** The lane of a driver; null for the core drivers (PostgreSQL's flavors are their lane's kinds). */
export const laneOfDriver = (driver: Driver): LaneId | null => DRIVER_LANE.get(driver) ?? null;

/** The lane of an extension SQL dialect; null for the core dialects. */
export const laneOfDialect = (dialect: SqlDialectId): LaneId | null =>
  DIALECT_LANE.get(dialect) ?? null;
