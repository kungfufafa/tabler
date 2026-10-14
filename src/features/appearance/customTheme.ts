import type { ThemeBase } from "./builtinThemes";
import { isThemeColor } from "./color";
import { isThemeToken, THEME_TOKEN_NAMES, type ThemeToken, type ThemeTokens } from "./tokens";

/** A theme the user made (or imported). Tokens it leaves out come from its base (Light / Dark). */
export interface CustomTheme {
  id: string;
  name: string;
  base: ThemeBase;
  tokens: Partial<ThemeTokens>;
}

export const THEME_FILE_EXTENSION = "tablertheme";
export const THEME_FILE_FORMAT = "tabler-theme";
export const THEME_FILE_VERSION = 1;
/** A `.tablertheme` file is a few hundred bytes of JSON; anything bigger is not one. */
export const MAX_THEME_FILE_BYTES = 32 * 1024;
export const MAX_THEME_NAME = 48;
/** Custom themes kept in the app DB. */
export const MAX_CUSTOM_THEMES = 100;

export const isThemeBase = (v: unknown): v is ThemeBase => v === "light" || v === "dark";

/** A theme's id: what `custom:<id>` in the theme preference refers to. */
export function newThemeId(): string {
  const random =
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID().replaceAll("-", "").slice(0, 12)
      : Math.random().toString(36).slice(2, 14).padEnd(12, "0");
  return `t${random}`;
}

const ID = /^[A-Za-z0-9_-]{1,64}$/;
export const isThemeId = (v: unknown): v is string => typeof v === "string" && ID.test(v);

/** A usable theme name: 1–48 characters after trimming, no control characters. */
export function cleanThemeName(name: unknown): string | null {
  if (typeof name !== "string") return null;
  const spaced = [...name].map((c) => (c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127 ? " " : c));
  const trimmed = spaced.join("").trim().replace(/\s+/g, " ");
  return trimmed.length >= 1 && trimmed.length <= MAX_THEME_NAME ? trimmed : null;
}

/** `name`, or `name 2`, `name 3`… when `taken` already has it (case-insensitive). */
export function uniqueThemeName(name: string, taken: readonly string[]): string {
  const used = new Set(taken.map((n) => n.toLowerCase()));
  if (!used.has(name.toLowerCase())) return name;
  const base = name.replace(/\s+\d+$/, "").slice(0, MAX_THEME_NAME - 4);
  for (let n = 2; ; n++) {
    const candidate = `${base} ${n}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}

export type TokenCheck = { ok: true; tokens: Partial<ThemeTokens> } | { ok: false; error: string };

/** Validates a token map: only known names, only colours. Values are trimmed. */
export function checkTokens(raw: unknown): TokenCheck {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw))
    return { ok: false, error: "The theme's colours must be an object." };
  const tokens: Partial<ThemeTokens> = {};
  for (const [name, value] of Object.entries(raw)) {
    if (!isThemeToken(name)) return { ok: false, error: `Unknown colour “${name.slice(0, 40)}”.` };
    if (!isThemeColor(value))
      return { ok: false, error: `“${name}” is not a colour (use #rrggbb or oklch(…)).` };
    tokens[name] = value.trim();
  }
  return { ok: true, tokens };
}

export type ThemeParse =
  { ok: true; theme: Omit<CustomTheme, "id"> } | { ok: false; error: string };

/** The text of a `.tablertheme` file: JSON with the tokens in their canonical order. */
export function serializeTheme(theme: Pick<CustomTheme, "name" | "base" | "tokens">): string {
  const tokens: Partial<Record<ThemeToken, string>> = {};
  for (const name of THEME_TOKEN_NAMES) {
    const value = theme.tokens[name];
    if (value !== undefined) tokens[name] = value;
  }
  return `${JSON.stringify(
    {
      format: THEME_FILE_FORMAT,
      version: THEME_FILE_VERSION,
      name: theme.name,
      base: theme.base,
      tokens,
    },
    null,
    2,
  )}\n`;
}

/** Reads a `.tablertheme` file. Everything is checked; the first problem comes back as the error. */
export function parseThemeFile(text: string): ThemeParse {
  if (new TextEncoder().encode(text).length > MAX_THEME_FILE_BYTES)
    return { ok: false, error: "This file is too large to be a theme (32 KB at most)." };
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return { ok: false, error: "This is not a theme file (it is not valid JSON)." };
  }
  if (raw === null || typeof raw !== "object" || Array.isArray(raw))
    return { ok: false, error: "This is not a theme file." };
  const file = raw as Record<string, unknown>;
  if (file.format !== THEME_FILE_FORMAT) return { ok: false, error: "This is not a Tabler theme." };
  if (file.version !== THEME_FILE_VERSION)
    return { ok: false, error: "This theme was made by a newer version of Tabler." };
  const name = cleanThemeName(file.name);
  if (!name)
    return { ok: false, error: `The theme needs a name of 1–${MAX_THEME_NAME} characters.` };
  if (!isThemeBase(file.base))
    return { ok: false, error: "The theme's base must be “light” or “dark”." };
  const checked = checkTokens(file.tokens);
  if (!checked.ok) return checked;
  return { ok: true, theme: { name, base: file.base, tokens: checked.tokens } };
}

/**
 * Reads the stored list of custom themes (app DB or the localStorage copy). Entries that do not
 * validate, repeat an id or exceed the limit are dropped, so stale or hand-edited data never breaks
 * the window.
 */
export function sanitizeThemes(raw: unknown): CustomTheme[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: CustomTheme[] = [];
  for (const entry of raw) {
    if (out.length >= MAX_CUSTOM_THEMES) break;
    if (entry === null || typeof entry !== "object") continue;
    const t = entry as Record<string, unknown>;
    const name = cleanThemeName(t.name);
    const checked = checkTokens(t.tokens);
    if (!isThemeId(t.id) || seen.has(t.id) || !name || !isThemeBase(t.base) || !checked.ok)
      continue;
    seen.add(t.id);
    out.push({ id: t.id, name, base: t.base, tokens: checked.tokens });
  }
  return out;
}
