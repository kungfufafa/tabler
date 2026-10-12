import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, placeholder } from "@codemirror/view";
import { useCallback, useRef } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { languageExtension } from "@/features/editor/cmExtensions";
import { editorTheme, sqlHighlightStyle } from "@/features/editor/cmTheme";
import type { Driver } from "@/lib/ipc";

const EXAMPLE: Record<Driver, string> = {
  postgres: "SET search_path TO app, public;",
  mysql: "SET SESSION sql_safe_updates = 1;",
  sqlite: "PRAGMA foreign_keys = ON;",
};

/**
 * "Bootstrap commands…": a small SQL editor for the statements run right after login on every
 * connect (each session connection: the editor and the browsing pool). Save keeps them in the
 * form; they are stored with the connection.
 */
export function BootstrapCommandsDialog({
  open,
  value,
  driver,
  onOpenChange,
  onSave,
}: {
  open: boolean;
  value: string;
  driver: Driver;
  onOpenChange: (open: boolean) => void;
  onSave: (sql: string) => void;
}) {
  const viewRef = useRef<EditorView | null>(null);
  // The editor is created when the dialog's content mounts, with the value at that time.
  const initial = useRef(value);
  initial.current = value;

  const save = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;
    onSave(view.state.doc.toString());
    onOpenChange(false);
  }, [onSave, onOpenChange]);
  const saveRef = useRef(save);
  saveRef.current = save;

  const mount = useCallback(
    (el: HTMLDivElement | null) => {
      viewRef.current?.destroy();
      viewRef.current = null;
      if (!el) return;
      const view = new EditorView({
        parent: el,
        state: EditorState.create({
          doc: initial.current,
          extensions: [
            lineNumbers(),
            history(),
            keymap.of([
              ...["Mod-Enter", "Mod-s"].map((key) => ({
                key,
                run: () => {
                  saveRef.current();
                  return true;
                },
              })),
              indentWithTab,
              ...defaultKeymap,
              ...historyKeymap,
            ]),
            languageExtension({ dialect: driver }),
            syntaxHighlighting(sqlHighlightStyle),
            editorTheme,
            EditorView.lineWrapping,
            placeholder(EXAMPLE[driver]),
            EditorView.contentAttributes.of({ "aria-label": "Bootstrap commands" }),
          ],
        }),
      });
      viewRef.current = view;
      view.focus();
    },
    [driver],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex flex-col gap-3 sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Bootstrap commands</DialogTitle>
          <DialogDescription>
            Run right after login, every time this connection opens a session (reconnects included).
            Separate statements with semicolons.
          </DialogDescription>
        </DialogHeader>
        <div
          ref={mount}
          data-testid="bootstrap-editor"
          className="border-border h-56 overflow-hidden rounded-md border text-[13px]"
        />
        <DialogFooter>
          <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" size="sm" onClick={save}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
