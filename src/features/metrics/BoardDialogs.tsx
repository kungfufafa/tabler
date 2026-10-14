import { Loader2 } from "lucide-react";
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
import { Label } from "@/components/ui/label";
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
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!board) return;
    setPassword("");
    setConfirm("");
    setError(null);
    setBusy(false);
  }, [board]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!board) return;
    if (password !== confirm) {
      setError("The passwords do not match");
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
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>Export Board</DialogTitle>
            <DialogDescription>
              “{board?.name}” with its objects and queries, as a .tablerboard file. A password
              encrypts the file.
            </DialogDescription>
          </DialogHeader>
          <div className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-2">
            <Label htmlFor={`${id}-pw`} className="text-xs">
              Password
            </Label>
            <Input
              id={`${id}-pw`}
              type="password"
              autoComplete="new-password"
              placeholder="Optional"
              value={password}
              className="h-7 text-xs md:text-xs"
              onChange={(e) => {
                setPassword(e.target.value);
                setError(null);
              }}
            />
            <Label htmlFor={`${id}-confirm`} className="text-xs">
              Confirm
            </Label>
            <Input
              id={`${id}-confirm`}
              type="password"
              autoComplete="new-password"
              value={confirm}
              disabled={password === ""}
              className="h-7 text-xs md:text-xs"
              onChange={(e) => {
                setConfirm(e.target.value);
                setError(null);
              }}
            />
          </div>
          {error && (
            <p role="alert" className="text-error text-xs">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              {busy && <Loader2 className="animate-spin" />}
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
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>Import Board</DialogTitle>
            <DialogDescription>This board file is encrypted. Enter its password.</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-pw`} className="text-xs">
              Password
            </Label>
            <Input
              id={`${id}-pw`}
              type="password"
              autoFocus
              value={password}
              aria-invalid={error !== null}
              className="h-7 text-xs md:text-xs"
              onChange={(e) => {
                setPassword(e.target.value);
                setError(null);
              }}
            />
            {error && (
              <p role="alert" className="text-error text-xs">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || password === ""}>
              {busy && <Loader2 className="animate-spin" />}
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
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete “{board?.name}”?</AlertDialogTitle>
          <AlertDialogDescription>
            Its charts, inputs and queries are deleted too. This cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
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
