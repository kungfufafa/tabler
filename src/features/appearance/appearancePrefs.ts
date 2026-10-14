import { isThemeColor } from "./color";
import { EDITOR_COLOR_TOKENS, GRID_COLOR_TOKENS, type ThemeToken } from "./tokens";

/**
 * The value types and validators of the Fonts & Colors preferences (`prefs.ts` stores them).
 * Kept apart from the store so this module has no dependency on it.
 */

export const EDITOR_LINE_HEIGHT_RANGE = { min: 1, max: 2.5, step: 0.05 } as const;
export const DEFAULT_EDITOR_LINE_HEIGHT = 1.6;

export const GRID_FONT_SIZE_RANGE = { min: 9, max: 18 } as const;
export const DEFAULT_GRID_FONT_SIZE = 12;
export const GRID_LINE_HEIGHT_RANGE = { min: 1, max: 2.5, step: 0.05 } as const;
/** 12px × 1.35 rounds to the 16px line the grid has always had. */
export const DEFAULT_GRID_LINE_HEIGHT = 1.35;
/** Vertical padding of a grid row, in px; the row is the line height plus this twice. */
export const GRID_ROW_PADDING_RANGE = { min: 0, max: 12 } as const;
export const DEFAULT_GRID_ROW_PADDING = 3;
/** Compact / Default / Comfortable. */
export const GRID_ROW_PADDING_PRESETS = [
  { id: "compact", label: "Compact", padding: 1 },
  { id: "default", label: "Default", padding: DEFAULT_GRID_ROW_PADDING },
  { id: "comfortable", label: "Comfortable", padding: 6 },
] as const;

export const SIDEBAR_FONT_SIZE_RANGE = { min: 10, max: 16 } as const;
/** What the sidebar's rows use without an override (`text-xs`). */
export const DEFAULT_SIDEBAR_FONT_SIZE = 11;

/** Longest font-family text accepted. */
export const MAX_FONT_FAMILY_LENGTH = 120;

/** The monospaced fonts the editor's font field suggests (any installed font can be typed). */
export const MONO_FONT_SUGGESTIONS = [
  "SF Mono",
  "Menlo",
  "Monaco",
  "JetBrains Mono",
  "Fira Code",
  "Source Code Pro",
  "Courier New",
] as const;

/** Fonts the data table's field suggests. */
export const GRID_FONT_SUGGESTIONS = [
  "SF Pro Text",
  "Helvetica Neue",
  "Arial",
  ...MONO_FONT_SUGGESTIONS,
] as const;

const FAMILY_CHARS = /^[\p{L}\p{N} ._+\-,]*$/u;

/** A font-family text: names separated by commas; letters, digits, spaces and `. _ + -`. */
export function isFontFamily(value: unknown): value is string {
  return (
    typeof value === "string" && value.length <= MAX_FONT_FAMILY_LENGTH && FAMILY_CHARS.test(value)
  );
}

/**
 * A CSS `font-family` value for the text a user typed: every name quoted, then `fallback`
 * (a generic family or stack). "" and invalid text give null (the stylesheet default applies).
 */
export function fontFamilyCss(text: string, fallback: string): string | null {
  if (!isFontFamily(text)) return null;
  const names = text
    .split(",")
    .map((n) => n.trim().replace(/\s+/g, " "))
    .filter(Boolean);
  if (names.length === 0) return null;
  return `${names.map((n) => `"${n}"`).join(", ")}, ${fallback}`;
}

/** The tokens whose colour the user can override (Settings ▸ Appearance): editor and data table. */
export const OVERRIDABLE_TOKENS: readonly ThemeToken[] = [
  ...EDITOR_COLOR_TOKENS,
  ...GRID_COLOR_TOKENS,
];

/** Per theme (see `ThemeSelection.key`), the colours that replace the theme's own. */
export type ColorOverrides = Readonly<
  Record<string, Readonly<Partial<Record<ThemeToken, string>>>>
>;

export const EMPTY_COLOR_OVERRIDES: ColorOverrides = Object.freeze({});

const MAX_OVERRIDE_THEMES = 200;
const KEY = /^(?:light|dark|solarized-light|solarized-dark|custom:[A-Za-z0-9_-]{1,64})$/;

export function isColorOverrides(value: unknown): value is ColorOverrides {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const entries = Object.entries(value);
  if (entries.length > MAX_OVERRIDE_THEMES) return false;
  return entries.every(([key, colors]) => {
    if (!KEY.test(key) || colors === null || typeof colors !== "object" || Array.isArray(colors))
      return false;
    return Object.entries(colors).every(
      ([token, color]) =>
        (OVERRIDABLE_TOKENS as readonly string[]).includes(token) && isThemeColor(color),
    );
  });
}

/** `overrides` with one colour of one theme set (`color` null removes it); same object if unchanged. */
export function withColorOverride(
  overrides: ColorOverrides,
  themeKey: string,
  token: ThemeToken,
  color: string | null,
): ColorOverrides {
  const current = overrides[themeKey] ?? {};
  if ((current[token] ?? null) === color) return overrides;
  const nextTheme: Partial<Record<ThemeToken, string>> = { ...current };
  if (color === null) delete nextTheme[token];
  else nextTheme[token] = color;
  const next: Record<string, Readonly<Partial<Record<ThemeToken, string>>>> = { ...overrides };
  if (Object.keys(nextTheme).length === 0) delete next[themeKey];
  else next[themeKey] = nextTheme;
  return next;
}

/** `overrides` without any colour of `themeKey`'s (e.g. when a custom theme is deleted). */
export function withoutThemeOverrides(overrides: ColorOverrides, themeKey: string): ColorOverrides {
  if (!(themeKey in overrides)) return overrides;
  const next: Record<string, Readonly<Partial<Record<ThemeToken, string>>>> = { ...overrides };
  delete next[themeKey];
  return next;
}

export const isNumberIn =
  (min: number, max: number) =>
  (v: unknown): boolean =>
    typeof v === "number" && Number.isFinite(v) && v >= min && v <= max;
