import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { formatCount } from "@/lib/format/time";
import type { RowCount } from "@/lib/ipc";
import { PAGE_SIZES } from "@/stores/prefs";

export function Pagination({
  offset,
  limit,
  pageRows,
  count,
  countPending,
  onChange,
  onCountExact,
}: {
  offset: number;
  limit: number;
  pageRows: number;
  count: RowCount | null;
  countPending: boolean;
  onChange: (next: { offset: number; limit: number }) => void;
  onCountExact: () => void;
}) {
  const total = count?.count ?? null;
  const page = Math.floor(offset / limit) + 1;
  const pages = total !== null ? Math.max(1, Math.ceil(total / limit)) : null;
  const from = pageRows === 0 ? 0 : offset + 1;
  const to = offset + pageRows;
  const hasNext = total !== null ? offset + limit < total : pageRows === limit;
  return (
    <div className="border-border bg-surface-2 text-muted-foreground flex h-8 shrink-0 items-center gap-1 border-t px-2 text-xs">
      <Select
        value={String(limit)}
        onValueChange={(v) => onChange({ offset: 0, limit: Number(v) })}
      >
        <SelectTrigger
          size="sm"
          className="h-6 w-auto gap-1 border-transparent bg-transparent px-1.5 text-xs shadow-none"
        >
          <SelectValue />
          <span className="text-muted-foreground">rows</span>
        </SelectTrigger>
        <SelectContent>
          {PAGE_SIZES.map((n) => (
            <SelectItem key={n} value={String(n)} className="text-xs">
              {n}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <span className="flex-1" />
      <span className="whitespace-nowrap tabular-nums">
        {from.toLocaleString()}–{to.toLocaleString()}
        {total !== null ? (
          <>
            {" of "}
            {count?.exact ? (
              formatCount(total)
            ) : (
              <button
                type="button"
                className="hover:text-foreground underline decoration-dotted underline-offset-2"
                title="Estimated — click for an exact count"
                onClick={onCountExact}
              >
                {formatCount(total, true)}
              </button>
            )}
          </>
        ) : countPending ? (
          " of …"
        ) : null}
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="First page"
        disabled={offset === 0}
        onClick={() => onChange({ offset: 0, limit })}
      >
        <ChevronsLeft />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Previous page"
        disabled={offset === 0}
        onClick={() => onChange({ offset: Math.max(0, offset - limit), limit })}
      >
        <ChevronLeft />
      </Button>
      <span className="min-w-14 text-center tabular-nums">
        {page}
        {pages !== null ? ` / ${pages.toLocaleString()}` : ""}
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Next page"
        disabled={!hasNext}
        onClick={() => onChange({ offset: offset + limit, limit })}
      >
        <ChevronRight />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Last page"
        disabled={pages === null || page >= pages}
        onClick={() => pages !== null && onChange({ offset: (pages - 1) * limit, limit })}
      >
        <ChevronsRight />
      </Button>
    </div>
  );
}
