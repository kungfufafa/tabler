import {
  EDITOR_FONT_SIZE_RANGE,
  setPref,
  type AutoSaveQueryTabs,
  type EditorAcceptKey,
  type HistoryRetention,
  type NewTabOpens,
  type QueryParamSyntax,
  type ResultViewMode,
} from "@/stores/prefs";

/** Font sizes the ⚙ menu offers. */
export const EDITOR_FONT_SIZES: readonly number[] = Array.from(
  { length: EDITOR_FONT_SIZE_RANGE.max - EDITOR_FONT_SIZE_RANGE.min + 1 },
  (_, i) => EDITOR_FONT_SIZE_RANGE.min + i,
);

/** The editor font size one step up (`delta` 1) or down (-1), within the allowed range. */
export function stepFontSize(size: number, delta: 1 | -1): number {
  return Math.max(EDITOR_FONT_SIZE_RANGE.min, Math.min(EDITOR_FONT_SIZE_RANGE.max, size + delta));
}

/** ⌘+ / ⌘−: the SQL editor font, for every tab (it is a preference). */
export function changeEditorFontSize(current: number, delta: 1 | -1): void {
  setPref("editorFontSize", stepFontSize(current, delta));
}

/** How each query params syntax is described in the menu (an example of the variable). */
export const QUERY_PARAM_EXAMPLES: Record<QueryParamSyntax, string> = {
  ":[\\w.]+": ":name",
  "%[\\w.]+%": "%name%",
  "\\?": "?",
  "\\$\\{[\\w.]+\\}": "${name}",
  "\\$[\\w.]+": "$name",
  "\\{\\{[\\w.]+\\}\\}": "{{name}}",
};

/** ⚙ ▸ Autocomplete Options ▸ Auto-complete Key. */
export const ACCEPT_KEY_LABELS: Record<EditorAcceptKey, string> = {
  both: "Enter or Tab",
  enter: "Enter",
  tab: "Tab",
};

/** ⚙ ▸ Auto Save SQL Query Tab (TablePlus's wording). */
export const AUTO_SAVE_TAB_LABELS: Record<AutoSaveQueryTabs, string> = {
  none: "Do Not Save the SQL Query (Always Open an Empty SQL Query Tab)",
  first: "Only Save the First SQL Query Tab",
  all: "Save All SQL Query Tabs",
};

/** ⚙ ▸ Result View Mode (TablePlus: one tab per result, up to 100, or one tab for all). */
export const RESULT_VIEW_LABELS: Record<ResultViewMode, string> = {
  tabs: "Split Results into Tabs (Max 100)",
  append: "Append All Results in One Tab",
};

/** General ▸ "Command + T should open". */
export const NEW_TAB_LABELS: Record<NewTabOpens, string> = {
  query: "SQL Query Editor tab",
  empty: "Empty tab",
};

/** "Keep history for". */
export const HISTORY_RETENTION_LABELS: Record<HistoryRetention, string> = {
  "1y": "1 year",
  "2y": "2 years",
  "3y": "3 years",
  forever: "Forever",
};
