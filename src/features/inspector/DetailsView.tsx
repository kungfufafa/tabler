import { Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Value } from "@/lib/ipc";
import type { InspectedCell, InspectedRow } from "@/stores/inspector";
import { FieldBlock } from "./FieldBlock";
import { matchesField } from "./fields";

const NULL: Value = { t: "null" };

/**
 * The Details view: a "Search for field…" box, then one block per column of the focused row
 * (hidden grid columns too, muted). On a table tab with an editable table the boxes stage edits
 * through `row.edit`; everywhere else they are read-only and copyable. With several rows selected
 * it reads "N rows selected": fields whose values differ show "Multiple values", and an edit
 * stages on every selected row.
 */
export function DetailsView({
  cell,
  onViewCell,
}: {
  cell: InspectedCell;
  /** Shows source column `col` of the row in the Cell view. */
  onViewCell: (col: number) => void;
}) {
  const row: InspectedRow = cell.row ?? {
    columns: [cell.column],
    cells: [cell.value],
    focused: 0,
  };
  const [query, setQuery] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const hidden = useMemo(() => new Set(row.hidden ?? []), [row.hidden]);
  const bulk = row.bulk;
  const rowKey = bulk
    ? `${cell.source}\u0000bulk\u0000${bulk.id}`
    : `${cell.source}\u0000${cell.rowIndex}`;
  const edit = row.status === "deleted" ? undefined : row.edit;
  const fields = row.columns
    .map((col, i) => ({ col, i }))
    .filter(({ col }) => matchesField(col, query));

  // Bring the grid's focused column into view when the focus moves (not on every re-publish).
  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>("[data-focused='true']")
      ?.scrollIntoView?.({ block: "nearest" });
  }, [row.focused, rowKey]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {bulk && (
        <div
          role="status"
          className="border-border text-foreground bg-brand-soft shrink-0 border-b px-3 py-1.5 text-xs font-medium"
        >
          {bulk.count.toLocaleString()} rows selected
          {edit && (
            <span className="text-muted-foreground font-normal"> · edits apply to all of them</span>
          )}
        </div>
      )}
      <div className="shrink-0 px-2 pt-2 pb-1">
        <div className="relative">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 size-3 -translate-y-1/2" />
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape" && query !== "") {
                e.preventDefault();
                setQuery("");
              }
            }}
            placeholder="Search for field…"
            aria-label="Search for field"
            spellCheck={false}
            autoComplete="off"
            className="border-border bg-surface placeholder:text-muted-foreground focus:border-ring focus:ring-ring/30 h-6 w-full min-w-0 rounded-md border pr-2 pl-6 text-xs outline-none focus:ring-2 [&::-webkit-search-cancel-button]:hidden"
          />
        </div>
      </div>
      <div
        ref={listRef}
        role="list"
        aria-label="Row fields"
        className="min-h-0 flex-1 overflow-y-auto px-1 pb-2"
      >
        {fields.length === 0 && (
          <div className="text-muted-foreground px-2 py-4 text-center text-xs">
            No matching fields
          </div>
        )}
        {fields.map(({ col, i }) => (
          <FieldBlock
            key={`${i}\u0000${col.id}`}
            col={col}
            value={row.cells[i] ?? NULL}
            rowKey={rowKey}
            focused={i === row.focused}
            hidden={hidden.has(col.id)}
            changed={row.changed?.has(i) ?? false}
            mixed={bulk?.mixed.has(i) ?? false}
            onEdit={edit && col.editable ? (value) => edit(i, value) : undefined}
            onViewCell={() => onViewCell(i)}
          />
        ))}
      </div>
    </div>
  );
}
