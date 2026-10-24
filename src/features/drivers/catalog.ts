import type { Dialect, Driver } from "@/lib/ipc";

/** The drivers of every Tabler build: PostgreSQL, MySQL / MariaDB and SQLite. */
export const CORE_DRIVERS = ["postgres", "mysql", "sqlite"] as const satisfies readonly Driver[];
export type CoreDriver = (typeof CORE_DRIVERS)[number];

export function isCoreDriver(driver: Driver): driver is CoreDriver {
  return (CORE_DRIVERS as readonly Driver[]).includes(driver);
}

/**
 * `driver` on a code path only the core drivers reach for now: their connections (no other
 * driver can connect in this build) and their connection form. Throws for any other driver
 * rather than treat its engine like PostgreSQL.
 */
export function coreDriver(driver: Driver): CoreDriver {
  if (isCoreDriver(driver)) return driver;
  throw new Error(`non-core driver ${driver} reached SQL-only UI`);
}

/** The SQL dialect of a core driver's connection (the names are the same). */
export const coreDialect: (driver: Driver) => Dialect = coreDriver;
