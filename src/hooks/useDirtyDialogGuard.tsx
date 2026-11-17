import type { ReactNode } from "react";
import { useCallback, useRef, useState } from "react";
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

export interface DirtyDialogGuard {
  /** Pass to `<Dialog onOpenChange>`: Escape, an outside click and the ✕ come through here. */
  onOpenChange: (open: boolean) => void;
  /** For Cancel buttons and other explicit closes that should still ask while dirty. */
  requestClose: () => void;
  /** Spread on `<DialogContent>`: Escape that only closes an editor popup stays with the editor. */
  contentProps: { onEscapeKeyDown: (e: KeyboardEvent) => void };
  /** Render once next to the dialog: the "Discard unsaved changes?" prompt. */
  confirm: ReactNode;
}

/**
 * Keeps a dialog with typed work from vanishing on a stray Escape or click outside it. While
 * `dirty`, every close that is not the dialog's own Save asks "Discard unsaved changes?" first
 * (Keep Editing / Discard); a clean dialog closes at once, as before.
 */
export function useDirtyDialogGuard({
  dirty,
  onClose,
  what = "your changes",
}: {
  dirty: boolean;
  onClose: () => void;
  /** Named in the prompt: "Discard <what>?" body text. */
  what?: string;
}): DirtyDialogGuard {
  const [asking, setAsking] = useState(false);
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  const requestClose = useCallback(() => {
    if (dirtyRef.current) setAsking(true);
    else closeRef.current();
  }, []);

  const onOpenChange = useCallback(
    (open: boolean) => {
      if (!open) requestClose();
    },
    [requestClose],
  );

  const onEscapeKeyDown = useCallback((e: KeyboardEvent) => {
    if (isEditorPopupEscape(e)) e.preventDefault();
  }, []);

  const confirm = asking ? (
    <AlertDialog open onOpenChange={(open) => !open && setAsking(false)}>
      <AlertDialogContent className="data-[size=default]:sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">Discard unsaved changes?</AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            Closing now throws away {what}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="sm" autoFocus>
            Keep Editing
          </AlertDialogCancel>
          <AlertDialogAction
            size="sm"
            variant="destructive"
            onClick={() => {
              setAsking(false);
              closeRef.current();
            }}
          >
            Discard
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  ) : null;

  return { onOpenChange, requestClose, contentProps: { onEscapeKeyDown }, confirm };
}

/**
 * Escape pressed in a CodeMirror editor whose autocomplete or another tooltip is open: the editor
 * closes its popup, so the dialog must stay.
 */
export function isEditorPopupEscape(e: KeyboardEvent): boolean {
  const target = e.target;
  if (!(target instanceof Element)) return false;
  const editor = target.closest(".cm-editor");
  return !!editor?.querySelector(".cm-tooltip-autocomplete, .cm-tooltip.cm-completionInfo");
}
