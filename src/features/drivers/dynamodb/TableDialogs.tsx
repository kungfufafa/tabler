import { QuietStatus } from "@/components/QuietLoading";
import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { toIpcError, type BrowsePage, type TableRef } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { displayValue } from "@/lib/values";
import { tableFacts, type IndexChoice, type TableInfo } from "./info";

/** The table info panel and the index browser, opened from a table's menu (`dialogs.ts`). */

/** A session call that only reads (`ObjectMenuContext.read`). */
export type ReadCall = <T = unknown>(method: string, args?: unknown) => Promise<T>;

function useOpen(close: () => void) {
  const [open, setOpen] = useState(true);
  const onOpenChange = (o: boolean) => {
    if (o) return;
    setOpen(false);
    close();
  };
  return { open, onOpenChange };
}

/** `tableInfo`'s facts of a table. */
export function TableInfoDialog({
  table,
  info,
  close,
}: {
  table: string;
  info: TableInfo;
  close: () => void;
}) {
  const { open, onOpenChange } = useOpen(close);
  const facts = tableFacts(info);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[34rem]">
        <DialogHeader>
          <DialogTitle className="text-sm">{table}</DialogTitle>
          <DialogDescription className="text-xs">
            DynamoDB refreshes the item count and size about every six hours.
          </DialogDescription>
        </DialogHeader>
        <dl
          className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-xs"
          data-testid="dynamo-table-info"
        >
          {facts.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="font-mono break-all">{value}</dd>
            </div>
          ))}
        </dl>
        <DialogFooter>
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Items of an index page shown at once. */
const PAGE = 50;

type PageState =
  { state: "loading" } | { state: "done"; page: BrowsePage } | { state: "failed"; message: string };

/**
 * Pages of a secondary index (`browseIndex`), with its cursor to the next page and the earlier
 * pages' cursors to go back: an index's items are read only (the grid edits the table's).
 */
export function IndexBrowserDialog({
  read,
  table,
  indexes,
  close,
}: {
  read: ReadCall;
  table: TableRef;
  indexes: readonly IndexChoice[];
  close: () => void;
}) {
  const { open, onOpenChange } = useOpen(close);
  const [index, setIndex] = useState(indexes[0]?.name ?? "");
  /** The cursors of the pages shown so far: null starts the first. */
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [page, setPage] = useState<PageState>({ state: "loading" });
  const cursor = cursors[cursors.length - 1] ?? null;

  const load = useCallback(async () => {
    setPage({ state: "loading" });
    try {
      const request = {
        table,
        columns: null,
        filter: null,
        sort: [],
        limit: PAGE,
        offset: 0,
        cursor,
      };
      const p = await read<BrowsePage>("browseIndex", {
        table: table.name,
        index,
        request,
      });
      setPage({ state: "done", page: p });
    } catch (err) {
      setPage({ state: "failed", message: toIpcError(err).message });
    }
  }, [read, table, index, cursor]);

  useEffect(() => {
    void load();
  }, [load]);

  const chosen = indexes.find((i) => i.name === index);
  const next = page.state === "done" ? (page.page.nextCursor ?? null) : null;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(40rem,calc(100dvh-4rem))] flex-col sm:max-w-[min(56rem,calc(100%-2rem))]">
        <DialogHeader>
          <DialogTitle className="text-sm">Browse an Index of {table.name}</DialogTitle>
          <DialogDescription className="text-xs">
            {chosen
              ? `${chosen.global ? "Global" : "Local"} index on ${chosen.keys.join(", ")}: its items are read only here.`
              : "Read only."}
          </DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-2">
          <Select
            value={index}
            onValueChange={(v) => {
              setIndex(v);
              setCursors([null]);
            }}
          >
            <SelectTrigger size="sm" className="w-64 text-xs" aria-label="Index">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {indexes.map((i) => (
                <SelectItem key={i.name} value={i.name} className="text-xs">
                  {i.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span
            className={cn(
              "text-xs tabular-nums",
              page.state === "failed" ? "text-error" : "text-muted-foreground",
            )}
            role="status"
          >
            {page.state === "loading" ? (
              <QuietStatus />
            ) : page.state === "failed" ? (
              page.message
            ) : (
              `Page ${cursors.length}: ${page.page.rows.length} item${page.page.rows.length === 1 ? "" : "s"}`
            )}
          </span>
        </div>
        <div className="min-h-0 flex-1 overflow-auto rounded-md border">
          {page.state === "done" && (
            <table className="w-full text-xs" data-testid="dynamo-index-page">
              <thead className="bg-surface-2/95 sticky top-0">
                <tr>
                  {page.page.columns.map((c) => (
                    <th
                      key={c.name}
                      className="text-muted-foreground h-7 px-2 text-left font-medium whitespace-nowrap"
                    >
                      {c.name}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {page.page.rows.length === 0 && (
                  <tr>
                    <td
                      colSpan={Math.max(page.page.columns.length, 1)}
                      className="text-muted-foreground py-6 text-center text-xs"
                    >
                      No items
                    </td>
                  </tr>
                )}
                {page.page.rows.map((row, r) => (
                  <tr key={r} className="hover:bg-row-hover border-t">
                    {row.map((v, c) => (
                      <td key={c} className="h-7 max-w-64 truncate px-2 font-mono">
                        {displayValue(v, true).text}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
        <DialogFooter className="sm:justify-between">
          <div className="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={cursors.length < 2 || page.state === "loading"}
              onClick={() => setCursors((c) => c.slice(0, -1))}
            >
              Previous
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={next === null || page.state === "loading"}
              onClick={() => setCursors((c) => [...c, next])}
            >
              Next
            </Button>
          </div>
          <Button size="sm" onClick={() => onOpenChange(false)}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
