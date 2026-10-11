import { AlertTriangle, Loader2 } from "lucide-react";
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
  onOpenChange,
  onApply,
}: {
  state: PreviewState;
  onOpenChange: (open: boolean) => void;
  onApply: () => void;
}) {
  return (
    <Sheet open={state.open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="flex w-[560px] max-w-full flex-col gap-0 p-0 sm:max-w-[560px]"
      >
        <SheetHeader className="border-border border-b px-5 py-4">
          <SheetTitle>Preview changes</SheetTitle>
          <SheetDescription>
            {state.statements.length} statement{state.statements.length === 1 ? "" : "s"} will run
            in a single transaction.
          </SheetDescription>
        </SheetHeader>
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
