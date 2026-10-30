import { ArrowUpRight } from "lucide-react";
import { memo } from "react";
import type { Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { cellDisplay, type CellDisplayClass } from "@/lib/values";
import { looksLikeWkt, splitEwkt } from "./geometry/wkt";
import { fksOf, type GridColumn, type RowStatus } from "./types";

/**
 * One font for every kind (TablePlus); the cell sets `tabular-nums` so digits line up. Only the
 * placeholders (NULL / DEFAULT / EMPTY, a blob's size) are muted.
 */
const CLS: Record<CellDisplayClass, string> = {
  null: "text-null italic",
  default: "text-null italic",
  // A field the row does not have (a document store's): apart from NULL.
  missing: "text-null",
  empty: "text-null text-[10px] tracking-wide italic",
  bool: "",
  num: "",
  json: "",
  bytes: "text-muted-foreground",
  other: "",
  // Settings ▸ Appearance ▸ Data table ▸ Date text.
  temporal: "text-(color:--grid-date)",
  uuid: "",
  text: "",
};

export interface GridCellProps {
  r: number;
  c: number;
  value: Value;
  col: GridColumn;
  left: number;
  width: number;
  rowStatus?: RowStatus;
  changed?: boolean;
}

export const GridCell = memo(function GridCell({
  r,
  c,
  value,
  col,
  left,
  width,
  rowStatus,
  changed,
}: GridCellProps) {
  const d = cellDisplay(value);
  // A spatial value reads as its WKT; the SRID shows in the hover preview and Quick Look.
  const text =
    col.kind === "geometry" && value.t === "other" && looksLikeWkt(d.text)
      ? splitEwkt(d.text).wkt
      : d.text;
  const fks = fksOf(col);
  const hasFk = fks.length > 0 && value.t !== "null";
  // No `title`: the grid sets one on hover for a cut-off value when "Allows expansion Tool Tips"
  // is on (Preferences ▸ Table Data, off by default as in TablePlus).
  return (
    <div
      data-cell=""
      data-r={r}
      data-c={c}
      className={cn(
        "border-gridline absolute top-0 flex h-full items-center overflow-hidden border-r border-b px-2 text-[12px] leading-4 whitespace-nowrap tabular-nums",
        // Numbers (int, bigint, decimal, float) right-aligned, everything else left-aligned.
        col.align === "right" && "justify-end",
        changed && "bg-changed",
        rowStatus === "inserted" && "bg-inserted",
        rowStatus === "deleted" && "bg-deleted line-through opacity-70",
      )}
      style={{ left, width }}
    >
      <span className={cn("truncate", CLS[d.cls])}>{text}</span>
      {hasFk && (
        <button
          type="button"
          data-fk=""
          tabIndex={-1}
          aria-label={fks.length > 1 ? "Open a referenced table" : `Open ${fks[0]!.table}`}
          title={fks.length > 1 ? `${fks.length} foreign keys: pick the table to open` : undefined}
          className="text-brand hover:bg-brand-soft ml-auto hidden size-4 shrink-0 items-center justify-center rounded group-hover/row:flex"
        >
          <ArrowUpRight className="size-3" />
        </button>
      )}
    </div>
  );
});
