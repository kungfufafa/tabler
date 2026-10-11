import { FileCode2, Table2 } from "lucide-react";
import { useState } from "react";
import { Kbd } from "@/components/Kbd";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { actions } from "@/features/palette/actions";
import { closeWindowNow } from "@/hooks/useUnsavedGuard";
import { comboFromEvent } from "@/lib/keys/combo";
import { useTabsStore, type CloseRequest } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";

const DISCARD_COMBO = "Mod+D";

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

function describe(req: CloseRequest): string {
  const only = req.dirty.length === 1 ? req.dirty[0] : undefined;
  const subject = !only
    ? `${plural(req.dirty.length, "tab")} have unsaved work`
    : only.kind === "query"
      ? `“${only.title}” has unsaved changes`
      : `“${only.title}” has ${plural(only.count, "uncommitted edit")}`;
  return req.kind === "window"
    ? `${subject} that will be lost if the window closes.`
    : `${subject} that will be lost.`;
}

/** Runs `fn` once the newly activated tab has rendered and registered its actions. */
function afterTabSwitch(fn: () => void) {
  setTimeout(fn, 0);
}

/**
 * The one confirmation for every close path (tab button, ⌘W, context menu, close others/all, and
 * the native window). Driven by `closeRequest` in the tabs store.
 */
export function UnsavedChangesDialog() {
  const live = useTabsStore((s) => s.closeRequest);
  const cancel = useTabsStore((s) => s.cancelClose);
  // Keep showing the last request while the dialog animates out.
  const [last, setLast] = useState<CloseRequest | null>(live);
  if (live && live !== last) setLast(live);
  const request = live ?? last;
  const reviewTarget = request?.dirty.find((d) => d.kind === "table") ?? null;

  const discard = () => {
    const req = useTabsStore.getState().confirmClose();
    if (req?.kind === "window") void closeWindowNow();
  };

  const review = () => {
    if (!reviewTarget) return;
    const tabs = useTabsStore.getState();
    tabs.cancelClose();
    tabs.activate(reviewTarget.id);
    useUiStore.getState().setView("workspace");
    // Always the Preview SQL sheet: "Review…" must not apply even when ⌘S skips the preview.
    afterTabSwitch(() => void actions.run("grid.preview"));
  };

  const n = request?.total ?? 0;
  return (
    <AlertDialog open={live !== null} onOpenChange={(open) => !open && cancel()}>
      <AlertDialogContent
        className="gap-3"
        onKeyDown={(e) => {
          if (comboFromEvent(e) === DISCARD_COMBO) {
            e.preventDefault();
            discard();
          }
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm font-semibold">
            Discard {plural(n, "pending change")}?
          </AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            {request ? describe(request) : null}
          </AlertDialogDescription>
        </AlertDialogHeader>
        {request && request.dirty.length > 1 && (
          <ul className="border-border bg-surface-2 max-h-32 overflow-y-auto rounded-md border py-1 text-xs">
            {request.dirty.map((d) => {
              const Icon = d.kind === "query" ? FileCode2 : Table2;
              return (
                <li key={d.id} className="flex h-6 items-center gap-1.5 px-2">
                  <Icon className="text-muted-foreground size-3.5 shrink-0" />
                  <span className="min-w-0 truncate">{d.title}</span>
                  <span className="text-muted-foreground ml-auto shrink-0 tabular-nums">
                    {d.kind === "query" ? "unsaved" : plural(d.count, "change")}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
        <AlertDialogFooter className="items-center px-4 py-3 sm:justify-between">
          {reviewTarget ? (
            <Button variant="ghost" size="sm" onClick={review} className="sm:mr-auto">
              Review…
            </Button>
          ) : (
            <span className="hidden sm:block" />
          )}
          <div className="flex flex-col-reverse gap-2 sm:flex-row">
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" size="sm" onClick={discard}>
              Discard
              <Kbd
                combo={DISCARD_COMBO}
                className="border-current/25 bg-transparent text-inherit"
              />
            </AlertDialogAction>
          </div>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
