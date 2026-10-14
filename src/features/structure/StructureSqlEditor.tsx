import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching } from "@codemirror/language";
import { EditorState, Prec } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { useEffect, useLayoutEffect, useRef } from "react";
import { languageExtension } from "@/features/editor/cmExtensions";
import { sqlEditorTheme } from "@/features/editor/cmTheme";
import type { Dialect } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/**
 * A small CodeMirror SQL editor for statements the structure view stages as they are written (a
 * trigger, a view definition). Uncontrolled after mount: `value` seeds the document, and a new
 * `resetKey` replaces it (discard, refresh). ⌘S calls `onSave`, ⌘⇧P `onPreview`.
 */
export function StructureSqlEditor({
  value,
  resetKey,
  onChange,
  dialect,
  label,
  onSave,
  onPreview,
  autoFocus = false,
  readOnly = false,
  className,
}: {
  value: string;
  resetKey?: string | number;
  onChange: (sql: string) => void;
  dialect: Dialect;
  label: string;
  onSave?: () => void;
  onPreview?: () => void;
  autoFocus?: boolean;
  readOnly?: boolean;
  className?: string;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const handlers = useRef({ onChange, onSave, onPreview });
  handlers.current = { onChange, onSave, onPreview };
  const initial = useRef(value);
  initial.current = value;

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const run = (fn: "onSave" | "onPreview") => () => {
      const f = handlers.current[fn];
      if (!f) return false;
      f();
      return true;
    };
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initial.current,
        extensions: [
          lineNumbers(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          bracketMatching(),
          sqlEditorTheme,
          languageExtension({ dialect }),
          EditorView.lineWrapping,
          EditorState.readOnly.of(readOnly),
          EditorView.editable.of(!readOnly),
          EditorView.contentAttributes.of({ "aria-label": label, spellcheck: "false" }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) handlers.current.onChange(u.state.doc.toString());
          }),
          Prec.highest(
            keymap.of([
              { key: "Mod-s", preventDefault: true, run: run("onSave") },
              { key: "Mod-Shift-p", preventDefault: true, run: run("onPreview") },
            ]),
          ),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        ],
      }),
    });
    viewRef.current = view;
    if (autoFocus) view.focus();
    return () => {
      viewRef.current = null;
      view.destroy();
    };
    // The editor is created once per mount (and per reset); later prop changes go through refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dialect, readOnly, label]);

  // A reset (discard / refresh) replaces the document with the current value.
  const lastReset = useRef(resetKey);
  useEffect(() => {
    if (lastReset.current === resetKey) return;
    lastReset.current = resetKey;
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === initial.current) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: initial.current } });
  }, [resetKey]);

  return (
    <div
      ref={hostRef}
      data-testid="structure-sql-editor"
      className={cn("border-border min-h-0 overflow-auto rounded-md border text-[12px]", className)}
    />
  );
}
