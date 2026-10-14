import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching } from "@codemirror/language";
import { EditorState, Prec } from "@codemirror/state";
import { drawSelection, EditorView, keymap, placeholder } from "@codemirror/view";
import { useLayoutEffect, useRef } from "react";
import { languageExtension } from "@/features/editor/cmExtensions";
import { sqlEditorTheme } from "@/features/editor/cmTheme";
import type { Dialect } from "@/lib/ipc";

/**
 * The inspector's SQL editor: the app's CodeMirror SQL setup (highlighting, keyword and function
 * completion) for one object's query. Uncontrolled: `value` seeds it when it mounts (give it a
 * `key` per object). ⌘↩ runs the query.
 */
export function BoardSqlEditor({
  value,
  dialect,
  label,
  hint,
  onChange,
  onRun,
}: {
  value: string;
  dialect: Dialect;
  label: string;
  hint: string;
  onChange: (sql: string) => void;
  onRun: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const handlers = useRef({ onChange, onRun });
  useLayoutEffect(() => {
    handlers.current = { onChange, onRun };
  });
  const initial = useRef(value);

  useLayoutEffect(() => {
    const parent = host.current;
    if (!parent) return;
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: initial.current,
        extensions: [
          history(),
          drawSelection(),
          bracketMatching(),
          sqlEditorTheme,
          languageExtension({ dialect }),
          EditorView.lineWrapping,
          placeholder(hint),
          EditorView.contentAttributes.of({ "aria-label": label, spellcheck: "false" }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) handlers.current.onChange(u.state.doc.toString());
          }),
          Prec.highest(
            keymap.of([
              {
                key: "Mod-Enter",
                preventDefault: true,
                run: () => {
                  handlers.current.onRun();
                  return true;
                },
              },
            ]),
          ),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        ],
      }),
    });
    return () => view.destroy();
  }, [dialect, label, hint]);

  return (
    <div
      ref={host}
      data-slot="metrics-sql-editor"
      className="border-border bg-background max-h-64 min-h-24 overflow-auto rounded-md border text-[12px]"
    />
  );
}
