import {
  autocompletion,
  closeBrackets,
  closeBracketsKeymap,
  type CompletionSource,
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
  toggleComment,
  toggleLineComment,
} from "@codemirror/commands";
import { MySQL, PostgreSQL, SQLite, type SQLNamespace } from "@codemirror/lang-sql";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  LanguageSupport,
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
import type { TableMeta, TableRef } from "@/lib/ipc";
import type { EditorAcceptKey, Prefs } from "@/stores/prefs";
import { sqlEditorTheme } from "./cmTheme";
import type { LaneLanguage } from "./language";
import { splitStatements, statementAt, type SqlDialect, type StatementRange } from "./scanner";
import {
  allFieldsSource,
  autoWhitespace,
  completionKeys,
  functionCompletionSource,
  joinCompletionSource,
  keywordSource,
  schemaSource,
  type RoutineName,
} from "./sqlCompletion";
import type { TableColumns } from "./statementTables";

/** The scanner dialect of the editor (provided by the language compartment). */
export const dialectFacet = Facet.define<SqlDialect, SqlDialect>({
  combine: (values) => values[0] ?? "postgres",
});

/** A lane's splitter, which replaces the core scanner's (provided by the language compartment). */
export const laneSplitFacet = Facet.define<
  LaneLanguage["split"] | null,
  LaneLanguage["split"] | null
>({
  combine: (values) => values[0] ?? null,
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
  /** The core dialect (ignored when `lane` is set). */
  dialect: SqlDialect;
  /**
   * A lane's language instead of a core dialect: its highlighting and splitter; completion is
   * the saved queries' keywords only.
   */
  lane?: LaneLanguage;
  schema?: SQLNamespace;
  defaultSchema?: string;
  upperCaseKeywords?: boolean;
  /** Connection profile of the tab: its saved queries' keywords complete (keyword binding). */
  profileId?: string | null;
  /** schema → table → columns (the completion metadata): `users.*`, JOIN … ON, prefixes. */
  tables?: TableColumns;
  /** The default schema's functions and procedures. */
  routines?: readonly RoutineName[];
  /** Loads a table's metadata (its foreign keys) for JOIN … ON suggestions. */
  tableMeta?: (table: TableRef) => Promise<TableMeta | null>;
  /** ⚙ ▸ Autocomplete Options ▸ Tables (schemas, tables, columns, JOIN, `users.*`). */
  completeTables?: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Keywords. */
  completeKeywords?: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Automatically Prefix Schemas. */
  prefixSchemas?: boolean;
  /** ⚙ ▸ Autocomplete Options ▸ Automatically Add a Whitespace. */
  addWhitespace?: boolean;
  /** Functions complete with their closing bracket (Insert Closing Braces and Quotes). */
  closeBrackets?: boolean;
}

/** The completion preferences a language configuration takes from `prefs`. */
export function languagePrefsOf(
  p: Readonly<Prefs>,
): Pick<
  LanguageConfig,
  | "upperCaseKeywords"
  | "completeTables"
  | "completeKeywords"
  | "prefixSchemas"
  | "addWhitespace"
  | "closeBrackets"
> {
  return {
    upperCaseKeywords: p.editorUppercaseKeywords,
    completeTables: p.editorCompleteTables,
    completeKeywords: p.editorCompleteKeywords,
    prefixSchemas: p.editorPrefixSchemas,
    addWhitespace: p.editorAddWhitespace,
    closeBrackets: p.editorCloseBrackets,
  };
}

/**
 * The SQL language with its completion sources: schema / table / column names (lang-sql, with
 * the schema prefix when asked for), `users.*` → all fields, JOIN … ON from foreign keys,
 * keywords, functions, saved-query keywords.
 */
export function languageExtension(cfg: LanguageConfig): Extension {
  if (cfg.lane)
    return [
      laneSplitFacet.of(cfg.lane.split),
      cfg.lane.extension(),
      savedQueryCompletionExtension(cfg.profileId ?? null),
    ];
  const d = CM_DIALECTS[cfg.dialect];
  const upperCase = cfg.upperCaseKeywords ?? true;
  const tablesOn = cfg.completeTables !== false;
  const sources: CompletionSource[] = [];
  if (tablesOn && cfg.schema)
    sources.push(
      schemaSource({
        d,
        dialect: cfg.dialect,
        namespace: cfg.schema,
        defaultSchema: cfg.defaultSchema,
        tables: cfg.tables,
        prefixSchemas: cfg.prefixSchemas === true,
      }),
    );
  if (tablesOn && cfg.tables) {
    const tables = { dialect: cfg.dialect, tables: cfg.tables, defaultSchema: cfg.defaultSchema };
    sources.push(allFieldsSource(tables));
    if (cfg.tableMeta)
      sources.push(joinCompletionSource({ ...tables, upperCase, tableMeta: cfg.tableMeta }));
  }
  if (cfg.completeKeywords !== false) sources.push(keywordSource(d, cfg.dialect, upperCase));
  sources.push(
    functionCompletionSource({
      dialect: cfg.dialect,
      upperCase,
      closeBrackets: cfg.closeBrackets !== false,
      routines: cfg.routines,
    }),
  );
  return [
    dialectFacet.of(cfg.dialect),
    new LanguageSupport(
      d.language,
      sources.map((autocomplete) => d.language.data.of({ autocomplete })),
    ),
    cfg.addWhitespace ? autoWhitespace(new Set(Object.keys(cfg.tables ?? {}))) : [],
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
  /** Which key accepts a suggestion (default either). */
  acceptKey?: EditorAcceptKey;
  /** Indent with tabs (`tabSize` wide) instead of spaces. */
  indentWithTabs?: boolean;
  /** The editor's scroll bars (default shown). */
  showScrollers?: boolean;
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
    acceptKey: p.editorAcceptKey,
    indentWithTabs: p.editorIndentWithTabs,
    showScrollers: p.editorShowScrollers,
  };
}

/** ⚙ ▸ Show Scrollers off: the editor still scrolls, without the bars. */
const hiddenScrollers = EditorView.theme({
  ".cm-scroller": { scrollbarWidth: "none" },
  ".cm-scroller::-webkit-scrollbar": { display: "none" },
});

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
    indentUnit.of(p.indentWithTabs ? "\t" : " ".repeat(p.tabSize)),
    p.showScrollers === false ? hiddenScrollers : [],
    p.lineWrap ? EditorView.lineWrapping : [],
    fontTheme(p.fontSize),
    p.highlightCurrentQuery === false ? [] : statementHighlighter,
    p.showInvisibles ? invisibles : [],
    p.closeBrackets === false ? [] : [closeBrackets(), keymap.of(closeBracketsKeymap)],
    autocompletion({
      activateOnTyping: p.suggestions !== false,
      activateOnTypingDelay: 80,
      maxRenderedOptions: 80,
      // The keys come from `completionKeys` (the accept key is a preference).
      defaultKeymap: false,
    }),
    completionKeys(p.acceptKey ?? "both"),
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

/** CodeMirror's own tab arrow, used as a mask so it takes the invisible-characters colour. */
const TAB_ARROW = `url('data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="200" height="20"><path stroke="black" stroke-width="1" fill="none" d="M1 10H196L190 5M190 15L196 10"/></svg>')`;

/** "Show Invisible Characters": dots for spaces, arrows for tabs, `¬` for line ends. */
const invisibles: Extension = [
  highlightWhitespace(),
  lineEnds,
  // The colour is Settings ▸ Appearance ▸ SQL editor ▸ Invisible characters (`--syn-invisible`).
  // CodeMirror draws the dots and tab arrows as grey images; these redraw them in that colour.
  EditorView.theme({
    ".cm-eol-marker": {
      color: "var(--syn-invisible, var(--muted-foreground))",
      opacity: "0.45",
      pointerEvents: "none",
    },
    ".cm-highlightSpace": {
      backgroundImage:
        "radial-gradient(circle at 50% 55%, var(--syn-invisible, var(--muted-foreground)) 20%, transparent 5%)",
    },
    ".cm-highlightTab": {
      backgroundImage: "none",
      backgroundColor: "var(--syn-invisible, var(--muted-foreground))",
      maskImage: TAB_ARROW,
      WebkitMaskImage: TAB_ARROW,
      maskSize: "auto 100%",
      WebkitMaskSize: "auto 100%",
      maskPosition: "right 90%",
      WebkitMaskPosition: "right 90%",
      maskRepeat: "no-repeat",
      WebkitMaskRepeat: "no-repeat",
    },
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
  private lane: LaneLanguage["split"] | null;

  constructor(view: EditorView) {
    this.dialect = view.state.facet(dialectFacet);
    this.lane = view.state.facet(laneSplitFacet);
    this.rescan(view.state);
    this.decorations = this.build(view);
  }

  update(u: ViewUpdate) {
    const dialect = u.state.facet(dialectFacet);
    const lane = u.state.facet(laneSplitFacet);
    const rescan = u.docChanged || dialect !== this.dialect || lane !== this.lane;
    if (rescan) {
      this.dialect = dialect;
      this.lane = lane;
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
    this.statements = this.lane ? this.lane(this.text) : splitStatements(this.text, this.dialect);
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
  // Toggle Line / Block Comment (Keymap ▸ SQL Editor, remappable): run on this editor itself.
  for (const d of KEYMAP) {
    const command = d.scope === "editor" ? COMMENT_COMMANDS[d.id] : undefined;
    if (command) bindings.push({ key: toCmKey(d.keys), preventDefault: true, run: command });
  }
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

/** The comment toggles' KEYMAP entries (`editorCommand`), bound by `actionKeymap`. */
const COMMENT_COMMANDS: Partial<Record<ActionId, Command>> = {
  "query.comment-line": toggleLineComment,
  "query.comment-block": toggleBlockCommentSmart,
};

/**
 * TablePlus's multi-caret keys on top of CodeMirror's defaults (which already give ⌘-click to add
 * a caret, ⌘D for the next occurrence and Esc back to one caret): ⌃⇧↑ / ⌃⇧↓ add a caret above /
 * below, ⌃⌘G selects every occurrence. The comment toggles (⌘/, ⌥⌘/) come from KEYMAP instead
 * (`actionKeymap`), so Settings ▸ Shortcuts can remap them.
 */
export const multiCaretKeymap: readonly KeyBinding[] = [
  { key: "Ctrl-Shift-ArrowUp", run: addCursorAbove, preventDefault: true },
  { key: "Ctrl-Shift-ArrowDown", run: addCursorBelow, preventDefault: true },
  { key: "Ctrl-Alt-g", mac: "Ctrl-Mod-g", run: selectAllOccurrences, preventDefault: true },
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
    // Syntax colours, the quote-kind colours and the chrome: shared with the other SQL editors.
    sqlEditorTheme,
    languageConf.of(languageExtension(setup.language)),
    prefsConf.of(prefsExtension(setup.prefs)),
    EditorView.updateListener.of(setup.onUpdate),
    EditorView.contentAttributes.of({ "aria-label": "SQL editor", spellcheck: "false" }),
    actionKeymapConf.of(actionKeymap()),
    keymap.of([
      ...multiCaretKeymap,
      // ⌘/ comes from KEYMAP (`actionKeymap`): a remapped Toggle Line Comment frees it.
      ...defaultKeymap.filter((b) => b.run !== toggleComment),
      ...searchKeymap,
      ...historyKeymap,
      ...foldKeymap,
      ...lintKeymap,
      indentWithTab,
    ]),
  ];
}
