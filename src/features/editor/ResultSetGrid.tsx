import { TriangleAlert } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { DataGrid } from "@/features/datagrid/DataGrid";
import type { ViewRow } from "@/features/datagrid/types";
import { formatCount } from "@/lib/format/time";
import type { Dialect, ResultColumn, SortSpec, Value } from "@/lib/ipc";
import { useInspectorStore } from "@/stores/inspector";
import { resultData, useResultRowCount } from "@/stores/resultData";
import { resultGridColumns, sortedOrder } from "./resultColumns";
import type { ResultSetInfo } from "./runModel";

/** Read-only grid over rows held outside React (the streamed buffer, or a plan's rows). */
function RowsGrid({
  columns: resultColumns,
  rowCount,
  rowsOf,
  loading,
  label,
  dialect,
}: {
  columns: readonly ResultColumn[];
  rowCount: number;
  rowsOf: () => readonly Value[][];
  loading: boolean;
  label: string;
  dialect: Dialect;
}) {
  const columns = useMemo(() => resultGridColumns(resultColumns), [resultColumns]);
  const [sort, setSort] = useState<SortSpec[]>([]);
  // Client-side sort; recomputed as rows stream in.
  const order = useMemo(
    () => (sort.length > 0 && rowCount > 0 ? sortedOrder(rowsOf(), sort) : null),
    [sort, rowCount, rowsOf],
  );
  const getRow = useCallback(
    (i: number): ViewRow | undefined => {
      const idx = order ? order[i] : i;
      const cells = idx === undefined ? undefined : rowsOf()[idx];
      return cells ? { key: String(idx), cells } : undefined;
    },
    [order, rowsOf],
  );
  const setInspector = useInspectorStore((s) => s.set);
  const table = resultColumns.find((c) => c.table)?.table ?? "result";
  return (
    <DataGrid
      columns={columns}
      rowCount={rowCount}
      getRow={getRow}
      loading={loading}
      emptyText={resultColumns.length === 0 ? "No columns" : "No rows"}
      sort={sort}
      onSortChange={setSort}
      copyContext={{ dialect, schema: null, table }}
      editable={false}
      onFocusCell={(cell) =>
        setInspector(
          cell
            ? {
                column: cell.column,
                value: cell.value,
                rowIndex: cell.pos.r,
                source: label,
                // Result rows cannot be refetched by key, so there is no `fetch`.
                row: { columns, cells: cell.row.cells, focused: cell.sourceIndex },
                fetch: null,
              }
            : null,
        )
      }
    />
  );
}

/** One streamed result set (`resultData` buffer `info.id`). */
export function ResultGrid({
  info,
  label,
  dialect,
}: {
  info: ResultSetInfo;
  label: string;
  dialect: Dialect;
}) {
  const rowCount = useResultRowCount(info.id);
  const rowsOf = useCallback(() => resultData.get(info.id)?.rows ?? [], [info.id]);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        <RowsGrid
          columns={info.columns}
          rowCount={rowCount}
          rowsOf={rowsOf}
          loading={!info.done && rowCount === 0}
          label={label}
          dialect={dialect}
        />
      </div>
      {info.truncated && (
        <div className="border-border bg-warning/10 text-warning flex h-6 shrink-0 items-center gap-1.5 border-t px-3 text-[11px]">
          <TriangleAlert className="size-3 shrink-0" />
          <span className="truncate">
            Truncated at {formatCount(rowCount)} rows. Add a LIMIT or a narrower WHERE to see the
            rest.
          </span>
        </div>
      )}
    </div>
  );
}

/** Rows that arrived in one piece (an EXPLAIN result). */
export function StaticGrid({
  columns,
  rows,
  label,
  dialect,
}: {
  columns: readonly ResultColumn[];
  rows: readonly Value[][];
  label: string;
  dialect: Dialect;
}) {
  const rowsOf = useCallback(() => rows, [rows]);
  return (
    <RowsGrid
      columns={columns}
      rowCount={rows.length}
      rowsOf={rowsOf}
      loading={false}
      label={label}
      dialect={dialect}
    />
  );
}
