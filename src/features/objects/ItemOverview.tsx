import { useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Loader2, RefreshCw } from "lucide-react";
import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { toast } from "sonner";
import { TableIcon } from "@/components/TypeIcon";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { invalidateSchema } from "@/features/structure/invalidate";
import { KIND_LABEL } from "@/features/structure/tableActions";
import { formatCount } from "@/lib/format/time";
import type { OverviewItem } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useTabsStore } from "@/stores/tabs";
import { useUiStore } from "@/stores/ui";
import { guardedApply } from "./apply";
import {
  commentEditable,
  DEFAULT_OVERVIEW_SORT,
  formatBytes,
  nextSort,
  overviewColumns,
  sortOverview,
  type OverviewColumnDef,
  type OverviewSort,
} from "./overview";
import { useOverview } from "./queries";
import { compileSearch } from "./search";
import type { OverviewRequest } from "./store";

/** The sort survives closing and reopening (and refreshing) the overview. */
let lastSort: OverviewSort = DEFAULT_OVERVIEW_SORT;

function cellText(item: OverviewItem, col: OverviewColumnDef): string {
  switch (col.id) {
    case "kind":
      return KIND_LABEL[item.kind];
    case "estimatedRows":
      return formatCount(item.estimatedRows);
    case "totalSize":
    case "dataSize":
    case "indexSize":
      return formatBytes(item[col.id]);
    default:
      return item[col.id] ?? "";
  }
}

function CommentCell({
  item,
  editable,
  onSave,
}: {
  item: OverviewItem;
  editable: boolean;
  onSave: (comment: string) => Promise<boolean>;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const done = useRef(false);
  const commit = async () => {
    if (draft === null || saving) return;
    if (draft === (item.comment ?? "")) {
      setDraft(null);
      return;
    }
    setSaving(true);
    const ok = await onSave(draft);
    setSaving(false);
    if (ok) setDraft(null);
  };
  if (!editable) return <span className="text-muted-foreground truncate">{item.comment}</span>;
  if (draft === null)
    return (
      <button
        type="button"
        aria-label={`Edit comment of ${item.name}`}
        title="Click to edit the comment"
        onClick={() => {
          done.current = false;
          setDraft(item.comment ?? "");
        }}
        onDoubleClick={(e) => e.stopPropagation()}
        className="hover:bg-surface-2 focus-visible:ring-ring/50 block w-full truncate rounded-sm px-1 text-left outline-none focus-visible:ring-1"
      >
        {item.comment || <span className="text-muted-foreground/50 italic">—</span>}
      </button>
    );
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      done.current = true;
      void commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      done.current = true;
      setDraft(null);
    }
  };
  return (
    <div className="flex items-center gap-1">
      <Input
        autoFocus
        aria-label={`Comment of ${item.name}`}
        value={draft}
        disabled={saving}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={() => {
          if (!done.current) void commit();
        }}
        onDoubleClick={(e) => e.stopPropagation()}
        className="h-6 rounded-sm px-1 text-xs md:text-xs"
      />
      {saving && <Loader2 className="text-muted-foreground size-3 shrink-0 animate-spin" />}
    </div>
  );
}

/**
 * Item Overview (spec §11.2): every table and view of a schema with its statistics, sortable by
 * any column. The comment is editable; double-clicking a row opens the item.
 */
export function ItemOverviewDialog({
  request,
  onClose,
}: {
  request: OverviewRequest;
  onClose: () => void;
}) {
  const { connectionId, dialect, schema } = request;
  const qc = useQueryClient();
  const overview = useOverview(connectionId, schema);
  const [sort, setSortState] = useState<OverviewSort>(lastSort);
  const [filter, setFilter] = useState("");
  const columns = overviewColumns(dialect);
  const rows = useMemo(() => {
    const matches = compileSearch(filter);
    return sortOverview(
      (overview.data ?? []).filter((i) => matches(i.name)),
      sort,
    );
  }, [overview.data, filter, sort]);

  const setSort = (s: OverviewSort) => {
    lastSort = s;
    setSortState(s);
  };

  const open = (item: OverviewItem) => {
    useTabsStore
      .getState()
      .openTable(connectionId, { schema: item.schema, name: item.name }, "table");
    useUiStore.getState().setView("workspace");
    onClose();
  };

  const saveComment = async (item: OverviewItem, comment: string): Promise<boolean> => {
    const outcome = await guardedApply(qc, connectionId, {
      op: "setComment",
      target: { table: { schema: item.schema, name: item.name }, kind: item.kind },
      comment: comment.trim() === "" ? null : comment,
    });
    if (outcome.status === "cancelled") return false;
    if (outcome.status === "failed") {
      toast.error(`Could not change the comment of ${item.name}`, {
        description: outcome.error.message,
      });
      return false;
    }
    invalidateSchema(qc, connectionId, { schema: item.schema, name: item.name });
    return true;
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex h-[80vh] max-h-[80vh] flex-col gap-3 sm:max-w-6xl">
        <DialogHeader className="flex-row items-center gap-3 space-y-0">
          <div className="min-w-0 flex-1">
            <DialogTitle className="text-sm">Item Overview</DialogTitle>
            <DialogDescription className="text-xs">
              <span className="font-mono">{schema}</span> ·{" "}
              {overview.data ? `${overview.data.length} items` : "Loading…"}
            </DialogDescription>
          </div>
          <Input
            aria-label="Filter items"
            placeholder="Filter…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="h-7 w-48 text-xs md:text-xs"
          />
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Refresh"
            title="Refresh"
            onClick={() => void overview.refetch()}
            disabled={overview.isFetching}
            className="mr-6"
          >
            <RefreshCw className={cn(overview.isFetching && "animate-spin")} />
          </Button>
        </DialogHeader>
        <div className="border-border min-h-0 flex-1 overflow-auto rounded-md border">
          {overview.isError ? (
            <p role="alert" className="text-error p-3 text-xs">
              {overview.error.message}
            </p>
          ) : (
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="text-muted-foreground [&>th]:bg-surface-2 h-7 text-left text-[11px] [&>th]:sticky [&>th]:top-0 [&>th]:z-10 [&>th]:shadow-[inset_0_-1px_0_var(--color-border)]">
                  {columns.map((c) => {
                    const active = sort.column === c.id;
                    return (
                      <th
                        key={c.id}
                        aria-sort={active ? (sort.desc ? "descending" : "ascending") : "none"}
                        className={cn(
                          "px-0 font-medium whitespace-nowrap",
                          c.numeric && "text-right",
                        )}
                      >
                        <button
                          type="button"
                          onClick={() => setSort(nextSort(sort, c))}
                          className={cn(
                            "hover:text-foreground flex w-full items-center gap-1 px-2 py-1 font-mono",
                            c.numeric && "justify-end",
                            active && "text-foreground",
                          )}
                        >
                          {c.label}
                          {active &&
                            (sort.desc ? (
                              <ArrowDown className="size-3" />
                            ) : (
                              <ArrowUp className="size-3" />
                            ))}
                        </button>
                      </th>
                    );
                  })}
                </tr>
              </thead>
              <tbody>
                {rows.map((item) => (
                  <tr
                    key={`${item.schema ?? ""}.${item.name}`}
                    onDoubleClick={() => open(item)}
                    className="border-gridline h-7 border-b hover:bg-black/[0.035] dark:hover:bg-white/[0.05]"
                  >
                    {columns.map((c) => (
                      <td
                        key={c.id}
                        className={cn(
                          "max-w-[28rem] px-2 whitespace-nowrap",
                          c.numeric && "text-right font-mono tabular-nums",
                          c.id === "comment" && "min-w-48 px-1",
                        )}
                      >
                        {c.id === "name" ? (
                          <span className="flex items-center gap-1.5">
                            <TableIcon kind={item.kind} />
                            <span className="truncate">{item.name}</span>
                          </span>
                        ) : c.id === "comment" ? (
                          <CommentCell
                            item={item}
                            editable={commentEditable(dialect, item)}
                            onSave={(comment) => saveComment(item, comment)}
                          />
                        ) : (
                          <span className="block truncate">{cellText(item, c)}</span>
                        )}
                      </td>
                    ))}
                  </tr>
                ))}
                {overview.isPending && (
                  <tr>
                    <td colSpan={columns.length} className="p-3">
                      <Loader2 className="text-muted-foreground size-3.5 animate-spin" />
                    </td>
                  </tr>
                )}
                {overview.data && rows.length === 0 && (
                  <tr>
                    <td
                      colSpan={columns.length}
                      className="text-muted-foreground p-3 text-center text-[11px] italic"
                    >
                      {filter ? "No items match" : "This schema has no tables or views"}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          )}
        </div>
        <p className="text-muted-foreground text-[11px]">
          Estimates come from the server's statistics. Double-click a row to open it.
        </p>
      </DialogContent>
    </Dialog>
  );
}
