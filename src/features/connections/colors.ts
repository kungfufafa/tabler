import type { CSSProperties } from "react";
import type { Driver, MysqlVersion } from "@/lib/ipc";
import { accentColor, HEX_COLOR, type AccentPreset } from "@/lib/theme";
import type { BuiltinTag } from "./schema";

export { ACCENT_PRESETS, type AccentPreset } from "@/lib/theme";
export { ACCENT_CSS, accentColor } from "@/lib/theme";

export const DRIVER_LABEL = {
  postgres: "PostgreSQL",
  mysql: "MySQL / MariaDB",
  sqlite: "SQLite",
} as const;
export const DRIVER_SHORT = { postgres: "PG", mysql: "MY", sqlite: "SQ" } as const;
export const DRIVERS = ["postgres", "mysql", "sqlite"] as const satisfies readonly Driver[];
/**
 * The databases of the new-connection picker, TablePlus's list: MariaDB has its own tile and badge
 * although the MySQL driver serves it (a MySQL connection with Ver. ▸ MariaDB).
 */
export type DatabaseKind = Driver | "mariadb";
export const DATABASE_KINDS = [
  "postgres",
  "mysql",
  "mariadb",
  "sqlite",
] as const satisfies readonly DatabaseKind[];
export const KIND_LABEL: Record<DatabaseKind, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  mariadb: "MariaDB",
  sqlite: "SQLite",
};
export const KIND_SHORT: Record<DatabaseKind, string> = { ...DRIVER_SHORT, mariadb: "MA" };
/** Tile colour of each database in the new-connection picker. */
export const KIND_TINT: Record<DatabaseKind, AccentPreset> = {
  postgres: "blue",
  mysql: "orange",
  mariadb: "purple",
  sqlite: "teal",
};

/** The database a connection is for: its driver, or MariaDB for a MySQL one set to Ver. ▸ MariaDB. */
export function databaseKind(driver: Driver, mysqlVersion?: MysqlVersion | null): DatabaseKind {
  return driver === "mysql" && mysqlVersion === "mariadb" ? "mariadb" : driver;
}

/** The driver that serves a database of the picker. */
export function kindDriver(kind: DatabaseKind): Driver {
  return kind === "mariadb" ? "mysql" : kind;
}

/** The five built-in tags' names and TablePlus's colours (custom tags: see `tags.ts`). */
export const TAG_LABEL: Record<BuiltinTag, string> = {
  local: "Local",
  testing: "Testing",
  development: "Development",
  staging: "Staging",
  production: "Production",
};
/** TablePlus's tag colours: local green, testing blue, development purple, staging orange, production red. */
export const TAG_COLOR: Record<BuiltinTag, AccentPreset> = {
  local: "green",
  testing: "blue",
  development: "purple",
  staging: "orange",
  production: "red",
};

/** A `#rrggbb` colour (custom tags and connections may have one instead of a preset). */
export { HEX_COLOR } from "@/lib/theme";

/** CSS colour of an accent preset name or a `#rrggbb` colour. */
export function cssColor(color: string | null | undefined): string {
  return accentColor(color);
}

/** Sets `--tint` to an accent preset (or a `#rrggbb` colour), for the `TINTED` classes. */
export function tintStyle(color: string | null | undefined): CSSProperties {
  return { "--tint": cssColor(color) } as CSSProperties;
}

/**
 * Light tint background with a darker (light theme) / lighter (dark theme) text of `--tint`:
 * readable for every preset, yellow included. Pair with `tintStyle`.
 */
export const TINTED =
  "bg-[color-mix(in_oklab,var(--tint)_15%,transparent)] text-[color-mix(in_oklab,var(--tint)_78%,black)] dark:bg-[color-mix(in_oklab,var(--tint)_24%,transparent)] dark:text-[color-mix(in_oklab,var(--tint)_68%,white)]";

/** `#rrggbb` (lowercase) from what was typed in the Hex field (`#` optional), or null. */
export function parseHex(text: string): string | null {
  const t = text.trim().replace(/^#?/, "#").toLowerCase();
  if (HEX_COLOR.test(t)) return t;
  // #rgb shorthand.
  if (/^#[0-9a-f]{3}$/.test(t)) return `#${[...t.slice(1)].map((c) => c + c).join("")}`;
  return null;
}
