import { toast } from "sonner";
import { create } from "zustand";
import { ipc, type AssistantProvider, type SafeModeLevel } from "@/lib/ipc";
import { broadcastEvent, RENDERER_ID } from "@/lib/ipc/events";
import { EMPTY_OVERRIDES, isKeymapOverrides, type KeymapOverrides } from "@/lib/keys/overrides";
import {
  DEFAULT_TOOLBAR_ITEMS,
  isToolbarItems,
  TOOLBAR_DISPLAYS,
  type ToolbarDisplay,
  type ToolbarItemId,
} from "@/app/toolbarLayout";
import {
  ANTHROPIC_MODEL_IDS,
  ASSISTANT_PROVIDERS,
  DEFAULT_ANTHROPIC_MODEL,
  DEFAULT_OLLAMA_BASE_URL,
  DEFAULT_OPENAI_BASE_URL,
  isBaseUrl,
  isModelName,
} from "@/features/assistant/providers";
import {
  EMPTY_PLUGIN_STATES,
  isPluginStates,
  type PluginStates,
} from "@/features/plugins/pluginPrefs";
import {
  DEFAULT_EDITOR_LINE_HEIGHT,
  DEFAULT_GRID_FONT_SIZE,
  DEFAULT_GRID_LINE_HEIGHT,
  DEFAULT_GRID_ROW_PADDING,
  DEFAULT_SIDEBAR_FONT_SIZE,
  EDITOR_LINE_HEIGHT_RANGE,
  EMPTY_COLOR_OVERRIDES,
  GRID_FONT_SIZE_RANGE,
  GRID_LINE_HEIGHT_RANGE,
  GRID_ROW_PADDING_RANGE,
  isColorOverrides,
  isFontFamily,
  isNumberIn,
  SIDEBAR_FONT_SIZE_RANGE,
  type ColorOverrides,
} from "@/features/appearance/appearancePrefs";

/**
 * User preferences. One JSON object stored under the `prefs` key of the app DB settings table,
 * mirrored to localStorage so the first paint already uses them. Theme lives in the UI store.
 */

/** App DB settings key holding the whole preferences object. */
export const PREFS_KEY = "prefs";
/** localStorage mirror for instant startup (the app DB copy wins once loaded). */
export const PREFS_LOCAL_KEY = "tabler.prefs";
export const SAVE_DEBOUNCE_MS = 300;

/** Interface text size in px; the root font-size is scaled so rem-based UI follows. */
export const UI_FONT_SIZES = { small: 12, default: 13, large: 14 } as const;
export type UiFontSize = keyof typeof UI_FONT_SIZES;

export const PAGE_SIZES = [100, 200, 300, 500, 1000] as const;
export type PageSize = (typeof PAGE_SIZES)[number];

export const TAB_SIZES = [2, 4] as const;
export type TabSize = (typeof TAB_SIZES)[number];

export const EDITOR_FONT_SIZE_RANGE = { min: 11, max: 18 } as const;
export const CONNECT_TIMEOUT_RANGE = { min: 1, max: 300 } as const;
/** Query time out in seconds (0 = no limit); TablePlus defaults to 300 s. */
export const QUERY_TIMEOUT_RANGE = { min: 0, max: 86_400 } as const;

/** SQL editor results: one tab per result set, or every result set stacked in one tab. */
export const RESULT_VIEW_MODES = ["tabs", "append"] as const;
export type ResultViewMode = (typeof RESULT_VIEW_MODES)[number];

/** Structure view ▸ "Order columns by": the table's ordinal position, or their names. */
export const STRUCTURE_COLUMN_ORDERS = ["ordinal", "name"] as const;
export type StructureColumnOrder = (typeof STRUCTURE_COLUMN_ORDERS)[number];

export const SAFE_MODE_LEVELS = [
  "silent",
  "alert",
  "alertNonSelect",
  "safe",
  "safeNonSelect",
] as const satisfies readonly SafeModeLevel[];

/**
 * The variable syntaxes the SQL editor's query parameters can use (⚙ ▸ Query Params Options ▸
 * Change Query Params Regex), as TablePlus lists them; there is no custom regex.
 */
export const QUERY_PARAM_SYNTAXES = [
  ":[\\w.]+",
  "%[\\w.]+%",
  "\\?",
  "\\$\\{[\\w.]+\\}",
  "\\$[\\w.]+",
  "\\{\\{[\\w.]+\\}\\}",
] as const;
export type QueryParamSyntax = (typeof QUERY_PARAM_SYNTAXES)[number];

/** Filter bar ⚙ ▸ Default filter column / operator / state (TablePlus's choices). */
export const FILTER_DEFAULT_COLUMNS = ["primaryKey", "anyColumn", "rawSql"] as const;
export type FilterDefaultColumn = (typeof FILTER_DEFAULT_COLUMNS)[number];
export const FILTER_DEFAULT_OPERATORS = ["eq", "contains"] as const;
export type FilterDefaultOperator = (typeof FILTER_DEFAULT_OPERATORS)[number];
export const FILTER_DEFAULT_STATES = ["restore", "show", "hide"] as const;
export type FilterDefaultState = (typeof FILTER_DEFAULT_STATES)[number];
/** "Estimate count when table has more than [N] rows". */
export const ESTIMATE_COUNT_RANGE = { min: 1_000, max: 1_000_000_000 } as const;

/**
 * SQL editor ⚙ ▸ Auto Save SQL Query Tab: which query tabs keep their text across restarts.
 * "none": none ("Do not save the SQL query"), "first": the first query tab of each workspace,
 * "all": every one (TablePlus's default since b632).
 */
export const AUTO_SAVE_QUERY_TABS = ["none", "first", "all"] as const;

/** General ▸ Application ▸ "At launch" (TablePlus's older "Connection startup" preference). */
export const STARTUP_ACTIONS = ["welcome", "connection"] as const;
export type StartupAction = (typeof STARTUP_ACTIONS)[number];
export type AutoSaveQueryTabs = (typeof AUTO_SAVE_QUERY_TABS)[number];

/**
 * General ▸ CSV default options (TablePlus): Copy Rows As ▸ CSV, the export dialog's CSV defaults
 * and the CSV import's fallback when its detection is not sure. Quote: `"` / `'` quote every value,
 * "ifNeeded" only the values that need it.
 */
export const CSV_DELIMITER_PREFS = [",", "\t", ";"] as const;
export type CsvDelimiterPref = (typeof CSV_DELIMITER_PREFS)[number];
export const CSV_QUOTE_PREFS = ["double", "single", "space", "ifNeeded"] as const;
export type CsvQuotePref = (typeof CSV_QUOTE_PREFS)[number];
export const CSV_LINE_BREAK_PREFS = ["\n", "\r\n", "\r"] as const;
export type CsvLineBreakPref = (typeof CSV_LINE_BREAK_PREFS)[number];
/** The decimal separator: `.`, `,` or one custom character (see `isCsvDecimal`). */
export const CSV_DECIMAL_PREFS = [".", ","] as const;
export type CsvDecimalPref = string;
/**
 * A valid decimal separator: `.`, `,` or one printable ASCII character that is not a digit or a
 * quote (the export backend refuses the same).
 */
export const isCsvDecimal = (v: unknown): boolean =>
  typeof v === "string" &&
  v.length === 1 &&
  v.charCodeAt(0) >= 0x20 &&
  v.charCodeAt(0) <= 0x7e &&
  !/[0-9"']/.test(v);

/** SQL editor "Auto-complete key": Enter, Tab, or either (TablePlus: "Enter or Tab"). */
export const EDITOR_ACCEPT_KEYS = ["both", "enter", "tab"] as const;
export type EditorAcceptKey = (typeof EDITOR_ACCEPT_KEYS)[number];
/** General ▸ New tab: "Command + T should open" a SQL Query Editor tab or an empty one. */
export const NEW_TAB_OPENS = ["query", "empty"] as const;
export type NewTabOpens = (typeof NEW_TAB_OPENS)[number];
/** History ▸ "Keep history for": 1, 2 or 3 years, or forever. */
export const HISTORY_RETENTIONS = ["1y", "2y", "3y", "forever"] as const;
export type HistoryRetention = (typeof HISTORY_RETENTIONS)[number];

/** Settings ▸ Security ▸ "Auto lock if away for": minutes without activity; 0 = Never. */
export const AUTO_LOCK_MINUTES = [0, 1, 5, 15, 30, 60] as const;
export type AutoLockMinutes = (typeof AUTO_LOCK_MINUTES)[number];

export interface Prefs {
  /** Interface text size (Small 12px / Default 13px / Large 14px). */
  uiFontSize: UiFontSize;
  /** Rows per page for newly opened table tabs. */
  pageSize: PageSize;
  /**
   * Show the Preview SQL sheet on ⌘S; when false (the default, as in TablePlus) staged changes are
   * applied immediately and ⇧⌘P previews them.
   */
  confirmCommit: boolean;
  /** SQL editor font size in px (11–18). */
  editorFontSize: number;
  editorTabSize: TabSize;
  editorLineWrap: boolean;
  /** Uppercase keywords when formatting SQL. */
  editorUppercaseKeywords: boolean;
  /** Default connect timeout for new connections, in seconds. */
  connectTimeout: number;
  /** Safe-mode level preselected for new connections (Preferences → Security). */
  defaultSafeMode: SafeModeLevel;
  /** Ping every open connection every 30 s so idle sessions are not dropped. */
  keepAlive: boolean;
  /** SQL editor statements still running after this many seconds are cancelled; 0 = never. */
  queryTimeout: number;
  /** Row numbers in the SQL editor's result grids. */
  resultRowNumbers: boolean;
  /** Results header menu ▸ Split Results into Tabs / Append All Results in One Tab. */
  resultViewMode: ResultViewMode;
  /** SQL editor: highlight the statement under the cursor. */
  editorHighlightCurrentQuery: boolean;
  /** SQL editor: show spaces, tabs and line ends. */
  editorShowInvisibles: boolean;
  /** SQL editor: no completion popup while typing (⌃Space still asks). */
  editorDisableSuggestions: boolean;
  /** SQL editor: typing an opening bracket or quote inserts the closing one. */
  editorCloseBrackets: boolean;
  /** SQL editor: a run with variables (see `editorQueryParamsRegex`) asks for their values. */
  editorQueryParams: boolean;
  editorQueryParamsRegex: QueryParamSyntax;
  /** Variables inside quoted strings count too. */
  editorQueryParamsInQuotes: boolean;
  /** "Return on Error": a multi-statement run stops at the first error. */
  editorStopOnError: boolean;
  /** "Enable Raise Notice Notifications": PostgreSQL notices raised by a run show as toasts. */
  editorRaiseNotices: boolean;
  /** Queries pane: a double-click inserts the query into the SQL editor instead of running it. */
  queriesDoubleClickInsert: boolean;
  /** Filter bar: the column a new filter row starts on. */
  filterDefaultColumn: FilterDefaultColumn;
  /** Filter bar: the operator a new filter row starts with. */
  filterDefaultOperator: FilterDefaultOperator;
  /** Filter bar on opening a table: its last state (per table), always shown, or always hidden. */
  filterDefaultState: FilterDefaultState;
  /** Table Data: zebra-striped grid rows. */
  gridAlternatingRows: boolean;
  /** Table Data: unfiltered tables over `gridEstimateThreshold` rows show a "~" planner estimate. */
  gridEstimateCount: boolean;
  gridEstimateThreshold: number;
  /** Table Data: an empty numeric / UUID / JSON / array value is staged as NULL. */
  gridAutoNullEmpty: boolean;
  /** Table Data: a row-number gutter in table grids. */
  gridRowNumbers: boolean;
  /** Structure bottom bar ▸ Order columns by: ordinal position (as stored) or name. */
  structureColumnOrder: StructureColumnOrder;
  /** Table Data ▸ Allows expansion Tool Tips: a truncated cell shows its whole value on hover. */
  gridExpansionTooltips: boolean;
  /** Table Data ▸ Auto hide table scrollers: overlay scroll bars that fade out when idle. */
  gridAutoHideScrollers: boolean;
  /** General ▸ Application ▸ Use loading wallpaper: a placeholder pattern while a page loads. */
  gridLoadingWallpaper: boolean;
  /** General ▸ Data Decoder: PostGIS / MySQL spatial values decode to WKT instead of hex. */
  geometryToWkt: boolean;
  /** General ▸ Application: reconnect the workspaces (and their tabs) open at the last quit. */
  reopenWorkspaces: boolean;
  /**
   * General ▸ Application ▸ "At launch": the welcome screen, or open the connection
   * `startupProfileId` (its form instead with `startupPromptForm`). Reopened workspaces win.
   */
  startupAction: StartupAction;
  startupProfileId: string;
  startupPromptForm: boolean;
  autoSaveQueryTabs: AutoSaveQueryTabs;
  /** Settings ▸ Shortcuts: remapped bindings (see `KeymapOverrides`). */
  keymapOverrides: KeymapOverrides;
  /** General ▸ CSV default options (see `CSV_DELIMITER_PREFS`). */
  csvDelimiter: CsvDelimiterPref;
  csvQuote: CsvQuotePref;
  csvLineBreak: CsvLineBreakPref;
  csvDecimal: CsvDecimalPref;
  /** Table Data: "Show open containing folder popup after exporting data" (exports, backups). */
  exportRevealPopup: boolean;
  /** Native notifications when an export / import / backup / restore ends in the background. */
  showNotifications: boolean;
  /** SQL editor ⚙ ▸ Autocomplete Options ▸ Tables: schemas, tables and columns are suggested. */
  editorCompleteTables: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Keywords: SQL keywords are suggested. */
  editorCompleteKeywords: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Automatically Add a Whitespace after an accepted suggestion. */
  editorAddWhitespace: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Automatically Prefix Schemas: tables complete as `schema.table`. */
  editorPrefixSchemas: boolean;
  /** "Auto-complete key": which key accepts a suggestion. */
  editorAcceptKey: EditorAcceptKey;
  /** "Indent with": tabs instead of spaces (`editorTabSize` wide). */
  editorIndentWithTabs: boolean;
  /** SQL editor ⚙ ▸ Show Scrollers: the editor's scroll bars (off: hidden). */
  editorShowScrollers: boolean;
  /** "Automatically save queries while editing": file-backed query tabs save as you type. */
  editorAutoSaveFiles: boolean;
  /** "Command + T should open": a query tab with the most recent script, or an empty one. */
  newTabOpens: NewTabOpens;
  /** "Keep history for": older history is pruned at startup. */
  historyRetention: HistoryRetention;
  /** Queries ▸ "Auto Save Favorite": the query editor popup saves while you type. */
  queriesAutoSave: boolean;
  /** Security ▸ Unlock Safe Mode with Touch ID (Safe Mode prompts offer Touch ID). */
  safeModeTouchId: boolean;
  /** Security ▸ Passcode ▸ Unlock Passcode with Touch ID. */
  passcodeTouchId: boolean;
  /** Security ▸ Passcode ▸ Auto lock if away for (minutes; 0 = Never). */
  autoLockMinutes: AutoLockMinutes;
  /** Copy as URL with the password was confirmed once (it is not asked again). */
  copyUrlPasswordConfirmed: boolean;
  /** Customize Toolbar…: the items, left → right. */
  toolbarItems: readonly ToolbarItemId[];
  /** Customize Toolbar… ▸ Show: Icon Only / Icon and Text. */
  toolbarDisplay: ToolbarDisplay;
  /**
   * Connections ▸ MCP Server: `tabler --mcp` serves MCP clients (off: it exits). The `--mcp`
   * process reads these three from the app DB (`src-tauri/src/mcp/prefs.rs`).
   */
  mcpEnabled: boolean;
  /** "Allow MCP clients to change data" (only on Silent Mode, non read-only connections). */
  mcpAllowWrites: boolean;
  /** "Expose production connections": Production-tagged connections are hidden otherwise. */
  mcpExposeProduction: boolean;
  /** Security ▸ AI Assistant ▸ Enable Assistant tab on the right sidebar. */
  assistantEnabled: boolean;
  /** AI Assistant: where chat requests go, and each provider's model / server. */
  assistantProvider: AssistantProvider;
  assistantAnthropicModel: string;
  assistantOpenaiBaseUrl: string;
  assistantOpenaiModel: string;
  assistantOllamaBaseUrl: string;
  assistantOllamaModel: string;
  /**
   * General ▸ Sidebar ▸ "Lazy load schema items": the default for connections whose form leaves it
   * unset; the sidebar then lists the schemas and loads one's items when it is expanded.
   */
  sidebarLazyLoad: boolean;
  /** Plugins ▸ Manage Plugins…: per plugin, enabled / permissions granted / links confirmed. */
  pluginStates: PluginStates;
  /** Settings ▸ Appearance ▸ SQL editor: font family ("" = the system monospace) and line height. */
  editorFontFamily: string;
  editorLineHeight: number;
  /** Settings ▸ Appearance ▸ Data table: font ("" = the system font), size, line height, padding. */
  gridFontFamily: string;
  gridFontSize: number;
  gridLineHeight: number;
  /** Vertical padding of a grid row in px (Compact 1 / Default 3 / Comfortable 6). */
  gridRowPadding: number;
  /** Settings ▸ Appearance ▸ Left sidebar font size in px. */
  sidebarFontSize: number;
  /** Settings ▸ Appearance ▸ Left sidebar font family ("" = the UI font). */
  sidebarFontFamily: string;
  /** Per theme, the colours that replace the theme's own (editor syntax, change markers, dates). */
  colorOverrides: ColorOverrides;
  /** General ▸ Table Data ▸ Disable scroll elastically (no rubber-band bounce in the grid). */
  gridNoElastic: boolean;
  /** General ▸ Table Data ▸ Auto expand text column: text columns open as wide as their content. */
  gridAutoExpandText: boolean;
}

export const DEFAULT_PREFS: Readonly<Prefs> = Object.freeze({
  uiFontSize: "default",
  pageSize: 300,
  confirmCommit: false,
  editorFontSize: 13,
  editorTabSize: 4,
  editorLineWrap: false,
  editorUppercaseKeywords: true,
  connectTimeout: 60,
  defaultSafeMode: "silent",
  keepAlive: true,
  queryTimeout: 300,
  resultRowNumbers: true,
  resultViewMode: "tabs",
  editorHighlightCurrentQuery: true,
  editorShowInvisibles: false,
  editorDisableSuggestions: false,
  editorCloseBrackets: true,
  editorQueryParams: false,
  editorQueryParamsRegex: ":[\\w.]+",
  editorQueryParamsInQuotes: false,
  editorStopOnError: true,
  editorRaiseNotices: true,
  queriesDoubleClickInsert: false,
  filterDefaultColumn: "primaryKey",
  filterDefaultOperator: "eq",
  filterDefaultState: "restore",
  gridAlternatingRows: true,
  gridEstimateCount: true,
  gridEstimateThreshold: 500_000,
  gridAutoNullEmpty: true,
  gridRowNumbers: false,
  structureColumnOrder: "ordinal",
  gridExpansionTooltips: false,
  gridAutoHideScrollers: false,
  gridLoadingWallpaper: true,
  geometryToWkt: true,
  reopenWorkspaces: true,
  startupAction: "welcome",
  startupProfileId: "",
  startupPromptForm: false,
  autoSaveQueryTabs: "all",
  keymapOverrides: EMPTY_OVERRIDES,
  csvDelimiter: ",",
  csvQuote: "ifNeeded",
  csvLineBreak: "\n",
  csvDecimal: ".",
  exportRevealPopup: true,
  showNotifications: true,
  editorCompleteTables: true,
  editorCompleteKeywords: true,
  editorAddWhitespace: false,
  editorPrefixSchemas: false,
  editorAcceptKey: "both",
  editorIndentWithTabs: false,
  editorShowScrollers: true,
  editorAutoSaveFiles: true,
  newTabOpens: "query",
  historyRetention: "forever",
  queriesAutoSave: false,
  safeModeTouchId: false,
  passcodeTouchId: false,
  autoLockMinutes: 5,
  copyUrlPasswordConfirmed: false,
  toolbarItems: DEFAULT_TOOLBAR_ITEMS,
  toolbarDisplay: "icon",
  mcpEnabled: false,
  mcpAllowWrites: false,
  mcpExposeProduction: false,
  assistantEnabled: true,
  assistantProvider: "anthropic",
  assistantAnthropicModel: DEFAULT_ANTHROPIC_MODEL,
  assistantOpenaiBaseUrl: DEFAULT_OPENAI_BASE_URL,
  assistantOpenaiModel: "",
  assistantOllamaBaseUrl: DEFAULT_OLLAMA_BASE_URL,
  assistantOllamaModel: "",
  sidebarLazyLoad: false,
  pluginStates: EMPTY_PLUGIN_STATES,
  editorFontFamily: "",
  editorLineHeight: DEFAULT_EDITOR_LINE_HEIGHT,
  gridFontFamily: "",
  gridFontSize: DEFAULT_GRID_FONT_SIZE,
  gridLineHeight: DEFAULT_GRID_LINE_HEIGHT,
  gridRowPadding: DEFAULT_GRID_ROW_PADDING,
  sidebarFontSize: DEFAULT_SIDEBAR_FONT_SIZE,
  sidebarFontFamily: "",
  colorOverrides: EMPTY_COLOR_OVERRIDES,
  gridNoElastic: false,
  gridAutoExpandText: false,
});

const PREF_KEYS = Object.keys(DEFAULT_PREFS) as (keyof Prefs)[];

const oneOf =
  (values: readonly unknown[]) =>
  (v: unknown): boolean =>
    values.includes(v);
const intIn =
  (min: number, max: number) =>
  (v: unknown): boolean =>
    typeof v === "number" && Number.isInteger(v) && v >= min && v <= max;
const isBool = (v: unknown): boolean => typeof v === "boolean";

const VALID: { [K in keyof Prefs]: (v: unknown) => boolean } = {
  uiFontSize: oneOf(Object.keys(UI_FONT_SIZES)),
  pageSize: oneOf(PAGE_SIZES),
  confirmCommit: isBool,
  editorFontSize: intIn(EDITOR_FONT_SIZE_RANGE.min, EDITOR_FONT_SIZE_RANGE.max),
  editorTabSize: oneOf(TAB_SIZES),
  editorLineWrap: isBool,
  editorUppercaseKeywords: isBool,
  connectTimeout: intIn(CONNECT_TIMEOUT_RANGE.min, CONNECT_TIMEOUT_RANGE.max),
  defaultSafeMode: oneOf(SAFE_MODE_LEVELS),
  keepAlive: isBool,
  queryTimeout: intIn(QUERY_TIMEOUT_RANGE.min, QUERY_TIMEOUT_RANGE.max),
  resultRowNumbers: isBool,
  resultViewMode: oneOf(RESULT_VIEW_MODES),
  editorHighlightCurrentQuery: isBool,
  editorShowInvisibles: isBool,
  editorDisableSuggestions: isBool,
  editorCloseBrackets: isBool,
  editorQueryParams: isBool,
  editorQueryParamsRegex: oneOf(QUERY_PARAM_SYNTAXES),
  editorQueryParamsInQuotes: isBool,
  editorStopOnError: isBool,
  editorRaiseNotices: isBool,
  queriesDoubleClickInsert: isBool,
  filterDefaultColumn: oneOf(FILTER_DEFAULT_COLUMNS),
  filterDefaultOperator: oneOf(FILTER_DEFAULT_OPERATORS),
  filterDefaultState: oneOf(FILTER_DEFAULT_STATES),
  gridAlternatingRows: isBool,
  gridEstimateCount: isBool,
  gridEstimateThreshold: intIn(ESTIMATE_COUNT_RANGE.min, ESTIMATE_COUNT_RANGE.max),
  gridAutoNullEmpty: isBool,
  gridRowNumbers: isBool,
  structureColumnOrder: oneOf(STRUCTURE_COLUMN_ORDERS),
  gridExpansionTooltips: isBool,
  gridAutoHideScrollers: isBool,
  gridLoadingWallpaper: isBool,
  geometryToWkt: isBool,
  reopenWorkspaces: isBool,
  startupAction: oneOf(STARTUP_ACTIONS),
  startupProfileId: (v: unknown) => typeof v === "string" && v.length <= 64,
  startupPromptForm: isBool,
  autoSaveQueryTabs: oneOf(AUTO_SAVE_QUERY_TABS),
  keymapOverrides: isKeymapOverrides,
  csvDelimiter: oneOf(CSV_DELIMITER_PREFS),
  csvQuote: oneOf(CSV_QUOTE_PREFS),
  csvLineBreak: oneOf(CSV_LINE_BREAK_PREFS),
  csvDecimal: isCsvDecimal,
  exportRevealPopup: isBool,
  showNotifications: isBool,
  editorCompleteTables: isBool,
  editorCompleteKeywords: isBool,
  editorAddWhitespace: isBool,
  editorPrefixSchemas: isBool,
  editorAcceptKey: oneOf(EDITOR_ACCEPT_KEYS),
  editorIndentWithTabs: isBool,
  editorShowScrollers: isBool,
  editorAutoSaveFiles: isBool,
  newTabOpens: oneOf(NEW_TAB_OPENS),
  historyRetention: oneOf(HISTORY_RETENTIONS),
  queriesAutoSave: isBool,
  safeModeTouchId: isBool,
  passcodeTouchId: isBool,
  autoLockMinutes: oneOf(AUTO_LOCK_MINUTES),
  copyUrlPasswordConfirmed: isBool,
  toolbarItems: isToolbarItems,
  toolbarDisplay: oneOf(TOOLBAR_DISPLAYS),
  mcpEnabled: isBool,
  mcpAllowWrites: isBool,
  mcpExposeProduction: isBool,
  assistantEnabled: isBool,
  assistantProvider: oneOf(ASSISTANT_PROVIDERS),
  assistantAnthropicModel: oneOf(ANTHROPIC_MODEL_IDS),
  assistantOpenaiBaseUrl: isBaseUrl,
  assistantOpenaiModel: isModelName,
  assistantOllamaBaseUrl: isBaseUrl,
  assistantOllamaModel: isModelName,
  sidebarLazyLoad: isBool,
  pluginStates: isPluginStates,
  editorFontFamily: isFontFamily,
  editorLineHeight: isNumberIn(EDITOR_LINE_HEIGHT_RANGE.min, EDITOR_LINE_HEIGHT_RANGE.max),
  gridFontFamily: isFontFamily,
  gridFontSize: intIn(GRID_FONT_SIZE_RANGE.min, GRID_FONT_SIZE_RANGE.max),
  gridLineHeight: isNumberIn(GRID_LINE_HEIGHT_RANGE.min, GRID_LINE_HEIGHT_RANGE.max),
  gridRowPadding: intIn(GRID_ROW_PADDING_RANGE.min, GRID_ROW_PADDING_RANGE.max),
  sidebarFontSize: intIn(SIDEBAR_FONT_SIZE_RANGE.min, SIDEBAR_FONT_SIZE_RANGE.max),
  sidebarFontFamily: isFontFamily,
  colorOverrides: isColorOverrides,
  gridNoElastic: isBool,
  gridAutoExpandText: isBool,
};

function assign<K extends keyof Prefs>(target: Prefs, key: K, value: unknown): void {
  target[key] = value as Prefs[K];
}

/**
 * Overlays the valid known keys of `raw` onto `base`. Unknown keys are dropped and invalid values
 * (wrong type, out of range) keep the `base` value, so stale or hand-edited data never breaks the UI.
 */
export function sanitizePrefs(raw: unknown, base: Readonly<Prefs> = DEFAULT_PREFS): Prefs {
  const out: Prefs = { ...base };
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return out;
  const src = raw as Record<string, unknown>;
  for (const key of PREF_KEYS) {
    if (Object.hasOwn(src, key) && VALID[key](src[key])) assign(out, key, src[key]);
  }
  return out;
}

export function samePrefs(a: Readonly<Prefs>, b: Readonly<Prefs>): boolean {
  return PREF_KEYS.every((k) => a[k] === b[k]);
}

function readLocal(): unknown {
  try {
    const raw = globalThis.localStorage?.getItem(PREFS_LOCAL_KEY);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function writeLocal(prefs: Prefs): void {
  try {
    globalThis.localStorage?.setItem(PREFS_LOCAL_KEY, JSON.stringify(prefs));
  } catch {
    // Storage unavailable or full: the app DB copy is the source of truth anyway.
  }
}

// --- persistence --------------------------------------------------------------

let saveTimer: ReturnType<typeof setTimeout> | null = null;
let loadPromise: Promise<void> | null = null;
let editedBeforeLoad = false;
/** The preferences as last saved to (or read from) the app DB: a save tells the others the rest. */
let lastSaved: Prefs | null = null;

/** The keys whose values differ between `before` and `after`, with their `after` values. */
export function changedPrefs(before: Readonly<Prefs>, after: Readonly<Prefs>): Partial<Prefs> {
  const out: Record<string, unknown> = {};
  for (const key of PREF_KEYS) if (before[key] !== after[key]) out[key] = after[key];
  return out as Partial<Prefs>;
}

async function saveNow(): Promise<void> {
  const prefs = usePrefsStore.getState().prefs;
  try {
    await ipc.settings.set(PREFS_KEY, prefs);
  } catch (err) {
    console.warn("[prefs] save failed", err);
    toast.error("Could not save preferences", {
      id: "prefs-save",
      description: err instanceof Error ? err.message : String(err),
    });
    return;
  }
  const changed = changedPrefs(lastSaved ?? DEFAULT_PREFS, prefs);
  lastSaved = prefs;
  // The other windows apply the change now (`applyPrefsFromOtherWindow`), not at their restart.
  if (Object.keys(changed).length > 0)
    void broadcastEvent("prefs:changed", { from: RENDERER_ID, changed }).catch((err: unknown) =>
      console.warn("[prefs] could not tell the other windows", err),
    );
}

/**
 * Another window saved preferences (`prefs:changed`): its changed keys are applied here without
 * saving them again. Edits of this window that are still waiting to be saved keep their keys.
 */
export function applyPrefsFromOtherWindow(changed: unknown): void {
  const current = usePrefsStore.getState().prefs;
  const next = sanitizePrefs(changed, current);
  lastSaved = sanitizePrefs(changed, lastSaved ?? DEFAULT_PREFS);
  if (samePrefs(next, current)) return;
  usePrefsStore.setState({ prefs: next });
  writeLocal(next);
}

function scheduleSave(): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void saveNow();
  }, SAVE_DEBOUNCE_MS);
}

/** Writes a pending (debounced) save right away, e.g. when the window is going away. */
export function flushPrefs(): Promise<void> {
  if (saveTimer === null) return Promise.resolve();
  clearTimeout(saveTimer);
  saveTimer = null;
  return saveNow();
}

/**
 * Reads the app DB copy once per session (later calls return the same promise). Edits made
 * before it resolves win over the stored copy; a localStorage-only copy (first run, or the
 * in-memory mock backend) is pushed to the app DB.
 */
export function loadPrefs(): Promise<void> {
  loadPromise ??= (async () => {
    let stored: unknown;
    try {
      stored = (await ipc.settings.get())[PREFS_KEY];
    } catch (err) {
      console.warn("[prefs] load failed; using local copy", err);
      usePrefsStore.setState({ loaded: true });
      return;
    }
    const current = usePrefsStore.getState().prefs;
    if (editedBeforeLoad || stored === undefined || stored === null) {
      usePrefsStore.setState({ loaded: true });
      if (!editedBeforeLoad && !samePrefs(current, DEFAULT_PREFS)) scheduleSave();
      return;
    }
    const prefs = sanitizePrefs(stored);
    lastSaved = prefs;
    usePrefsStore.setState({ prefs: samePrefs(prefs, current) ? current : prefs, loaded: true });
    writeLocal(prefs);
  })();
  return loadPromise;
}

// --- store --------------------------------------------------------------------

interface PrefsState {
  prefs: Prefs;
  /** True once the app DB copy has been read (or failed to load). */
  loaded: boolean;
  setPref: <K extends keyof Prefs>(key: K, value: Prefs[K]) => void;
  /** Applies the valid keys of `patch`; invalid values are ignored. */
  patchPrefs: (patch: Partial<Prefs>) => void;
}

export const usePrefsStore = create<PrefsState>()((set, get) => ({
  prefs: sanitizePrefs(readLocal()),
  loaded: false,
  setPref: (key, value) => get().patchPrefs({ [key]: value } as Partial<Prefs>),
  patchPrefs: (patch) => {
    const current = get().prefs;
    const next = sanitizePrefs(patch, current);
    if (samePrefs(next, current)) return;
    if (!get().loaded) editedBeforeLoad = true;
    set({ prefs: next });
    writeLocal(next);
    scheduleSave();
  },
}));

/** Subscribes to a slice of the preferences, e.g. `usePrefs((p) => p.editorFontSize)`. */
export function usePrefs<T>(selector: (prefs: Prefs) => T): T {
  return usePrefsStore((s) => selector(s.prefs));
}

/** Current preferences outside React (event handlers, stores). */
export function getPrefs(): Readonly<Prefs> {
  return usePrefsStore.getState().prefs;
}

export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]): void {
  usePrefsStore.getState().setPref(key, value);
}

/** Test-only: back to a fresh, unloaded store with no pending save. */
export function resetPrefsForTests(prefs: Readonly<Prefs> = DEFAULT_PREFS): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = null;
  loadPromise = null;
  editedBeforeLoad = false;
  lastSaved = null;
  usePrefsStore.setState({ prefs: { ...prefs }, loaded: false });
}
