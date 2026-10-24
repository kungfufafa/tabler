import type {
  AppInfo,
  ConnectionProfile,
  DatabaseKind,
  Dialect,
  Driver,
  DriverDescriptor,
  KindDescriptor,
  MysqlVersion,
  PgFlavor,
} from "@/lib/ipc";
import { DRIVER_CATALOG } from "@/lib/ipc/bindings/driverCatalog";

/** The facts of each driver (ports, labels, fields, TLS, read-only mode…), from the backend. */
export const DESCRIPTORS: Readonly<Record<Driver, DriverDescriptor>> = DRIVER_CATALOG.drivers;

/** The facts of each database of the new-connection picker. */
export const KIND_DESCRIPTORS: Readonly<Record<DatabaseKind, KindDescriptor>> =
  DRIVER_CATALOG.kinds;

/** Every database of the new-connection picker, in its order. */
export const KINDS: readonly KindDescriptor[] = Object.values(KIND_DESCRIPTORS);

/** The drivers of every Tabler build: PostgreSQL, MySQL / MariaDB and SQLite. */
export const CORE_DRIVERS = ["postgres", "mysql", "sqlite"] as const satisfies readonly Driver[];
export type CoreDriver = (typeof CORE_DRIVERS)[number];

/** The databases of every Tabler build, in the picker's order (no cargo feature builds them). */
export const CORE_KINDS: readonly DatabaseKind[] = KINDS.filter((k) => k.cargoFeature === null).map(
  (k) => k.kind,
);

export function isCoreDriver(driver: Driver): driver is CoreDriver {
  return CORE_DRIVERS.some((d) => d === driver);
}

export function isCoreKind(kind: DatabaseKind): boolean {
  return CORE_KINDS.includes(kind);
}

/** The driver's name ("PostgreSQL", "Redis"). */
export function driverLabel(driver: Driver): string {
  return DESCRIPTORS[driver].label;
}

/**
 * `record` with `f` applied to each value: the catalog's descriptors give one value per driver
 * or kind, and none is left out.
 */
export function mapRecord<K extends string, V, W>(
  record: Readonly<Record<K, V>>,
  f: (value: V) => W,
): Record<K, W> {
  const out = {} as Record<K, W>;
  for (const key in record) out[key] = f(record[key]);
  return out;
}

/**
 * The database of the picker a connection is for: a flavor's own (CockroachDB, MariaDB) when its
 * driver is set to one, else the driver's.
 */
export function databaseKind(
  driver: Driver,
  mysqlVersion?: MysqlVersion | null,
  pgFlavor?: PgFlavor | null,
): DatabaseKind {
  const flavored = KINDS.find(
    (k) =>
      k.driver === driver &&
      ((k.pgFlavor !== null && k.pgFlavor === pgFlavor) ||
        (k.mysqlVersion !== null && k.mysqlVersion === mysqlVersion)),
  );
  // Every driver also names a database of its own (catalog.test).
  return flavored?.kind ?? driver;
}

/** The database of the picker a profile is for. */
export function kindOf(profile: Pick<ConnectionProfile, "driver" | "options">): DatabaseKind {
  return databaseKind(profile.driver, profile.options.mysqlVersion, profile.options.pgFlavor);
}

/** The driver that serves a database of the picker (MariaDB: MySQL; CockroachDB: PostgreSQL). */
export function kindDriver(kind: DatabaseKind): Driver {
  return KIND_DESCRIPTORS[kind].driver;
}

/**
 * The databases the new-connection picker offers: the ones this build can connect to
 * (`AppInfo.kinds`), in the picker's order. Before the backend has answered (and in tests), the
 * core ones, so the picker never waits.
 */
export function visibleKinds(info?: Pick<AppInfo, "kinds"> | null): readonly DatabaseKind[] {
  const built = info?.kinds;
  if (!built) return CORE_KINDS;
  return KINDS.filter((k) => built.includes(k.kind)).map((k) => k.kind);
}

/**
 * `driver` on a code path only the core drivers reach: their connection form, which a profile of
 * another driver never opens (`openConnectionDialog`). Throws for any other driver rather than
 * treat its engine like PostgreSQL.
 */
export function coreDriver(driver: Driver): CoreDriver {
  if (isCoreDriver(driver)) return driver;
  throw new Error(`non-core driver ${driver} reached SQL-only UI`);
}

/**
 * The SQL dialect of a core driver's connections (the names are the same); null for any other
 * driver, whose SQL the core lexers, generators and classifiers must not handle. Never throws:
 * panes and menus call it while they render, for every connection.
 */
export function coreDialect(driver: Driver): Dialect | null {
  return isCoreDriver(driver) ? driver : null;
}

/** What a core-only feature says on a connection of another driver. */
export function notAvailableFor(driver: Driver, what: string): string {
  return `${what} is not available for ${driverLabel(driver)} connections`;
}

/**
 * `coreDialect` on an action (a click, a command), never while rendering: throws for another
 * driver, with a message the action's error toast can show.
 */
export function requireCoreDialect(driver: Driver, what = "This"): Dialect {
  const dialect = coreDialect(driver);
  if (dialect) return dialect;
  throw new Error(notAvailableFor(driver, what));
}
