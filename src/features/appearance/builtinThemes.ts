import type { ThemeTokens } from "./tokens";

/**
 * The built-in themes as data. `src/index.css` declares the same values as CSS custom properties
 * (`:root` is Light, `.dark` Dark, `[data-theme-name]` blocks the Solarized pair); a test keeps the
 * two in step. The data side is what "New Theme…" copies, the colour fields show as theme defaults
 * and the contrast tests check.
 */

export type ThemeBase = "light" | "dark";

export type BuiltinThemeId = "light" | "dark" | "solarized-light" | "solarized-dark";

export interface BuiltinTheme {
  id: BuiltinThemeId;
  label: string;
  base: ThemeBase;
  tokens: ThemeTokens;
}

const LIGHT: ThemeTokens = {
  background: "oklch(0.985 0.002 90)",
  surface: "oklch(1 0 0)",
  "surface-2": "oklch(0.965 0.002 90)",
  sidebar: "oklch(0.955 0.003 90)",
  border: "oklch(0.9 0.003 90)",
  "border-strong": "oklch(0.84 0.003 90)",
  gridline: "oklch(0.93 0.002 90)",
  "row-hover": "oklch(0.97 0.003 90)",
  accent: "oklch(0.94 0.003 90)",
  foreground: "oklch(0.22 0.01 260)",
  "muted-foreground": "oklch(0.53 0.01 260)",
  null: "oklch(0.7 0.01 260)",
  brand: "oklch(0.58 0.17 255)",
  "primary-foreground": "oklch(0.99 0 0)",
  error: "oklch(0.58 0.2 27)",
  success: "oklch(0.6 0.15 150)",
  warning: "oklch(0.72 0.16 75)",
  "change-new": "oklch(0.95 0.06 150)",
  "change-modified": "oklch(0.94 0.09 75)",
  "change-deleted": "oklch(0.93 0.06 18)",
  "syn-comment": "oklch(0.53 0.01 260)",
  "syn-number": "oklch(0.55 0.17 60)",
  "syn-string": "oklch(0.5 0.15 150)",
  "syn-string-double": "oklch(0.5 0.15 150)",
  "syn-string-backtick": "oklch(0.5 0.15 150)",
  "syn-string-bracket": "oklch(0.5 0.15 150)",
  "syn-keyword": "oklch(0.5 0.2 290)",
  "editor-statement-bg": "oklch(0.58 0.17 255 / 0.06)",
  "editor-selection-bg": "oklch(0.58 0.17 255 / 0.24)",
  "syn-error": "oklch(0.58 0.2 27)",
  "syn-other": "oklch(0.22 0.01 260)",
  "syn-invisible": "oklch(0.53 0.01 260)",
  "grid-date": "oklch(0.22 0.01 260)",
};

const DARK: ThemeTokens = {
  background: "oklch(0.215 0.004 260)",
  surface: "oklch(0.245 0.004 260)",
  "surface-2": "oklch(0.265 0.004 260)",
  sidebar: "oklch(0.225 0.004 260)",
  border: "oklch(0.32 0.004 260)",
  "border-strong": "oklch(0.4 0.004 260)",
  gridline: "oklch(0.285 0.004 260)",
  "row-hover": "oklch(0.275 0.004 260)",
  accent: "oklch(0.3 0.004 260)",
  foreground: "oklch(0.92 0.005 90)",
  "muted-foreground": "oklch(0.66 0.008 90)",
  null: "oklch(0.55 0.01 260)",
  brand: "oklch(0.63 0.17 255)",
  "primary-foreground": "oklch(0.99 0 0)",
  error: "oklch(0.7 0.18 27)",
  success: "oklch(0.72 0.15 150)",
  warning: "oklch(0.8 0.15 80)",
  "change-new": "oklch(0.38 0.07 150 / 0.5)",
  "change-modified": "oklch(0.42 0.08 65 / 0.55)",
  "change-deleted": "oklch(0.38 0.08 25 / 0.5)",
  "syn-comment": "oklch(0.66 0.008 90)",
  "syn-number": "oklch(0.78 0.14 60)",
  "syn-string": "oklch(0.75 0.13 150)",
  "syn-string-double": "oklch(0.75 0.13 150)",
  "syn-string-backtick": "oklch(0.75 0.13 150)",
  "syn-string-bracket": "oklch(0.75 0.13 150)",
  "syn-keyword": "oklch(0.75 0.15 290)",
  "editor-statement-bg": "oklch(0.63 0.17 255 / 0.06)",
  "editor-selection-bg": "oklch(0.63 0.17 255 / 0.24)",
  "syn-error": "oklch(0.7 0.18 27)",
  "syn-other": "oklch(0.92 0.005 90)",
  "syn-invisible": "oklch(0.66 0.008 90)",
  "grid-date": "oklch(0.92 0.005 90)",
};

/**
 * Solarized (Ethan Schoonover's palette), with the text and accent colours nudged darker / lighter
 * where the original pairs fall under WCAG AA.
 */
const SOLARIZED_LIGHT: ThemeTokens = {
  background: "#f7f1de",
  surface: "#fdf6e3",
  "surface-2": "#f3ecd8",
  sidebar: "#eee8d5",
  border: "#e1dac4",
  "border-strong": "#cfc8b0",
  gridline: "#eee7d2",
  "row-hover": "#f5eedb",
  accent: "#e9e2cc",
  foreground: "#3b5159",
  "muted-foreground": "#506870",
  null: "#9aa7a5",
  brand: "#1d73b3",
  "primary-foreground": "#ffffff",
  error: "#c4302d",
  success: "#5f7400",
  warning: "#8a6d00",
  "change-new": "#e3edc0",
  "change-modified": "#fcdba0",
  "change-deleted": "#f8cdd0",
  "syn-comment": "#6a7e85",
  "syn-number": "#b8430f",
  "syn-string": "#1d7f78",
  "syn-string-double": "#1d7f78",
  "syn-string-backtick": "#1d7f78",
  "syn-string-bracket": "#1d7f78",
  "syn-keyword": "#667700",
  "editor-statement-bg": "#1d73b30f",
  "editor-selection-bg": "#1d73b33d",
  "syn-error": "#c4302d",
  "syn-other": "#3b5159",
  "syn-invisible": "#506870",
  "grid-date": "#3b5159",
};

const SOLARIZED_DARK: ThemeTokens = {
  background: "#002b36",
  surface: "#00313d",
  "surface-2": "#073642",
  sidebar: "#00262f",
  border: "#0f4552",
  "border-strong": "#1c5666",
  gridline: "#0a3945",
  "row-hover": "#063a46",
  accent: "#0b4452",
  foreground: "#b2bebe",
  "muted-foreground": "#9aaeb1",
  null: "#5d777e",
  brand: "#3da5ee",
  "primary-foreground": "#00212b",
  error: "#f2605c",
  success: "#a0b800",
  warning: "#e0a800",
  "change-new": "#2e9e5538",
  "change-modified": "#e08a2c3a",
  "change-deleted": "#dc322f40",
  "syn-comment": "#7b9298",
  "syn-number": "#e8743b",
  "syn-string": "#35bdb2",
  "syn-string-double": "#35bdb2",
  "syn-string-backtick": "#35bdb2",
  "syn-string-bracket": "#35bdb2",
  "syn-keyword": "#a3bf00",
  "editor-statement-bg": "#3da5ee14",
  "editor-selection-bg": "#3da5ee40",
  "syn-error": "#f2605c",
  "syn-other": "#b2bebe",
  "syn-invisible": "#9aaeb1",
  "grid-date": "#b2bebe",
};

export const BUILTIN_THEMES: readonly BuiltinTheme[] = [
  { id: "light", label: "Light", base: "light", tokens: LIGHT },
  { id: "dark", label: "Dark", base: "dark", tokens: DARK },
  { id: "solarized-light", label: "Solarized Light", base: "light", tokens: SOLARIZED_LIGHT },
  { id: "solarized-dark", label: "Solarized Dark", base: "dark", tokens: SOLARIZED_DARK },
];

export function builtinTheme(id: string): BuiltinTheme | undefined {
  return BUILTIN_THEMES.find((t) => t.id === id);
}

/** The tokens of Light / Dark: what a custom theme of that base falls back to. */
export function baseTokens(base: ThemeBase): ThemeTokens {
  return base === "dark" ? DARK : LIGHT;
}
