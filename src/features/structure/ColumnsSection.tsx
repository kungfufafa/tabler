import { KeyRound, ListPlus, Plus, SlidersHorizontal, Trash2, Undo2 } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import type { Dialect, FkDef, TableRef } from "@/lib/ipc";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { CellPopover, ChoiceCell, FlagsCell, PresetMenu, CELL_BUTTON } from "./cells";
import {
  GRID_LABEL,
  gridColumns,
  supportsColumnCollation,
  supportsColumnPosition,
  typeSuggestions,
  type ColumnField,
  type GridColumn,
} from "./columnEdits";
import { defaultPresets, type DefaultPreset } from "./defaults";
import { cellByKey, rowDeleteKey } from "./dom";
import { ForeignKeyForm } from "./KeyDialogs";
import type { ColumnRow, FkRow, StructureView } from "./model";
import { columnCheck } from "./objects";
import { CellInput, Field, HEAD_ROW, IconAction, SectionHeader } from "./parts";

export interface ColumnHandlers {
  onEdit: (key: string, field: ColumnField, raw: string | boolean) => void;
  /** A `column_default` preset; `value` is the typed constant for "Constant…". */
  onPreset: (key: string, preset: DefaultPreset, value?: string) => void;
  /** PostgreSQL `check` cell of the column called `column` (null / "" removes it). */
  onSetCheck: (column: string, expression: string) => void;
  /** Creates (`replaces` null) or replaces the foreign key row `replaces`. */
  onSaveFk: (replaces: string | null, fk: FkDef) => void;
  onDeleteFk: (key: string) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onAdd: (after?: string | null) => void;
}

const NULLABLE = [
  { value: "YES", label: "YES" },
  { value: "NO", label: "NO" },
] as const;

const COL_WIDTH: Record<GridColumn, string> = {
  name: "w-[17%]",
  dataType: "w-[17%]",
  nullable: "w-[76px]",
  check: "w-[15%]",
  defaultExpr: "w-[17%]",
  extra: "w-[13%]",
  foreignKey: "w-[14%]",
  comment: "",
};

/** The foreign key a column takes part in (first live one). */
function fkOf(fks: readonly FkRow[], column: string): FkRow | null {
  return fks.find((f) => f.status !== "dropped" && f.columns.includes(column)) ?? null;
}

export function ColumnsSection({
  view,
  dialect,
  editable,
  connectionId,
  table,
  filter,
  userTypes,
  collations,
  handlers,
}: {
  view: StructureView;
  dialect: Dialect;
  editable: boolean;
  connectionId: string;
  table: TableRef;
  /** Column-name search (⌘F): rows whose name contains it. */
  filter: string;
  /** The database's own column types (PG enums, domains…), offered in data_type. */
  userTypes: readonly string[];
  /** MySQL collations, offered in the column details. */
  collations: readonly string[];
  handlers: ColumnHandlers;
}) {
  const typesId = useId();
  const newTypesId = useId();
  const collationsId = useId();
  const rows = view.columns;
  const live = rows.filter((r) => r.status !== "dropped").length;
  const newKeys = shortcutFor("grid.new-row");
  const cols = gridColumns(dialect);
  const needle = filter.trim().toLowerCase();
  const shown = rows
    .map((r, i) => ({ r, i }))
    .filter(({ r }) => !needle || r.name.toLowerCase().includes(needle));
  const liveNames = rows.filter((r) => r.status !== "dropped").map((r) => r.name);
  return (
    <section aria-label="Columns">
      <SectionHeader title="Columns" count={live}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={() => handlers.onAdd()}>
            <Plus /> Column
            {newKeys && <Kbd combo={newKeys} className="ml-0.5" />}
          </Button>
        )}
      </SectionHeader>
      {/* Existing columns are altered (ALTER … TYPE), new ones created: different type lists. */}
      <datalist id={typesId}>
        {[...typeSuggestions(dialect, true), ...userTypes].map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <datalist id={newTypesId}>
        {[...typeSuggestions(dialect), ...userTypes].map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <datalist id={collationsId}>
        {collations.map((c) => (
          <option key={c} value={c} />
        ))}
      </datalist>
      <table data-grid="columns" className="w-full table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-9" />
          {cols.map((c) => (
            <col key={c} className={COL_WIDTH[c] || undefined} />
          ))}
          <col className="w-[72px]" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <th className="px-2 text-right font-normal">#</th>
            {cols.map((c) => (
              <th
                key={c}
                className={cn("truncate px-2.5 font-mono font-medium", c === "nullable" && "px-2")}
              >
                {GRID_LABEL[c]}
              </th>
            ))}
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {shown.map(({ r, i }) => (
            <ColumnRowView
              key={r.key}
              row={r}
              index={i}
              view={view}
              dialect={dialect}
              cols={cols}
              editable={editable}
              connectionId={connectionId}
              table={table}
              liveNames={liveNames}
              typesId={r.status === "added" ? newTypesId : typesId}
              collationsId={collationsId}
              handlers={handlers}
            />
          ))}
          {shown.length === 0 && (
            <tr>
              <td
                colSpan={cols.length + 2}
                className="text-muted-foreground px-3 py-2 text-[11px] italic"
              >
                {rows.length === 0 ? "No columns" : `No column matches “${filter.trim()}”`}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

function ColumnRowView({
  row: r,
  index,
  view,
  dialect,
  cols,
  editable,
  connectionId,
  table,
  liveNames,
  typesId,
  collationsId,
  handlers,
}: {
  row: ColumnRow;
  index: number;
  view: StructureView;
  dialect: Dialect;
  cols: GridColumn[];
  editable: boolean;
  connectionId: string;
  table: TableRef;
  liveNames: string[];
  typesId: string;
  collationsId: string;
  handlers: ColumnHandlers;
}) {
  const { onEdit, onDrop, onRestore, onAdd } = handlers;
  const dropped = r.status === "dropped";
  const added = r.status === "added";
  const canEdit = editable && !dropped;
  const changed = (...f: ColumnField[]) => f.some((x) => r.changed.includes(x)) && "bg-changed";
  const strike = dropped && "line-through";
  const text = (
    field: "name" | "dataType" | "defaultExpr" | "comment",
    className?: string,
    placeholder?: string,
  ) => (
    <CellInput
      value={(r[field] as string | null) ?? ""}
      onCommit={(raw) => onEdit(r.key, field, raw)}
      cell={`${index}|${field}`}
      cellKey={`${r.key}:${field}`}
      label={`${r.name} ${field === "dataType" ? "type" : field === "defaultExpr" ? "default" : field}`}
      readOnly={!canEdit}
      placeholder={placeholder}
      list={field === "dataType" && canEdit ? typesId : undefined}
      className={cn(strike, className)}
    />
  );

  const cell = (c: GridColumn) => {
    switch (c) {
      case "name":
        return (
          <td key={c} className={cn("px-1", changed("name"))}>
            <div className="flex items-center">
              {text("name", "font-medium")}
              {r.isPk && (
                <KeyRound
                  aria-label="Primary key"
                  className={cn(
                    "mr-1 size-3 shrink-0",
                    r.meta && !r.meta.isPk ? "text-brand" : "text-warning",
                  )}
                />
              )}
            </div>
          </td>
        );
      case "dataType":
        return (
          <td key={c} className={cn("px-1", changed("dataType"))}>
            {text("dataType", "font-mono text-[11px]")}
          </td>
        );
      case "nullable": {
        const pkChanged = r.meta !== null && r.isPk !== r.meta.isPk;
        return (
          <td key={c} className={cn("px-1", (changed("nullable") || pkChanged) && "bg-changed")}>
            <ChoiceCell
              value={r.nullable ? "YES" : "NO"}
              options={NULLABLE}
              keys={{ y: "YES", n: "NO" }}
              onChange={(v) => onEdit(r.key, "nullable", v === "YES")}
              label={`${r.name} nullable`}
              cellKey={`${r.key}:nullable`}
              disabled={!canEdit || r.isPk}
              className={cn("font-mono text-[11px]", strike)}
            />
          </td>
        );
      }
      case "check": {
        const check = columnCheck(view, r.name);
        return (
          <td
            key={c}
            className={cn(
              "px-1",
              check &&
                check.status !== "existing" &&
                (check.status === "added" ? "bg-inserted" : "bg-changed"),
            )}
          >
            <CellInput
              value={check?.expression ?? ""}
              onCommit={(raw) => handlers.onSetCheck(r.name, raw)}
              cell={`${index}|check`}
              cellKey={`${r.key}:check`}
              label={`${r.name} check`}
              readOnly={!canEdit || added}
              className={cn("font-mono text-[11px]", strike)}
            />
          </td>
        );
      }
      case "defaultExpr":
        return (
          <DefaultCell
            key={c}
            row={r}
            dialect={dialect}
            canEdit={canEdit}
            changedClass={changed("defaultExpr", "autoIncrement")}
            input={text("defaultExpr", "font-mono text-[11px]", placeholderOf(dialect, r))}
            onPreset={(p, v) => handlers.onPreset(r.key, p, v)}
          />
        );
      case "extra": {
        const other = otherExtra(r.meta?.extra ?? null);
        return (
          <td key={c} className={cn("px-1", changed("autoIncrement", "onUpdate"))}>
            <FlagsCell
              label={`${r.name} extra`}
              cellKey={`${r.key}:extra`}
              disabled={!canEdit}
              readOnly={other}
              flags={[
                {
                  id: "autoIncrement",
                  label: "auto_increment",
                  checked: r.autoIncrement,
                  // ADD COLUMN cannot make the column a key in the same statement (MySQL 1075).
                  disabled: added,
                },
                {
                  id: "onUpdate",
                  label: `on update ${r.onUpdate ?? "CURRENT_TIMESTAMP"}`,
                  checked: r.onUpdate !== null,
                },
              ]}
              onToggle={(id, on) => {
                if (id === "autoIncrement") onEdit(r.key, "autoIncrement", on);
                else onEdit(r.key, "onUpdate", on ? "CURRENT_TIMESTAMP" : "");
              }}
            />
          </td>
        );
      }
      case "foreignKey":
        return (
          <td key={c} className="px-1">
            <FkCell
              row={r}
              fk={fkOf(view.foreignKeys, r.name)}
              canEdit={canEdit}
              connectionId={connectionId}
              table={table}
              columns={liveNames}
              onSave={handlers.onSaveFk}
              onDelete={handlers.onDeleteFk}
            />
          </td>
        );
      case "comment":
        return (
          <td key={c} className={cn("px-1", changed("comment"))}>
            {text("comment")}
          </td>
        );
    }
  };

  return (
    <tr
      data-column-key={r.key}
      data-row-kind="column"
      data-row-key={r.key}
      data-status={r.status}
      onKeyDown={(e) => {
        if (editable) rowDeleteKey(e, () => (dropped ? onRestore(r.key) : onDrop(r.key)));
      }}
      className={cn(
        "group border-gridline h-7 border-b",
        r.status === "existing" && "focus-within:bg-row-hover",
        added && "bg-inserted",
        dropped && "bg-deleted text-muted-foreground",
      )}
    >
      <td className="px-1 text-right">
        <button
          type="button"
          tabIndex={-1}
          aria-label={`Select column ${r.name}`}
          className="text-muted-foreground focus-visible:text-foreground focus:bg-accent w-full rounded-sm px-1 text-right font-mono text-[10px] tabular-nums outline-none"
        >
          {index + 1}
        </button>
      </td>
      {cols.map(cell)}
      <td className="px-1">
        {editable && (
          <div
            className={cn(
              "flex items-center justify-end gap-0.5 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
              dropped && "opacity-100",
            )}
          >
            {!dropped && supportsColumnCollation(dialect, r.dataType) && (
              <DetailsButton row={r} collationsId={collationsId} onEdit={onEdit} />
            )}
            {supportsColumnPosition(dialect) && !dropped && (
              <IconAction label="Insert column after" onClick={() => onAdd(r.name)} tabIndex={-1}>
                <ListPlus />
              </IconAction>
            )}
            {dropped ? (
              <IconAction label="Restore column" onClick={() => onRestore(r.key)} tabIndex={-1}>
                <Undo2 />
              </IconAction>
            ) : (
              <IconAction
                label={added ? "Remove new column" : "Drop column"}
                onClick={() => onDrop(r.key)}
                className="hover:text-error"
                tabIndex={-1}
              >
                <Trash2 />
              </IconAction>
            )}
          </div>
        )}
      </td>
    </tr>
  );
}

/** What an empty `column_default` stands for. */
function placeholderOf(dialect: Dialect, r: ColumnRow): string {
  if (r.autoIncrement && dialect === "postgres") return "identity";
  if (r.autoIncrement && dialect === "sqlite") return "rowid";
  if (r.nullable) return "NULL";
  return "—";
}

/** MySQL EXTRA words besides auto_increment / on update (DEFAULT_GENERATED, VIRTUAL GENERATED…). */
function otherExtra(extra: string | null): string | null {
  const rest = (extra ?? "")
    .replace(/\bauto_increment\b/i, "")
    .replace(/\bon update [a-z_]+(\(\d*\))?/i, "")
    .replace(/\bDEFAULT_GENERATED\b/i, "")
    .trim();
  return rest || null;
}

function DefaultCell({
  row: r,
  dialect,
  canEdit,
  changedClass,
  input,
  onPreset,
}: {
  row: ColumnRow;
  dialect: Dialect;
  canEdit: boolean;
  changedClass: string | false;
  input: ReactNode;
  onPreset: (preset: DefaultPreset, value?: string) => void;
}) {
  const presets = defaultPresets(dialect, r.dataType);
  const [constant, setConstant] = useState<DefaultPreset | null>(null);
  const [value, setValue] = useState("");
  const added = r.status === "added";
  const entries = presets.map((p, i) => ({
    id: String(i),
    label: p.label,
    hint: p.hint,
    // A new MySQL column cannot be AUTO_INCREMENT (ADD COLUMN cannot also key it).
    disabled: p.kind === "autoIncrement" && added && dialect !== "postgres",
  }));
  const pick = (id: string) => {
    const p = presets[Number(id)];
    if (!p) return false;
    if (p.kind === "constant") {
      setValue("");
      setConstant(p);
      return true;
    }
    if (p.kind === "expression") {
      const el = cellByKey(`${r.key}:defaultExpr`);
      el?.focus();
      if (el instanceof HTMLInputElement) el.select();
      return true;
    }
    onPreset(p);
    return false;
  };
  return (
    <td className={cn("px-1", changedClass)}>
      <Popover open={constant !== null} onOpenChange={(o) => !o && setConstant(null)}>
        <PopoverAnchor asChild>
          <div className="flex items-center">
            {input}
            {canEdit && (
              <PresetMenu label={`${r.name} default presets`} entries={entries} onPick={pick} />
            )}
          </div>
        </PopoverAnchor>
        <PopoverContent align="start" className="w-64 p-2.5">
          <form
            className="flex flex-col gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (constant) onPreset(constant, value);
              setConstant(null);
            }}
          >
            <Field label={`Constant default of ${r.name}`} htmlFor={`${r.key}-constant`}>
              <Input
                id={`${r.key}-constant`}
                autoFocus
                value={value}
                onChange={(e) => setValue(e.target.value)}
                placeholder="value (quoted for you)"
                className="h-7 font-mono text-xs md:text-xs"
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
            <div className="flex justify-end gap-2">
              <Button type="button" variant="ghost" size="xs" onClick={() => setConstant(null)}>
                Cancel
              </Button>
              <Button type="submit" size="xs">
                Set default
              </Button>
            </div>
          </form>
        </PopoverContent>
      </Popover>
    </td>
  );
}

function FkCell({
  row: r,
  fk,
  canEdit,
  connectionId,
  table,
  columns,
  onSave,
  onDelete,
}: {
  row: ColumnRow;
  fk: FkRow | null;
  canEdit: boolean;
  connectionId: string;
  table: TableRef;
  columns: string[];
  onSave: (replaces: string | null, fk: FkDef) => void;
  onDelete: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const label = fk ? `${fk.refTable.name}(${fk.refColumns.join(", ")})` : "";
  const trigger = (
    <button
      type="button"
      aria-label={`${r.name} foreign key`}
      data-cell-key={`${r.key}:foreignKey`}
      disabled={!canEdit && !fk}
      className={cn(
        CELL_BUTTON,
        "font-mono text-[11px]",
        fk?.status === "added" && "bg-inserted",
        !canEdit && "pointer-events-none",
      )}
      title={label || undefined}
    >
      <span className={cn("truncate", fk && "text-brand")}>{label}</span>
    </button>
  );
  if (!canEdit) return trigger;
  return (
    <CellPopover open={open} onOpenChange={setOpen} trigger={trigger} className="w-[26rem]">
      <p className="mb-2 text-xs font-medium">
        {fk ? "Foreign key" : "Create a foreign key on column"}{" "}
        <span className="font-mono">{r.name}</span>
      </p>
      <ForeignKeyForm
        connectionId={connectionId}
        table={table}
        columns={columns}
        initial={
          fk
            ? {
                columns: fk.columns,
                refTable: fk.refTable,
                refColumns: fk.refColumns,
                onUpdate: fk.onUpdate,
                onDelete: fk.onDelete,
              }
            : { columns: [r.name] }
        }
        showName={false}
        submitLabel="Ok"
        onSubmit={(def) => {
          onSave(fk?.key ?? null, def);
          setOpen(false);
        }}
        onCancel={() => setOpen(false)}
        onDelete={
          fk
            ? () => {
                onDelete(fk.key);
                setOpen(false);
              }
            : undefined
        }
      />
    </CellPopover>
  );
}

function DetailsButton({
  row: r,
  collationsId,
  onEdit,
}: {
  row: ColumnRow;
  collationsId: string;
  onEdit: (key: string, field: ColumnField, raw: string | boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState(r.collation ?? "");
  const id = useId();
  return (
    <CellPopover
      open={open}
      onOpenChange={(o) => {
        if (o) setValue(r.collation ?? "");
        setOpen(o);
      }}
      className="w-72"
      trigger={
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          tabIndex={-1}
          aria-label="Column details"
          className={cn(
            "text-muted-foreground hover:text-foreground",
            r.changed.includes("collation") && "text-brand",
          )}
        >
          <SlidersHorizontal />
        </Button>
      }
    >
      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          onEdit(r.key, "collation", value);
          setOpen(false);
        }}
      >
        <p className="text-xs font-medium">
          <span className="font-mono">{r.name}</span> details
        </p>
        <Field label="Collation" htmlFor={id} hint="Empty: the table's default collation.">
          <Input
            id={id}
            list={collationsId}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="utf8mb4_0900_ai_ci"
            className="h-7 font-mono text-xs md:text-xs"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" size="xs" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button type="submit" size="xs">
            Ok
          </Button>
        </div>
      </form>
    </CellPopover>
  );
}
