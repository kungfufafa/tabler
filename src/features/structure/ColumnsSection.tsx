import { Check, KeyRound, Link2, ListPlus, Plus, Trash2, Undo2 } from "lucide-react";
import { useId } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { Dialect } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import {
  supportsAddAutoIncrement,
  supportsColumnPosition,
  supportsComments,
  typeSuggestions,
  type ColumnField,
} from "./columnEdits";
import type { ColumnRow } from "./model";
import { CellInput, HEAD_ROW, IconAction, SectionHeader } from "./parts";

export function ColumnsSection({
  rows,
  dialect,
  editable,
  fkColumns,
  onEdit,
  onDrop,
  onRestore,
  onAdd,
}: {
  rows: ColumnRow[];
  dialect: Dialect;
  editable: boolean;
  /** Current names of columns that take part in a foreign key. */
  fkColumns: ReadonlySet<string>;
  onEdit: (key: string, field: ColumnField, raw: string | boolean) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onAdd: (after?: string | null) => void;
}) {
  const typesId = useId();
  const newTypesId = useId();
  const comments = supportsComments(dialect);
  const live = rows.filter((r) => r.status !== "dropped").length;
  return (
    <section aria-label="Columns">
      <SectionHeader title="Columns" count={live}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={() => onAdd()}>
            <Plus /> Column
            <Kbd combo="Mod+N" className="ml-0.5" />
          </Button>
        )}
      </SectionHeader>
      {/* Existing columns are altered (ALTER … TYPE), new ones created: different type lists. */}
      <datalist id={typesId}>
        {typeSuggestions(dialect, true).map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <datalist id={newTypesId}>
        {typeSuggestions(dialect).map((t) => (
          <option key={t} value={t} />
        ))}
      </datalist>
      <table data-grid="columns" className="w-full table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-9" />
          <col className={comments ? "w-[17%]" : "w-[22%]"} />
          <col className={comments ? "w-[20%]" : "w-[26%]"} />
          <col className="w-14" />
          <col className={comments ? "w-[15%]" : undefined} />
          <col className="w-9" />
          <col className="w-9" />
          {comments && <col />}
          <col className="w-16" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <th className="px-2 text-right font-normal">#</th>
            <th className="px-2.5 font-medium">Name</th>
            <th className="px-2.5 font-medium">Type</th>
            <th className="px-1 text-center font-medium">Nullable</th>
            <th className="px-2.5 font-medium">Default</th>
            <th className="px-1 text-center font-medium" title="Primary key">
              PK
            </th>
            <th className="px-1 text-center font-medium" title="Auto increment">
              AI
            </th>
            {comments && <th className="px-2.5 font-medium">Comment</th>}
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <ColumnRowView
              key={r.key}
              row={r}
              index={i}
              dialect={dialect}
              editable={editable}
              typesId={r.status === "added" ? newTypesId : typesId}
              isFk={fkColumns.has(r.name)}
              onEdit={onEdit}
              onDrop={onDrop}
              onRestore={onRestore}
              onAdd={onAdd}
            />
          ))}
          {rows.length === 0 && (
            <tr>
              <td colSpan={9} className="text-muted-foreground px-3 py-2 text-[11px] italic">
                No columns
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
  dialect,
  editable,
  typesId,
  isFk,
  onEdit,
  onDrop,
  onRestore,
  onAdd,
}: {
  row: ColumnRow;
  index: number;
  dialect: Dialect;
  editable: boolean;
  typesId: string;
  isFk: boolean;
  onEdit: (key: string, field: ColumnField, raw: string | boolean) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onAdd: (after?: string | null) => void;
}) {
  const dropped = r.status === "dropped";
  const added = r.status === "added";
  const canEdit = editable && !dropped;
  const comments = supportsComments(dialect);
  const changed = (f: ColumnField) => r.changed.includes(f) && "bg-changed";
  const strike = dropped && "line-through";
  const text = (field: Exclude<ColumnField, "nullable" | "autoIncrement">, className?: string) => (
    <CellInput
      value={(r[field] as string | null) ?? ""}
      onCommit={(raw) => onEdit(r.key, field, raw)}
      cell={`${index}|${field}`}
      cellKey={`${r.key}:${field}`}
      label={`${r.name} ${field === "dataType" ? "type" : field === "defaultExpr" ? "default" : field}`}
      readOnly={!canEdit}
      placeholder={field === "defaultExpr" ? "—" : undefined}
      list={field === "dataType" && canEdit ? typesId : undefined}
      className={cn(strike, className)}
    />
  );

  return (
    <tr
      data-column-key={r.key}
      data-status={r.status}
      className={cn(
        "group border-gridline h-7 border-b",
        added && "bg-inserted",
        dropped && "bg-deleted text-muted-foreground",
      )}
    >
      <td className="text-muted-foreground px-2 text-right font-mono text-[10px] tabular-nums">
        {index + 1}
      </td>
      <td className={cn("px-1", changed("name"))}>
        <div className="flex items-center">
          {text("name", "font-medium")}
          {isFk && (
            <Link2
              aria-label="Foreign key"
              className="text-muted-foreground mr-1 size-3 shrink-0"
            />
          )}
        </div>
      </td>
      <td className={cn("px-1", changed("dataType"))}>
        {text("dataType", "font-mono text-[11px]")}
      </td>
      <td className={cn("px-1 text-center", changed("nullable"))}>
        <Checkbox
          aria-label={`${r.name} nullable`}
          checked={r.nullable}
          disabled={!canEdit || r.isPk}
          onCheckedChange={(v) => onEdit(r.key, "nullable", v === true)}
          className="mx-auto size-3.5"
        />
      </td>
      <td className={cn("px-1", changed("defaultExpr"))}>
        {text("defaultExpr", "font-mono text-[11px]")}
      </td>
      <td className="px-1 text-center">
        {r.isPk && <KeyRound aria-label="Primary key" className="text-warning mx-auto size-3.5" />}
      </td>
      <td className="px-1 text-center">
        {added && supportsAddAutoIncrement(dialect) ? (
          <Checkbox
            aria-label={`${r.name} auto increment`}
            checked={r.autoIncrement}
            disabled={!canEdit}
            onCheckedChange={(v) => onEdit(r.key, "autoIncrement", v === true)}
            className="mx-auto size-3.5"
          />
        ) : (
          r.autoIncrement && (
            <Check aria-label="Auto increment" className="text-muted-foreground mx-auto size-3.5" />
          )
        )}
      </td>
      {comments && <td className={cn("px-1", changed("comment"))}>{text("comment")}</td>}
      <td className="px-1">
        {editable && (
          <div
            className={cn(
              "flex items-center justify-end gap-0.5 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
              dropped && "opacity-100",
            )}
          >
            {supportsColumnPosition(dialect) && !dropped && (
              <IconAction label="Insert column after" onClick={() => onAdd(r.name)}>
                <ListPlus />
              </IconAction>
            )}
            {dropped ? (
              <IconAction label="Restore column" onClick={() => onRestore(r.key)}>
                <Undo2 />
              </IconAction>
            ) : (
              <IconAction
                label={added ? "Remove new column" : "Drop column"}
                onClick={() => onDrop(r.key)}
                className="hover:text-error"
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
