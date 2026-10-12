import type { ConnectionInfo, ConnectionTag, Driver } from "@/lib/ipc";
import { accentColor, type AccentPreset } from "@/lib/theme";

/** Two-letter badge of a driver (workspace rail). */
export const DRIVER_SHORT: Record<Driver, string> = {
  postgres: "PG",
  mysql: "MY",
  sqlite: "SQ",
};

const DRIVER_NAME: Record<Driver, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  sqlite: "SQLite",
};

/** TablePlus's tag labels and colours (local green, testing blue, …, production red). */
export const TAG_STYLE: Record<ConnectionTag, { label: string; color: AccentPreset }> = {
  local: { label: "LOCAL", color: "green" },
  testing: { label: "TESTING", color: "blue" },
  development: { label: "DEV", color: "purple" },
  staging: { label: "STAGING", color: "orange" },
  production: { label: "PRODUCTION", color: "red" },
};

export function tagColor(tag: ConnectionTag): string {
  return accentColor(TAG_STYLE[tag].color);
}

/**
 * "PostgreSQL 17.6" / "MySQL 9.6.0" / "MariaDB 11.8.2" / "SQLite 3.54.0" from the server's version
 * string (PostgreSQL's `version()` carries the build platform after the number).
 */
export function serverLabel(info: Pick<ConnectionInfo, "driver" | "serverVersion">): string {
  const raw = info.serverVersion.trim();
  const named = /^(postgresql|mysql|mariadb|sqlite)\s+v?(\d[\w.]*)/i.exec(raw);
  if (named) {
    const product = named[1]!.toLowerCase();
    const name =
      product === "mariadb"
        ? "MariaDB"
        : (Object.values(DRIVER_NAME).find((n) => n.toLowerCase() === product) ?? named[1]!);
    return `${name} ${named[2]}`;
  }
  const number = /^v?(\d[\w.]*)/.exec(raw)?.[1];
  return number ? `${DRIVER_NAME[info.driver]} ${number}` : DRIVER_NAME[info.driver];
}

/** The database segment: SQLite reports its file path, which shows as the file name. */
export function databaseLabel(info: Pick<ConnectionInfo, "driver" | "currentDatabase">): string {
  const db = info.currentDatabase;
  if (!db) return "";
  if (info.driver !== "sqlite") return db;
  return db.split(/[\\/]/).pop() || db;
}
