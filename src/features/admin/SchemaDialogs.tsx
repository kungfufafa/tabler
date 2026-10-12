import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { useId, useState, type FormEvent } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import { useSchemas } from "@/features/sidebar/useSchemaTree";
import { ipc } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";
import { nameError } from "./databaseForm";
import { adminKeys } from "./keys";
import { ErrorLine, Field, NativeSelect } from "./parts";
import { runAdminDdl, type AdminErrorInfo } from "./run";

/** New Schema… (PostgreSQL): name and an optional owner (`AUTHORIZATION`). */
export function CreateSchemaDialog({
  connectionId,
  currentUser,
  onClose,
}: {
  connectionId: string;
  currentUser: string | null;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const schemas = useSchemas(connectionId);
  // Owner suggestions; a user without access to the role list simply gets none.
  const roles = useQuery({
    queryKey: adminKeys.userList(connectionId, true),
    queryFn: () => ipc.admin.users(connectionId, true),
    staleTime: 60_000,
    retry: false,
  });
  const [name, setName] = useState("");
  const [owner, setOwner] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AdminErrorInfo | null>(null);
  const problem = nameError(
    name,
    (schemas.data ?? []).map((s) => s.name),
    "schema",
  );

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setSubmitted(true);
    if (problem || busy) return;
    setBusy(true);
    setError(null);
    const schema = name.trim();
    const outcome = await runAdminDdl(qc, connectionId, {
      op: "createSchema",
      name: schema,
      owner: owner.trim() === "" ? null : owner.trim(),
    });
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") return setError(outcome.error);
    toast.success(`Created schema ${schema}`, {
      action: {
        label: "Show",
        onClick: () => useConnectionsStore.getState().setCurrentSchema(connectionId, schema),
      },
    });
    onClose();
  };

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle className="text-sm">New Schema</DialogTitle>
            <DialogDescription className="text-xs">
              Created in the database this connection is using.
            </DialogDescription>
          </DialogHeader>
          <Field
            label="Name"
            htmlFor={`${id}-name`}
            hint={submitted && problem ? <span className="text-error">{problem}</span> : null}
          >
            <Input
              id={`${id}-name`}
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="new_schema"
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
          <Field label="Owner (optional)" htmlFor={`${id}-owner`}>
            <Input
              id={`${id}-owner`}
              value={owner}
              list={`${id}-roles`}
              onChange={(e) => setOwner(e.target.value)}
              placeholder={currentUser ?? "current user"}
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
            <datalist id={`${id}-roles`}>
              {(roles.data ?? []).map((r) => (
                <option key={r.name} value={r.name} />
              ))}
            </datalist>
          </Field>
          <ErrorLine error={error} />
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              {busy && <Loader2 className="animate-spin" />}
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Drop Schema… (PostgreSQL), with CASCADE. Without a schema, the dialog asks which. */
export function DropSchemaDialog({
  connectionId,
  schema,
  onClose,
}: {
  connectionId: string;
  schema: string | null;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const schemas = useSchemas(connectionId);
  const [picked, setPicked] = useState(schema ?? "");
  const [cascade, setCascade] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AdminErrorInfo | null>(null);
  const target = schema ?? picked;

  const run = async () => {
    if (target === "" || busy) return;
    setBusy(true);
    setError(null);
    const outcome = await runAdminDdl(qc, connectionId, {
      op: "dropSchema",
      name: target,
      cascade,
    });
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") return setError(outcome.error);
    toast.success(`Dropped schema ${target}`);
    onClose();
  };

  return (
    <AlertDialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">
            {target ? `Drop schema ${target}?` : "Drop Schema"}
          </AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            {cascade
              ? "The schema and every object in it (tables, views, functions…) are permanently deleted."
              : "Only an empty schema can be dropped without CASCADE."}{" "}
            This cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        {schema === null && (
          <Field label="Schema" htmlFor={`${id}-schema`}>
            <NativeSelect
              id={`${id}-schema`}
              value={picked}
              onChange={(e) => setPicked(e.target.value)}
            >
              <option value="" disabled>
                {schemas.isPending ? "Loading…" : "Choose a schema"}
              </option>
              {(schemas.data ?? []).map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                </option>
              ))}
            </NativeSelect>
          </Field>
        )}
        <label className="flex items-center gap-2 text-xs">
          <Checkbox checked={cascade} onCheckedChange={(v) => setCascade(v === true)} />
          CASCADE (also drop every object in the schema)
        </label>
        <ErrorLine error={error} />
        <AlertDialogFooter>
          <AlertDialogCancel size="sm" disabled={busy}>
            Cancel
          </AlertDialogCancel>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => void run()}
            disabled={busy || target === ""}
          >
            {busy && <Loader2 className="animate-spin" />}
            Drop
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
