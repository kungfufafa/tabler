import { AlertTriangle, Check, Info, Loader2 } from "lucide-react";
import type { KeyboardEvent } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import type { Dialect, TableRef } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  changesSchema,
  executedFlags,
  nonTransactionalNote,
  type DdlErrorInfo,
  type DdlPreviewState,
} from "./previewState";
import { SqlBlock } from "./SqlBlock";

function ErrorText({ error }: { error: DdlErrorInfo }) {
  return (
    <div className="selectable flex flex-col gap-0.5 break-words whitespace-pre-wrap">
      <span>{error.message}</span>
      {error.detail && <span className="opacity-80">{error.detail}</span>}
      {error.hint && <span className="opacity-80">Hint: {error.hint}</span>}
    </div>
  );
}

/**
 * "Review structure changes": the exact statements `apply_ddl` will run, the generator's warnings,
 * whether the plan is transactional and, after a failure, which statement failed and which ones
 * already ran.
 */
export function DdlPreviewSheet({
  state,
  dialect,
  table,
  onOpenChange,
  onApply,
  onDiscardAndRefresh,
  returnFocus,
  staleRan = 0,
}: {
  state: DdlPreviewState;
  dialect: Dialect;
  table: TableRef;
  onOpenChange: (open: boolean) => void;
  onApply: () => void;
  onDiscardAndRefresh: () => void;
  returnFocus?: () => void;
  /** Statements an earlier apply already ran before it stopped (the staged ops are stale). */
  staleRan?: number;
}) {
  const { plan, error, failedIndex, executed } = state;
  const statements = plan?.statements ?? [];
  const ran = executed ? executedFlags(statements, executed) : statements.map(() => false);
  // Statements that ran and changed the schema (the SQLite rebuild's PRAGMAs do not count).
  const ranCount = ran.filter((r, i) => r && changesSchema(statements[i]!)).length;
  const failed = error !== null && executed !== null;
  const partial = failed && ranCount > 0;
  // An earlier attempt (sheet since closed) stopped partway: the plan targets the old table.
  const stale = !partial && staleRan > 0;
  const canApply = !state.loading && !state.applying && statements.length > 0 && !partial;

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && canApply) {
      e.preventDefault();
      onApply();
    }
  };

  const describe = () => {
    if (state.loading) return "Generating SQL…";
    if (!plan) return "Could not generate the statements.";
    const n = `${statements.length} statement${statements.length === 1 ? "" : "s"}`;
    return plan.transactional
      ? `${n} · run in a single transaction`
      : `${n} · run one by one, not in a single transaction`;
  };

  return (
    <Sheet open={state.open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        onKeyDown={onKeyDown}
        onCloseAutoFocus={(e) => {
          if (!returnFocus) return;
          e.preventDefault();
          returnFocus();
        }}
        className="flex w-[620px] max-w-full flex-col gap-0 p-0 sm:max-w-[620px]"
      >
        <SheetHeader className="border-border border-b px-5 py-4">
          <SheetTitle>Review structure changes</SheetTitle>
          <SheetDescription>
            <span className="font-mono">
              {table.schema ? `${table.schema}.` : ""}
              {table.name}
            </span>{" "}
            · {describe()}
          </SheetDescription>
        </SheetHeader>
        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto px-5 py-3">
          {state.loading && (
            <div className="text-muted-foreground flex items-center gap-2 text-xs">
              <Loader2 className="size-3.5 animate-spin" /> Generating SQL…
            </div>
          )}
          {plan && plan.warnings.length > 0 && (
            <div
              role="alert"
              className="border-warning/50 bg-warning/10 text-warning flex items-start gap-2 rounded-md border p-2.5 text-xs"
            >
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <ul className="flex flex-col gap-1">
                {plan.warnings.map((w, i) => (
                  <li key={i} className="selectable">
                    {w}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {plan && !plan.transactional && (
            <div className="border-border text-muted-foreground flex items-start gap-2 rounded-md border p-2 text-[11px]">
              <Info className="mt-px size-3.5 shrink-0" />
              <span>{nonTransactionalNote(dialect)}</span>
            </div>
          )}
          {partial && (
            <div
              role="alert"
              className="border-warning/50 bg-warning/10 text-warning rounded-md border p-2.5 text-xs"
            >
              {ranCount} statement{ranCount === 1 ? "" : "s"} already ran and stay applied. The
              table is partly changed: discard the staged changes and refresh before trying again.
            </div>
          )}
          {stale && (
            <div
              role="alert"
              className="border-warning/50 bg-warning/10 text-warning rounded-md border p-2.5 text-xs"
            >
              An earlier apply stopped partway: {staleRan} statement{staleRan === 1 ? "" : "s"}{" "}
              already ran, so these statements are planned against the table as it was before.
              Discard the staged changes and refresh before trying again.
            </div>
          )}
          {failed && !partial && (
            <div
              role="alert"
              className="border-error/40 bg-error/10 text-error rounded-md border p-2.5 text-xs"
            >
              {plan?.transactional
                ? "The transaction was rolled back: nothing was changed."
                : "Nothing was changed."}{" "}
              Fix the staged changes and apply again.
            </div>
          )}
          {statements.length > 0 && (
            <ol className="flex flex-col gap-2">
              {statements.map((sql, i) => {
                const isFailed = failed && failedIndex === i;
                const notRun = failed && !ran[i] && !isFailed;
                return (
                  <li
                    key={i}
                    data-testid={`ddl-statement-${i}`}
                    data-failed={isFailed || undefined}
                    data-executed={ran[i] || undefined}
                    className={cn(
                      "border-border bg-surface-2 overflow-hidden rounded-md border",
                      isFailed && "border-error bg-error/10",
                      notRun && "opacity-60",
                    )}
                  >
                    <div className="border-border/70 text-muted-foreground flex h-6 items-center gap-2 border-b px-2 text-[10px]">
                      <span className="font-mono tabular-nums">{i + 1}</span>
                      {ran[i] && (
                        <span className="text-success inline-flex items-center gap-0.5 font-medium">
                          <Check className="size-3" /> ran
                        </span>
                      )}
                      {isFailed && <span className="text-error font-medium">failed</span>}
                      {notRun && <span>not run</span>}
                    </div>
                    <SqlBlock sql={sql} dialect={dialect} className="px-2.5 py-2" />
                    {isFailed && error && (
                      <div className="border-error/30 text-error border-t px-2.5 py-1.5 text-xs">
                        <ErrorText error={error} />
                      </div>
                    )}
                  </li>
                );
              })}
            </ol>
          )}
          {error && !(failed && failedIndex !== null && failedIndex < statements.length) && (
            <div
              role="alert"
              className="border-error/40 bg-error/10 text-error flex items-start gap-2 rounded-md border p-2 text-xs"
            >
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <ErrorText error={error} />
            </div>
          )}
        </div>
        <SheetFooter className="border-border flex-row items-center justify-end gap-2 border-t px-5 py-3">
          <span className="text-muted-foreground min-w-0 flex-1 text-[11px] leading-4">
            {plan && !partial && !stale
              ? plan.transactional
                ? "Rolled back entirely on the first error."
                : "Stops at the first error; earlier statements stay applied."
              : ""}
          </span>
          {(partial || stale) && (
            <Button variant="outline" size="sm" onClick={onDiscardAndRefresh}>
              Discard &amp; refresh
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onOpenChange(false)}
            disabled={state.applying}
          >
            Cancel
          </Button>
          <Button size="sm" onClick={onApply} disabled={!canApply}>
            {state.applying && <Loader2 className="animate-spin" />}
            Apply
            <Kbd
              combo="Mod+Enter"
              className="border-primary-foreground/30 ml-1 bg-transparent text-inherit"
            />
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}
