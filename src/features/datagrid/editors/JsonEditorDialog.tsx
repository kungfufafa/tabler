import { useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import { KbdHint } from "@/components/KbdHint";
import { useDirtyDialogGuard } from "@/hooks/useDirtyDialogGuard";
import { cn } from "@/lib/utils";
import type { GridColumn } from "../types";
import { useDraftWork } from "../useDraftWork";
import { jsonError, reformatJson } from "./json";
import { commitTabSoon, isSaveKey } from "./saveKey";
import { insertAtCaret } from "./useGrabFocus";

/** JSON cell editor: validates live; Format/Minify only rewrite whitespace (numbers stay exact). */
export function JsonEditorDialog({
  col,
  initialText,
  onSave,
  onSaveAndCommit,
  onSetNull,
  onCancel,
}: {
  col: GridColumn;
  initialText: string;
  onSave: (text: string) => void;
  /**
   * ⌘S: stages the text like Save does, without moving the grid's focus, and says whether it did;
   * the tab is committed after it. Without it ⌘S is Save followed by the commit.
   */
  onSaveAndCommit?: (text: string) => boolean;
  /** Shown as "Set NULL" when the column is nullable. */
  onSetNull?: () => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initialText);
  const ref = useRef<HTMLTextAreaElement>(null);
  const error = useMemo(() => jsonError(text), [text]);
  useDraftWork(text !== initialText, col.name);
  // A stray Escape or click beside the dialog asks before it throws the edited JSON away.
  const guard = useDirtyDialogGuard({
    dirty: text !== initialText,
    onClose: onCancel,
    what: "the edited JSON",
  });
  const save = () => {
    if (!error) onSave(text);
  };
  /** ⌘S: the text is staged (an invalid one is not), then the whole tab is committed. */
  const saveAndCommit = () => {
    if (error) return;
    if (onSaveAndCommit) {
      if (!onSaveAndCommit(text)) return;
    } else onSave(text);
    commitTabSoon();
  };
  const reformat = (indent: number | null) => {
    if (!error) setText(reformatJson(text, indent));
    ref.current?.focus();
  };

  return (
    <Dialog open onOpenChange={guard.onOpenChange}>
      <DialogContent
        {...guard.contentProps}
        data-scope="grid-edit"
        className="flex h-[min(40rem,calc(100dvh-4rem))] flex-col sm:max-w-[min(56rem,calc(100%-2rem))]"
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          ref.current?.focus({ preventScroll: true });
        }}
        // The grid takes focus back itself once the editor closes.
        onCloseAutoFocus={(e) => e.preventDefault()}
        onPointerDown={(e) => e.stopPropagation()}
        onDoubleClick={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <DialogHeader>
          <DialogTitle className="text-sm">{`Edit JSON — ${col.name} (${col.dataType})`}</DialogTitle>
          <DialogDescription className="sr-only">
            Edit the JSON value. Save with Command-Return, cancel with Escape.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          ref={ref}
          value={text}
          aria-label={`JSON value of ${col.name}`}
          aria-invalid={error ? true : undefined}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === "Tab" && !e.shiftKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
              e.preventDefault();
              insertAtCaret(e.currentTarget, "  ");
            } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              save();
            } else if (isSaveKey(e.nativeEvent)) {
              e.preventDefault();
              saveAndCommit();
            }
          }}
          className="[field-sizing:fixed] min-h-32 flex-1 resize-none font-mono text-xs leading-5 md:text-xs"
        />
        <div
          role={error ? "alert" : undefined}
          className={cn("min-h-4 truncate text-xs", error ? "text-error" : "text-muted-foreground")}
          title={error ?? undefined}
        >
          {error ?? "Valid JSON"}
        </div>
        <DialogFooter className="items-center sm:justify-between">
          <div className="flex gap-1.5">
            <Button variant="outline" size="sm" disabled={!!error} onClick={() => reformat(2)}>
              Format
            </Button>
            <Button variant="outline" size="sm" disabled={!!error} onClick={() => reformat(null)}>
              Minify
            </Button>
          </div>
          <div className="flex gap-1.5">
            <Button variant="outline" size="sm" onClick={guard.requestClose}>
              Cancel
            </Button>
            {onSetNull && (
              <Button variant="outline" size="sm" onClick={onSetNull}>
                Set NULL
              </Button>
            )}
            <Button size="sm" disabled={!!error} onClick={save}>
              Save
              <KbdHint combo="Mod+Enter" />
            </Button>
          </div>
        </DialogFooter>
        {/* Inside the content, whose handlers keep its keys and clicks away from the grid. */}
        {guard.confirm}
      </DialogContent>
    </Dialog>
  );
}
