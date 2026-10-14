import type { Prefs } from "@/stores/prefs";
import {
  DEFAULT_EDITOR_LINE_HEIGHT,
  DEFAULT_GRID_FONT_SIZE,
  DEFAULT_GRID_LINE_HEIGHT,
  DEFAULT_SIDEBAR_FONT_SIZE,
  fontFamilyCss,
  OVERRIDABLE_TOKENS,
} from "./appearancePrefs";
import { isThemeColor } from "./color";
import { gridLineHeightPx, gridRowHeight } from "./gridMetrics";
import { tokenVar } from "./tokens";

/** What the fonts-and-colours preferences set on <html>, as data (so it can be tested). */
export interface AppearanceStyle {
  /** CSS custom properties (`--name` → value). */
  vars: Record<string, string>;
  /** `data-*` attributes (`data-grid-no-elastic` → ""). */
  attrs: Record<string, string>;
  /** The grid's row height in px. */
  rowHeight: number;
}

export type AppearancePrefs = Pick<
  Prefs,
  | "editorFontFamily"
  | "editorLineHeight"
  | "gridFontFamily"
  | "gridFontSize"
  | "gridLineHeight"
  | "gridRowPadding"
  | "sidebarFontSize"
  | "sidebarFontFamily"
  | "colorOverrides"
  | "gridNoElastic"
>;

const MONO_FALLBACK = "ui-monospace, Menlo, Monaco, monospace";
const SANS_FALLBACK = "-apple-system, BlinkMacSystemFont, system-ui, sans-serif";

/**
 * The custom properties and attributes for the preferences and the theme in effect (`themeKey`,
 * for the per-theme colour overrides). A preference left at its default adds nothing, so the
 * stylesheet's own values stay in charge.
 */
export function computeAppearanceStyle(prefs: AppearancePrefs, themeKey: string): AppearanceStyle {
  const vars: Record<string, string> = {};
  const attrs: Record<string, string> = {};

  const editorFont = fontFamilyCss(prefs.editorFontFamily, MONO_FALLBACK);
  if (editorFont) vars["--editor-font-family"] = editorFont;
  if (prefs.editorLineHeight !== DEFAULT_EDITOR_LINE_HEIGHT)
    vars["--editor-line-height"] = String(prefs.editorLineHeight);

  const gridFont = fontFamilyCss(prefs.gridFontFamily, SANS_FALLBACK);
  if (gridFont) vars["--grid-font-family"] = gridFont;
  if (prefs.gridFontSize !== DEFAULT_GRID_FONT_SIZE)
    vars["--grid-font-size"] = `${prefs.gridFontSize}px`;
  const linePx = gridLineHeightPx(prefs.gridFontSize, prefs.gridLineHeight);
  if (linePx !== gridLineHeightPx(DEFAULT_GRID_FONT_SIZE, DEFAULT_GRID_LINE_HEIGHT))
    vars["--grid-line-height"] = `${linePx}px`;

  if (prefs.sidebarFontSize !== DEFAULT_SIDEBAR_FONT_SIZE) {
    vars["--sidebar-font-size"] = `${prefs.sidebarFontSize}px`;
    attrs["data-sidebar-font"] = "";
  }
  const sidebarFont = fontFamilyCss(prefs.sidebarFontFamily, SANS_FALLBACK);
  if (sidebarFont) vars["--sidebar-font-family"] = sidebarFont;
  if (prefs.gridNoElastic) attrs["data-grid-no-elastic"] = "";

  const colors = prefs.colorOverrides[themeKey] ?? {};
  for (const token of OVERRIDABLE_TOKENS) {
    const value = colors[token];
    if (value !== undefined && isThemeColor(value)) vars[tokenVar(token)] = value;
  }

  return {
    vars,
    attrs,
    rowHeight: gridRowHeight(prefs.gridFontSize, prefs.gridLineHeight, prefs.gridRowPadding),
  };
}

/** Every custom property and attribute the preferences can set (what a re-apply clears first). */
const ALL_VARS: readonly string[] = [
  "--editor-font-family",
  "--editor-line-height",
  "--grid-font-family",
  "--grid-font-size",
  "--grid-line-height",
  "--sidebar-font-size",
  "--sidebar-font-family",
  ...OVERRIDABLE_TOKENS.map(tokenVar),
];
const ALL_ATTRS: readonly string[] = ["data-sidebar-font", "data-grid-no-elastic"];

/** Applies `style` to `root`, removing whatever an earlier apply set that it no longer does. */
export function applyAppearanceStyle(
  style: AppearanceStyle,
  root: HTMLElement = document.documentElement,
): void {
  for (const name of ALL_VARS) {
    const value = style.vars[name];
    if (value === undefined) root.style.removeProperty(name);
    else if (root.style.getPropertyValue(name) !== value) root.style.setProperty(name, value);
  }
  for (const name of ALL_ATTRS) {
    const value = style.attrs[name];
    if (value === undefined) root.removeAttribute(name);
    else if (root.getAttribute(name) !== value) root.setAttribute(name, value);
  }
}
