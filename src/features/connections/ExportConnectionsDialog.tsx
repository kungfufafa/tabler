import { Loader2 } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
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
import { ipc, toIpcError } from "@/lib/ipc";
import { pickExportPath, type ExportScope } from "./ioStore";

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/**
 * "Export Connections ▸ …": choose whether database passwords and SSH secrets go along, and a
 * file password (required with secrets; it encrypts the whole file), then where to save the
 * `.tablerconnections` file.
 */
export function ExportConnectionsDialog({
  scope,
  onClose,
}: {
  scope: ExportScope | null;
  onClose: () => void;
}) {
  // Keep showing the last scope while the dialog animates out.
  const [last, setLast] = useState(scope);
  if (scope && scope !== last) setLast(scope);
  const shown = scope ?? last;
  const id = useId();
  const [passwords, setPasswords] = useState(false);
  const [sshSecrets, setSshSecrets] = useState(false);
  const [filePassword, setFilePassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!scope) return;
    setPasswords(false);
    setSshSecrets(false);
    setFilePassword("");
    setError(null);
    setBusy(false);
  }, [scope]);

  const secrets = passwords || sshSecrets;
  const count = shown?.profileIds.length ?? 0;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!scope) return;
    if (secrets && filePassword === "") {
      setError("Set a file password to include passwords");
      return;
    }
    const path = (await pickExportPath(scope.fileName))?.trim();
    if (!path) return;
    setBusy(true);
    try {
      const n = await ipc.profiles.exportConnections({
        profileIds: scope.profileIds,
        groups: scope.groups,
        includePasswords: passwords,
        includeSshSecrets: sshSecrets,
        filePassword: filePassword === "" ? null : filePassword,
        path,
      });
      toast.success(`Exported ${plural(n, "connection")}`, { description: path });
      onClose();
    } catch (err) {
      setError(toIpcError(err).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={scope !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{shown?.title}</DialogTitle>
            <DialogDescription>
              {plural(count, "connection")} with their groups, tags, colours and safe mode, as a
              .tablerconnections file.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-2.5">
            <label className="flex items-center gap-2 text-[13px]">
              <Checkbox
                checked={passwords}
                onCheckedChange={(v) => {
                  setPasswords(v === true);
                  setError(null);
                }}
              />
              Include database passwords
            </label>
            <label className="flex items-center gap-2 text-[13px]">
              <Checkbox
                checked={sshSecrets}
                onCheckedChange={(v) => {
                  setSshSecrets(v === true);
                  setError(null);
                }}
              />
              Include SSH passwords and passphrases
            </label>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={`${id}-password`} className="text-xs font-normal">
              File password{secrets ? "" : " (optional)"}
            </Label>
            <Input
              id={`${id}-password`}
              type="password"
              value={filePassword}
              onChange={(e) => {
                setFilePassword(e.target.value);
                setError(null);
              }}
              autoComplete="new-password"
              aria-invalid={error ? true : undefined}
              className="font-mono"
            />
            {error ? (
              <p role="alert" className="text-error text-[11px]">
                {error}
              </p>
            ) : (
              <p className="text-muted-foreground text-[11px]">
                {secrets
                  ? "Required: it encrypts the whole file. Importing asks for it."
                  : "With a password the whole file is encrypted."}
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || count === 0}>
              {busy ? <Loader2 className="animate-spin" /> : null}
              Export…
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
