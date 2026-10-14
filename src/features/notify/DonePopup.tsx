import { CircleCheck, FolderOpen } from "lucide-react";
import { useRef } from "react";
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
import { useSingleHost } from "@/features/export/singleHost";
import { closeDonePopup, revealFile, useDoneStore } from "./done";

/** Name of the single-host slot of the popup. */
const DONE_HOST = "done-popup";

/**
 * The popup after an export or a backup ("Show open containing folder popup after exporting
 * data"): what was written, "Show in Finder" and OK (Return / Esc). Mounted by `NotifyHost`.
 */
export function DonePopup() {
  const owner = useSingleHost(DONE_HOST);
  const shown = useDoneStore((s) => s.shown);
  const last = useDoneStore((s) => s.last);
  const notice = shown ?? last;
  const ok = useRef<HTMLButtonElement>(null);
  if (!owner) return null;
  const path = notice?.path ?? null;
  return (
    <AlertDialog open={shown !== null} onOpenChange={(open) => !open && closeDonePopup()}>
      <AlertDialogContent
        className="sm:max-w-[400px]"
        // OK takes Return (Radix would focus the cancel button: Show in Finder).
        onOpenAutoFocus={(e) => {
          e.preventDefault();
          ok.current?.focus();
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2 text-sm">
            <CircleCheck className="text-success size-4" />
            {notice?.popupTitle ?? notice?.title ?? ""}
          </AlertDialogTitle>
          <AlertDialogDescription className="flex flex-col gap-1 text-xs">
            {notice?.popupTitle && <span className="text-foreground">{notice.title}</span>}
            <span className="font-mono break-all">{notice?.description ?? ""}</span>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          {path !== null && (
            <AlertDialogCancel
              onClick={() => {
                revealFile(path);
                closeDonePopup();
              }}
            >
              <FolderOpen /> Show in Finder
            </AlertDialogCancel>
          )}
          <AlertDialogAction ref={ok} onClick={closeDonePopup}>
            OK
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
