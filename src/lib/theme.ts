import { isTauri } from "@/lib/ipc/transport";
import type { ThemePref } from "@/stores/ui";

export type ResolvedTheme = "light" | "dark";

/**
 * The light / dark base of a theme choice. Solarized Light / Dark have their own base, and a custom
 * theme declares one: `customBase` looks it up (a theme that does not exist, e.g. one that was
 * deleted in another window, follows the system).
 */
export function resolveTheme(
  pref: ThemePref,
  systemDark: boolean,
  customBase?: (id: string) => ResolvedTheme | undefined,
): ResolvedTheme {
  switch (pref) {
    case "light":
    case "solarized-light":
      return "light";
    case "dark":
    case "solarized-dark":
      return "dark";
    case "system":
      return systemDark ? "dark" : "light";
    default:
      return customBase?.(pref.slice("custom:".length)) ?? (systemDark ? "dark" : "light");
  }
}

export function applyTheme(theme: ResolvedTheme): void {
  const root = document.documentElement;
  root.classList.toggle("dark", theme === "dark");
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
}

/**
 * What the native side (menu bar, open / save panels, message dialogs, title-bar chrome) should
 * look like: `null` = follow the system (Theme "Auto", or a custom theme that no longer exists),
 * otherwise the light / dark base of the chosen theme (Solarized and custom themes included).
 */
export function nativeThemeFor(
  pref: ThemePref,
  customBase?: (id: string) => ResolvedTheme | undefined,
): ResolvedTheme | null {
  if (pref === "system") return null;
  if (pref.startsWith("custom:") && !customBase?.(pref.slice("custom:".length))) return null;
  return resolveTheme(pref, false, customBase);
}

/** Native theme changes run one after the other, so the last one requested is the one that sticks. */
let nativeQueue: Promise<void> = Promise.resolve();

async function applyNativeTheme(theme: ResolvedTheme | null): Promise<void> {
  try {
    const { setTheme } = await import("@tauri-apps/api/app");
    await setTheme(theme);
    return;
  } catch {
    // The app-wide call is unavailable (older runtime, or not permitted): try this window alone.
  }
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTheme(theme);
  } catch {
    // Cosmetic: the web content already follows the theme.
  }
}

/**
 * Sets the whole app's native appearance (macOS NSApplication appearance and the like): the
 * Settings ▸ Appearance theme then also drives the menu bar, file panels, native message boxes and
 * the window chrome, not only the web content. `null` hands control back to the system. Does
 * nothing in a browser; never rejects.
 */
export function setNativeTheme(theme: ResolvedTheme | null): Promise<void> {
  if (!isTauri()) return Promise.resolve();
  nativeQueue = nativeQueue.then(() => applyNativeTheme(theme));
  return nativeQueue;
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

/** A custom connection colour (the colour picker / Hex field): `#rrggbb`. */
export const HEX_COLOR = /^#[0-9a-f]{6}$/i;

export function isHexColor(value: string | null | undefined): value is string {
  return typeof value === "string" && HEX_COLOR.test(value);
}

/** CSS colour of a preset name or a `#rrggbb` custom colour (unknown names: blue). */
export function accentColor(name: string | null | undefined): string {
  if (isHexColor(name)) return name;
  return ACCENT_CSS[(name ?? "blue") as AccentPreset] ?? ACCENT_CSS.blue;
}

/** Relative luminance (WCAG) of a `#rrggbb` colour, 0–1. */
export function hexLuminance(hex: string): number {
  const channel = (i: number) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

/** Text colour on a filled `#rrggbb` background: near-black on light colours, else near-white. */
export function hexForeground(hex: string): string {
  return hexLuminance(hex) > 0.45 ? "oklch(0.2 0 0)" : "oklch(0.99 0 0)";
}

/**
 * Sets the window-wide accent (from the active connection's colour): a preset name, or a
 * `#rrggbb` custom colour (`data-accent="custom"`, with `--brand` and the text colour on it set
 * inline; `--brand-soft` follows `--brand`). `null` restores the default blue.
 */
export function applyAccent(accent: AccentPreset | string | null): void {
  const root = document.documentElement;
  root.style.removeProperty("--brand");
  root.style.removeProperty("--primary-foreground");
  if (isHexColor(accent)) {
    root.dataset.accent = "custom";
    root.style.setProperty("--brand", accent);
    root.style.setProperty("--primary-foreground", hexForeground(accent));
  } else if (accent) root.dataset.accent = accent;
  else delete root.dataset.accent;
}
