import { ArrowUpRight } from "lucide-react";
import { memo } from "react";
import type { Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { cellDisplay, type CellDisplayClass } from "@/lib/values";
import type { GridColumn, RowStatus } from "./types";

/**
 * One font for every kind (TablePlus); the cell sets `tabular-nums` so digits line up. Only the
 * placeholders (NULL / DEFAULT / EMPTY, a blob's size) are muted.
 */
const CLS: Record<CellDisplayClass, string> = {
  null: "text-null italic",
  default: "text-null italic",
  empty: "text-null text-[10px] tracking-wide italic",
  bool: "",
  num: "",
  json: "",
  bytes: "text-muted-foreground",
  other: "",
  temporal: "",
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
  const hasFk = col.fk !== undefined && value.t !== "null";
  return (
    <div
      data-cell=""
      data-r={r}
      data-c={c}
      title={!d.placeholder && d.text.length > 40 ? d.text.slice(0, 500) : undefined}
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
      <span className={cn("truncate", CLS[d.cls])}>{d.text}</span>
      {hasFk && (
        <button
          type="button"
          data-fk=""
          tabIndex={-1}
          aria-label={`Open ${col.fk!.table}`}
          className="text-brand hover:bg-brand-soft ml-auto hidden size-4 shrink-0 items-center justify-center rounded group-hover/row:flex"
        >
          <ArrowUpRight className="size-3" />
        </button>
      )}
    </div>
  );
});
