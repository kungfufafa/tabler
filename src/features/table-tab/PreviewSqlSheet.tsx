import { AlertTriangle, Loader2 } from "lucide-react";
import { useRef } from "react";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { cn } from "@/lib/utils";
import type { PreviewState } from "./previewState";

/** TablePlus-style "Preview SQL" sheet: shows the exact statements, then applies them in one transaction. */
export function PreviewSqlSheet({
  state,
  warning,
  onOpenChange,
  onApply,
}: {
  state: PreviewState;
  /** Shown above the statements (e.g. how rows of a table without a primary key are matched). */
  warning?: string;
  onOpenChange: (open: boolean) => void;
  onApply: () => void;
}) {
  // A modal Radix sheet returns focus to its trigger, and this one has none (⌘S, the status bar's
  // Commit): put focus back where it was (usually the grid), so ⌘Z / ⌘S / arrows keep working.
  const returnFocus = useRef<HTMLElement | null>(null);
  return (
    <Sheet open={state.open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        onOpenAutoFocus={() => {
          const el = document.activeElement;
          returnFocus.current = el instanceof HTMLElement && el !== document.body ? el : null;
        }}
        onCloseAutoFocus={(e) => {
          const el = returnFocus.current;
          returnFocus.current = null;
          if (!el?.isConnected) return;
          e.preventDefault();
          el.focus({ preventScroll: true });
        }}
        className="flex w-[560px] max-w-full flex-col gap-0 p-0 sm:max-w-[560px]"
      >
        <SheetHeader className="border-border border-b px-5 py-4">
          <SheetTitle>Preview changes</SheetTitle>
          <SheetDescription>
            {state.statements.length} statement{state.statements.length === 1 ? "" : "s"} will run
            in a single transaction.
          </SheetDescription>
        </SheetHeader>
        {warning && (
          <div className="border-warning/40 bg-warning/10 text-warning flex shrink-0 items-start gap-2 border-b px-5 py-2 text-[11px] leading-4">
            <AlertTriangle className="mt-px size-3.5 shrink-0" />
            <span>{warning}</span>
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-auto px-5 py-3">
          {state.loading ? (
            <div className="text-muted-foreground flex items-center gap-2 text-xs">
              <Loader2 className="size-3.5 animate-spin" /> Generating SQL…
            </div>
          ) : (
            <ol className="flex flex-col gap-2">
              {state.statements.map((sql, i) => (
                <li
                  key={i}
                  className={cn(
                    "selectable border-border bg-surface-2 rounded-md border p-2 font-mono text-[11.5px] leading-4 break-words whitespace-pre-wrap",
                    state.failedIndex === i && "border-error bg-error/10",
                  )}
                >
                  {sql}
                </li>
              ))}
            </ol>
          )}
          {state.error && (
            <div className="border-error/40 bg-error/10 text-error mt-3 flex items-start gap-2 rounded-md border p-2 text-xs">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span className="selectable break-words whitespace-pre-wrap">{state.error}</span>
            </div>
          )}
        </div>
        <SheetFooter className="border-border flex-row items-center justify-end gap-2 border-t px-5 py-3">
          <span className="text-muted-foreground mr-auto text-[11px]">
            Rolled back entirely on the first error.
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onOpenChange(false)}
            disabled={state.applying}
          >
            Cancel
          </Button>
          <Button
            size="sm"
            onClick={onApply}
            disabled={state.loading || state.applying || state.statements.length === 0}
          >
            {state.applying && <Loader2 className="animate-spin" />}
            Apply
            <kbd className="ml-1 text-[10px] opacity-70">⌘↩</kbd>
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
