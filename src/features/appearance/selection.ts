import type { ThemePref } from "@/stores/ui";
import { resolveTheme } from "@/lib/theme";
import { baseTokens, builtinTheme, type ThemeBase } from "./builtinThemes";
import type { CustomTheme } from "./customTheme";
import { isThemeColor } from "./color";
import { THEME_TOKEN_NAMES, tokenVar, type ThemeTokens } from "./tokens";

/** The theme in effect: what the window is painted with. */
export interface ThemeSelection {
  /**
   * Identifies the theme for per-theme colour overrides: `light`, `dark`, `solarized-light`,
   * `solarized-dark` or `custom:<id>`. "Auto" is the light or dark it currently resolves to.
   */
  key: string;
  /** Light / dark base (colour-scheme and the `.dark` class). */
  base: ThemeBase;
  /** The `data-theme-name` of <html>; null for plain Light / Dark. */
  name: "solarized-light" | "solarized-dark" | "custom" | null;
  custom: CustomTheme | null;
  /** The theme's own colour of every token (before the user's overrides). */
  tokens: ThemeTokens;
}

/** The selection for a theme choice. `customs` is the list of custom themes. */
export function resolveSelection(
  pref: ThemePref,
  systemDark: boolean,
  customs: readonly CustomTheme[],
): ThemeSelection {
  const customOf = (id: string) => customs.find((t) => t.id === id);
  const base = resolveTheme(pref, systemDark, (id) => customOf(id)?.base);
  if (pref.startsWith("custom:")) {
    const custom = customOf(pref.slice("custom:".length));
    if (custom)
      return {
        key: pref,
        base: custom.base,
        name: "custom",
        custom,
        tokens: { ...baseTokens(custom.base), ...custom.tokens },
      };
  }
  const named = builtinTheme(pref);
  if (named)
    return {
      key: named.id,
      base: named.base,
      name: named.id === "light" || named.id === "dark" ? null : named.id,
      custom: null,
      tokens: named.tokens,
    };
  // Auto, or a custom theme that no longer exists.
  return { key: base, base, name: null, custom: null, tokens: baseTokens(base) };
}

/**
 * The CSS of a custom theme: its colours on `[data-theme-name="custom"]` (more specific than the
 * `:root` and `.dark` blocks, so they win), and the accent pair under `:not([data-accent])` so a
 * connection's own colour still wins. Only known tokens with valid colours are written, so the text
 * is safe to inject.
 */
export function customThemeCss(theme: CustomTheme): string {
  const decls: string[] = [];
  const accent: string[] = [];
  for (const name of THEME_TOKEN_NAMES) {
    const value = theme.tokens[name];
    if (value === undefined || !isThemeColor(value)) continue;
    const line = `  ${tokenVar(name)}: ${value};`;
    if (name === "brand" || name === "primary-foreground") accent.push(line);
    else decls.push(line);
  }
  const blocks = [`:root[data-theme-name="custom"] {\n${decls.join("\n")}\n}`];
  if (accent.length > 0)
    blocks.push(`:root[data-theme-name="custom"]:not([data-accent]) {\n${accent.join("\n")}\n}`);
  return blocks.join("\n");
}

const STYLE_ID = "tabler-custom-theme";

/** Puts the selection on <html>: `data-theme-name`, and the custom theme's style element. */
export function applyThemeName(
  selection: ThemeSelection,
  root: HTMLElement = document.documentElement,
): void {
  if (selection.name) root.dataset.themeName = selection.name;
  else delete root.dataset.themeName;
  const doc = root.ownerDocument;
  let style = doc.getElementById(STYLE_ID);
  if (selection.custom) {
    if (!style) {
      style = doc.createElement("style");
      style.id = STYLE_ID;
      doc.head.appendChild(style);
    }
    const css = customThemeCss(selection.custom);
    if (style.textContent !== css) style.textContent = css;
  } else style?.remove();
}
