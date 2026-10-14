import {
  addCursorAbove,
  addCursorBelow,
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  insertNewline,
} from "@codemirror/commands";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import {
  Annotation,
  Compartment,
  EditorState,
  Prec,
  Transaction,
  type Extension,
} from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  keymap,
  lineNumbers,
  placeholder as placeholderExtension,
} from "@codemirror/view";
import { useEffect, useRef } from "react";
import { editorTheme } from "@/features/editor/cmTheme";
import type { Scope } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { languageExtension, type ValueLanguage } from "./languages";

/** Marks the changes `text` made to the document, which are not the user's edits. */
const fromProps = Annotation.define<boolean>();

/**
 * Replaces the whole document with `text` as a NEW document, not as an edit: it is not an undo
 * step, and nothing typed before it can be undone into it (⌘Z must never bring back another row's
 * value). The undo history is dropped by taking its extension out of the state and putting it back
 * (the history state is created empty again), one transaction apiece.
 */
function replaceDocument(v: EditorView, undoHistory: Compartment, text: string) {
  v.dispatch({
    changes: { from: 0, to: v.state.doc.length, insert: text },
    annotations: [fromProps.of(true), Transaction.addToHistory.of(false)],
    effects: undoHistory.reconfigure([]),
  });
  v.dispatch({ effects: undoHistory.reconfigure(history()) });
}

/**
 * Compact: a field of the Details sidebar. The box grows with its text up to about twelve lines,
 * then scrolls; the placeholder (NULL, EMPTY…) reads like the native fields'.
 */
const compactTheme = Prec.high(
  EditorView.theme({
    "&": { height: "auto", maxHeight: "15rem", backgroundColor: "transparent" },
    ".cm-scroller": { overflow: "auto" },
    ".cm-content": { padding: "3px 0" },
    ".cm-line": { padding: "0 8px 0 6px" },
    ".cm-placeholder": { color: "var(--null)", fontStyle: "italic" },
  }),
);

/** TablePlus's caret keys, as in the SQL editor: ⌃⇧↑ / ⌃⇧↓ add a caret above / below. */
const caretKeys = [
  { key: "Ctrl-Shift-ArrowUp", run: addCursorAbove, preventDefault: true },
  { key: "Ctrl-Shift-ArrowDown", run: addCursorBelow, preventDefault: true },
];

/** The attributes of the editable content that follow the props (`label`, `invalid`). */
function contentAttributes(label: string, invalid: boolean): Extension {
  return EditorView.contentAttributes.of({
    "aria-label": label,
    ...(invalid ? { "aria-invalid": "true" } : {}),
  });
}

/**
 * The Quick Look Editor's CodeMirror: line numbers, undo, search & replace (⌘F), multiple carets
 * (⌘-click, ⌃⇧↑ / ⌃⇧↓), optional wrapping, and JSON / XML colouring (`language`, see
 * `languages.ts`). Controlled: `text` replaces the document when it differs from it, and that
 * replacement is a new document: it can not be undone, and it empties the undo history (a field
 * that follows the selected row never undoes into the previous row's value).
 *
 * `compact` makes it a field of the Details sidebar: no line numbers, a plain ↩ is just a line
 * break, ⇥ moves on to the next field, and the box grows with its text (up to about twelve lines).
 */
export function CodeView({
  text,
  onChange,
  readOnly = false,
  wrap,
  label,
  autoFocus = false,
  language = null,
  className,
  compact = false,
  placeholder,
  invalid = false,
  shortcutScope,
  onKeyDown,
  onBlur,
  testId = "quick-look-code",
}: {
  text: string;
  onChange?: (text: string) => void;
  readOnly?: boolean;
  wrap: boolean;
  label: string;
  autoFocus?: boolean;
  /** Syntax colouring; null for plain text. */
  language?: ValueLanguage | null;
  className?: string;
  /** A field of a side panel (see above). Fixed for the life of the editor. */
  compact?: boolean;
  /** Shown while the document is empty. */
  placeholder?: string;
  /** The text is not valid (`aria-invalid`). */
  invalid?: boolean;
  /**
   * The keyboard scope the app's shortcuts see for this editor (`resolveScope`). Without it a
   * CodeMirror is a SQL editor (`editor`); a field says what it is (`grid-edit` while editable).
   */
  shortcutScope?: Scope;
  /**
   * Sees every key pressed in the text first; returning true means it was handled (CodeMirror's
   * own keymap does not get it). Not called while an input method composes text.
   */
  onKeyDown?: (event: KeyboardEvent, view: EditorView) => boolean;
  /** The text lost the focus (not to its own search panel). */
  onBlur?: () => void;
  testId?: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const wrapping = useRef(new Compartment());
  const editable = useRef(new Compartment());
  const lang = useRef(new Compartment());
  const attrs = useRef(new Compartment());
  const hint = useRef(new Compartment());
  const past = useRef(new Compartment());
  const changed = useRef(onChange);
  changed.current = onChange;
  const keyed = useRef(onKeyDown);
  keyed.current = onKeyDown;
  const blurred = useRef(onBlur);
  blurred.current = onBlur;

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: text,
        extensions: [
          compact ? [] : [lineNumbers(), highlightActiveLine()],
          past.current.of(history()),
          drawSelection(),
          highlightSelectionMatches(),
          EditorState.allowMultipleSelections.of(true),
          search({ top: true }),
          // Sees the key before CodeMirror's keymaps (a field's ⌘S / ⌘↩ / ⎋).
          Prec.highest(
            EditorView.domEventHandlers({
              keydown: (e, view) => !e.isComposing && keyed.current?.(e, view) === true,
            }),
          ),
          EditorView.domEventObservers({
            blur: (e, view) => {
              const next = e.relatedTarget;
              if (next instanceof Node && view.dom.contains(next)) return;
              blurred.current?.();
            },
          }),
          keymap.of([
            ...caretKeys,
            // A field's text is exactly what was typed: ↩ adds a bare line break, and ⇥ is left to
            // the browser (the next field) rather than indenting.
            ...(compact ? [{ key: "Enter", shift: insertNewline, run: insertNewline }] : []),
            ...searchKeymap,
            ...historyKeymap,
            ...(compact ? [] : [indentWithTab]),
            ...defaultKeymap,
          ]),
          editorTheme,
          compact ? compactTheme : [],
          attrs.current.of(contentAttributes(label, invalid)),
          hint.current.of(placeholder ? placeholderExtension(placeholder) : []),
          wrapping.current.of(wrap ? EditorView.lineWrapping : []),
          lang.current.of(language ? languageExtension(language) : []),
          editable.current.of([
            EditorState.readOnly.of(readOnly),
            EditorView.editable.of(!readOnly),
          ]),
          EditorView.updateListener.of((u) => {
            if (!u.docChanged) return;
            // Only what the user typed: not the document following `text`.
            if (u.transactions.some((tr) => tr.annotation(fromProps))) return;
            changed.current?.(u.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = v;
    if (autoFocus) v.focus();
    return () => {
      v.destroy();
      view.current = null;
    };
    // The editor is created once; later prop changes are dispatched below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const v = view.current;
    if (!v || v.state.doc.toString() === text) return;
    replaceDocument(v, past.current, text);
  }, [text]);
  useEffect(() => {
    view.current?.dispatch({
      effects: wrapping.current.reconfigure(wrap ? EditorView.lineWrapping : []),
    });
  }, [wrap]);
  useEffect(() => {
    view.current?.dispatch({
      effects: lang.current.reconfigure(language ? languageExtension(language) : []),
    });
  }, [language]);
  useEffect(() => {
    view.current?.dispatch({
      effects: editable.current.reconfigure([
        EditorState.readOnly.of(readOnly),
        EditorView.editable.of(!readOnly),
      ]),
    });
  }, [readOnly]);
  useEffect(() => {
    view.current?.dispatch({
      effects: attrs.current.reconfigure(contentAttributes(label, invalid)),
    });
  }, [label, invalid]);
  useEffect(() => {
    view.current?.dispatch({
      effects: hint.current.reconfigure(placeholder ? placeholderExtension(placeholder) : []),
    });
  }, [placeholder]);

  return (
    <div
      ref={host}
      data-testid={testId}
      data-cm-scope={shortcutScope}
      className={cn(
        "border-border overflow-hidden rounded-md border text-[12px]",
        !compact && "min-h-0 flex-1 [&_.cm-editor]:h-full",
        className,
      )}
    />
  );
}
