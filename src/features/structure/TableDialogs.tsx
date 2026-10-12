import { keepPreviousData, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, ChevronRight, Loader2, Plus, Trash2 } from "lucide-react";
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent,
} from "react";
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
import { guardExecution } from "@/features/safety/guard";
import { ipc, type DdlOp, type Dialect, type TableRef, type TableSummary } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useTabsStore } from "@/stores/tabs";
import { supportsComments, typeSuggestions } from "./columnEdits";
import {
  buildCreateTable,
  initialColumns,
  moveDraft,
  newColumnDraft,
  type ColumnDraft,
} from "./createTable";
import { invalidateSchema } from "./invalidate";
import { Field, IconAction } from "./parts";
import { errorInfo, payloadInfo, type DdlErrorInfo } from "./previewState";
import { followRename, tabsOf } from "./rename";
import { SqlBlock } from "./SqlBlock";
import { useStructureStore } from "./store";
import {
  dropTableOp,
  dropViewOp,
  isViewKind,
  KIND_LABEL,
  refOf,
  renameTableOp,
  tableNameError,
  truncateTableOp,
} from "./tableActions";

/** Which table dialog the sidebar context menu opened. */
export type TableDialogState =
  | { kind: "create"; schema: string | null }
  | { kind: "rename"; table: TableSummary }
  | { kind: "truncate"; table: TableSummary }
  | { kind: "drop"; table: TableSummary };

/** `error` null: the safe-mode check was declined (nothing ran). */
type RunResult = { ok: true; durationMs: number } | { ok: false; error: DdlErrorInfo | null };

/**
 * Applies one op after the connection's safe-mode check (on the statements the op plans to);
 * the DDL result's error and a rejected call both come back as `error`, a declined check as
 * `cancelled`.
 */
async function runOne(connectionId: string, table: TableRef | null, op: DdlOp): Promise<RunResult> {
  try {
    const plan = await ipc.ddl.preview(connectionId, table, [op]);
    if (!(await guardExecution(connectionId, { kind: "commit", statements: plan.statements })))
      return { ok: false, error: null };
    const r = await ipc.ddl.apply(connectionId, table, [op]);
    return r.error
      ? { ok: false, error: payloadInfo(r.error) }
      : { ok: true, durationMs: r.durationMs };
  } catch (e) {
    return { ok: false, error: errorInfo(e) };
  }
}

function qualified(t: TableRef): string {
  return t.schema ? `${t.schema}.${t.name}` : t.name;
}

function ErrorLine({ error }: { error: DdlErrorInfo | null }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-error selectable text-xs break-words">
      {error.message}
      {error.detail && <span className="block opacity-80">{error.detail}</span>}
    </p>
  );
}

/** Mounts the dialog matching `state` (one at a time). */
export function TableDialogs({
  connectionId,
  dialect,
  state,
  onClose,
}: {
  connectionId: string;
  dialect: Dialect;
  state: TableDialogState | null;
  onClose: () => void;
}) {
  const onOpenChange = (open: boolean) => {
    if (!open) onClose();
  };
  return (
    <>
      <CreateTableDialog
        open={state?.kind === "create"}
        onOpenChange={onOpenChange}
        connectionId={connectionId}
        dialect={dialect}
        schema={state?.kind === "create" ? state.schema : null}
      />
      {state?.kind === "rename" && (
        <RenameTableDialog
          key={`rename:${state.table.name}`}
          onOpenChange={onOpenChange}
          connectionId={connectionId}
          dialect={dialect}
          table={state.table}
        />
      )}
      {state?.kind === "truncate" && (
        <TruncateTableDialog
          key={`truncate:${state.table.name}`}
          onOpenChange={onOpenChange}
          connectionId={connectionId}
          dialect={dialect}
          table={state.table}
        />
      )}
      {state?.kind === "drop" && (
        <DropTableDialog
          key={`drop:${state.table.name}`}
          onOpenChange={onOpenChange}
          connectionId={connectionId}
          dialect={dialect}
          table={state.table}
        />
      )}
    </>
  );
}

// --- create ------------------------------------------------------------------------------------

export function CreateTableDialog({
  open,
  onOpenChange,
  connectionId,
  dialect,
  schema,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  dialect: Dialect;
  schema: string | null;
}) {
  const qc = useQueryClient();
  const openTable = useTabsStore((s) => s.openTable);
  const id = useId();
  const gridRef = useRef<HTMLTableSectionElement>(null);
  const focusRow = useRef<string | null>(null);
  const [name, setName] = useState("");
  const [rows, setRows] = useState<ColumnDraft[]>(() => initialColumns(dialect));
  const [showSql, setShowSql] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DdlErrorInfo | null>(null);

  useEffect(() => {
    if (!open) return;
    setName("");
    setRows(initialColumns(dialect));
    setShowSql(false);
    setSubmitted(false);
    setBusy(false);
    setError(null);
  }, [open, dialect]);

  useEffect(() => {
    const rowId = focusRow.current;
    if (!rowId) return;
    focusRow.current = null;
    gridRef.current?.querySelector<HTMLInputElement>(`[data-draft="${rowId}"]`)?.focus();
  }, [rows]);

  const result = useMemo(
    () => buildCreateTable(dialect, schema, name, rows),
    [dialect, schema, name, rows],
  );
  const preview = useQuery({
    queryKey: ["ddlPreview", connectionId, result.ok ? result.op : null],
    queryFn: () => {
      const op = (result as Extract<typeof result, { ok: true }>).op;
      return ipc.ddl.preview(connectionId, op.table, [op]);
    },
    enabled: open && showSql && result.ok,
    placeholderData: keepPreviousData,
    staleTime: Infinity,
    retry: false,
  });

  const update = (rowId: string, patch: Partial<ColumnDraft>) =>
    setRows((rs) =>
      rs.map((r) => {
        if (r.id !== rowId) return r;
        const next = { ...r, ...patch };
        if (patch.primaryKey === true) next.nullable = false;
        return next;
      }),
    );
  const addRow = () => {
    const row = newColumnDraft(dialect);
    focusRow.current = row.id;
    setRows((rs) => [...rs, row]);
  };

  const create = async () => {
    setSubmitted(true);
    if (!result.ok || busy) return;
    setBusy(true);
    setError(null);
    const r = await runOne(connectionId, result.op.table, result.op);
    setBusy(false);
    if (!r.ok) {
      setError(r.error);
      return;
    }
    toast.success(`Created table ${result.op.table.name}`);
    invalidateSchema(qc, connectionId);
    onOpenChange(false);
    openTable(connectionId, result.op.table, "structure");
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void create();
    }
  };
  const comments = supportsComments(dialect);
  const errors = submitted && !result.ok ? result.errors : [];
  const cell = "h-7 rounded-sm px-1.5 text-xs md:text-xs";

  return (
    // While CREATE runs, Esc / the overlay / the close button must not dismiss the dialog: its
    // outcome (an error to show, or the new table's tab) would land in a closed dialog.
    <Dialog open={open} onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent
        className="flex max-h-[85vh] flex-col gap-3 sm:max-w-3xl"
        onKeyDown={onKeyDown}
      >
        <DialogHeader>
          <DialogTitle className="text-sm">New table</DialogTitle>
          <DialogDescription className="text-xs">
            {schema ? (
              <>
                In <span className="font-mono">{schema}</span>.{" "}
              </>
            ) : null}
            Columns can be changed later in the Structure tab.
          </DialogDescription>
        </DialogHeader>
        <Field label="Table name" htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="new_table"
            className="h-7 font-mono text-xs md:text-xs"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        <datalist id={`${id}-types`}>
          {typeSuggestions(dialect).map((t) => (
            <option key={t} value={t} />
          ))}
        </datalist>
        <div className="border-border min-h-0 overflow-auto rounded-md border">
          <table className="w-full min-w-[640px] table-fixed border-collapse text-xs">
            <colgroup>
              <col className="w-[22%]" />
              <col className="w-[20%]" />
              <col className="w-16" />
              <col className={comments ? "w-[18%]" : undefined} />
              <col className="w-10" />
              <col className="w-10" />
              {comments && <col />}
              <col className="w-20" />
            </colgroup>
            <thead>
              <tr className="text-muted-foreground [&>th]:bg-surface-2 h-7 text-left text-[11px] [&>th]:sticky [&>th]:top-0 [&>th]:z-10 [&>th]:shadow-[inset_0_-1px_0_var(--color-border)]">
                <th className="px-2 font-medium">Name</th>
                <th className="px-2 font-medium">Type</th>
                <th className="px-1 text-center font-medium">Nullable</th>
                <th className="px-2 font-medium">Default</th>
                <th className="px-1 text-center font-medium" title="Primary key">
                  PK
                </th>
                <th className="px-1 text-center font-medium" title="Auto increment">
                  AI
                </th>
                {comments && <th className="px-2 font-medium">Comment</th>}
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody ref={gridRef}>
              {rows.map((r, i) => (
                <tr key={r.id} className="border-gridline group h-8 border-b last:border-b-0">
                  <td className="px-1">
                    <Input
                      data-draft={r.id}
                      aria-label={`Column ${i + 1} name`}
                      value={r.name}
                      onChange={(e) => update(r.id, { name: e.target.value })}
                      placeholder={`column_${i + 1}`}
                      className={cn(cell, "font-mono")}
                      spellCheck={false}
                      autoComplete="off"
                    />
                  </td>
                  <td className="px-1">
                    <Input
                      aria-label={`Column ${i + 1} type`}
                      value={r.dataType}
                      list={`${id}-types`}
                      onChange={(e) => update(r.id, { dataType: e.target.value })}
                      className={cn(cell, "font-mono")}
                      spellCheck={false}
                      autoComplete="off"
                    />
                  </td>
                  <td className="px-1 text-center">
                    <Checkbox
                      aria-label={`Column ${i + 1} nullable`}
                      checked={r.nullable && !r.primaryKey}
                      disabled={r.primaryKey}
                      onCheckedChange={(v) => update(r.id, { nullable: v === true })}
                      className="mx-auto size-3.5"
                    />
                  </td>
                  <td className="px-1">
                    <Input
                      aria-label={`Column ${i + 1} default`}
                      value={r.defaultExpr}
                      onChange={(e) => update(r.id, { defaultExpr: e.target.value })}
                      placeholder="—"
                      className={cn(cell, "font-mono")}
                      spellCheck={false}
                      autoComplete="off"
                    />
                  </td>
                  <td className="px-1 text-center">
                    <Checkbox
                      aria-label={`Column ${i + 1} primary key`}
                      checked={r.primaryKey}
                      onCheckedChange={(v) => update(r.id, { primaryKey: v === true })}
                      className="mx-auto size-3.5"
                    />
                  </td>
                  <td className="px-1 text-center">
                    <Checkbox
                      aria-label={`Column ${i + 1} auto increment`}
                      checked={r.autoIncrement}
                      onCheckedChange={(v) => update(r.id, { autoIncrement: v === true })}
                      className="mx-auto size-3.5"
                    />
                  </td>
                  {comments && (
                    <td className="px-1">
                      <Input
                        aria-label={`Column ${i + 1} comment`}
                        value={r.comment}
                        onChange={(e) => update(r.id, { comment: e.target.value })}
                        className={cell}
                        autoComplete="off"
                      />
                    </td>
                  )}
                  <td className="px-1">
                    <div className="flex items-center justify-end">
                      <IconAction
                        label="Move up"
                        disabled={i === 0}
                        onClick={() => setRows((rs) => moveDraft(rs, i, -1))}
                      >
                        <ArrowUp />
                      </IconAction>
                      <IconAction
                        label="Move down"
                        disabled={i === rows.length - 1}
                        onClick={() => setRows((rs) => moveDraft(rs, i, 1))}
                      >
                        <ArrowDown />
                      </IconAction>
                      <IconAction
                        label="Remove column"
                        onClick={() => setRows((rs) => rs.filter((x) => x.id !== r.id))}
                        className="hover:text-error"
                      >
                        <Trash2 />
                      </IconAction>
                    </div>
                  </td>
                </tr>
              ))}
              {rows.length === 0 && (
                <tr>
                  <td
                    colSpan={8}
                    className="text-muted-foreground px-3 py-3 text-center text-[11px] italic"
                  >
                    No columns yet
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="xs" onClick={addRow}>
            <Plus /> Add column
          </Button>
          <span className="flex-1" />
          <Button
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
          <div className="border-border bg-surface-2 max-h-48 min-h-12 overflow-auto rounded-md border px-3 py-2">
            {!result.ok ? (
              errors.length > 0 ? (
                <p className="text-muted-foreground text-[11px]">
                  Fix the problems below to see the SQL.
                </p>
              ) : (
                <div className="flex flex-col gap-0.5 text-[11px]">
                  <p className="text-muted-foreground">Fix these problems to see the SQL:</p>
                  <ul className="text-error flex flex-col gap-0.5">
                    {result.errors.map((e) => (
                      <li key={e}>{e}</li>
                    ))}
                  </ul>
                </div>
              )
            ) : preview.isError ? (
              <p className="text-error text-xs">{preview.error.message}</p>
            ) : preview.data ? (
              <>
                <SqlBlock sql={preview.data.statements.join(";\n\n")} dialect={dialect} />
                {preview.data.warnings.map((w, i) => (
                  <p key={i} className="text-warning mt-1 text-[11px]">
                    {w}
                  </p>
                ))}
              </>
            ) : (
              <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
            )}
          </div>
        )}
        {errors.length > 0 && (
          <ul role="alert" className="text-error flex flex-col gap-0.5 text-[11px]">
            {errors.map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
        )}
        <ErrorLine error={error} />
        <DialogFooter className="items-center">
          <Button variant="ghost" size="sm" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button size="sm" onClick={() => void create()} disabled={busy}>
            {busy && <Loader2 className="animate-spin" />}
            Create table
            <Kbd
              combo="Mod+Enter"
              className="border-primary-foreground/30 ml-1 bg-transparent text-inherit"
            />
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// --- rename ------------------------------------------------------------------------------------

export function RenameTableDialog({
  onOpenChange,
  connectionId,
  table,
}: {
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  dialect: Dialect;
  table: TableSummary;
}) {
  const qc = useQueryClient();
  const id = useId();
  const [name, setName] = useState(table.name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DdlErrorInfo | null>(null);
  const nameError = tableNameError(name, table.name);
  const ref = refOf(table);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (nameError || busy) return;
    setBusy(true);
    setError(null);
    const newName = name.trim();
    const r = await runOne(connectionId, ref, renameTableOp(ref, newName));
    setBusy(false);
    if (!r.ok) {
      setError(r.error);
      return;
    }
    // Open tabs follow the table to its new name, and so do their staged structure edits.
    followRename(qc, connectionId, ref, newName);
    toast.success(`Renamed ${table.name} to ${newName}`);
    onOpenChange(false);
  };

  return (
    <Dialog open onOpenChange={(o) => !busy && onOpenChange(o)}>
      <DialogContent className="sm:max-w-sm">
        <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle className="text-sm">
              Rename {KIND_LABEL[table.kind].toLowerCase()}
            </DialogTitle>
            <DialogDescription className="font-mono text-xs">{qualified(ref)}</DialogDescription>
          </DialogHeader>
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
          <ErrorLine error={error} />
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => onOpenChange(false)}
              disabled={busy}
            >
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={busy || nameError !== null}>
              {busy && <Loader2 className="animate-spin" />}
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// --- truncate ----------------------------------------------------------------------------------

export function TruncateTableDialog({
  onOpenChange,
  connectionId,
  dialect,
  table,
}: {
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  dialect: Dialect;
  table: TableSummary;
}) {
  const qc = useQueryClient();
  const [restartIdentity, setRestartIdentity] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DdlErrorInfo | null>(null);
  const ref = refOf(table);

  const run = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    const r = await runOne(connectionId, ref, truncateTableOp(ref, dialect, restartIdentity));
    setBusy(false);
    if (!r.ok) {
      setError(r.error);
      return;
    }
    invalidateSchema(qc, connectionId, ref);
    toast.success(`Truncated ${table.name}`);
    onOpenChange(false);
  };

  return (
    <AlertDialog open onOpenChange={(o) => !busy && onOpenChange(o)}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="text-sm">Truncate {table.name}?</AlertDialogTitle>
          <AlertDialogDescription className="text-xs">
            Every row of <span className="font-mono">{qualified(ref)}</span> is deleted. This cannot
            be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        {dialect === "postgres" && (
          <label className="flex items-center gap-2 text-xs">
            <Checkbox
              checked={restartIdentity}
              onCheckedChange={(v) => setRestartIdentity(v === true)}
            />
            Restart identity (reset the table's sequences)
          </label>
        )}
        <ErrorLine error={error} />
        <AlertDialogFooter>
          <AlertDialogCancel size="sm" disabled={busy}>
            Cancel
          </AlertDialogCancel>
          <Button variant="destructive" size="sm" onClick={() => void run()} disabled={busy}>
            {busy && <Loader2 className="animate-spin" />}
            Truncate
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// --- drop --------------------------------------------------------------------------------------

export function DropTableDialog({
  onOpenChange,
  connectionId,
  dialect,
  table,
}: {
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  dialect: Dialect;
  table: TableSummary;
}) {
  const qc = useQueryClient();
  const id = useId();
  const [typed, setTyped] = useState("");
  const [cascade, setCascade] = useState(false);
  const confirmRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<DdlErrorInfo | null>(null);
  const ref = refOf(table);
  const view = isViewKind(table.kind);
  const what = KIND_LABEL[table.kind].toLowerCase();
  const confirmed = typed === table.name;

  const run = async () => {
    if (!confirmed || busy) return;
    setBusy(true);
    setError(null);
    const op = view
      ? dropViewOp(ref, table.kind, dialect, cascade)
      : dropTableOp(ref, dialect, cascade);
    const r = await runOne(connectionId, view ? null : ref, op);
    setBusy(false);
    if (!r.ok) {
      setError(r.error);
      return;
    }
    const ids = tabsOf(connectionId, ref);
    useStructureStore.getState().remove(ids);
    useTabsStore.getState().forceClose(ids);
    qc.removeQueries({ queryKey: ["table", connectionId, ref.schema ?? "", ref.name] });
    invalidateSchema(qc, connectionId);
    toast.success(`Dropped ${what} ${table.name}`);
    onOpenChange(false);
  };

  return (
    <AlertDialog open onOpenChange={(o) => !busy && onOpenChange(o)}>
      <AlertDialogContent
        onOpenAutoFocus={(e) => {
          // Start in the confirmation field (AlertDialog focuses Cancel by default).
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
              Drop {what} {table.name}?
            </AlertDialogTitle>
            <AlertDialogDescription className="text-xs">
              <span className="font-mono">{qualified(ref)}</span>
              {view ? " is removed." : " and all of its rows are permanently deleted."} This cannot
              be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <Field label={`Type ${table.name} to confirm`} htmlFor={`${id}-confirm`}>
            <Input
              id={`${id}-confirm`}
              ref={confirmRef}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              placeholder={table.name}
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
          {dialect === "postgres" && (
            <label className="flex items-center gap-2 text-xs">
              <Checkbox checked={cascade} onCheckedChange={(v) => setCascade(v === true)} />
              CASCADE (also drop objects that depend on it)
            </label>
          )}
          <ErrorLine error={error} />
          <AlertDialogFooter>
            <AlertDialogCancel type="button" size="sm" disabled={busy}>
              Cancel
            </AlertDialogCancel>
            <Button type="submit" variant="destructive" size="sm" disabled={!confirmed || busy}>
              {busy && <Loader2 className="animate-spin" />}
              Drop {what}
            </Button>
          </AlertDialogFooter>
        </form>
      </AlertDialogContent>
    </AlertDialog>
  );
}
