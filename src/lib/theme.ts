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

/** Sets the window-wide accent (from the active connection's colour). `null` restores the default blue. */
export function applyAccent(accent: AccentPreset | null): void {
  const root = document.documentElement;
  if (accent) root.dataset.accent = accent;
  else delete root.dataset.accent;
}
