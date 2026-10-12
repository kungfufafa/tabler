import { ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Settings2 } from "lucide-react";
import { useId, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
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
import { BarIconButton, BarTooltip } from "./BarParts";

export interface PageChange {
  offset: number;
  limit: number;
}

/** The page sizes offered for `limit` (the tab's own size is kept when it is not a preset). */
function pageSizes(limit: number): number[] {
  const sizes: number[] = [...PAGE_SIZES];
  return sizes.includes(limit) ? sizes : [...sizes, limit].sort((a, b) => a - b);
}

/**
 * TablePlus's `‹ ⚙ ›` paging control: previous / next page, and a popover with the page size
 * (Limit), a free Offset, the first / last page and the exact row count.
 */
export function Pagination({
  offset,
  limit,
  pageRows,
  count,
  onChange,
  onCountExact,
}: {
  offset: number;
  limit: number;
  pageRows: number;
  count: RowCount | null;
  onChange: (next: PageChange) => void;
  onCountExact: () => void;
}) {
  const total = count?.count ?? null;
  const page = Math.floor(offset / limit) + 1;
  const pages = total !== null ? Math.max(1, Math.ceil(total / limit)) : null;
  const hasNext = total !== null ? offset + limit < total : pageRows === limit;
  const id = useId();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState({ limit: String(limit), offset: String(offset) });
  const openChange = (next: boolean) => {
    if (next) setDraft({ limit: String(limit), offset: String(offset) });
    setOpen(next);
  };
  /** Closes the popover first: the change may ask about pending edits in a dialog. */
  const go = (next: PageChange) => {
    setOpen(false);
    onChange(next);
  };
  const offsetValue = Number(draft.offset);
  const offsetValid =
    draft.offset.trim() !== "" && Number.isFinite(offsetValue) && offsetValue >= 0;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!offsetValid) return;
    go({ offset: Math.floor(offsetValue), limit: Number(draft.limit) });
  };
  return (
    <div className="flex shrink-0 items-center">
      <BarIconButton
        label="Previous page"
        disabled={offset === 0}
        onClick={() => onChange({ offset: Math.max(0, offset - limit), limit })}
      >
        <ChevronLeft />
      </BarIconButton>
      <Popover open={open} onOpenChange={openChange}>
        <BarTooltip label="Page size and offset">
          <PopoverTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Pagination settings"
              className="text-muted-foreground hover:text-foreground"
            >
              <Settings2 />
            </Button>
          </PopoverTrigger>
        </BarTooltip>
        <PopoverContent side="top" align="end" className="w-60 gap-2 p-2.5 text-xs">
          <div className="text-muted-foreground flex items-center justify-between tabular-nums">
            <span>
              Page {page.toLocaleString()}
              {pages !== null ? ` of ${pages.toLocaleString()}` : ""}
            </span>
            {total !== null && (
              <span title={count?.exact ? "Exact row count" : "Estimated row count"}>
                {formatCount(total, !count?.exact)} rows
              </span>
            )}
          </div>
          <form
            onSubmit={submit}
            className="grid grid-cols-[auto_1fr] items-center gap-x-2 gap-y-1.5"
          >
            <label htmlFor={`${id}-limit`} className="text-muted-foreground">
              Limit
            </label>
            <Select
              value={draft.limit}
              onValueChange={(v) => setDraft((d) => ({ ...d, limit: v }))}
            >
              <SelectTrigger
                id={`${id}-limit`}
                size="sm"
                aria-label="Limit"
                className="h-7 w-full text-xs"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {pageSizes(limit).map((n) => (
                  <SelectItem key={n} value={String(n)} className="text-xs tabular-nums">
                    {n.toLocaleString()} rows
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <label htmlFor={`${id}-offset`} className="text-muted-foreground">
              Offset
            </label>
            <Input
              id={`${id}-offset`}
              type="number"
              min={0}
              step={1}
              inputMode="numeric"
              value={draft.offset}
              onChange={(e) => setDraft((d) => ({ ...d, offset: e.target.value }))}
              aria-invalid={!offsetValid || undefined}
              className="h-7 text-xs tabular-nums"
            />
            <div className="col-span-2 flex items-center gap-1 pt-0.5">
              <Button
                type="button"
                variant="outline"
                size="xs"
                disabled={offset === 0}
                onClick={() => go({ offset: 0, limit })}
              >
                <ChevronsLeft />
                First page
              </Button>
              <Button
                type="button"
                variant="outline"
                size="xs"
                disabled={pages === null || page >= pages}
                onClick={() => pages !== null && go({ offset: (pages - 1) * limit, limit })}
              >
                Last page
                <ChevronsRight />
              </Button>
              <span className="flex-1" />
              <Button type="submit" size="xs" disabled={!offsetValid}>
                Go
              </Button>
            </div>
          </form>
          <div className="border-border flex items-center border-t pt-2">
            <Button
              type="button"
              variant="ghost"
              size="xs"
              className="-ml-1"
              disabled={count?.exact === true}
              onClick={() => {
                setOpen(false);
                onCountExact();
              }}
            >
              Count rows exactly
            </Button>
          </div>
        </PopoverContent>
      </Popover>
      <BarIconButton
        label="Next page"
        disabled={!hasNext}
        onClick={() => onChange({ offset: offset + limit, limit })}
      >
        <ChevronRight />
      </BarIconButton>
    </div>
  );
}
