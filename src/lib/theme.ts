import type { ThemePref } from "@/stores/ui";

export type ResolvedTheme = "light" | "dark";

export function resolveTheme(pref: ThemePref, systemDark: boolean): ResolvedTheme {
  if (pref === "system") return systemDark ? "dark" : "light";
  return pref;
}

export function applyTheme(theme: ResolvedTheme): void {
  const root = document.documentElement;
  root.classList.toggle("dark", theme === "dark");
  root.style.colorScheme = theme;
}

export const ACCENT_PRESETS = [
  "red",
  "orange",
  "yellow",
  "green",
  "teal",
  "blue",
  "purple",
  "gray",
] as const;
export type AccentPreset = (typeof ACCENT_PRESETS)[number];

/** CSS colour for each preset (mirrors the `[data-accent]` rules in index.css). */
export const ACCENT_CSS: Record<AccentPreset, string> = {
  red: "oklch(0.6 0.19 25)",
  orange: "oklch(0.66 0.17 55)",
  yellow: "oklch(0.72 0.15 85)",
  green: "oklch(0.6 0.15 150)",
  teal: "oklch(0.62 0.12 195)",
  blue: "oklch(0.58 0.17 255)",
  purple: "oklch(0.58 0.17 300)",
  gray: "oklch(0.5 0.01 260)",
};

export function accentColor(name: string | null | undefined): string {
  return ACCENT_CSS[(name ?? "blue") as AccentPreset] ?? ACCENT_CSS.blue;
}

/** Sets the window-wide accent (from the active connection's colour). `null` restores the default blue. */
export function applyAccent(accent: AccentPreset | null): void {
  const root = document.documentElement;
  if (accent) root.dataset.accent = accent;
  else delete root.dataset.accent;
}
