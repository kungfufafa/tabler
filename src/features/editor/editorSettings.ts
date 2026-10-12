import { EDITOR_FONT_SIZE_RANGE, setPref, type QueryParamSyntax } from "@/stores/prefs";

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
