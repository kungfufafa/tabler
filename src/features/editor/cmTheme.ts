import { HighlightStyle } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

/**
 * Editor chrome and syntax colours. Everything reads CSS variables from `index.css`, so the same
 * theme follows light / dark mode and the connection accent without reconfiguring the editor.
 */
export const editorTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--surface)",
    color: "var(--foreground)",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.6",
    overscrollBehavior: "contain",
  },
  ".cm-content": { caretColor: "var(--brand)", padding: "6px 0" },
  ".cm-line": { padding: "0 10px 0 6px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--brand)", borderLeftWidth: "2px" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection":
    { backgroundColor: "color-mix(in oklab, var(--brand) 24%, transparent)" },
  ".cm-selectionBackground": {
    backgroundColor: "color-mix(in oklab, var(--brand) 14%, transparent)",
  },
  ".cm-gutters": {
    backgroundColor: "var(--surface)",
    color: "var(--muted-foreground)",
    border: "none",
    borderRight: "1px solid var(--gridline)",
    fontSize: "0.92em",
  },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 6px 0 10px", minWidth: "32px" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--foreground)" },
  ".cm-activeLine": { backgroundColor: "color-mix(in oklab, var(--foreground) 3.5%, transparent)" },
  ".cm-current-statement": { backgroundColor: "var(--editor-statement-bg)" },
  ".cm-activeLine.cm-current-statement": {
    backgroundColor: "color-mix(in oklab, var(--brand) 10%, transparent)",
  },
  ".cm-foldGutter .cm-gutterElement": { padding: "0 4px", cursor: "pointer" },
  ".cm-foldPlaceholder": {
    backgroundColor: "var(--surface-2)",
    border: "1px solid var(--border)",
    color: "var(--muted-foreground)",
    borderRadius: "3px",
    padding: "0 4px",
  },
  ".cm-selectionMatch": { backgroundColor: "var(--brand-soft)" },
  ".cm-searchMatch": {
    backgroundColor: "color-mix(in oklab, var(--warning) 30%, transparent)",
    outline: "1px solid color-mix(in oklab, var(--warning) 60%, transparent)",
  },
  ".cm-searchMatch.cm-searchMatch-selected": {
    backgroundColor: "color-mix(in oklab, var(--warning) 55%, transparent)",
  },
  "&.cm-focused .cm-matchingBracket": {
    backgroundColor: "var(--brand-soft)",
    outline: "1px solid color-mix(in oklab, var(--brand) 50%, transparent)",
  },
  "&.cm-focused .cm-nonmatchingBracket": {
    backgroundColor: "color-mix(in oklab, var(--error) 20%, transparent)",
  },
  ".cm-panels": {
    backgroundColor: "var(--surface-2)",
    color: "var(--foreground)",
    fontFamily: "var(--font-sans)",
    fontSize: "12px",
  },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--border)" },
  ".cm-search": {
    padding: "4px 8px",
    display: "flex",
    flexWrap: "wrap",
    alignItems: "center",
    gap: "4px",
  },
  ".cm-search br": { flexBasis: "100%", height: 0, content: '""' },
  ".cm-search label": {
    display: "inline-flex",
    alignItems: "center",
    gap: "3px",
    fontSize: "11px",
  },
  ".cm-textfield": {
    backgroundColor: "var(--surface)",
    color: "var(--foreground)",
    border: "1px solid var(--border)",
    borderRadius: "4px",
    padding: "2px 6px",
    fontSize: "12px",
    outline: "none",
  },
  ".cm-textfield:focus": { borderColor: "var(--brand)" },
  ".cm-button": {
    backgroundImage: "none",
    backgroundColor: "var(--surface)",
    color: "var(--foreground)",
    border: "1px solid var(--border)",
    borderRadius: "4px",
    padding: "1px 8px",
    fontSize: "11px",
  },
  ".cm-button:active": { backgroundImage: "none", backgroundColor: "var(--accent)" },
  ".cm-panel.cm-search [name=close]": { color: "var(--muted-foreground)", fontSize: "16px" },
  ".cm-tooltip": {
    backgroundColor: "var(--popover)",
    color: "var(--popover-foreground)",
    border: "1px solid var(--border)",
    borderRadius: "6px",
    boxShadow: "0 6px 24px rgb(0 0 0 / 0.14)",
    overflow: "hidden",
  },
  ".cm-tooltip-autocomplete > ul": {
    fontFamily: "var(--font-mono)",
    fontSize: "12px",
    maxHeight: "16em",
  },
  ".cm-tooltip-autocomplete > ul > li": { padding: "2px 8px 2px 4px", lineHeight: "1.5" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
    backgroundColor: "var(--brand)",
    color: "var(--primary-foreground)",
  },
  ".cm-completionDetail": {
    color: "var(--muted-foreground)",
    fontStyle: "normal",
    marginLeft: "8px",
  },
  ".cm-tooltip-autocomplete > ul > li[aria-selected] .cm-completionDetail": {
    color: "inherit",
    opacity: 0.8,
  },
  ".cm-completionMatchedText": { textDecoration: "none", fontWeight: "600" },
  ".cm-completionIcon": { opacity: 0.6, width: "1.1em" },
  ".cm-tooltip-lint": { fontFamily: "var(--font-sans)", fontSize: "12px" },
  ".cm-diagnostic": { padding: "4px 8px", whiteSpace: "pre-wrap" },
  ".cm-diagnostic-error": { borderLeft: "3px solid var(--error)" },
  ".cm-diagnosticSource": { color: "var(--muted-foreground)", fontSize: "11px" },
  ".cm-lintRange-error": {
    backgroundImage: "none",
    textDecoration: "underline wavy var(--error)",
    textDecorationSkipInk: "none",
    textUnderlineOffset: "3px",
  },
  ".cm-gutter-lint": { width: "12px" },
  ".cm-gutter-lint .cm-gutterElement": { padding: "0 2px" },
});

export const sqlHighlightStyle = HighlightStyle.define([
  {
    tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword],
    color: "var(--syn-keyword)",
  },
  { tag: [t.typeName, t.standard(t.name)], color: "var(--syn-keyword)", fontStyle: "italic" },
  { tag: [t.string, t.special(t.string), t.character], color: "var(--syn-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--syn-number)" },
  {
    tag: [t.comment, t.lineComment, t.blockComment],
    color: "var(--syn-comment)",
    fontStyle: "italic",
  },
  { tag: [t.operator, t.compareOperator, t.arithmeticOperator], color: "var(--syn-operator)" },
  { tag: [t.special(t.name), t.quote], color: "var(--syn-ident)" },
  { tag: [t.punctuation, t.paren, t.bracket, t.separator], color: "var(--muted-foreground)" },
  { tag: [t.variableName, t.labelName], color: "var(--syn-ident)" },
  { tag: t.invalid, color: "var(--error)" },
]);
