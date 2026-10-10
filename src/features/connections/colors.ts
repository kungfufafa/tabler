export { ACCENT_PRESETS, type AccentPreset } from "@/lib/theme";
export { ACCENT_CSS, accentColor } from "@/lib/theme";

export const DRIVER_LABEL = {
  postgres: "PostgreSQL",
  mysql: "MySQL / MariaDB",
  sqlite: "SQLite",
} as const;
export const DRIVER_SHORT = { postgres: "PG", mysql: "MY", sqlite: "SQ" } as const;
