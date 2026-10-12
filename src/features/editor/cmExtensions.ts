import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  completionKeymap,
} from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
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
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import {
  Compartment,
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
  keymap,
  lineNumbers,
  rectangularSelection,
  ViewPlugin,
  type DecorationSet,
  type KeyBinding,
  type ViewUpdate,
} from "@codemirror/view";
import { actions, type ActionId } from "@/features/palette/actions";
import { KEYMAP } from "@/lib/keys/keymap";
import { editorTheme, sqlHighlightStyle } from "./cmTheme";
import { splitStatements, statementAt, type SqlDialect, type StatementRange } from "./scanner";

/** The scanner dialect of the editor (provided by the language compartment). */
export const dialectFacet = Facet.define<SqlDialect, SqlDialect>({
  combine: (values) => values[0] ?? "postgres",
});

/** SQL language + schema completion; reconfigured when the driver or the schema changes. */
export const languageConf = new Compartment();
/** Font size, tab size and line wrapping from the preferences. */
export const prefsConf = new Compartment();

const CM_DIALECTS = { postgres: PostgreSQL, mysql: MySQL, sqlite: SQLite } as const;

export interface LanguageConfig {
  dialect: SqlDialect;
  schema?: SQLNamespace;
  defaultSchema?: string;
  upperCaseKeywords?: boolean;
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
  ];
}

export interface EditorPrefs {
  fontSize: number;
  tabSize: number;
  lineWrap: boolean;
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
  ];
}

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
]);

/** "Mod+Shift+Enter" → "Mod-Shift-Enter", "Alt+Shift+F" → "Alt-Shift-f". */
export function toCmKey(combo: string): string {
  const parts = combo.split("+");
  const key = parts.pop() ?? "";
  return [...parts, key.length === 1 ? key.toLowerCase() : key].join("-");
}

/**
 * The query shortcuts, derived from KEYMAP (editor bindings, and global ones allowed in the
 * editor). Normally the app's capture-phase listener handles them first (and stops the event), so
 * these only fire when it did not, e.g. no handler registered yet or the editor used outside the
 * KeyboardProvider; they never double-dispatch. They also win over CodeMirror's own keymaps (⌘I
 * is Beautify here, not "select parent syntax"; ⌘↩ runs instead of inserting a blank line).
 */
function actionKeymap(): Extension {
  const bindings: KeyBinding[] = KEYMAP.filter(
    (d) =>
      EDITOR_ACTIONS.has(d.id) &&
      (d.scope === "editor" || (d.scope === "global" && d.allowInEditor === true)),
  ).map((d) => ({
    key: toCmKey(d.keys),
    preventDefault: true,
    run: () => {
      if (!actions.has(d.id)) return false;
      void actions.run(d.id);
      return true;
    },
  }));
  // ⌘R would reload the webview and throw the buffer away.
  bindings.push({ key: "Mod-r", preventDefault: true, run: () => true });
  return Prec.highest(keymap.of(bindings));
}

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
    closeBrackets(),
    autocompletion({ activateOnTypingDelay: 80, maxRenderedOptions: 80 }),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    search({ top: true }),
    syntaxHighlighting(sqlHighlightStyle),
    editorTheme,
    languageConf.of(languageExtension(setup.language)),
    prefsConf.of(prefsExtension(setup.prefs)),
    statementHighlighter,
    EditorView.updateListener.of(setup.onUpdate),
    EditorView.contentAttributes.of({ "aria-label": "SQL editor", spellcheck: "false" }),
    actionKeymap(),
    keymap.of([
      ...closeBracketsKeymap,
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
