import { useEffect, useId, useState } from "react";
import { toast } from "sonner";
import { FormField } from "@/components/FormField";
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
      <DialogContent className="sm:max-w-[34rem]">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle className="text-sm">{shown?.title}</DialogTitle>
            <DialogDescription className="text-xs">
              {plural(count, "connection")} with their groups, tags, colors and safe mode, as a
              .tablerconnections file.
            </DialogDescription>
          </DialogHeader>
          <div className="flex flex-col">
            <div className="flex flex-col">
              <label className="flex h-7 items-center gap-2 text-xs">
                <Checkbox
                  checked={passwords}
                  aria-describedby={`${id}-passwords-hint`}
                  onCheckedChange={(v) => {
                    setPasswords(v === true);
                    setError(null);
                  }}
                />
                Include database passwords
              </label>
              <p
                id={`${id}-passwords-hint`}
                className="text-muted-foreground pl-6 text-xs leading-4"
              >
                Also includes IAM password commands.
              </p>
            </div>
            <label className="flex h-7 items-center gap-2 text-xs">
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
          <FormField
            label="File password"
            htmlFor={`${id}-password`}
            error={error}
            hint={
              secrets
                ? "Required: it encrypts the whole file. Importing asks for it."
                : "With a password the whole file is encrypted."
            }
          >
            <Input
              id={`${id}-password`}
              type="password"
              value={filePassword}
              onChange={(e) => {
                setFilePassword(e.target.value);
                setError(null);
              }}
              autoComplete="new-password"
              placeholder={secrets ? undefined : "Optional"}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? `${id}-password-error` : `${id}-password-hint`}
              className="h-7 rounded-md font-mono text-xs md:text-xs"
            />
          </FormField>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || count === 0}>
              Export…
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
