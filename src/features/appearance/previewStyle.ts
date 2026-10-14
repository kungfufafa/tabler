import type { CSSProperties } from "react";
import { baseTokens, type ThemeBase } from "./builtinThemes";
import { THEME_TOKEN_NAMES, tokenVar, type ThemeTokens } from "./tokens";

/** The custom properties of a theme's tokens, to scope a preview to the theme being edited. */
export function tokenStyle(base: ThemeBase, tokens: Partial<ThemeTokens>): CSSProperties {
  const merged = { ...baseTokens(base), ...tokens };
  const style: Record<string, string> = {};
  for (const name of THEME_TOKEN_NAMES) style[tokenVar(name)] = merged[name];
  return style as CSSProperties;
}
