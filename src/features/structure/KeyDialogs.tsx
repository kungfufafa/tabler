import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useMemo, useState, type FormEvent } from "react";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { schemaKeys } from "@/features/sidebar/useSchemaTree";
import { ipc, type Dialect, type FkDef, type IndexDef, type TableRef } from "@/lib/ipc";
import { Field, OrderedPicker } from "./parts";
import {
  FK_ACTIONS,
  indexMethods,
  isAlterable,
  suggestIndexName,
  supportsPartialIndex,
  validateForeignKey,
  validateIndex,
} from "./tableActions";

const DEFAULT = "__default";

function Errors({ errors }: { errors: string[] }) {
  if (errors.length === 0) return null;
  return <p className="text-error mr-auto self-center text-[11px]">{errors[0]}</p>;
}

export function AddIndexDialog({
  open,
  onOpenChange,
  dialect,
  table,
  columns,
  existingNames,
  onAdd,
  returnFocus,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  dialect: Dialect;
  table: TableRef;
  /** Current (staged) column names. */
  columns: readonly string[];
  existingNames: readonly string[];
  onAdd: (index: IndexDef) => void;
  /** Where focus goes once the dialog closed (keeps the grid's shortcuts working). */
  returnFocus?: () => void;
}) {
  const id = useId();
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [unique, setUnique] = useState(false);
  const [method, setMethod] = useState(DEFAULT);
  const [predicate, setPredicate] = useState("");
  const [submitted, setSubmitted] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName("");
    setNameTouched(false);
    setSelected([]);
    setUnique(false);
    setMethod(DEFAULT);
    setPredicate("");
    setSubmitted(false);
  }, [open]);

  const effectiveName = nameTouched ? name : suggestIndexName(table.name, selected, unique);
  const result = validateIndex(
    { name: effectiveName, columns: selected, unique, method, predicate },
    dialect,
    existingNames,
  );
  const methods = indexMethods(dialect);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setSubmitted(true);
    if (!result.ok) return;
    onAdd(result.index);
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-md"
        onCloseAutoFocus={(e) => {
          if (!returnFocus) return;
          e.preventDefault();
          returnFocus();
        }}
      >
        <form onSubmit={submit} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle className="text-sm">Add index</DialogTitle>
            <DialogDescription className="text-xs">
              On <span className="font-mono">{table.name}</span>. Staged until you apply.
            </DialogDescription>
          </DialogHeader>
          <Field label="Columns (in index order)">
            <OrderedPicker
              label="Index columns"
              options={columns}
              value={selected}
              onChange={setSelected}
            />
          </Field>
          <Field label="Name" htmlFor={`${id}-name`}>
            <Input
              id={`${id}-name`}
              value={effectiveName}
              onChange={(e) => {
                setNameTouched(true);
                setName(e.target.value);
              }}
              className="h-7 font-mono text-xs md:text-xs"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
          <label className="flex items-center gap-2 text-xs">
            <Checkbox checked={unique} onCheckedChange={(v) => setUnique(v === true)} />
            Unique
          </label>
          {methods.length > 0 && (
            <Field label={dialect === "mysql" ? "Type / method" : "Method"}>
              <Select value={method} onValueChange={setMethod}>
                <SelectTrigger
                  size="sm"
                  className="w-44 text-xs"
                  aria-label={dialect === "mysql" ? "Type / method" : "Method"}
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={DEFAULT} className="text-xs">
                    Default
                  </SelectItem>
                  {methods.map((m) => (
                    <SelectItem key={m} value={m} className="font-mono text-xs">
                      {m}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          )}
          {supportsPartialIndex(dialect) && (
            <Field label="Partial index condition (WHERE)" htmlFor={`${id}-where`}>
              <Input
                id={`${id}-where`}
                value={predicate}
                onChange={(e) => setPredicate(e.target.value)}
                placeholder="deleted_at IS NULL"
                className="h-7 font-mono text-xs md:text-xs"
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
          )}
          <DialogFooter className="items-center">
            <Errors errors={submitted && !result.ok ? result.errors : []} />
            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={submitted && !result.ok}>
              Add index
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
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
  const id = useId();
  const schema = table.schema ?? "";
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [refName, setRefName] = useState("");
  const [refColumns, setRefColumns] = useState<string[]>([]);
  const [onUpdate, setOnUpdate] = useState(DEFAULT);
  const [onDelete, setOnDelete] = useState(DEFAULT);
  const [submitted, setSubmitted] = useState(false);
  // Table whose primary key was pre-selected as the referenced columns ("" = none yet).
  const [prefilledFor, setPrefilledFor] = useState("");

  useEffect(() => {
    if (!open) return;
    setName("");
    setSelected([]);
    setRefName("");
    setRefColumns([]);
    setPrefilledFor("");
    setOnUpdate(DEFAULT);
    setOnDelete(DEFAULT);
    setSubmitted(false);
  }, [open]);

  const tables = useQuery({
    queryKey: schemaKeys.tables(connectionId, schema),
    queryFn: () => ipc.schema.tables(connectionId, schema),
    enabled: open && schema !== "",
    staleTime: 60_000,
  });
  const refTable: TableRef | null = refName ? { schema: table.schema, name: refName } : null;
  const refMeta = useQuery({
    queryKey: schemaKeys.table(connectionId, table.schema, refName),
    queryFn: () => ipc.schema.table(connectionId, refTable!),
    enabled: open && refTable !== null,
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
    onDelete: onDelete === DEFAULT ? "" : onDelete,
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setSubmitted(true);
    if (!result.ok) return;
    onAdd(result.fk);
    onOpenChange(false);
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
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-lg"
        onCloseAutoFocus={(e) => {
          if (!returnFocus) return;
          e.preventDefault();
          returnFocus();
        }}
      >
        <form onSubmit={submit} className="flex flex-col gap-3">
          <DialogHeader>
            <DialogTitle className="text-sm">Add foreign key</DialogTitle>
            <DialogDescription className="text-xs">
              From <span className="font-mono">{table.name}</span> to a table of the same schema.
            </DialogDescription>
          </DialogHeader>
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
            {actionSelect("On delete", onDelete, setOnDelete)}
          </div>
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
          <DialogFooter className="items-center">
            <Errors errors={submitted && !result.ok ? result.errors : []} />
            <Button type="button" variant="ghost" size="sm" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={submitted && !result.ok}>
              Add foreign key
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
