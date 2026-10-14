import { ipc, isMock } from "@/lib/ipc";
import { getPrefs, setPref } from "@/stores/prefs";
import { useUiStore, type ThemePref } from "@/stores/ui";
import { builtinTheme } from "./builtinThemes";
import { withoutThemeOverrides } from "./appearancePrefs";
import {
  cleanThemeName,
  parseThemeFile,
  serializeTheme,
  THEME_FILE_EXTENSION,
  type CustomTheme,
} from "./customTheme";
import { resolveSelection, type ThemeSelection } from "./selection";
import { getCustomThemes, useThemes } from "./themesStore";
import { THEME_TOKEN_NAMES, type ThemeTokens } from "./tokens";

/** A theme being made or edited: what the theme editor holds before Save. */
export type ThemeDraft = Pick<CustomTheme, "name" | "base" | "tokens">;

/** The label of a theme choice (Auto, Light, a custom theme's name…). */
export function themeLabel(
  pref: ThemePref,
  customs: readonly CustomTheme[] = getCustomThemes(),
): string {
  if (pref === "system") return "Auto";
  if (pref.startsWith("custom:")) {
    const id = pref.slice("custom:".length);
    return customs.find((t) => t.id === id)?.name ?? "Custom theme";
  }
  return builtinTheme(pref)?.label ?? pref;
}

/**
 * "New Theme…" / "Duplicate": a copy of the theme in effect, with the user's colour overrides
 * folded in, so the copy looks exactly like the window does now.
 */
export function draftFromSelection(
  selection: ThemeSelection,
  overrides: Readonly<Partial<ThemeTokens>> = {},
  label?: string,
): ThemeDraft {
  const baseName = label ?? themeLabel(selection.key as ThemePref);
  return {
    name: cleanThemeName(`${baseName} copy`) ?? "My theme",
    base: selection.base,
    tokens: { ...selection.tokens, ...overrides },
  };
}

/** The draft for "New Theme…" from the current choice. */
export function newThemeDraft(): ThemeDraft {
  const { themePref } = useUiStore.getState();
  const systemDark =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches;
  const selection = resolveSelection(themePref, systemDark, getCustomThemes());
  return draftFromSelection(selection, getPrefs().colorOverrides[selection.key]);
}

/** Saves a draft as a new custom theme and (by default) switches to it. */
export function saveNewTheme(draft: ThemeDraft, select = true): CustomTheme | null {
  const created = useThemes.getState().add(draft);
  if (created && select) useUiStore.getState().setThemePref(`custom:${created.id}`);
  return created;
}

/** Saves the editor's changes to a custom theme. */
export function saveThemeEdit(id: string, draft: ThemeDraft): void {
  useThemes.getState().update(id, draft);
}

/** Deletes a custom theme, its colour overrides, and falls back to Auto when it was in use. */
export function deleteCustomTheme(id: string): void {
  const key = `custom:${id}`;
  if (useUiStore.getState().themePref === key) useUiStore.getState().setThemePref("system");
  useThemes.getState().remove(id);
  setPref("colorOverrides", withoutThemeOverrides(getPrefs().colorOverrides, key));
}

/** The theme in effect as the text of a `.tablertheme` file (colour overrides included). */
export function exportText(selection: ThemeSelection, name: string): string {
  const overrides = getPrefs().colorOverrides[selection.key] ?? {};
  const tokens: Partial<ThemeTokens> = {};
  for (const token of THEME_TOKEN_NAMES) {
    const value = overrides[token] ?? selection.tokens[token];
    tokens[token] = value;
  }
  return serializeTheme({ name, base: selection.base, tokens });
}

export type ImportResult = { ok: true; theme: CustomTheme } | { ok: false; error: string };

/** Imports the text of a `.tablertheme` file as a new custom theme and switches to it. */
export function importThemeText(text: string): ImportResult {
  const parsed = parseThemeFile(text);
  if (!parsed.ok) return parsed;
  const theme = saveNewTheme(parsed.theme);
  if (!theme) return { ok: false, error: "There is no room for more custom themes." };
  return { ok: true, theme };
}

// --- files ---------------------------------------------------------------------

const FILTERS = [{ name: "Tabler theme", extensions: [THEME_FILE_EXTENSION] }];

/** Asks where to save a theme file (save panel). */
export async function pickThemeSavePath(name: string): Promise<string | null> {
  const file = `${name.replace(/[/\\:*?"<>|]+/g, "-")}.${THEME_FILE_EXTENSION}`;
  if (isMock) return window.prompt("Export theme to", `/Users/me/Downloads/${file}`);
  const { save } = await import("@tauri-apps/plugin-dialog");
  const picked = await save({ title: "Export Theme", defaultPath: file, filters: FILTERS });
  return typeof picked === "string" ? picked : null;
}

/** Asks for a theme file to import (open panel). */
export async function pickThemeOpenPath(): Promise<string | null> {
  if (isMock)
    return window.prompt("Import theme from", `/Users/me/Downloads/theme.${THEME_FILE_EXTENSION}`);
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    title: "Import Theme",
    multiple: false,
    directory: false,
    filters: FILTERS,
  });
  return typeof picked === "string" ? picked : null;
}

/** Export Theme…: picks the file and writes the theme. Resolves to the path, or null when cancelled. */
export async function exportThemeToFile(
  selection: ThemeSelection,
  name: string,
): Promise<string | null> {
  const path = (await pickThemeSavePath(name))?.trim();
  if (!path) return null;
  await ipc.files.writeText(path, exportText(selection, name));
  return path;
}

/** Import Theme…: picks the file, validates it and adds the theme. Null when cancelled. */
export async function importThemeFromFile(): Promise<ImportResult | null> {
  const path = (await pickThemeOpenPath())?.trim();
  if (!path) return null;
  let text: string;
  try {
    text = await ipc.files.readText(path);
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  return importThemeText(text);
}
