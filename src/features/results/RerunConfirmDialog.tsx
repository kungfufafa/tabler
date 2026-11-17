import { useState } from "react";
import { Kbd } from "@/components/Kbd";
import { KbdHint } from "@/components/KbdHint";
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
import { comboFromEvent } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { answerRerunConfirm, useRerunConfirmStore, type RerunConfirmRequest } from "./rerunConfirm";

const CONFIRM_COMBO = "Mod+Enter";

/**
 * Asks before a run from the SQL editor replaces results that hold uncommitted edits (the
 * `confirmRerunDiscard` question). Commit and Run is the way on that keeps them: it commits as ⌘S
 * does, through its Preview sheet, then runs. ⌘↩ discards and runs, like running again; Esc
 * cancels and leaves the edits staged. Mount once.
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
        className="data-[size=default]:sm:max-w-md"
        onKeyDown={(e) => {
          if (comboFromEvent(e) !== CONFIRM_COMBO) return;
          e.preventDefault();
          answerRerunConfirm("discard");
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">Discard {changes} and run?</AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            Running the query again replaces the result {n === 1 ? "it was" : "they were"} made in.
            Commit with{" "}
            <Kbd combo={shortcutFor("grid.commit") ?? "Mod+S"} className="mx-0.5 align-middle" />{" "}
            first to keep {n === 1 ? "it" : "them"}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            size="sm"
            onClick={() => answerRerunConfirm("discard")}
          >
            Discard and Run
            <KbdHint combo={CONFIRM_COMBO} />
          </AlertDialogAction>
          <AlertDialogAction size="sm" onClick={() => answerRerunConfirm("commit")}>
            Commit and Run…
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
