import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { FormError } from "@/components/FormError";
import { FormField } from "@/components/FormField";
import { ipc, toIpcError, type MetricsBoard } from "@/lib/ipc";
import { pickBoardSavePath } from "./open";

/** Export Board…: an optional file password (it encrypts the file), then where to save it. */
export function ExportBoardDialog({
  board,
  onClose,
}: {
  board: { id: string; name: string } | null;
  onClose: () => void;
}) {
  const id = useId();
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  /** The passwords differ: shown on the Confirm field, unlike a failed export. */
  const [mismatch, setMismatch] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!board) return;
    setPassword("");
    setConfirm("");
    setError(null);
    setMismatch(false);
    setBusy(false);
  }, [board]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!board) return;
    if (password !== confirm) {
      setMismatch(true);
      return;
    }
    const path = (await pickBoardSavePath(board.name))?.trim();
    if (!path) return;
    setBusy(true);
    try {
      await ipc.metrics.exportBoards([board.id], path, password === "" ? null : password);
      toast.success(`Exported “${board.name}”`, { description: path });
      onClose();
    } catch (err) {
      setError(toIpcError(err).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={board !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle className="text-sm">Export Board</DialogTitle>
            <DialogDescription className="text-xs">
              “{board?.name}” with its objects and queries, as a .tablerboard file. A password
              encrypts the file.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-3">
            <FormField label="Password" htmlFor={`${id}-pw`}>
              <Input
                id={`${id}-pw`}
                type="password"
                autoComplete="new-password"
                placeholder="Optional"
                value={password}
                className="h-7 rounded-md text-xs md:text-xs"
                onChange={(e) => {
                  setPassword(e.target.value);
                  setError(null);
                  setMismatch(false);
                }}
              />
            </FormField>
            <FormField
              label="Confirm"
              htmlFor={`${id}-confirm`}
              error={mismatch ? "The passwords do not match" : null}
            >
              <Input
                id={`${id}-confirm`}
                type="password"
                autoComplete="new-password"
                value={confirm}
                disabled={password === ""}
                aria-invalid={mismatch}
                aria-describedby={mismatch ? `${id}-confirm-error` : undefined}
                className="h-7 rounded-md text-xs md:text-xs"
                onChange={(e) => {
                  setConfirm(e.target.value);
                  setError(null);
                  setMismatch(false);
                }}
              />
            </FormField>
          </div>
          {error && <FormError>{error}</FormError>}
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              Export…
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Import Board… of an encrypted file: asks for its password (a wrong one says so). */
export function ImportPasswordDialog({
  path,
  profileId,
  onImported,
  onClose,
}: {
  path: string | null;
  profileId: string | null;
  onImported: (boards: MetricsBoard[]) => void;
  onClose: () => void;
}) {
  const id = useId();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setPassword("");
    setError(null);
    setBusy(false);
  }, [path]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!path || password === "") return;
    setBusy(true);
    try {
      const outcome = await ipc.metrics.importBoards(path, password, profileId);
      if (outcome.needsPassword) setError("Enter the file's password");
      else {
        onImported(outcome.boards);
        onClose();
      }
    } catch (err) {
      setError(toIpcError(err).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={path !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle className="text-sm">Import Board</DialogTitle>
            <DialogDescription className="text-xs">
              This board file is encrypted. Enter its password.
            </DialogDescription>
          </DialogHeader>
          <FormField label="Password" htmlFor={`${id}-pw`} error={error}>
            <Input
              id={`${id}-pw`}
              type="password"
              autoFocus
              value={password}
              aria-invalid={error !== null}
              aria-describedby={error ? `${id}-pw-error` : undefined}
              className="text-sm"
              onChange={(e) => {
                setPassword(e.target.value);
                setError(null);
              }}
            />
          </FormField>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || password === ""}>
              Import
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Delete Board…: asks first (a board's objects and queries go with it). */
export function DeleteBoardDialog({
  board,
  onConfirm,
  onClose,
}: {
  board: { id: string; name: string } | null;
  onConfirm: (id: string) => void;
  onClose: () => void;
}) {
  return (
    <AlertDialog open={board !== null} onOpenChange={(open) => !open && onClose()}>
      <AlertDialogContent className="data-[size=default]:sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">Delete “{board?.name}”?</AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            Its charts, inputs and queries are deleted too. This cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
          <AlertDialogAction
            size="sm"
            variant="destructive"
            onClick={() => {
              if (board) onConfirm(board.id);
              onClose();
            }}
          >
            Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
