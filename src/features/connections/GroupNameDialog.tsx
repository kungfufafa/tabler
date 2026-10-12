import { useEffect, useId, useState } from "react";
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
import { groupNameError } from "./groups";
import type { GroupDialogRequest } from "./ioStore";

/**
 * "New Group…" / "Rename Group…": one name field, checked against the existing groups (names
 * are compared without case).
 */
export function GroupNameDialog({
  request,
  groups,
  onSubmit,
  onClose,
}: {
  request: GroupDialogRequest | null;
  /** Every existing group name. */
  groups: readonly string[];
  onSubmit: (request: GroupDialogRequest, name: string) => Promise<boolean>;
  onClose: () => void;
}) {
  const [last, setLast] = useState(request);
  if (request && request !== last) setLast(request);
  const shown = request ?? last;
  const id = useId();
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!request) return;
    setName(request.kind === "rename" ? request.group : "");
    setError(null);
    setBusy(false);
  }, [request]);

  const current = shown?.kind === "rename" ? shown.group : null;
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!request) return;
    const problem = groupNameError(name, groups, current);
    if (problem) {
      setError(problem);
      return;
    }
    if (current !== null && name.trim() === current) {
      onClose();
      return;
    }
    setBusy(true);
    const ok = await onSubmit(request, name.trim());
    setBusy(false);
    if (ok) onClose();
  };

  return (
    <Dialog open={request !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{shown?.kind === "rename" ? "Rename Group" : "New Group"}</DialogTitle>
            <DialogDescription>
              {shown?.kind === "rename"
                ? `The connections of “${shown.group}” move with it.`
                : "Groups sort connections on the welcome screen. Drag connections onto a group to move them."}
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1">
            <Input
              id={`${id}-name`}
              aria-label="Group name"
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                setError(null);
              }}
              aria-invalid={error ? true : undefined}
              placeholder="Group name"
              autoComplete="off"
              autoFocus
            />
            {error && (
              <p role="alert" className="text-error text-[11px]">
                {error}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              {shown?.kind === "rename" ? "Rename" : "Create"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
