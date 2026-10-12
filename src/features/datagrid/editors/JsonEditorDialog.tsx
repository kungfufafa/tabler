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
import { cn } from "@/lib/utils";
import type { GridColumn } from "../types";
import { jsonError, reformatJson } from "./json";
import { insertAtCaret } from "./useGrabFocus";

/** JSON cell editor: validates live; Format/Minify only rewrite whitespace (numbers stay exact). */
export function JsonEditorDialog({
  col,
  initialText,
  onSave,
  onSetNull,
  onCancel,
}: {
  col: GridColumn;
  initialText: string;
  onSave: (text: string) => void;
  /** Shown as "Set NULL" when the column is nullable. */
  onSetNull?: () => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initialText);
  const ref = useRef<HTMLTextAreaElement>(null);
  const error = useMemo(() => jsonError(text), [text]);
  const save = () => {
    if (!error) onSave(text);
  };
  const reformat = (indent: number | null) => {
    if (!error) setText(reformatJson(text, indent));
    ref.current?.focus();
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onCancel()}>
      <DialogContent
        data-scope="grid-edit"
        className="flex max-h-[85vh] flex-col gap-3 sm:max-w-2xl"
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
            }
          }}
          className="[field-sizing:fixed] h-72 min-h-32 resize-y font-mono text-xs leading-5 md:text-xs"
        />
        <div
          role={error ? "alert" : undefined}
          className={cn(
            "min-h-4 truncate text-[11px]",
            error ? "text-error" : "text-muted-foreground",
          )}
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
            {onSetNull && (
              <Button variant="ghost" size="sm" onClick={onSetNull}>
                Set NULL
              </Button>
            )}
            <Button variant="ghost" size="sm" onClick={onCancel}>
              Cancel
            </Button>
            <Button size="sm" disabled={!!error} onClick={save}>
              Save
              <kbd className="ml-1 text-[10px] opacity-70">⌘↩</kbd>
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
