import type { CSSProperties } from "react";
import type { ConnectionTag, Driver } from "@/lib/ipc";
import { accentColor, type AccentPreset } from "@/lib/theme";

export { ACCENT_PRESETS, type AccentPreset } from "@/lib/theme";
export { ACCENT_CSS, accentColor } from "@/lib/theme";

export const DRIVER_LABEL = {
  postgres: "PostgreSQL",
  mysql: "MySQL / MariaDB",
  sqlite: "SQLite",
} as const;
export const DRIVER_SHORT = { postgres: "PG", mysql: "MY", sqlite: "SQ" } as const;
export const DRIVERS = ["postgres", "mysql", "sqlite"] as const satisfies readonly Driver[];
/** Tile colour of each driver in the new-connection driver picker. */
export const DRIVER_TINT: Record<Driver, AccentPreset> = {
  postgres: "blue",
  mysql: "orange",
  sqlite: "teal",
};

export const TAG_LABEL: Record<ConnectionTag, string> = {
  local: "Local",
  testing: "Testing",
  development: "Development",
  staging: "Staging",
  production: "Production",
};
/** TablePlus's tag colours: local green, testing blue, development purple, staging orange, production red. */
export const TAG_COLOR: Record<ConnectionTag, AccentPreset> = {
  local: "green",
  testing: "blue",
  development: "purple",
  staging: "orange",
  production: "red",
};

/** Sets `--tint` to an accent preset, for the `TINTED` classes. */
export function tintStyle(color: string | null | undefined): CSSProperties {
  return { "--tint": accentColor(color) } as CSSProperties;
}

/**
 * Light tint background with a darker (light theme) / lighter (dark theme) text of `--tint`:
 * readable for every preset, yellow included. Pair with `tintStyle`.
 */
export const TINTED =
  "bg-[color-mix(in_oklab,var(--tint)_15%,transparent)] text-[color-mix(in_oklab,var(--tint)_78%,black)] dark:bg-[color-mix(in_oklab,var(--tint)_24%,transparent)] dark:text-[color-mix(in_oklab,var(--tint)_68%,white)]";
