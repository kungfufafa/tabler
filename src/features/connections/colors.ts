import type { CSSProperties } from "react";
import { CORE_DRIVERS, DESCRIPTORS, KIND_DESCRIPTORS, mapRecord } from "@/features/drivers/catalog";
import type { DatabaseKind, Driver } from "@/lib/ipc";
import { ACCENT_PRESETS, accentColor, HEX_COLOR, type AccentPreset } from "@/lib/theme";
import type { BuiltinTag } from "./schema";

export { ACCENT_PRESETS, type AccentPreset } from "@/lib/theme";
export { ACCENT_CSS, accentColor } from "@/lib/theme";

/**
 * The databases of the new-connection picker, TablePlus's list: MariaDB has its own tile and badge
 * although the MySQL driver serves it (a MySQL connection with Ver. ▸ MariaDB), and so do the
 * PostgreSQL flavors.
 */
export type { DatabaseKind } from "@/lib/ipc";
export { databaseKind, kindDriver } from "@/features/drivers/catalog";

/**
 * The name of each driver (the form's Driver select, the launcher's sort); MySQL's names both
 * databases it serves.
 */
export const DRIVER_LABEL: Readonly<Record<Driver, string>> = {
  ...mapRecord(DESCRIPTORS, (d) => d.label),
  mysql: "MySQL / MariaDB",
};
/** The connection form's Driver choices. */
export const DRIVERS = CORE_DRIVERS;
export const KIND_LABEL: Readonly<Record<DatabaseKind, string>> = mapRecord(
  KIND_DESCRIPTORS,
  (k) => k.label,
);
export const KIND_SHORT: Readonly<Record<DatabaseKind, string>> = mapRecord(
  KIND_DESCRIPTORS,
  (k) => k.short,
);
/** Tile colour of each database in the new-connection picker. */
export const KIND_TINT: Readonly<Record<DatabaseKind, AccentPreset>> = mapRecord(
  KIND_DESCRIPTORS,
  (k) => ACCENT_PRESETS.find((p) => p === k.tint) ?? "blue",
);

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
