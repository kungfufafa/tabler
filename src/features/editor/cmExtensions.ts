import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import {
  addCursorAbove,
  addCursorBelow,
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  toggleBlockComment,
  toggleBlockCommentByLine,
} from "@codemirror/commands";
import { MySQL, PostgreSQL, SQLite, sql, type SQLNamespace } from "@codemirror/lang-sql";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from "@codemirror/language";
import { lintGutter, lintKeymap } from "@codemirror/lint";
import {
  highlightSelectionMatches,
  search,
  searchKeymap,
  selectSelectionMatches,
} from "@codemirror/search";
import {
  Compartment,
  EditorSelection,
  EditorState,
  Facet,
  Prec,
  RangeSetBuilder,
  type Extension,
} from "@codemirror/state";
import {
  crosshairCursor,
  Decoration,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  highlightWhitespace,
  keymap,
  lineNumbers,
  rectangularSelection,
  ViewPlugin,
  WidgetType,
  type Command,
  type DecorationSet,
  type KeyBinding,
  type ViewUpdate,
} from "@codemirror/view";
import { actions, type ActionId } from "@/features/palette/actions";
import { savedQueryCompletionExtension } from "@/features/queries/completion";
import { KEYMAP, type ShortcutDef } from "@/lib/keys/keymap";
import { runShortcut } from "@/lib/keys/useShortcuts";
import type { Prefs } from "@/stores/prefs";
import { editorTheme, sqlHighlightStyle } from "./cmTheme";
import { splitStatements, statementAt, type SqlDialect, type StatementRange } from "./scanner";

/** The scanner dialect of the editor (provided by the language compartment). */
export const dialectFacet = Facet.define<SqlDialect, SqlDialect>({
  combine: (values) => values[0] ?? "postgres",
});

/** SQL language + schema completion; reconfigured when the driver or the schema changes. */
export const languageConf = new Compartment();
/**
 * The editor preferences: font size, tab size, line wrapping, the current-statement highlight,
 * invisible characters, closing brackets and completion while typing.
 */
export const prefsConf = new Compartment();

const CM_DIALECTS = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite } as const;

export interface LanguageConfig {
  dialect: SqlDialect;
  schema?: SQLNamespace;
  defaultSchema?: string;
  upperCaseKeywords?: boolean;
  /** Connection profile of the tab: its saved queries' keywords complete (keyword binding). */
  profileId?: string | null;
}

export function languageExtension(cfg: LanguageConfig): Extension {
  return [
    dialectFacet.of(cfg.dialect),
    sql({
      dialect: CM_DIALECTS[cfg.dialect],
      schema: cfg.schema,
      defaultSchema: cfg.defaultSchema,
      upperCaseKeywords: cfg.upperCaseKeywords ?? true,
    }),
    savedQueryCompletionExtension(cfg.profileId ?? null),
  ];
}

export interface EditorPrefs {
  fontSize: number;
  tabSize: number;
  lineWrap: boolean;
  /** Tint the lines of the statement under the cursor (default on). */
  highlightCurrentQuery?: boolean;
  /** Show spaces, tabs and line ends. */
  showInvisibles?: boolean;
  /** Typing an opening bracket or quote inserts the closing one (default on). */
  closeBrackets?: boolean;
  /** The completion list opens while typing (default on); ⌃Space always asks. */
  suggestions?: boolean;
}

/** The editor's slice of the preferences. */
export function editorPrefsOf(p: Readonly<Prefs>): EditorPrefs {
  return {
    fontSize: p.editorFontSize,
    tabSize: p.editorTabSize,
    lineWrap: p.editorLineWrap,
    highlightCurrentQuery: p.editorHighlightCurrentQuery,
    showInvisibles: p.editorShowInvisibles,
    closeBrackets: p.editorCloseBrackets,
    suggestions: !p.editorDisableSuggestions,
  };
}

const fontThemes = new Map<number, Extension>();
function fontTheme(px: number): Extension {
  let theme = fontThemes.get(px);
  if (!theme) {
    theme = EditorView.theme({ "&": { fontSize: `${px}px` } });
    fontThemes.set(px, theme);
  }
  return theme;
}

export function prefsExtension(p: EditorPrefs): Extension {
  return [
    EditorState.tabSize.of(p.tabSize),
    indentUnit.of(" ".repeat(p.tabSize)),
    p.lineWrap ? EditorView.lineWrapping : [],
    fontTheme(p.fontSize),
    p.highlightCurrentQuery === false ? [] : statementHighlighter,
    p.showInvisibles ? invisibles : [],
    p.closeBrackets === false ? [] : [closeBrackets(), keymap.of(closeBracketsKeymap)],
    autocompletion({
      activateOnTyping: p.suggestions !== false,
      activateOnTypingDelay: 80,
      maxRenderedOptions: 80,
    }),
  ];
}

// --- invisible characters ----------------------------------------------------

class LineEndMarker extends WidgetType {
  eq(): boolean {
    return true;
  }
  toDOM(): HTMLElement {
    const el = document.createElement("span");
    el.className = "cm-eol-marker";
    el.setAttribute("aria-hidden", "true");
    el.textContent = "¬";
    return el;
  }
  ignoreEvent(): boolean {
    return false;
  }
}

const lineEnd = Decoration.widget({ widget: new LineEndMarker(), side: 1 });

/** A `¬` at the end of every visible line but the last one. */
const lineEnds = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = this.build(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) this.decorations = this.build(u.view);
    }
    build(view: EditorView): DecorationSet {
      const builder = new RangeSetBuilder<Decoration>();
      const doc = view.state.doc;
      for (const { from, to } of view.visibleRanges) {
        for (let pos = from; pos <= to;) {
          const line = doc.lineAt(pos);
          if (line.number < doc.lines) builder.add(line.to, line.to, lineEnd);
          pos = line.to + 1;
        }
      }
      return builder.finish();
    }
  },
  { decorations: (v) => v.decorations },
);

/** "Show Invisible Characters": dots for spaces, arrows for tabs, `¬` for line ends. */
const invisibles: Extension = [
  highlightWhitespace(),
  lineEnds,
  EditorView.theme({
    ".cm-eol-marker": { color: "var(--muted-foreground)", opacity: "0.45", pointerEvents: "none" },
    ".cm-highlightSpace, .cm-highlightTab": { color: "var(--muted-foreground)" },
  }),
];

// --- current statement highlight ----------------------------------------------

/** Above this size the doc is not re-scanned on every keystroke. */
const MAX_SCAN = 1_000_000;
const statementLine = Decoration.line({ class: "cm-current-statement" });

class StatementHighlighter {
  decorations: DecorationSet = Decoration.none;
  private statements: StatementRange[] = [];
  private text = "";
  private dialect: SqlDialect;

  constructor(view: EditorView) {
    this.dialect = view.state.facet(dialectFacet);
    this.rescan(view.state);
    this.decorations = this.build(view);
  }

  update(u: ViewUpdate) {
    const dialect = u.state.facet(dialectFacet);
    const rescan = u.docChanged || dialect !== this.dialect;
    if (rescan) {
      this.dialect = dialect;
      this.rescan(u.state);
    }
    if (rescan || u.selectionSet || u.viewportChanged) this.decorations = this.build(u.view);
  }

  private rescan(state: EditorState) {
    if (state.doc.length > MAX_SCAN) {
      this.text = "";
      this.statements = [];
      return;
    }
    this.text = state.doc.toString();
    this.statements = splitStatements(this.text, this.dialect);
  }

  private build(view: EditorView): DecorationSet {
    const sel = view.state.selection.main;
    if (!sel.empty || this.statements.length === 0) return Decoration.none;
    const st = statementAt(this.statements, this.text, sel.head);
    if (!st) return Decoration.none;
    const doc = view.state.doc;
    const first = doc.lineAt(st.start).number;
    const last = doc.lineAt(st.end).number;
    const builder = new RangeSetBuilder<Decoration>();
    let done = 0;
    for (const { from, to } of view.visibleRanges) {
      const start = Math.max(first, doc.lineAt(from).number, done + 1);
      const end = Math.min(last, doc.lineAt(to).number);
      for (let n = start; n <= end; n++) {
        const line = doc.line(n);
        builder.add(line.from, line.from, statementLine);
        done = n;
      }
    }
    return builder.finish();
  }
}

const statementHighlighter = ViewPlugin.fromClass(StatementHighlighter, {
  decorations: (v) => v.decorations,
});

// --- shortcuts -----------------------------------------------------------------

const EDITOR_ACTIONS: ReadonlySet<ActionId> = new Set<ActionId>([
  "query.run",
  "query.run-current",
  "query.cancel",
  "query.explain",
  "query.format",
  "query.minify",
  "query.toggle-results",
  "query.history",
  "query.save",
  "query.open",
  "query.add-to-queries",
  "query.save-selection",
  "query.font-bigger",
  "query.font-smaller",
  "query.split",
]);

/** "Mod+Shift+Enter" → "Mod-Shift-Enter", "Alt+Shift+F" → "Alt-Shift-f". */
export function toCmKey(combo: string): string {
  const parts = combo.split("+");
  const key = parts.pop() ?? "";
  return [...parts, key.length === 1 ? key.toLowerCase() : key].join("-");
}

/**
 * Runs `def`'s query action or, when that has nothing to do (⌘. Cancel while nothing runs), the
 * app action allowed in the editor on the same key (⌘. Process List).
 */
function runEditorBinding(def: ShortcutDef): void {
  if (actions.has(def.id)) return runShortcut(def.id);
  const fallback = KEYMAP.find(
    (d) =>
      d.keys === def.keys && d.scope === "global" && d.allowInEditor === true && actions.has(d.id),
  );
  if (fallback) runShortcut(fallback.id);
}

/**
 * The query shortcuts, derived from KEYMAP (editor bindings, and global ones allowed in the
 * editor). Normally the app's capture-phase listener handles them first (and stops the event), so
 * these only fire when it did not, e.g. no handler registered or the editor used outside the
 * KeyboardProvider; they never double-dispatch. They also win over CodeMirror's own keymaps (⌘I
 * is Beautify here, not "select parent syntax"; ⌘↩ runs instead of inserting a blank line). They
 * live in `actionKeymapConf`: editorStates.ts rebuilds them in every editor when the user remaps a
 * shortcut (Settings ▸ Shortcuts).
 */
export function actionKeymap(): Extension {
  const bindings: KeyBinding[] = KEYMAP.filter(
    (d) =>
      EDITOR_ACTIONS.has(d.id) &&
      (d.scope === "editor" || (d.scope === "global" && d.allowInEditor === true)),
  ).map((d) => ({
    key: toCmKey(d.keys),
    preventDefault: true,
    run: () => {
      runEditorBinding(d);
      return true;
    },
  }));
  // ⌘R would reload the webview and throw the buffer away.
  bindings.push({ key: "Mod-r", preventDefault: true, run: () => true });
  return Prec.highest(keymap.of(bindings));
}

/** Holds `actionKeymap()`; reconfigured when the keymap changes. */
export const actionKeymapConf = new Compartment();

/**
 * ⌃⌘G (TablePlus): selects every occurrence of the selection, or of the word under the caret
 * when nothing is selected.
 */
export const selectAllOccurrences: Command = (view) => {
  const { state } = view;
  const main = state.selection.main;
  if (main.empty) {
    const word = state.wordAt(main.head);
    if (!word) return false;
    view.dispatch({ selection: EditorSelection.single(word.from, word.to) });
  } else if (state.selection.ranges.length > 1) {
    view.dispatch({ selection: EditorSelection.create([main]) });
  }
  return selectSelectionMatches(view);
};

/**
 * ⌥⌘/: wraps the selection in a block comment (or removes it), or the current line when nothing
 * is selected.
 */
export const toggleBlockCommentSmart: Command = (view) =>
  view.state.selection.ranges.every((r) => r.empty)
    ? toggleBlockCommentByLine(view)
    : toggleBlockComment(view);

/**
 * TablePlus's multi-caret and comment keys on top of CodeMirror's defaults (which already give
 * ⌘-click to add a caret, ⌘D for the next occurrence, Esc back to one caret and ⌘/ for a line
 * comment): ⌃⇧↑ / ⌃⇧↓ add a caret above / below, ⌃⌘G selects every occurrence and ⌥⌘/ toggles
 * a block comment.
 */
export const multiCaretKeymap: readonly KeyBinding[] = [
  { key: "Ctrl-Shift-ArrowUp", run: addCursorAbove, preventDefault: true },
  { key: "Ctrl-Shift-ArrowDown", run: addCursorBelow, preventDefault: true },
  { key: "Ctrl-Alt-g", mac: "Ctrl-Mod-g", run: selectAllOccurrences, preventDefault: true },
  { key: "Mod-Alt-/", run: toggleBlockCommentSmart, preventDefault: true },
];

export interface EditorSetup {
  language: LanguageConfig;
  prefs: EditorPrefs;
  onUpdate: (update: ViewUpdate) => void;
}

export function editorExtensions(setup: EditorSetup): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    foldGutter(),
    lintGutter(),
    highlightSpecialChars(),
    history(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    bracketMatching(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    syntaxHighlighting(sqlHighlightStyle),
    editorTheme,
    languageConf.of(languageExtension(setup.language)),
    prefsConf.of(prefsExtension(setup.prefs)),
    EditorView.updateListener.of(setup.onUpdate),
    EditorView.contentAttributes.of({ "aria-label": "SQL editor", spellcheck: "false" }),
    actionKeymapConf.of(actionKeymap()),
    keymap.of([
      ...multiCaretKeymap,
      ...defaultKeymap,
      ...searchKeymap,
      ...historyKeymap,
      ...foldKeymap,
      ...completionKeymap,
      ...lintKeymap,
      indentWithTab,
    ]),
  ];
}
