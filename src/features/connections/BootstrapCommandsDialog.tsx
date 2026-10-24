import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, placeholder } from "@codemirror/view";
import { useCallback, useRef } from "react";
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
import type { CoreDriver } from "@/features/drivers/catalog";
import { languageExtension } from "@/features/editor/cmExtensions";
import { sqlEditorTheme } from "@/features/editor/cmTheme";

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
  driver: CoreDriver;
  onOpenChange: (open: boolean) => void;
  onSave: (sql: string, script: string) => void;
}) {
  const viewRef = useRef<EditorView | null>(null);
  const scriptRef = useRef<HTMLTextAreaElement | null>(null);
  // The editor is created when the dialog's content mounts, with the value at that time.
  const initial = useRef(value);
  initial.current = value;

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
            sqlEditorTheme,
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
            SQL run right after login, every time this connection opens a session (reconnects
            included). Separate statements with semicolons.
          </DialogDescription>
        </DialogHeader>
        <div
          ref={mount}
          data-testid="bootstrap-editor"
          className="border-border h-44 overflow-hidden rounded-md border text-[13px]"
        />
        <div className="flex flex-col gap-1.5">
          <label htmlFor="bootstrap-script" className="text-foreground text-xs font-medium">
            Shell script, run before connecting
          </label>
          <Textarea
            id="bootstrap-script"
            ref={scriptRef}
            defaultValue={script}
            placeholder={SCRIPT_EXAMPLE[driver]}
            rows={4}
            spellCheck={false}
            autoComplete="off"
            className="min-h-0 resize-none font-mono text-xs"
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && (e.key === "s" || e.key === "Enter")) {
                e.preventDefault();
                save();
              }
            }}
          />
          <p className="text-muted-foreground text-[11px]">
            Runs in your login shell (<span className="font-mono">$SHELL -lc</span>, 60 s limit)
            before every connect and test, e.g. to start a proxy or copy the database file. A
            failure stops the connect with its output. It gets{" "}
            <span className="font-mono">$TABLER_CONNECTION</span>,{" "}
            <span className="font-mono">$TABLER_HOST</span>,{" "}
            <span className="font-mono">$TABLER_PORT</span>,{" "}
            <span className="font-mono">$TABLER_DATABASE</span>,{" "}
            <span className="font-mono">$TABLER_SQLITE_PATH</span>.
          </p>
        </div>
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
