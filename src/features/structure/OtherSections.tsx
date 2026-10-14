import { Pencil, Plus, Trash2, Undo2 } from "lucide-react";
import type { ReactNode } from "react";
import { Kbd } from "@/components/Kbd";
import { Button } from "@/components/ui/button";
import type { Dialect, IndexDef, TableRef } from "@/lib/ipc";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { ChoiceCell, ColumnsCell } from "./cells";
import { requiresIndexPrefix, supportsIndexPrefix } from "./columnEdits";
import { focusRowOnMouseDown, rowDeleteKey } from "./dom";
import {
  entryBase,
  type CheckRow,
  type FkRow,
  type IndexRow,
  type RowStatus,
  type TriggerRow,
} from "./model";
import { CellInput, HEAD_ROW, IconAction, SectionHeader, type PrefixLengths } from "./parts";
import {
  indexMethods,
  supportsIncludeColumns,
  supportsIndexComment,
  supportsPartialIndex,
  supportsTriggerToggle,
} from "./tableActions";

function rowClass(status: RowStatus) {
  return cn(
    "group border-gridline h-7 border-b",
    status === "existing" && "focus-within:bg-row-hover",
    status === "added" && "bg-inserted",
    status === "changed" && "bg-changed",
    status === "dropped" && "bg-deleted text-muted-foreground line-through",
  );
}

function Th({ children, className }: { children?: ReactNode; className?: string }) {
  return <th className={cn("truncate px-3 font-mono font-medium", className)}>{children}</th>;
}

/** The grids a double-click on blank space adds a row to (`data-blank-row`, see StructureTab). */
export type BlankKind = "columns" | "indexes" | "triggers";

/** "No indexes": a double-click on it adds one while the table can be changed (`kind`). */
export function EmptyRow({ span, text, kind }: { span: number; text: string; kind?: BlankKind }) {
  return (
    <tr data-blank-row={kind}>
      <td colSpan={span} className="text-muted-foreground px-3 py-2 text-[11px] italic">
        {text}
      </td>
    </tr>
  );
}

/**
 * The empty row that ends an editable grid: a double-click on it (or on any blank space of the
 * grid) adds a row, as TablePlus's "double-click a blank row".
 */
export function BlankRow({ span, kind, text }: { span: number; kind: BlankKind; text: string }) {
  return (
    <tr data-blank-row={kind} className="h-7">
      <td colSpan={span} className="text-muted-foreground/50 px-3 text-[11px] italic select-none">
        {text}
      </td>
    </tr>
  );
}

function DropToggle({
  status,
  what,
  onDrop,
  onRestore,
  disabled,
  children,
}: {
  status: RowStatus;
  what: string;
  onDrop: () => void;
  onRestore: () => void;
  disabled?: boolean;
  children?: ReactNode;
}) {
  if (disabled) return null;
  const reverting = status === "dropped" || status === "changed";
  return (
    <div
      className={cn(
        "flex justify-end gap-0.5 opacity-0 group-focus-within:opacity-100 group-hover:opacity-100",
        reverting && "opacity-100",
      )}
    >
      {children}
      {reverting && (
        <IconAction
          label={status === "changed" ? `Revert ${what}` : `Restore ${what}`}
          onClick={onRestore}
          tabIndex={-1}
        >
          <Undo2 />
        </IconAction>
      )}
      {status !== "dropped" && (
        <IconAction
          label={status === "added" ? `Remove new ${what}` : `Drop ${what}`}
          onClick={onDrop}
          className="hover:text-error"
          tabIndex={-1}
        >
          <Trash2 />
        </IconAction>
      )}
    </div>
  );
}

const BOOL = [
  { value: "TRUE", label: "TRUE" },
  { value: "FALSE", label: "FALSE" },
] as const;

const DEFAULT_METHOD = "(default)";

/**
 * Indexes grid (TablePlus): index_name, index_algorithm, is_unique, column_name, include (PG
 * `INCLUDE` columns, set on new indexes), condition (PG / SQLite partial index) and comment
 * (MySQL / PG). Every other attribute is edited inline; an existing index is dropped and created
 * again with the change.
 */
export function IndexesSection({
  rows,
  dialect,
  editable,
  columns,
  columnTypes,
  onAdd,
  onEdit,
  onDrop,
  onRestore,
}: {
  rows: IndexRow[];
  dialect: Dialect;
  editable: boolean;
  /** Live column names (for column_name). */
  columns: readonly string[];
  /** Live column name → data type (MySQL: which columns take an index prefix length). */
  columnTypes?: ReadonlyMap<string, string>;
  onAdd: () => void;
  onEdit: (key: string, patch: Partial<IndexDef>) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
}) {
  const partial = supportsPartialIndex(dialect);
  const comments = supportsIndexComment(dialect);
  const include = supportsIncludeColumns(dialect);
  const methods = indexMethods(dialect);
  const methodChoices = [
    { value: DEFAULT_METHOD, label: DEFAULT_METHOD },
    ...methods.map((m) => ({ value: m, label: m })),
  ];
  const span = 5 + (include ? 1 : 0) + (partial ? 1 : 0) + (comments ? 1 : 0);
  const newKeys = shortcutFor("grid.new-row");
  const mysql = dialect === "mysql";
  /** MySQL: a prefix length per char / text / blob column (not for FULLTEXT and SPATIAL). */
  const lengthsOf = (r: IndexRow): PrefixLengths | undefined => {
    if (!mysql) return undefined;
    const whole = ["FULLTEXT", "SPATIAL"].includes((r.method ?? "").toUpperCase());
    const type = (c: string) => columnTypes?.get(c) ?? "";
    return {
      eligible: (c) => !whole && supportsIndexPrefix(type(c)),
      required: (c) => !whole && requiresIndexPrefix(type(c)),
    };
  };
  // (an entry `name(10)` is the chip of its column, not a chip of its own)
  const optionsOf = (r: IndexRow) => [
    ...new Set([...columns, ...r.columns.filter((e) => !mysql || !columns.includes(entryBase(e)))]),
  ];
  return (
    <section aria-label="Indexes">
      <SectionHeader title="Indexes" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Index
            {newKeys && <Kbd combo={newKeys} className="ml-0.5" />}
          </Button>
        )}
      </SectionHeader>
      <table
        data-grid="indexes"
        className="w-full min-w-[640px] table-fixed border-collapse text-xs"
      >
        <colgroup>
          <col className="w-[22%]" />
          {methods.length > 0 && <col className="w-28" />}
          <col className="w-20" />
          <col className="w-[22%]" />
          {include && <col className="w-[14%]" />}
          {partial && <col />}
          {comments && <col className={partial ? "w-[16%]" : undefined} />}
          <col className="w-16" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>index_name</Th>
            {methods.length > 0 && <Th>index_algorithm</Th>}
            <Th>is_unique</Th>
            <Th>column_name</Th>
            {include && <Th>include</Th>}
            {partial && <Th>condition</Th>}
            {comments && <Th>comment</Th>}
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const canEdit = editable && !r.primary && r.status !== "dropped";
            const text = (field: "name" | "predicate" | "comment", mono = true) => (
              <CellInput
                value={(field === "name" ? r.name : r[field]) ?? ""}
                onCommit={(raw) => {
                  const v = raw.trim();
                  if (field === "name") {
                    if (v) onEdit(r.key, { name: v });
                  } else onEdit(r.key, { [field]: v === "" ? null : raw.trim() });
                }}
                cell={`${i}|${field}`}
                cellKey={`${r.key}:${field}`}
                label={`${r.name} ${field === "predicate" ? "condition" : field}`}
                readOnly={!canEdit}
                className={cn(mono && "font-mono text-[11px]")}
              />
            );
            return (
              <tr
                key={r.key}
                className={cn(rowClass(r.status), "outline-none")}
                data-status={r.status}
                data-row-kind="index"
                data-row-key={r.key}
                data-index-key={r.key}
                // a row the Delete key can act on
                tabIndex={-1}
                onMouseDown={focusRowOnMouseDown}
                onKeyDown={(e) => {
                  if (editable && !r.primary)
                    rowDeleteKey(e, () =>
                      r.status === "dropped" ? onRestore(r.key) : onDrop(r.key),
                    );
                }}
              >
                <td className="px-1">
                  <div className="flex items-center">
                    {text("name")}
                    {r.primary && (
                      <span className="bg-brand-soft text-brand mr-1 rounded px-1 py-px font-sans text-[9px] font-semibold tracking-wide no-underline">
                        PRIMARY
                      </span>
                    )}
                  </div>
                </td>
                {methods.length > 0 && (
                  <td className="px-1">
                    <ChoiceCell
                      value={r.method ?? DEFAULT_METHOD}
                      options={
                        r.method && !methods.includes(r.method)
                          ? [...methodChoices, { value: r.method, label: r.method }]
                          : methodChoices
                      }
                      onChange={(v) => onEdit(r.key, { method: v === DEFAULT_METHOD ? null : v })}
                      label={`${r.name} algorithm`}
                      cellKey={`${r.key}:method`}
                      disabled={!canEdit}
                      className="font-mono text-[11px]"
                    />
                  </td>
                )}
                <td className="px-1">
                  <ChoiceCell
                    value={r.unique ? "TRUE" : "FALSE"}
                    options={BOOL}
                    keys={{ t: "TRUE", f: "FALSE", y: "TRUE", n: "FALSE" }}
                    onChange={(v) => onEdit(r.key, { unique: v === "TRUE" })}
                    label={`${r.name} unique`}
                    cellKey={`${r.key}:unique`}
                    disabled={!canEdit}
                    className="font-mono text-[11px]"
                  />
                </td>
                <td className="px-1">
                  <ColumnsCell
                    value={r.columns}
                    options={optionsOf(r)}
                    onChange={(next) => onEdit(r.key, { columns: next })}
                    label={`${r.name} columns`}
                    cellKey={`${r.key}:columns`}
                    disabled={!canEdit}
                    lengths={lengthsOf(r)}
                  />
                </td>
                {include && (
                  <td className="px-1">
                    <ColumnsCell
                      value={r.include}
                      options={[...new Set([...columns, ...r.include])]}
                      onChange={(next) => onEdit(r.key, { include: next })}
                      label={`${r.name} include`}
                      cellKey={`${r.key}:include`}
                      // Set on a new index; an existing one keeps its INCLUDE columns.
                      disabled={!canEdit || r.status !== "added"}
                      allowEmpty
                      placeholder={r.status === "added" && canEdit ? "none" : ""}
                      hint="INCLUDE columns (stored, not searched), in order"
                    />
                  </td>
                )}
                {partial && <td className="px-1">{text("predicate")}</td>}
                {comments && <td className="px-1">{text("comment", false)}</td>}
                <td className="px-1">
                  <DropToggle
                    status={r.status}
                    what="index"
                    disabled={!editable || r.primary}
                    onDrop={() => onDrop(r.key)}
                    onRestore={() => onRestore(r.key)}
                  />
                </td>
              </tr>
            );
          })}
          {rows.length === 0 && (
            <EmptyRow span={span} text="No indexes" kind={editable ? "indexes" : undefined} />
          )}
          {editable && (
            <BlankRow span={span} kind="indexes" text="+ double-click to add an index" />
          )}
        </tbody>
      </table>
    </section>
  );
}

/** CHECK constraints (every check of the table, multi-column ones too). */
export function ChecksSection({
  rows,
  editable,
  onAdd,
  onEdit,
  onDrop,
  onRestore,
}: {
  rows: CheckRow[];
  editable: boolean;
  onAdd: () => void;
  onEdit: (key: string, patch: { name?: string | null; expression?: string }) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
}) {
  return (
    <section aria-label="Checks">
      <SectionHeader title="Checks" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Check
          </Button>
        )}
      </SectionHeader>
      <table
        data-grid="checks"
        className="w-full min-w-[640px] table-fixed border-collapse text-xs"
      >
        <colgroup>
          <col className="w-[26%]" />
          <col />
          <col className="w-[20%]" />
          <col className="w-16" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>constraint_name</Th>
            <Th>check_clause</Th>
            <Th>column_name</Th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => {
            const canEdit = editable && r.status !== "dropped";
            return (
              <tr
                key={r.key}
                className={cn(rowClass(r.status), "outline-none")}
                data-status={r.status}
                data-row-kind="check"
                data-row-key={r.key}
                tabIndex={-1}
                onMouseDown={focusRowOnMouseDown}
                onKeyDown={(e) => {
                  if (editable)
                    rowDeleteKey(e, () =>
                      r.status === "dropped" ? onRestore(r.key) : onDrop(r.key),
                    );
                }}
              >
                <td className="px-1">
                  <CellInput
                    value={r.name}
                    onCommit={(raw) => onEdit(r.key, { name: raw.trim() || null })}
                    cell={`${i}|name`}
                    cellKey={`${r.key}:name`}
                    label={`Check ${r.name || i + 1} name`}
                    readOnly={!canEdit}
                    placeholder="chosen by the database"
                    className="font-mono text-[11px]"
                  />
                </td>
                <td className="px-1">
                  <CellInput
                    value={r.expression}
                    onCommit={(raw) => onEdit(r.key, { expression: raw })}
                    // A new check left without an expression goes away again.
                    onLeave={(raw) => {
                      if (r.status === "added" && raw.trim() === "") onDrop(r.key);
                    }}
                    cell={`${i}|expression`}
                    cellKey={`${r.key}:expression`}
                    label={`Check ${r.name || i + 1} expression`}
                    readOnly={!canEdit}
                    placeholder="price > 0"
                    className="font-mono text-[11px]"
                  />
                </td>
                <td
                  className="text-muted-foreground truncate px-3 font-mono text-[11px]"
                  title={r.columns.join(", ")}
                >
                  {r.columns.join(", ")}
                </td>
                <td className="px-1">
                  <DropToggle
                    status={r.status}
                    what="check"
                    disabled={!editable}
                    onDrop={() => onDrop(r.key)}
                    onRestore={() => onRestore(r.key)}
                  />
                </td>
              </tr>
            );
          })}
          {rows.length === 0 && <EmptyRow span={4} text="No CHECK constraints" />}
        </tbody>
      </table>
    </section>
  );
}

export function ForeignKeysSection({
  rows,
  editable,
  onAdd,
  onDrop,
  onRestore,
  onOpenTable,
}: {
  rows: FkRow[];
  editable: boolean;
  onAdd: () => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onOpenTable: (table: TableRef) => void;
}) {
  return (
    <section aria-label="Foreign keys">
      <SectionHeader title="Foreign keys" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Foreign key
          </Button>
        )}
      </SectionHeader>
      <table className="w-full min-w-[640px] table-fixed border-collapse text-xs">
        <colgroup>
          <col className="w-[26%]" />
          <col className="w-[18%]" />
          <col />
          <col className="w-24" />
          <col className="w-24" />
          <col className="w-12" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>constraint_name</Th>
            <Th>column_name</Th>
            <Th>references</Th>
            <Th>on_update</Th>
            <Th>on_delete</Th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr
              key={r.key}
              className={rowClass(r.status)}
              data-status={r.status}
              data-row-kind="fk"
              data-row-key={r.key}
              tabIndex={-1}
              onKeyDown={(e) => {
                if (editable && (r.status !== "existing" || r.name))
                  rowDeleteKey(e, () =>
                    r.status === "dropped" ? onRestore(r.key) : onDrop(r.key),
                  );
              }}
            >
              <td className="truncate px-3 font-mono text-[11px]" title={r.name ?? undefined}>
                {r.name ?? <span className="text-muted-foreground italic">auto</span>}
              </td>
              <td className="truncate px-3 font-mono text-[11px]">{r.columns.join(", ")}</td>
              <td className="truncate px-3 font-mono text-[11px]">
                <button
                  type="button"
                  className="text-brand hover:underline"
                  onClick={() => onOpenTable(r.refTable)}
                  title="Open structure"
                >
                  {r.refTable.name}
                </button>
                <span className="text-muted-foreground">({r.refColumns.join(", ")})</span>
              </td>
              <td className="text-muted-foreground truncate px-3 text-[11px]">
                {r.onUpdate ?? "default"}
              </td>
              <td className="text-muted-foreground truncate px-3 text-[11px]">
                {r.onDelete ?? "default"}
              </td>
              <td className="px-1">
                <DropToggle
                  status={r.status}
                  what="foreign key"
                  disabled={!editable || (r.status === "existing" && !r.name)}
                  onDrop={() => onDrop(r.key)}
                  onRestore={() => onRestore(r.key)}
                />
              </td>
            </tr>
          ))}
          {rows.length === 0 && <EmptyRow span={6} text="No foreign keys" />}
        </tbody>
      </table>
    </section>
  );
}

/**
 * Triggers grid (TablePlus "Triggers" view): name, timing, events, enabled (PostgreSQL: staged
 * `ENABLE` / `DISABLE TRIGGER`, `t` / `f` keys; a disabled trigger's row is muted) and the
 * statement. "+ Trigger" and Edit open the trigger's CREATE statement in an editor; Delete stages
 * a drop.
 */
export function TriggersSection({
  rows,
  dialect,
  editable,
  onAdd,
  onOpen,
  onDrop,
  onRestore,
  onSetEnabled,
}: {
  rows: TriggerRow[];
  dialect: Dialect;
  editable: boolean;
  onAdd: () => void;
  onOpen: (key: string) => void;
  onDrop: (key: string) => void;
  onRestore: (key: string) => void;
  onSetEnabled?: (key: string, enabled: boolean) => void;
}) {
  const enabledColumn = supportsTriggerToggle(dialect);
  return (
    <section aria-label="Triggers" className="pb-6">
      <SectionHeader title="Triggers" count={rows.filter((r) => r.status !== "dropped").length}>
        {editable && (
          <Button variant="ghost" size="xs" onClick={onAdd}>
            <Plus /> Trigger
          </Button>
        )}
      </SectionHeader>
      <table
        data-grid="triggers"
        className="w-full min-w-[640px] table-fixed border-collapse text-xs"
      >
        <colgroup>
          <col className="w-[24%]" />
          <col className="w-24" />
          <col className="w-[16%]" />
          {enabledColumn && <col className="w-20" />}
          <col />
          <col className="w-16" />
        </colgroup>
        <thead>
          <tr className={HEAD_ROW}>
            <Th>trigger_name</Th>
            <Th>action_timing</Th>
            <Th>event</Th>
            {enabledColumn && <Th className="text-center">enabled</Th>}
            <Th>statement</Th>
            <th aria-label="Actions" />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const statement = (r.sql ?? r.definition).replace(/\s+/g, " ").trim();
            const disabled = enabledColumn && !r.enabled && r.status !== "dropped";
            return (
              <tr
                key={r.key}
                className={cn(rowClass(r.status), disabled && "text-muted-foreground")}
                data-status={r.status}
                data-enabled={enabledColumn ? String(r.enabled) : undefined}
                data-row-kind="trigger"
                data-row-key={r.key}
                tabIndex={-1}
                onDoubleClick={() => r.status !== "dropped" && onOpen(r.key)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && e.target === e.currentTarget && r.status !== "dropped")
                    onOpen(r.key);
                  else if (editable)
                    rowDeleteKey(e, () =>
                      r.status === "dropped" ? onRestore(r.key) : onDrop(r.key),
                    );
                }}
              >
                <td
                  className={cn("truncate px-3 font-mono text-[11px]", disabled && "italic")}
                  title={disabled ? `${r.name} (disabled)` : r.name}
                >
                  {r.name}
                </td>
                <td className="text-muted-foreground truncate px-3 text-[11px]">
                  {r.timing ?? ""}
                </td>
                <td className="truncate px-3 text-[11px]">{r.events.join(" OR ")}</td>
                {enabledColumn && (
                  <td
                    className={cn(
                      "px-1",
                      r.enabledAt !== null && r.status === "existing" && "bg-changed",
                    )}
                    // the cell's own clicks do not open the trigger's statement
                    onDoubleClick={(e) => e.stopPropagation()}
                  >
                    <ChoiceCell
                      value={r.enabled ? "TRUE" : "FALSE"}
                      options={BOOL}
                      keys={{ t: "TRUE", f: "FALSE", y: "TRUE", n: "FALSE" }}
                      onChange={(v) => onSetEnabled?.(r.key, v === "TRUE")}
                      label={`${r.name} enabled`}
                      cellKey={`${r.key}:enabled`}
                      disabled={!editable || !onSetEnabled || r.status === "dropped"}
                      className="justify-center font-mono text-[11px]"
                    />
                  </td>
                )}
                <td
                  className="text-muted-foreground selectable truncate px-3 font-mono text-[11px]"
                  title={statement}
                >
                  {statement}
                </td>
                <td className="px-1">
                  <DropToggle
                    status={r.status}
                    what="trigger"
                    disabled={!editable}
                    onDrop={() => onDrop(r.key)}
                    onRestore={() => onRestore(r.key)}
                  >
                    {r.status !== "dropped" && (
                      <IconAction label="Edit trigger" onClick={() => onOpen(r.key)} tabIndex={-1}>
                        <Pencil />
                      </IconAction>
                    )}
                  </DropToggle>
                  {!editable && (
                    <div className="flex justify-end">
                      <IconAction label="Show trigger" onClick={() => onOpen(r.key)} tabIndex={-1}>
                        <Pencil />
                      </IconAction>
                    </div>
                  )}
                </td>
              </tr>
            );
          })}
          {rows.length === 0 && (
            <EmptyRow
              span={enabledColumn ? 6 : 5}
              text="No triggers"
              kind={editable ? "triggers" : undefined}
            />
          )}
          {editable && (
            <BlankRow
              span={enabledColumn ? 6 : 5}
              kind="triggers"
              text="+ double-click to add a trigger"
            />
          )}
        </tbody>
      </table>
    </section>
  );
}
