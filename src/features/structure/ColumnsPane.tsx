import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { type ColumnField } from "./columnEdits";
import { MIXED, commonValue, type ColumnSelection } from "./columnSelection";
import type { ColumnRow } from "./model";

const MULTIPLE = "Multiple values";

interface FieldSpec {
  field: ColumnField;
  label: string;
  /** The text the field starts from: the columns' common value, "" when they differ. */
  text: string;
  mixed: boolean;
  placeholder?: string;
}

function TextField({
  spec,
  readOnly,
  onCommit,
}: {
  spec: FieldSpec;
  readOnly: boolean;
  onCommit: (raw: string) => void;
}) {
  const [draft, setDraft] = useState(spec.text);
  // A new common value (an edit staged, another selection) starts the box afresh.
  useEffect(() => setDraft(spec.text), [spec.text, spec.mixed]);
  const dirty = draft !== spec.text;
  const commit = () => {
    if (dirty) onCommit(draft);
  };
  return (
    <input
      aria-label={spec.label}
      value={draft}
      readOnly={readOnly}
      placeholder={spec.mixed ? MULTIPLE : spec.placeholder}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape") {
          e.preventDefault();
          setDraft(spec.text);
        }
      }}
      spellCheck={false}
      autoComplete="off"
      className={cn(
        "border-border bg-surface placeholder:text-muted-foreground focus:border-ring focus:ring-ring/30 h-6 w-full min-w-0 rounded-md border px-2 font-mono text-[11px] outline-none focus:ring-2",
        spec.mixed && !dirty && "placeholder:italic",
        readOnly && "opacity-70",
      )}
    />
  );
}

function NullableField({
  mixed,
  value,
  readOnly,
  onCommit,
}: {
  mixed: boolean;
  value: boolean;
  readOnly: boolean;
  onCommit: (nullable: boolean) => void;
}) {
  return (
    <select
      aria-label="Nullable"
      disabled={readOnly}
      value={mixed ? "" : value ? "YES" : "NO"}
      onChange={(e) => {
        if (e.target.value) onCommit(e.target.value === "YES");
      }}
      className="border-border bg-surface focus:border-ring focus:ring-ring/30 h-6 w-full min-w-0 rounded-md border px-1.5 font-mono text-[11px] outline-none focus:ring-2 disabled:opacity-70"
    >
      {mixed && (
        <option value="" disabled>
          {MULTIPLE}
        </option>
      )}
      <option value="YES">YES</option>
      <option value="NO">NO</option>
    </select>
  );
}

/**
 * The right sidebar while columns are selected in a structure view (TablePlus: bulk edit of
 * columns, Space): the common attributes of the selected columns — type, nullable, default,
 * comment — with "Multiple values" where they differ; an edit stages on all of them. One
 * selected column also shows its name.
 */
export function ColumnsPane({ selection }: { selection: ColumnSelection }) {
  const { rows, editable, onEdit } = selection;
  const live = rows.filter((r) => r.status !== "dropped");
  // Dropped columns are shown but not edited.
  const target = live.length > 0 ? live : rows;
  const canEdit = editable && live.length > 0;
  const text = (pick: (r: ColumnRow) => string | null) => {
    const v = commonValue(target, (r) => pick(r) ?? "");
    return { text: v === MIXED ? "" : v, mixed: v === MIXED };
  };
  const nullable = commonValue(target, (r) => r.nullable);
  const spec = (
    field: ColumnField,
    label: string,
    pick: (r: ColumnRow) => string | null,
    placeholder?: string,
  ): FieldSpec => ({
    field,
    label,
    placeholder,
    ...text(pick),
  });
  const textField = (s: FieldSpec) => (
    <label key={s.field} className="flex flex-col gap-1 text-[11px] font-medium">
      {s.label}
      <TextField spec={s} readOnly={!canEdit} onCommit={(raw) => onEdit(s.field, raw)} />
    </label>
  );
  const title =
    rows.length === 1 ? (rows[0]?.name ?? "") : `${rows.length.toLocaleString()} columns selected`;
  return (
    <div className="flex min-h-0 flex-1 flex-col" data-slot="structure-columns-pane">
      <div
        role="status"
        className="border-border text-foreground bg-brand-soft shrink-0 border-b px-3 py-1.5 text-xs font-medium"
      >
        <span className="truncate">{title}</span>
        {rows.length > 1 && canEdit && (
          <span className="text-muted-foreground font-normal"> · edits apply to all of them</span>
        )}
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-2.5 overflow-y-auto px-3 py-2.5">
        {target.length === 1 && textField(spec("name", "Name", (r) => r.name))}
        {textField(spec("dataType", "Type", (r) => r.dataType))}
        <label className="flex flex-col gap-1 text-[11px] font-medium">
          Nullable
          <NullableField
            mixed={nullable === MIXED}
            value={nullable === MIXED ? true : nullable}
            readOnly={!canEdit}
            onCommit={(next) => onEdit("nullable", next)}
          />
        </label>
        {textField(spec("defaultExpr", "Default", (r) => r.defaultExpr, "NULL"))}
        {textField(spec("comment", "Comment", (r) => r.comment))}
        {!canEdit && (
          <p className="text-muted-foreground text-[11px]">
            {editable ? "Dropped columns cannot be edited." : "This structure is read-only."}
          </p>
        )}
      </div>
    </div>
  );
}
