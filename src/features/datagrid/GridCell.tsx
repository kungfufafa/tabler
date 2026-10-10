import { ArrowUpRight } from "lucide-react";
import { memo } from "react";
import type { Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { displayValue } from "@/lib/values";
import type { GridColumn, RowStatus } from "./types";

const CLS: Record<string, string> = {
  null: "text-null italic",
  default: "text-null italic font-mono",
  bool: "font-mono text-syn-keyword",
  num: "font-mono tabular-nums",
  json: "font-mono text-muted-foreground",
  bytes: "font-mono text-muted-foreground",
  other: "font-mono text-muted-foreground",
  temporal: "font-mono",
  uuid: "font-mono text-muted-foreground",
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
  const d = displayValue(value);
  const hasFk = col.fk !== undefined && value.t !== "null";
  return (
    <div
      data-cell=""
      data-r={r}
      data-c={c}
      title={d.text.length > 40 ? d.text.slice(0, 500) : undefined}
      className={cn(
        "border-gridline absolute top-0 flex h-full items-center overflow-hidden border-r border-b px-2 text-[12.5px] leading-7 whitespace-nowrap",
        col.align === "right" && "justify-end",
        CLS[d.cls],
        changed && "bg-changed",
        rowStatus === "inserted" && "bg-inserted",
        rowStatus === "deleted" && "bg-deleted line-through opacity-70",
      )}
      style={{ left, width }}
    >
      <span className="truncate">{d.text}</span>
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
