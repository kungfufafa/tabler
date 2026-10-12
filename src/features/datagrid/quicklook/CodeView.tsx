import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { useEffect, useRef } from "react";
import { editorTheme } from "@/features/editor/cmTheme";
import { cn } from "@/lib/utils";

/**
 * A plain CodeMirror editor (no language: the app has no JSON grammar installed) for the Quick
 * Look Editor: line numbers, undo, search & replace (⌘F), multiple carets, optional wrapping.
 * Controlled: `text` replaces the document when it differs from it.
 */
export function CodeView({
  text,
  onChange,
  readOnly = false,
  wrap,
  label,
  autoFocus = false,
  className,
}: {
  text: string;
  onChange?: (text: string) => void;
  readOnly?: boolean;
  wrap: boolean;
  label: string;
  autoFocus?: boolean;
  className?: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const wrapping = useRef(new Compartment());
  const editable = useRef(new Compartment());
  const changed = useRef(onChange);
  changed.current = onChange;

  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          EditorState.allowMultipleSelections.of(true),
          search({ top: true }),
          keymap.of([...searchKeymap, ...historyKeymap, indentWithTab, ...defaultKeymap]),
          editorTheme,
          EditorView.contentAttributes.of({ "aria-label": label }),
          wrapping.current.of(wrap ? EditorView.lineWrapping : []),
          editable.current.of([
            EditorState.readOnly.of(readOnly),
            EditorView.editable.of(!readOnly),
          ]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) changed.current?.(u.state.doc.toString());
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
    v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } });
  }, [text]);
  useEffect(() => {
    view.current?.dispatch({
      effects: wrapping.current.reconfigure(wrap ? EditorView.lineWrapping : []),
    });
  }, [wrap]);
  useEffect(() => {
    view.current?.dispatch({
      effects: editable.current.reconfigure([
        EditorState.readOnly.of(readOnly),
        EditorView.editable.of(!readOnly),
      ]),
    });
  }, [readOnly]);

  return (
    <div
      ref={host}
      data-testid="quick-look-code"
      className={cn(
        "border-border min-h-0 flex-1 overflow-hidden rounded-md border text-[12px] [&_.cm-editor]:h-full",
        className,
      )}
    />
  );
}
