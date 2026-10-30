import { FileDown, Filter, Loader2, Plus } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { formatCount, formatDuration } from "@/lib/format/time";
import type { ColumnMeta, RowCount } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { BarIconButton, BarSeparator, BarTooltip, BottomBarFrame } from "./BarParts";
import { ColumnsMenu } from "./ColumnsMenu";
import { Pagination, type CursorPaging, type PageChange } from "./Pagination";
import type { DefaultOrder } from "./tableViewMemory";

/**
 * "1–200 of ~1,000 rows · 4 ms": the page's place in the table, and how long it took. While rows
 * are selected: "15 of ~999,985 rows selected · 4 ms" (TablePlus): the table's row count, with the
 * same "~" when it is an estimate, once it is known (the page's rows until then).
 */
function RowSummary({
  offset,
  pageRows,
  selectedRows,
  shownRows,
  count,
  countPending,
  loading,
  durationMs,
  sql,
  onCountExact,
}: {
  offset: number;
  pageRows: number;
  selectedRows: number;
  shownRows: number;
  count: RowCount | null;
  countPending: boolean;
  loading: boolean;
  durationMs: number | null;
  sql: string | null;
  onCountExact: () => void;
}) {
  const total = count?.count ?? null;
  // The rows the selection is out of: the table's count plus the staged new rows (the grid shows
  // them too), never fewer than the rows shown (a stale estimate).
  const known = total === null ? null : total + Math.max(0, shownRows - pageRows);
  const outOf = known !== null && known >= shownRows ? known : shownRows;
  const outOfEstimated = known !== null && known >= shownRows && count?.exact === false;
  const range =
    pageRows === 0
      ? "0"
      : `${(offset + 1).toLocaleString()}–${(offset + pageRows).toLocaleString()}`;
  return (
    <span
      data-testid="row-summary"
      className="flex min-w-0 items-center gap-1 whitespace-nowrap tabular-nums"
      title={sql ?? undefined}
    >
      {selectedRows > 0 ? (
        <span className="text-foreground/80 min-w-0 truncate">
          {`${formatCount(selectedRows)} of ${formatCount(outOf, outOfEstimated)} ${outOf === 1 ? "row" : "rows"} selected`}
        </span>
      ) : (
        <span className="text-foreground/80 min-w-0 truncate">
          {range}
          {total !== null && total > 0 ? (
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
          {total === 1 ? " row" : " rows"}
        </span>
      )}
      {loading ? (
        <Loader2 aria-label="Loading" className="size-3 shrink-0 animate-spin" />
      ) : (
        durationMs !== null && (
          <span className="text-muted-foreground/80 shrink-0 @max-xl:hidden">
            · {formatDuration(durationMs)}
          </span>
        )
      )}
    </span>
  );
}

export interface TableBottomBarProps {
  /** The Data | Structure switch, at the far left. */
  leading?: ReactNode;
  /** `[+ Row]` inserts a row (disabled on read-only tables). */
  canAddRow: boolean;
  onAddRow: () => void;
  // ---- the row summary and paging
  offset: number;
  limit: number;
  pageRows: number;
  /**
   * Rows selected in the grid: the summary reads "N of M rows selected" while there are any, M
   * being the table's row count (`count`; the rows shown while it is pending).
   */
  selectedRows?: number;
  /** Rows the grid shows (the page plus staged new rows); `pageRows` when not given. */
  shownRows?: number;
  count: RowCount | null;
  countPending: boolean;
  /** The page's query time; null before the first page loaded. */
  durationMs: number | null;
  /** The page's SELECT (the row summary's tooltip). */
  sql: string | null;
  loading: boolean;
  onPageChange: (next: PageChange) => void;
  onCountExact: () => void;
  // ---- filters
  filtersVisible: boolean;
  /** Enabled filter conditions that are applied. */
  activeFilters: number;
  onToggleFilters: () => void;
  // ---- columns
  columns: ColumnMeta[];
  hidden: string[];
  columnsOpen: boolean;
  onColumnsOpenChange: (open: boolean) => void;
  onToggleColumn: (name: string, visible: boolean) => void;
  onShowAllColumns: () => void;
  onExport: () => void;
  /** ⚙ ▸ Default order (the table's Default content order). */
  defaultOrder?: DefaultOrder;
  onDefaultOrderChange?: (order: DefaultOrder) => void;
  hasPrimaryKey?: boolean;
  /** A table read by cursor: Previous / Next only (`offset` is the rows before this page). */
  cursor?: CursorPaging | null;
  /** The exact count is offered (default: yes). */
  countable?: boolean;
  /** The filter bar has something to offer (default: yes). */
  filterable?: boolean;
}

/**
 * The table tab's bottom bar (TablePlus): `[Data | Structure] [+ Row]`, the row summary in the
 * middle, then `[Filters] [Columns] Export ‹ ⚙ ›`.
 */
export function TableBottomBar(p: TableBottomBarProps) {
  const filtered = p.activeFilters > 0;
  return (
    <BottomBarFrame aria-label="Table data">
      {p.leading && <div className="flex shrink-0 items-center">{p.leading}</div>}
      <BarTooltip label="Add a row" action="grid.new-row">
        <Button
          variant="ghost"
          size="xs"
          aria-label="Add row"
          disabled={!p.canAddRow}
          onClick={p.onAddRow}
          className="text-muted-foreground hover:text-foreground h-6 px-1.5"
        >
          <Plus />
          Row
        </Button>
      </BarTooltip>
      <div className="flex min-w-0 flex-1 justify-center px-1">
        <RowSummary
          offset={p.offset}
          pageRows={p.pageRows}
          selectedRows={p.selectedRows ?? 0}
          shownRows={p.shownRows ?? p.pageRows}
          count={p.count}
          countPending={p.countPending}
          loading={p.loading}
          durationMs={p.durationMs}
          sql={p.sql}
          onCountExact={p.onCountExact}
        />
      </div>
      <BarTooltip label={p.filtersVisible ? "Hide filters" : "Show filters"} action="grid.filter">
        <Button
          variant="ghost"
          size="xs"
          aria-label="Filters"
          aria-pressed={p.filtersVisible}
          disabled={p.filterable === false}
          onClick={p.onToggleFilters}
          className={cn(
            "text-muted-foreground hover:text-foreground h-6 px-1.5",
            p.filtersVisible && "bg-muted text-foreground",
            filtered && "text-brand hover:text-brand",
          )}
        >
          <Filter />
          <span className="@max-2xl:hidden">Filters</span>
          {filtered && (
            <span className="bg-brand-soft text-brand rounded px-1 text-[10px] leading-4 font-semibold tabular-nums">
              {p.activeFilters}
            </span>
          )}
        </Button>
      </BarTooltip>
      <ColumnsMenu
        columns={p.columns}
        hidden={p.hidden}
        open={p.columnsOpen}
        onOpenChange={p.onColumnsOpenChange}
        onToggleColumn={p.onToggleColumn}
        onShowAll={p.onShowAllColumns}
      />
      {/* ⇧⌘E is the multi-table export; this one exports this table's filtered rows. */}
      <BarIconButton
        label="Export…"
        tooltip="Export the table's rows (the filtered ones)"
        onClick={p.onExport}
      >
        <FileDown />
      </BarIconButton>
      <BarSeparator />
      <Pagination
        offset={p.offset}
        limit={p.limit}
        pageRows={p.pageRows}
        count={p.count}
        onChange={p.onPageChange}
        onCountExact={p.onCountExact}
        defaultOrder={p.defaultOrder}
        onDefaultOrderChange={p.onDefaultOrderChange}
        hasPrimaryKey={p.hasPrimaryKey}
        cursor={p.cursor}
        countable={p.countable}
      />
    </BottomBarFrame>
  );
}
