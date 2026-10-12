import { useQuery } from "@tanstack/react-query";
import { useId, useMemo, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, type FkDef, type TableRef } from "@/lib/ipc";
import { Field, OrderedPicker } from "./parts";
import { FK_ACTIONS, isAlterable, validateForeignKey } from "./tableActions";

const DEFAULT = "__default";

function Errors({ errors }: { errors: string[] }) {
  if (errors.length === 0) return null;
  return <p className="text-error mr-auto self-center text-[11px]">{errors[0]}</p>;
}

/**
 * The foreign key form (TablePlus "Create a foreign key on column"): Table, Columns, Referenced
 * Table, Referenced Columns, On Update, On Delete. Used by the "+ Foreign key" dialog and by the
 * popover of a column's `foreign_key` cell (which adds Delete). Mount it fresh for every edit:
 * its state starts from `initial`.
 */
export function ForeignKeyForm({
  connectionId,
  table,
  columns,
  initial,
  onSubmit,
  onCancel,
  onDelete,
  submitLabel = "Add foreign key",
  showName = true,
}: {
  connectionId: string;
  table: TableRef;
  columns: readonly string[];
  /** Starting values (an existing key being edited, or the column the popover is for). */
  initial?: Partial<FkDef>;
  onSubmit: (fk: FkDef) => void;
  onCancel: () => void;
  /** Shows a Delete button (the key exists or is staged). */
  onDelete?: () => void;
  submitLabel?: string;
  /** The constraint name field (TablePlus keeps the default name: the popover hides it). */
  showName?: boolean;
}) {
  const id = useId();
  const schema = table.schema ?? "";
  const [name, setName] = useState(initial?.name ?? "");
  const [selected, setSelected] = useState<string[]>(initial?.columns ?? []);
  const [refName, setRefName] = useState(initial?.refTable?.name ?? "");
  const [refColumns, setRefColumns] = useState<string[]>(initial?.refColumns ?? []);
  const [onUpdate, setOnUpdate] = useState(initial?.onUpdate ?? DEFAULT);
  const [onDelete_, setOnDelete] = useState(initial?.onDelete ?? DEFAULT);
  const [submitted, setSubmitted] = useState(false);
  // Table whose primary key was pre-selected as the referenced columns ("" = none yet).
  const [prefilledFor, setPrefilledFor] = useState(initial?.refTable?.name ?? "");

  const tables = useQuery({
    queryKey: schemaKeys.tables(connectionId, schema),
    queryFn: () => ipc.schema.tables(connectionId, schema),
    enabled: schema !== "",
    staleTime: 60_000,
  });
  const refTable: TableRef | null = refName ? { schema: table.schema, name: refName } : null;
  const refMeta = useQuery({
    queryKey: schemaKeys.table(connectionId, table.schema, refName),
    queryFn: () => ipc.schema.table(connectionId, refTable!),
    enabled: refTable !== null,
    staleTime: 5 * 60_000,
  });
  const tableOptions = useMemo(
    () => (tables.data ?? []).filter((t) => isAlterable(t.kind)).map((t) => t.name),
    [tables.data],
  );
  const refOptions = useMemo(() => refMeta.data?.columns.map((c) => c.name) ?? [], [refMeta.data]);

  // Picking a table pre-selects its primary key.
  if (refMeta.data && refMeta.data.table.name === refName && prefilledFor !== refName) {
    setPrefilledFor(refName);
    setRefColumns(refMeta.data.primaryKey);
  }

  const result = validateForeignKey({
    name,
    columns: selected,
    refTable,
    refColumns,
    onUpdate: onUpdate === DEFAULT ? "" : onUpdate,
    onDelete: onDelete_ === DEFAULT ? "" : onDelete_,
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setSubmitted(true);
    if (!result.ok) return;
    onSubmit(result.fk);
  };

  const actionSelect = (label: string, value: string, set: (v: string) => void) => (
    <Field label={label}>
      <Select value={value} onValueChange={set}>
        <SelectTrigger size="sm" className="w-full text-xs" aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT} className="text-xs">
            Default (NO ACTION)
          </SelectItem>
          {FK_ACTIONS.map((a) => (
            <SelectItem key={a} value={a} className="text-xs">
              {a}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </Field>
  );

  return (
    <form onSubmit={submit} className="flex flex-col gap-3" aria-label="Foreign key">
      <Field label="Table">
        <p className="font-mono text-xs">{table.name}</p>
      </Field>
      <Field label="Columns">
        <OrderedPicker
          label="Foreign key columns"
          options={columns}
          value={selected}
          onChange={setSelected}
        />
      </Field>
      <Field label="Referenced table">
        <Select
          value={refName}
          onValueChange={(v) => {
            setRefName(v);
            setRefColumns([]);
            setPrefilledFor("");
          }}
        >
          <SelectTrigger
            size="sm"
            className="w-full font-mono text-xs"
            aria-label="Referenced table"
          >
            <SelectValue placeholder={tables.isPending ? "Loading tables…" : "Pick a table"} />
          </SelectTrigger>
          <SelectContent>
            {tableOptions.map((t) => (
              <SelectItem key={t} value={t} className="font-mono text-xs">
                {t}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      {refTable && (
        <Field label="Referenced columns (same order)">
          {refMeta.isPending ? (
            <p className="text-muted-foreground text-[11px]">Loading columns…</p>
          ) : (
            <OrderedPicker
              label="Referenced columns"
              options={refOptions}
              value={refColumns}
              onChange={setRefColumns}
            />
          )}
        </Field>
      )}
      <div className="grid grid-cols-2 gap-3">
        {actionSelect("On update", onUpdate, setOnUpdate)}
        {actionSelect("On delete", onDelete_, setOnDelete)}
      </div>
      {showName && (
        <Field label="Constraint name (optional)" htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="chosen by the database"
            className="h-7 font-mono text-xs md:text-xs"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      )}
      <div className="flex items-center justify-end gap-2">
        <Errors errors={submitted && !result.ok ? result.errors : []} />
        {onDelete && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-error hover:text-error mr-auto"
            onClick={onDelete}
          >
            Delete
          </Button>
        )}
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="submit" size="sm" disabled={submitted && !result.ok}>
          {submitLabel}
        </Button>
      </div>
    </form>
  );
}

export function AddForeignKeyDialog({
  open,
  onOpenChange,
  connectionId,
  table,
  columns,
  onAdd,
  returnFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  connectionId: string;
  table: TableRef;
  columns: readonly string[];
  onAdd: (fk: FkDef) => void;
  returnFocus?: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-lg"
        onCloseAutoFocus={(e) => {
          if (!returnFocus) return;
          e.preventDefault();
          returnFocus();
        }}
      >
        <DialogHeader>
          <DialogTitle className="text-sm">Add foreign key</DialogTitle>
          <DialogDescription className="text-xs">
            From <span className="font-mono">{table.name}</span> to a table of the same schema.
          </DialogDescription>
        </DialogHeader>
        <ForeignKeyForm
          connectionId={connectionId}
          table={table}
          columns={columns}
          onSubmit={(fk) => {
            onAdd(fk);
            onOpenChange(false);
          }}
          onCancel={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}
