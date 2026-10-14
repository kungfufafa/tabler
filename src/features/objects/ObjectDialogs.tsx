import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
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
import { invalidateSchema } from "@/features/structure/invalidate";
import { Field } from "@/features/structure/parts";
import type { DdlErrorInfo } from "@/features/structure/previewState";
import { SqlBlock } from "@/features/structure/SqlBlock";
import { KIND_LABEL } from "@/features/structure/tableActions";
import {
  ipc,
  type DdlPlan,
  type Dialect,
  type ObjectOp,
  type RoutineRef,
  type TableRef,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { guardedApply, objectPreviewKey } from "./apply";
import { routineName, stageObjectOp, type StagedObjectOp } from "./pendingOps";
import type { ObjectDialog } from "./store";

function ErrorLine({ error }: { error: DdlErrorInfo | null }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-error selectable text-xs break-words">
      {error.message}
      {error.detail && <span className="block opacity-80">{error.detail}</span>}
      {error.hint && <span className="block opacity-80">{error.hint}</span>}
    </p>
  );
}

interface OptionDef {
  id: string;
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}

/**
 * Confirmation of an object operation: what it acts on, its options, and the SQL it will run
 * (previewed by the backend). The confirm button asks the safe mode, then applies; with `stage`
 * it hands the previewed plan over instead (a pending change, committed later with ⌘S).
 */
function OpDialog({
  connectionId,
  dialect,
  title,
  description,
  names,
  options = [],
  op,
  confirmLabel,
  destructive = true,
  children,
  stage,
  onDone,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  title: string;
  description: ReactNode;
  /** Every object it acts on, listed when there are several. */
  names: readonly string[];
  options?: OptionDef[];
  /** null while the form is invalid. */
  op: ObjectOp | null;
  confirmLabel: string;
  destructive?: boolean;
  children?: ReactNode;
  /** Stages the op (its previewed plan) instead of running it; `onDone` is not called. */
  stage?: (plan: DdlPlan) => void;
  /** After a successful run (also called, with `null`, after a failure that may have changed
   * something, so caches refresh). */
  onDone: (plan: DdlPlan | null) => void;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DdlErrorInfo | null>(null);
  const preview = useQuery({
    queryKey: objectPreviewKey(connectionId, op),
    queryFn: () => ipc.objects.preview(connectionId, op!),
    enabled: op !== null,
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    retry: false,
  });
  // Shown while the plan of the options just changed loads: not what the confirm button runs.
  const stale = preview.isPlaceholderData || preview.isFetching;

  const run = async () => {
    if (!op || busy || stale) return;
    if (stage) {
      // The plan of exactly these options (never the previous options' placeholder).
      const plan = qc.getQueryData<DdlPlan>(objectPreviewKey(connectionId, op));
      if (!plan) return;
      stage(plan);
      onClose();
      return;
    }
    setBusy(true);
    setError(null);
    const outcome = await guardedApply(qc, connectionId, op);
    setBusy(false);
    if (outcome.status === "cancelled") return;
    if (outcome.status === "failed") {
      setError(outcome.error);
      onDone(null);
      return;
    }
    onDone(outcome.plan);
    onClose();
  };

  const sql = preview.data ? preview.data.statements.map((s) => `${s};`).join("\n") : "";
  return (
    // While it runs, Esc / the overlay must not dismiss the dialog (its outcome would be lost).
    <Dialog open onOpenChange={(o) => !o && !busy && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-3 sm:max-w-lg">
        <DialogHeader>
          <DialogTitle className="text-sm">{title}</DialogTitle>
          <DialogDescription className="text-xs">{description}</DialogDescription>
        </DialogHeader>
        {names.length > 1 && (
          <ul
            aria-label="Items"
            className="border-border bg-surface max-h-28 overflow-auto rounded-md border px-2 py-1 font-mono text-[11px]"
          >
            {names.map((n, i) => (
              <li key={`${n}:${i}`} className="truncate">
                {n}
              </li>
            ))}
          </ul>
        )}
        {children}
        {options.length > 0 && (
          <div className="flex flex-col gap-1.5">
            {options.map((o) => (
              <label key={o.id} className="flex items-center gap-2 text-xs">
                <Checkbox checked={o.checked} onCheckedChange={(v) => o.onChange(v === true)} />
                {o.label}
              </label>
            ))}
          </div>
        )}
        <div
          aria-label="SQL preview"
          className="border-border bg-surface-2 max-h-48 min-h-10 overflow-auto rounded-md border px-3 py-2"
        >
          {op === null ? (
            <p className="text-muted-foreground text-[11px]">Fill in the form to see the SQL.</p>
          ) : preview.isError ? (
            <p className="text-error text-xs">{preview.error.message}</p>
          ) : preview.data ? (
            <div aria-busy={stale} className={cn(stale && "opacity-50")}>
              <SqlBlock sql={sql} dialect={dialect} />
            </div>
          ) : (
            <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
          )}
        </div>
        {preview.data?.warnings.map((w) => (
          <p key={w} className="text-warning text-[11px]">
            {w}
          </p>
        ))}
        <ErrorLine error={error} />
        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button
            size="sm"
            variant={destructive ? "destructive" : "default"}
            onClick={() => void run()}
            disabled={busy || op === null || preview.isError || !preview.data || stale}
          >
            {busy && <Loader2 className="animate-spin" />}
            {confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function qualified(t: TableRef): string {
  return t.schema ? `${t.schema}.${t.name}` : t.name;
}

/** "… is pending: ⌘S commits it" (TablePlus stages truncate / drop, spec §2.4). */
function stagedToast(what: string): void {
  const combo = shortcutFor("grid.commit");
  toast.message(`${what} is pending`, {
    description: `Commit it with ${combo ? formatCombo(combo) : "Commit"}, or discard it.`,
  });
}

/** Stages `op` on the connection (on the database it is on now). */
function stageOp(connectionId: string, op: StagedObjectOp, plan: DdlPlan): void {
  const database = useConnectionsStore.getState().byId[connectionId]?.info.currentDatabase ?? null;
  stageObjectOp(connectionId, op, plan.statements, database);
}

function DropTablesDialog({
  connectionId,
  dialect,
  dialog,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  dialog: Extract<ObjectDialog, { kind: "dropTables" }>;
  onClose: () => void;
}) {
  const [cascade, setCascade] = useState(dialog.cascade ?? false);
  const [ignoreFk, setIgnoreFk] = useState(dialog.ignoreForeignKeys ?? false);
  const { tables } = dialog;
  const first = tables[0];
  const views = tables.every((t) => t.kind === "view" || t.kind === "materializedView");
  const title =
    tables.length === 1 && first
      ? `Delete ${KIND_LABEL[first.kind].toLowerCase()} ${first.table.name}?`
      : `Delete ${tables.length} ${views ? "views" : "items"}?`;
  const options: OptionDef[] = [];
  if (dialect === "postgres")
    options.push({
      id: "cascade",
      label: "CASCADE (also drop the objects that depend on them)",
      checked: cascade,
      onChange: setCascade,
    });
  if (dialect === "mysql")
    options.push({
      id: "ignore-fk",
      label: "Ignore foreign key checks",
      checked: ignoreFk,
      onChange: setIgnoreFk,
    });
  const op: StagedObjectOp = {
    op: "dropTables",
    tables,
    cascade: dialect === "postgres" && cascade,
    ignoreForeignKeys: dialect === "mysql" && ignoreFk,
  };
  return (
    <OpDialog
      connectionId={connectionId}
      dialect={dialect}
      title={title}
      description={
        tables.length === 1 && first ? (
          <>
            <span className="font-mono">{qualified(first.table)}</span>
            {views ? " is removed" : " and all of its rows are permanently deleted"} when you
            commit. This cannot be undone.
          </>
        ) : (
          "They are permanently deleted, with all of their rows, when you commit. This cannot be undone."
        )
      }
      names={tables.map((t) => t.table.name)}
      options={options}
      op={op}
      confirmLabel="Delete"
      stage={(plan) => {
        stageOp(connectionId, op, plan);
        stagedToast(
          tables.length === 1 && first
            ? `Delete ${first.table.name}`
            : `Delete ${tables.length} items`,
        );
      }}
      onDone={() => undefined}
      onClose={onClose}
    />
  );
}

function TruncateTablesDialog({
  connectionId,
  dialect,
  tables,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  tables: TableRef[];
  onClose: () => void;
}) {
  const [cascade, setCascade] = useState(false);
  const [restart, setRestart] = useState(false);
  const [ignoreFk, setIgnoreFk] = useState(false);
  const first = tables[0];
  const options: OptionDef[] = [];
  if (dialect === "postgres")
    options.push(
      {
        id: "restart",
        label: "Restart identity (reset the tables' sequences)",
        checked: restart,
        onChange: setRestart,
      },
      {
        id: "cascade",
        label: "CASCADE (also truncate the tables that reference them)",
        checked: cascade,
        onChange: setCascade,
      },
    );
  if (dialect === "mysql")
    options.push({
      id: "ignore-fk",
      label: "Ignore foreign key checks",
      checked: ignoreFk,
      onChange: setIgnoreFk,
    });
  const op: StagedObjectOp = {
    op: "truncateTables",
    tables,
    cascade: dialect === "postgres" && cascade,
    restartIdentity: dialect === "postgres" && restart,
    ignoreForeignKeys: dialect === "mysql" && ignoreFk,
  };
  return (
    <OpDialog
      connectionId={connectionId}
      dialect={dialect}
      title={
        tables.length === 1 && first
          ? `Truncate ${first.name}?`
          : `Truncate ${tables.length} tables?`
      }
      description={
        tables.length === 1 && first ? (
          <>
            Every row of <span className="font-mono">{qualified(first)}</span> is deleted when you
            commit. This cannot be undone.
          </>
        ) : (
          "Every row of these tables is deleted when you commit. This cannot be undone."
        )
      }
      names={tables.map((t) => t.name)}
      options={options}
      op={op}
      confirmLabel="Truncate"
      stage={(plan) => {
        stageOp(connectionId, op, plan);
        stagedToast(
          tables.length === 1 && first
            ? `Truncate ${first.name}`
            : `Truncate ${tables.length} tables`,
        );
      }}
      onDone={() => undefined}
      onClose={onClose}
    />
  );
}

function qualifiedRoutine(r: RoutineRef): string {
  return r.schema ? `${r.schema}.${routineName(r)}` : routineName(r);
}

function DropRoutinesDialog({
  connectionId,
  dialect,
  dialog,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  dialog: Extract<ObjectDialog, { kind: "dropRoutines" }>;
  onClose: () => void;
}) {
  const [cascade, setCascade] = useState(false);
  const { routines, names } = dialog;
  const first = routines[0];
  const what = (k: (typeof routines)[number]["kind"]) =>
    k === "procedure" ? "procedure" : "function";
  const op: StagedObjectOp = {
    op: "dropRoutines",
    routines,
    cascade: dialect === "postgres" && cascade,
  };
  return (
    <OpDialog
      connectionId={connectionId}
      dialect={dialect}
      title={
        routines.length === 1 && first
          ? `Delete ${what(first.kind)} ${first.name}?`
          : `Delete ${routines.length} routines?`
      }
      description={
        routines.length === 1 && first ? (
          <>
            <span className="font-mono">{qualifiedRoutine(first)}</span> is dropped when you commit.
            This cannot be undone.
          </>
        ) : (
          "They are dropped when you commit. This cannot be undone."
        )
      }
      names={names}
      options={
        dialect === "postgres"
          ? [
              {
                id: "cascade",
                label: "CASCADE (also drop the objects that depend on them)",
                checked: cascade,
                onChange: setCascade,
              },
            ]
          : []
      }
      op={op}
      confirmLabel="Delete"
      stage={(plan) => {
        stageOp(connectionId, op, plan);
        stagedToast(
          routines.length === 1 && first
            ? `Delete ${first.name}`
            : `Delete ${routines.length} routines`,
        );
      }}
      onDone={() => undefined}
      onClose={onClose}
    />
  );
}

function DropSequencesDialog({
  connectionId,
  dialect,
  sequences,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  sequences: TableRef[];
  onClose: () => void;
}) {
  const [cascade, setCascade] = useState(false);
  const first = sequences[0];
  const op: StagedObjectOp = { op: "dropSequences", sequences, cascade };
  return (
    <OpDialog
      connectionId={connectionId}
      dialect={dialect}
      title={
        sequences.length === 1 && first
          ? `Delete sequence ${first.name}?`
          : `Delete ${sequences.length} sequences?`
      }
      description={
        sequences.length === 1 && first ? (
          <>
            <span className="font-mono">{qualified(first)}</span> is dropped when you commit. This
            cannot be undone.
          </>
        ) : (
          "They are dropped when you commit. This cannot be undone."
        )
      }
      names={sequences.map((s) => s.name)}
      options={[
        {
          id: "cascade",
          label: "CASCADE (also drop the column defaults that use them)",
          checked: cascade,
          onChange: setCascade,
        },
      ]}
      op={op}
      confirmLabel="Delete"
      stage={(plan) => {
        stageOp(connectionId, op, plan);
        stagedToast(
          sequences.length === 1 && first
            ? `Delete ${first.name}`
            : `Delete ${sequences.length} sequences`,
        );
      }}
      onDone={() => undefined}
      onClose={onClose}
    />
  );
}

function CloneTableDialog({
  connectionId,
  dialect,
  dialog,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  dialog: Extract<ObjectDialog, { kind: "clone" }>;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const id = useId();
  const source = dialog.table;
  const [name, setName] = useState(`${source.name}_copy`);
  const [withData, setWithData] = useState(true);
  const trimmed = name.trim();
  const valid = trimmed !== "" && trimmed !== source.name;
  const ref: TableRef = { schema: source.schema, name: source.name };
  return (
    <OpDialog
      connectionId={connectionId}
      dialect={dialect}
      title={`Clone ${source.name}`}
      description={
        <>
          Copies <span className="font-mono">{qualified(ref)}</span> into a new table of the same
          schema.
        </>
      }
      names={[]}
      op={valid ? { op: "cloneTable", source: ref, newName: trimmed, withData } : null}
      confirmLabel="Clone"
      destructive={false}
      onDone={(plan) => {
        invalidateSchema(qc, connectionId);
        if (plan) toast.success(`Cloned ${source.name} to ${trimmed}`);
      }}
      onClose={onClose}
    >
      <Field label="New table name" htmlFor={`${id}-name`}>
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
      <div role="radiogroup" aria-label="What to copy" className="flex flex-col gap-1.5 text-xs">
        {(
          [
            [false, "Structure only"],
            [true, "Structure and data"],
          ] as const
        ).map(([value, label]) => (
          <label key={label} className="flex items-center gap-2">
            <input
              type="radio"
              name={`${id}-what`}
              checked={withData === value}
              onChange={() => setWithData(value)}
              className={cn("accent-primary size-3.5")}
            />
            {label}
          </label>
        ))}
      </div>
    </OpDialog>
  );
}

/** The object dialog of `dialog` (table create / rename go through the structure dialogs). */
export function ObjectDialogView({
  connectionId,
  dialect,
  dialog,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  dialog: ObjectDialog;
  onClose: () => void;
}) {
  switch (dialog.kind) {
    case "dropTables":
      return (
        <DropTablesDialog
          connectionId={connectionId}
          dialect={dialect}
          dialog={dialog}
          onClose={onClose}
        />
      );
    case "truncateTables":
      return (
        <TruncateTablesDialog
          connectionId={connectionId}
          dialect={dialect}
          tables={dialog.tables}
          onClose={onClose}
        />
      );
    case "dropRoutines":
      return (
        <DropRoutinesDialog
          connectionId={connectionId}
          dialect={dialect}
          dialog={dialog}
          onClose={onClose}
        />
      );
    case "dropSequences":
      return (
        <DropSequencesDialog
          connectionId={connectionId}
          dialect={dialect}
          sequences={dialog.sequences}
          onClose={onClose}
        />
      );
    case "clone":
      return (
        <CloneTableDialog
          connectionId={connectionId}
          dialect={dialect}
          dialog={dialog}
          onClose={onClose}
        />
      );
    default:
      return null;
  }
}
