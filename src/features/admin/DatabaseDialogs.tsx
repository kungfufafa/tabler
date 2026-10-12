import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, Loader2 } from "lucide-react";
import { useId, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { Kbd } from "@/components/Kbd";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { switchDatabase } from "@/features/sidebar/switchDatabase";
import { useDatabases } from "@/features/sidebar/useSchemaTree";
import { SqlBlock } from "@/features/structure/SqlBlock";
import { ipc, type Dialect, type Driver } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  collationsFor,
  createDatabaseOp,
  currentDatabaseRefusal,
  defaultCollationOf,
  effectiveTemplate,
  emptyCreateForm,
  nameError,
  type CreateDatabaseForm,
} from "./databaseForm";
import { adminKeys } from "./keys";
import { ErrorLine, Field, NativeSelect } from "./parts";
import { errorInfo, runAdminDdl, type AdminErrorInfo } from "./run";

const dialectOf = (driver: Driver): Dialect => driver;

/** "+" / New Database… of the ⌘K database list: name, encoding, collation (and PG template). */
export function CreateDatabaseDialog({
  connectionId,
  driver,
  onClose,
}: {
  connectionId: string;
  driver: Driver;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const options = useQuery({
    queryKey: adminKeys.databaseOptions(connectionId),
    queryFn: () => ipc.admin.databaseOptions(connectionId),
    staleTime: 10 * 60_000,
    retry: false,
  });
  const databases = useDatabases(connectionId);
  const [form, setForm] = useState<CreateDatabaseForm>(() => emptyCreateForm(options.data));
  const [submitted, setSubmitted] = useState(false);
  const [showSql, setShowSql] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AdminErrorInfo | null>(null);

  // Options arriving after the first render preselect the default encoding.
  const encoding = form.encoding || options.data?.defaultEncoding || "";
  const current = { ...form, encoding };
  const collations = useMemo(() => collationsFor(options.data, encoding), [options.data, encoding]);
  const existing = useMemo(() => (databases.data ?? []).map((d) => d.name), [databases.data]);
  const problem = nameError(form.name, existing, "database");
  const op = createDatabaseOp(driver, options.data, current);
  const template = effectiveTemplate(driver, options.data, current);
  const autoTemplate0 = driver === "postgres" && form.template === null && template !== "";
  const defaultCollation =
    driver === "mysql"
      ? defaultCollationOf(options.data, encoding)
      : options.data?.defaultCollation;

  const preview = useQuery({
    queryKey: ["adminPreviewDdl", connectionId, op],
    queryFn: () => ipc.admin.previewDdl(connectionId, op),
    enabled: showSql && problem === null,
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    retry: false,
  });

  const update = (patch: Partial<CreateDatabaseForm>) => setForm((f) => ({ ...f, ...patch }));
  const setEncoding = (next: string) => {
    // A collation of another character set no longer applies.
    const keep = collationsFor(options.data, next).some((c) => c.name === form.collation);
    update({ encoding: next, collation: keep ? form.collation : "" });
  };

  const create = async () => {
    setSubmitted(true);
    if (problem || busy) return;
    setBusy(true);
    setError(null);
    const outcome = await runAdminDdl(qc, connectionId, op);
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") {
      setError(outcome.error);
      return;
    }
    toast.success(`Created database ${op.name}`, {
      action: {
        label: "Open",
        onClick: () => void switchDatabase(qc, connectionId, op.name),
      },
    });
    onClose();
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void create();
    }
  };

  const loading = options.isPending;
  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-md" onKeyDown={onKeyDown}>
        <form
          onSubmit={(e: FormEvent) => {
            e.preventDefault();
            void create();
          }}
          className="flex flex-col gap-3"
        >
          <DialogHeader>
            <DialogTitle className="text-sm">New Database</DialogTitle>
            <DialogDescription className="text-xs">
              Created on the server of this connection.
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
              value={form.name}
              onChange={(e) => update({ name: e.target.value })}
              placeholder="new_database"
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label={driver === "mysql" ? "Character set" : "Encoding"} htmlFor={`${id}-enc`}>
              <NativeSelect
                id={`${id}-enc`}
                value={encoding}
                disabled={loading}
                onChange={(e) => setEncoding(e.target.value)}
              >
                {loading && <option value="">Loading…</option>}
                {options.data?.encodings.map((e) => (
                  <option key={e.name} value={e.name}>
                    {e.description ? `${e.name} — ${e.description}` : e.name}
                  </option>
                ))}
              </NativeSelect>
            </Field>
            <Field label="Collation" htmlFor={`${id}-coll`}>
              <NativeSelect
                id={`${id}-coll`}
                value={form.collation}
                disabled={loading}
                onChange={(e) => update({ collation: e.target.value })}
              >
                <option value="">
                  {defaultCollation ? `Default (${defaultCollation})` : "Default"}
                </option>
                {collations.map((c) => (
                  <option key={`${c.name}\u0000${c.encoding ?? ""}`} value={c.name}>
                    {c.name}
                  </option>
                ))}
              </NativeSelect>
            </Field>
          </div>
          {driver === "postgres" && (
            <Field
              label="Template"
              htmlFor={`${id}-tpl`}
              hint={
                autoTemplate0
                  ? "template0: template1 only allows its own encoding and collation."
                  : undefined
              }
            >
              <NativeSelect
                id={`${id}-tpl`}
                value={template}
                disabled={loading}
                onChange={(e) => update({ template: e.target.value })}
              >
                <option value="">Default (template1)</option>
                {options.data?.templates.map((t) => (
                  <option key={t} value={t}>
                    {t}
                  </option>
                ))}
              </NativeSelect>
            </Field>
          )}
          {options.isError && <ErrorLine error={errorInfo(options.error)} />}
          <div className="flex items-center">
            <Button
              type="button"
              variant="ghost"
              size="xs"
              aria-expanded={showSql}
              onClick={() => setShowSql((v) => !v)}
            >
              <ChevronRight className={cn("transition-transform", showSql && "rotate-90")} />
              Preview SQL
            </Button>
          </div>
          {showSql && (
            <div className="border-border bg-surface-2 max-h-40 min-h-10 overflow-auto rounded-md border px-3 py-2">
              {problem ? (
                <p className="text-muted-foreground text-[11px]">{problem}</p>
              ) : preview.isError ? (
                <ErrorLine error={errorInfo(preview.error)} />
              ) : preview.data ? (
                <SqlBlock sql={preview.data.join(";\n")} dialect={dialectOf(driver)} />
              ) : (
                <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
              )}
            </div>
          )}
          <ErrorLine error={error} />
          <DialogFooter className="items-center">
            <Button type="button" variant="ghost" size="sm" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy}>
              {busy && <Loader2 className="animate-spin" />}
              Create
              <Kbd
                combo="Mod+Enter"
                className="border-primary-foreground/30 ml-1 bg-transparent text-inherit"
              />
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** ⌘K ▸ right-click ▸ Rename… (PostgreSQL). */
export function RenameDatabaseDialog({
  connectionId,
  database,
  currentDatabase,
  onClose,
}: {
  connectionId: string;
  database: string;
  currentDatabase: string | null;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const databases = useDatabases(connectionId);
  const [name, setName] = useState(database);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AdminErrorInfo | null>(null);
  const isCurrent = database === currentDatabase;
  const existing = (databases.data ?? []).map((d) => d.name).filter((n) => n !== database);
  const problem =
    name.trim() === database ? "Enter a new name" : nameError(name, existing, "database");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (problem || busy || isCurrent) return;
    setBusy(true);
    setError(null);
    const newName = name.trim();
    const outcome = await runAdminDdl(qc, connectionId, {
      op: "renameDatabase",
      name: database,
      newName,
    });
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") return setError(outcome.error);
    toast.success(`Renamed database ${database} to ${newName}`);
    onClose();
  };

  return (
    <Dialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle className="text-sm">Rename Database</DialogTitle>
            <DialogDescription className="font-mono text-xs">{database}</DialogDescription>
          </DialogHeader>
          {isCurrent ? (
            <ErrorLine error={currentDatabaseRefusal(database, "rename")} />
          ) : (
            <Field label="New name" htmlFor={`${id}-name`}>
              <Input
                id={`${id}-name`}
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                onFocus={(e) => e.currentTarget.select()}
                className="h-7 font-mono text-xs md:text-xs"
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
          )}
          <ErrorLine error={error} />
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || isCurrent || problem !== null}>
              {busy && <Loader2 className="animate-spin" />}
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** ⌘K ▸ right-click ▸ Drop… / "−": confirmed by typing the database name. */
export function DropDatabaseDialog({
  connectionId,
  database,
  currentDatabase,
  onClose,
}: {
  connectionId: string;
  /** Without one, the dialog lists the databases to pick from. */
  database: string | null;
  currentDatabase: string | null;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const databases = useDatabases(connectionId);
  const choices = (databases.data ?? []).map((d) => d.name).filter((n) => n !== currentDatabase);
  const [picked, setPicked] = useState<string>(database ?? "");
  const target = database ?? picked;
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AdminErrorInfo | null>(null);
  const confirmRef = useRef<HTMLInputElement>(null);
  const isCurrent = target !== "" && target === currentDatabase;
  const confirmed = target !== "" && typed === target && !isCurrent;

  const run = async () => {
    if (!confirmed || busy) return;
    setBusy(true);
    setError(null);
    const outcome = await runAdminDdl(qc, connectionId, { op: "dropDatabase", name: target });
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") return setError(outcome.error);
    toast.success(`Dropped database ${target}`);
    onClose();
  };

  return (
    <AlertDialog open onOpenChange={(open) => !open && !busy && onClose()}>
      <AlertDialogContent
        onOpenAutoFocus={(e) => {
          // Start in the confirmation field (AlertDialog focuses Cancel by default).
          if (database === null) return;
          e.preventDefault();
          confirmRef.current?.focus();
        }}
      >
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void run();
          }}
          className="grid gap-4"
        >
          <AlertDialogHeader>
            <AlertDialogTitle className="text-sm">
              {target ? `Drop database ${target}?` : "Drop Database"}
            </AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              The database and everything in it are permanently deleted. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {database === null && (
            <Field label="Database" htmlFor={`${id}-db`}>
              <NativeSelect
                id={`${id}-db`}
                value={picked}
                onChange={(e) => {
                  setPicked(e.target.value);
                  setTyped("");
                }}
              >
                <option value="" disabled>
                  {databases.isPending ? "Loading…" : "Choose a database"}
                </option>
                {choices.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </NativeSelect>
            </Field>
          )}
          {isCurrent ? (
            <ErrorLine error={currentDatabaseRefusal(target, "drop")} />
          ) : (
            <Field label={`Type ${target || "the name"} to confirm`} htmlFor={`${id}-confirm`}>
              <Input
                id={`${id}-confirm`}
                ref={confirmRef}
                value={typed}
                disabled={target === ""}
                onChange={(e) => setTyped(e.target.value)}
                placeholder={target}
                className="h-7 font-mono text-xs md:text-xs"
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
          )}
          <ErrorLine error={error} />
          <AlertDialogFooter>
            <AlertDialogCancel type="button" size="sm" disabled={busy}>
              Cancel
            </AlertDialogCancel>
            <Button type="submit" variant="destructive" size="sm" disabled={!confirmed || busy}>
              {busy && <Loader2 className="animate-spin" />}
              Drop
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  );
}
