import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, placeholder } from "@codemirror/view";
import { useCallback, useRef, useState } from "react";
import { FormField } from "@/components/FormField";
import { KbdHint } from "@/components/KbdHint";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { coreDialect, type CoreDriver } from "@/features/drivers/catalog";
import { languageExtension } from "@/features/editor/cmExtensions";
import { sqlEditorTheme } from "@/features/editor/cmTheme";
import { useDirtyDialogGuard } from "@/hooks/useDirtyDialogGuard";
import type { Driver } from "@/lib/ipc";

const EXAMPLE: Record<CoreDriver, string> = {
  postgres: "SET search_path TO app, public;",
  mysql: "SET SESSION sql_safe_updates = 1;",
  sqlite: "PRAGMA foreign_keys = ON;",
};

const SCRIPT_EXAMPLE: Record<CoreDriver, string> = {
  postgres: "cloud-sql-proxy my-project:region:db --port 5433 &>/tmp/proxy.log &\nsleep 2",
  mysql: "cloud-sql-proxy my-project:region:db --port 3307 &>/tmp/proxy.log &\nsleep 2",
  sqlite: 'rsync -a server:/var/lib/app/app.sqlite "$TABLER_SQLITE_PATH"',
};

/**
 * "Bootstrap commands…": a small SQL editor for the statements run right after login on every
 * connect (each session connection: the editor and the browsing pool), and the bootstrap CLI
 * script (b546) run in the login shell before every connect and test (start a proxy, copy a
 * SQLite file…). Save keeps both in the form; they are stored with the connection.
 */
export function BootstrapCommandsDialog({
  open,
  value,
  script = "",
  driver,
  onOpenChange,
  onSave,
}: {
  open: boolean;
  value: string;
  /** The shell script run before connecting. */
  script?: string;
  /** A driver beyond the core three gets a plain editor (its commands are its own). */
  driver: Driver;
  onOpenChange: (open: boolean) => void;
  onSave: (sql: string, script: string) => void;
}) {
  const viewRef = useRef<EditorView | null>(null);
  const scriptRef = useRef<HTMLTextAreaElement | null>(null);
  // The editor is created when the dialog's content mounts, with the value at that time.
  const initial = useRef(value);
  initial.current = value;
  const dialect = coreDialect(driver);
  /** Either box differs from what the form holds: closing other than by Save asks first. */
  const [dirty, setDirty] = useState(false);
  const checkDirty = useCallback(() => {
    const sql = viewRef.current?.state.doc.toString() ?? initial.current;
    setDirty(sql !== initial.current || (scriptRef.current?.value ?? script) !== script);
  }, [script]);
  const checkDirtyRef = useRef(checkDirty);
  checkDirtyRef.current = checkDirty;
  const guard = useDirtyDialogGuard({
    dirty,
    onClose: () => onOpenChange(false),
    what: "the edited commands",
  });

  const save = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;
    onSave(view.state.doc.toString(), scriptRef.current?.value ?? script);
    onOpenChange(false);
  }, [onSave, onOpenChange, script]);
  const saveRef = useRef(save);
  saveRef.current = save;

  const mount = useCallback(
    (el: HTMLDivElement | null) => {
      viewRef.current?.destroy();
      viewRef.current = null;
      // A fresh editor holds the form's commands: nothing is edited yet.
      setDirty(false);
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
            dialect ? languageExtension({ dialect }) : [],
            sqlEditorTheme,
            EditorView.lineWrapping,
            EditorView.updateListener.of((update) => {
              if (update.docChanged) checkDirtyRef.current();
            }),
            placeholder(dialect ? EXAMPLE[dialect] : ""),
            EditorView.contentAttributes.of({ "aria-label": "Bootstrap commands" }),
          ],
        }),
      });
      viewRef.current = view;
      view.focus();
    },
    [dialect],
  );

  return (
    <>
      <Dialog open={open} onOpenChange={guard.onOpenChange}>
        <DialogContent className="flex flex-col sm:max-w-[34rem]" {...guard.contentProps}>
          <DialogHeader>
            <DialogTitle className="text-sm">Bootstrap commands</DialogTitle>
            <DialogDescription className="text-xs">
              SQL run right after login, every time this connection opens a session (reconnects
              included). Separate statements with semicolons.
            </DialogDescription>
          </DialogHeader>
          <div
            ref={mount}
            data-testid="bootstrap-editor"
            className="border-border h-44 overflow-hidden rounded-md border text-sm"
          />
          <FormField
            label="Shell script, run before connecting"
            htmlFor="bootstrap-script"
            hint={
              <>
                Runs in your login shell (<span className="font-mono">$SHELL -lc</span>, 60 s limit)
                before every connect and test, e.g. to start a proxy or copy the database file. A
                failure stops the connect with its output. It gets{" "}
                <span className="font-mono">$TABLER_CONNECTION</span>,{" "}
                <span className="font-mono">$TABLER_HOST</span>,{" "}
                <span className="font-mono">$TABLER_PORT</span>,{" "}
                <span className="font-mono">$TABLER_DATABASE</span>,{" "}
                <span className="font-mono">$TABLER_SQLITE_PATH</span>.
              </>
            }
          >
            <Textarea
              id="bootstrap-script"
              aria-describedby="bootstrap-script-hint"
              ref={scriptRef}
              defaultValue={script}
              placeholder={SCRIPT_EXAMPLE[dialect ?? "postgres"]}
              rows={4}
              spellCheck={false}
              autoComplete="off"
              className="min-h-0 resize-none rounded-md font-mono text-xs md:text-xs"
              onChange={checkDirty}
              onKeyDown={(e) => {
                if ((e.metaKey || e.ctrlKey) && (e.key === "s" || e.key === "Enter")) {
                  e.preventDefault();
                  save();
                }
              }}
            />
          </FormField>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={guard.requestClose}>
              Cancel
            </Button>
            <Button type="button" size="sm" onClick={save}>
              Save
              <KbdHint combo="Mod+Enter" />
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {guard.confirm}
    </>
  );
}
