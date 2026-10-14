/**
 * The colour tokens a theme sets. Each is the CSS custom property of the same name
 * (`background` → `--background`), declared in `src/index.css` for the built-in themes and
 * injected for custom ones, so the whole window — chrome, data grid, SQL editor — follows a theme
 * with no component knowing about it.
 */

export const TOKEN_GROUPS = [
  "Window",
  "Text",
  "Accent and status",
  "Change markers",
  "SQL editor",
  "Data table",
] as const;
export type TokenGroup = (typeof TOKEN_GROUPS)[number];

export interface TokenDef {
  name: string;
  /** What the theme editor shows. TablePlus's wording where it has one (Fonts & Colors). */
  label: string;
  group: TokenGroup;
}

export const THEME_TOKENS = [
  { name: "background", label: "Background", group: "Window" },
  { name: "surface", label: "Content (grid, editor)", group: "Window" },
  { name: "surface-2", label: "Headers and gutters", group: "Window" },
  { name: "sidebar", label: "Left sidebar", group: "Window" },
  { name: "border", label: "Border", group: "Window" },
  { name: "border-strong", label: "Strong border", group: "Window" },
  { name: "gridline", label: "Grid lines", group: "Window" },
  { name: "row-hover", label: "Row under the pointer", group: "Window" },
  { name: "accent", label: "Hover highlight", group: "Window" },
  { name: "foreground", label: "Text", group: "Text" },
  { name: "muted-foreground", label: "Secondary text", group: "Text" },
  { name: "null", label: "NULL / DEFAULT placeholder", group: "Text" },
  { name: "brand", label: "Accent", group: "Accent and status" },
  { name: "primary-foreground", label: "Text on accent", group: "Accent and status" },
  { name: "error", label: "Error", group: "Accent and status" },
  { name: "success", label: "Success", group: "Accent and status" },
  { name: "warning", label: "Warning", group: "Accent and status" },
  { name: "change-new", label: "New row", group: "Change markers" },
  { name: "change-modified", label: "Modified value", group: "Change markers" },
  { name: "change-deleted", label: "Soft deleted row", group: "Change markers" },
  { name: "syn-comment", label: "Comment", group: "SQL editor" },
  { name: "syn-number", label: "Number", group: "SQL editor" },
  { name: "syn-string", label: "Single quote string", group: "SQL editor" },
  { name: "syn-string-double", label: "Double quote string", group: "SQL editor" },
  { name: "syn-string-backtick", label: "Backtick quote string", group: "SQL editor" },
  { name: "syn-string-bracket", label: "Bracket quote string", group: "SQL editor" },
  { name: "syn-keyword", label: "Reserved keyword", group: "SQL editor" },
  { name: "editor-statement-bg", label: "Current query", group: "SQL editor" },
  { name: "editor-selection-bg", label: "Selected background", group: "SQL editor" },
  { name: "syn-error", label: "Error highlight", group: "SQL editor" },
  { name: "syn-other", label: "Others", group: "SQL editor" },
  { name: "syn-invisible", label: "Invisible characters", group: "SQL editor" },
  { name: "grid-date", label: "Date text", group: "Data table" },
] as const satisfies readonly TokenDef[];

export type ThemeToken = (typeof THEME_TOKENS)[number]["name"];

export const THEME_TOKEN_NAMES: readonly ThemeToken[] = THEME_TOKENS.map((t) => t.name);

const TOKEN_SET: ReadonlySet<string> = new Set(THEME_TOKEN_NAMES);

export function isThemeToken(name: string): name is ThemeToken {
  return TOKEN_SET.has(name);
}

/** Every token as a value; custom themes may hold only some (the rest come from their base). */
export type ThemeTokens = Record<ThemeToken, string>;

export function tokenLabel(name: ThemeToken): string {
  return THEME_TOKENS.find((t) => t.name === name)?.label ?? name;
}

/** The CSS custom property of a token. */
export const tokenVar = (name: ThemeToken): string => `--${name}`;

/** The SQL editor's syntax colours, in the order Settings ▸ Appearance lists them. */
export const EDITOR_COLOR_TOKENS = [
  "syn-comment",
  "syn-number",
  "syn-string",
  "syn-string-double",
  "syn-string-backtick",
  "syn-string-bracket",
  "syn-keyword",
  "editor-statement-bg",
  "editor-selection-bg",
  "syn-error",
  "syn-other",
  "syn-invisible",
] as const satisfies readonly ThemeToken[];

/** The data table's change markers and date colour. */
export const GRID_COLOR_TOKENS = [
  "change-deleted",
  "change-modified",
  "change-new",
  "grid-date",
] as const satisfies readonly ThemeToken[];
