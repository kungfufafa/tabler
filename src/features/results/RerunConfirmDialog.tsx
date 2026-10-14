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
import { comboFromEvent, formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { answerRerunConfirm, useRerunConfirmStore, type RerunConfirmRequest } from "./rerunConfirm";

const CONFIRM_COMBO = "Mod+Enter";

/**
 * Asks before a run from the SQL editor replaces results that hold uncommitted edits (the
 * `confirmRerunDiscard` question). ⌘↩ confirms, like running again; Esc cancels and leaves the
 * edits staged. Mount once.
 */
export function RerunConfirmDialog() {
  const live = useRerunConfirmStore((s) => s.request);
  // Keep the text while the dialog animates out.
  const [last, setLast] = useState<RerunConfirmRequest | null>(live);
  if (live && live !== last) setLast(live);
  const n = (live ?? last)?.count ?? 0;
  const changes = `${n} uncommitted result change${n === 1 ? "" : "s"}`;
  return (
    <AlertDialog open={live !== null} onOpenChange={(open) => !open && answerRerunConfirm(false)}>
      <AlertDialogContent
        className="gap-3"
        onKeyDown={(e) => {
          if (comboFromEvent(e) !== CONFIRM_COMBO) return;
          e.preventDefault();
          answerRerunConfirm(true);
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm font-semibold">
            Discard {changes} and run?
          </AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            Running the query again replaces the result {n === 1 ? "it was" : "they were"} made in.
            Commit with {formatCombo(shortcutFor("grid.commit") ?? "Mod+S")} first to keep{" "}
            {n === 1 ? "it" : "them"}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="px-4 py-3">
          <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            size="sm"
            onClick={() => answerRerunConfirm(true)}
          >
            Discard &amp; Run
            <Kbd combo={CONFIRM_COMBO} className="border-current/25 bg-transparent text-inherit" />
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
