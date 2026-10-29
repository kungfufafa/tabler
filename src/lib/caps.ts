import { isCoreDriver } from "@/features/drivers/catalog";
import type { ConnectionInfo, Driver, Feature, SessionCaps } from "@/lib/ipc";
import { DRIVER_CATALOG } from "@/lib/ipc/bindings/driverCatalog";
import { useConnectionsStore } from "@/stores/connections";

/**
 * What a connection's session offers (plan A9): the caps it reported at connect
 * (`ConnectionInfo.capabilities`), or, from a backend that reports none, the caps its driver
 * starts from. The backend enforces them (`require(Feature)`); the renderer hides what a session
 * lacks, ANDed with its own checks, so the core drivers, which get every feature, look as before.
 */

/** The caps a connection of each driver starts from (`SessionCaps::base`). */
export const BASE_CAPS: Readonly<Record<Driver, SessionCaps>> = DRIVER_CATALOG.baseCaps;

type WithCaps = Pick<ConnectionInfo, "driver" | "capabilities">;

/** The caps of a connection. */
export function capsOf(info: WithCaps): SessionCaps {
  return info.capabilities ?? BASE_CAPS[info.driver];
}

/** Whether a connection (or its caps) offers `feature`. */
export function has(of: WithCaps | SessionCaps, feature: Feature): boolean {
  const caps = "features" in of ? of : capsOf(of);
  return caps.features.includes(feature);
}

/**
 * Whether the session is a core one: a core driver speaking its own dialect (PostgreSQL, MySQL /
 * MariaDB, SQLite, and the PostgreSQL flavors that speak PostgreSQL). Its data and namespace caps
 * are not read in v1; a Redshift session (PostgreSQL's driver, its own dialect) is not one.
 */
export function isCoreSession(caps: SessionCaps): boolean {
  return isCoreDriver(caps.driver) && caps.dialect === caps.driver;
}

/** The caps of an open connection, kept current; null when it is not open. */
export function useCaps(connectionId: string | null): SessionCaps | null {
  return useConnectionsStore((s) => {
    const conn = connectionId === null ? undefined : s.byId[connectionId];
    return conn ? capsOf(conn.info) : null;
  });
}
