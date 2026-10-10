import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "@/components/EmptyState";
import { Skeleton } from "@/components/ui/skeleton";
import { DataGrid } from "@/features/datagrid/DataGrid";
import {
  buildGridColumns,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import { useAction } from "@/features/palette/actions";
import { ipc, type BrowsePage, type Dialect, type SortSpec, type Value } from "@/lib/ipc";
import { valueText } from "@/lib/values";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  newFilterRow,
  useTableTab,
  useTableTabsStore,
  type FilterRowSpec,
} from "@/stores/tableTabs";
import { FilterBar } from "./FilterBar";
import { toFilterGroup } from "./filters";
import { Pagination } from "./Pagination";
import { TableToolbar } from "./TableToolbar";
import { rowKeys, useRowCount, useTableMeta, useTableRows } from "./useTableRows";

const DIALECT: Record<string, Dialect> = { postgres: "postgres", mysql: "mysql", sqlite: "sqlite" };

function rowKeyOf(page: BrowsePage, index: number): string {
  const row = page.rows[index]!;
  if (page.identity === "rowid" && page.rowIds) return `rid:${page.rowIds[index]}`;
  if (page.identity === "pk" && page.pkColumns.length > 0) {
    return `pk:${page.pkColumns.map((c) => valueText(row[page.columns.findIndex((col) => col.name === c)] ?? { t: "null" }) ?? "NULL").join("|")}`;
  }
  return `row:${page.offset + index}`;
}

export function TableTab({ tab }: { tab: Extract<Tab, { kind: "table" }> }) {
  const { connectionId, table } = tab;
  const qc = useQueryClient();
  const driver = useConnectionsStore((s) => s.byId[connectionId]?.info.driver ?? "postgres");
  const isActive = useTabsStore((s) => s.activeId === tab.id);
  const openTable = useTabsStore((s) => s.openTable);
  const ensure = useTableTabsStore((s) => s.ensure);
  const update = useTableTabsStore((s) => s.update);
  useEffect(() => {
    ensure(tab.id);
  }, [tab.id, ensure]);
  const state = useTableTab(tab.id);
  const setInspector = useInspectorStore((s) => s.set);

  const meta = useTableMeta(connectionId, table);
  const columns = useMemo(
    () => (meta.data ? buildGridColumns(meta.data.columns, meta.data.foreignKeys, true) : []),
    [meta.data],
  );

  // Draft filter rows live locally until "Apply"; the applied ones are in the slice.
  const [draft, setDraft] = useState<{
    rows: FilterRowSpec[];
    conjunction: typeof state.conjunction;
  }>({ rows: state.filters, conjunction: state.conjunction });
  useEffect(
    () => setDraft({ rows: state.filters, conjunction: state.conjunction }),
    [state.filters, state.conjunction],
  );
  const draftDirty = draft.rows !== state.filters || draft.conjunction !== state.conjunction;

  const filterGroup = useMemo(
    () => (meta.data ? toFilterGroup(state.filters, meta.data.columns, state.conjunction) : null),
    [meta.data, state.filters, state.conjunction],
  );
  const request = useMemo(
    () =>
      meta.data
        ? {
            columns: null,
            filter: filterGroup,
            sort: state.sort,
            limit: state.limit,
            offset: state.offset,
          }
        : null,
    [meta.data, filterGroup, state.sort, state.limit, state.offset],
  );
  const rows = useTableRows(connectionId, table, request, state.refreshToken);
  const count = useRowCount(
    connectionId,
    table,
    filterGroup,
    state.refreshToken,
    meta.data !== undefined,
  );

  useEffect(() => {
    if (rows.data)
      update(tab.id, {
        page: { rows: rows.data.rows.length, durationMs: rows.data.durationMs, sql: rows.data.sql },
      });
  }, [rows.data, tab.id, update]);
  useEffect(() => {
    if (count.data) update(tab.id, { count: count.data });
  }, [count.data, tab.id, update]);

  const page = rows.data;
  const viewRows = useMemo<ViewRow[]>(
    () => (page ? page.rows.map((cells, i) => ({ key: rowKeyOf(page, i), cells })) : []),
    [page],
  );
  const getRow = useCallback((i: number) => viewRows[i], [viewRows]);

  const applyFilters = useCallback(
    () => update(tab.id, { filters: draft.rows, conjunction: draft.conjunction, offset: 0 }),
    [draft, tab.id, update],
  );
  const refresh = useCallback(() => {
    update(tab.id, (s) => ({ refreshToken: s.refreshToken + 1 }));
    void qc.invalidateQueries({ queryKey: rowKeys.meta(connectionId, table) });
  }, [tab.id, update, qc, connectionId, table]);

  useAction("view.refresh", isActive ? refresh : null);
  useAction(
    "grid.filter",
    isActive ? () => update(tab.id, (s) => ({ filtersVisible: !s.filtersVisible })) : null,
  );
  useAction(
    "grid.open-fk",
    isActive
      ? () => {
          const cell = useInspectorStore.getState().cell;
          if (cell?.column.fk && cell.value.t !== "null") openFk(cell.column, cell.value);
        }
      : null,
  );

  const openFk = (column: GridColumn, value: Value) => {
    const fk = column.fk!;
    const target = { schema: fk.schema ?? table.schema, name: fk.table };
    const id = openTable(connectionId, target, "table");
    const filter = newFilterRow(fk.column, "eq", valueText(value) ?? "");
    useTableTabsStore.getState().ensure(id);
    useTableTabsStore
      .getState()
      .update(id, { filters: [filter], conjunction: "and", filtersVisible: true, offset: 0 });
  };

  const onSortChange = (sort: SortSpec[]) => update(tab.id, { sort, offset: 0 });
  const onColumnStateChange = (columnState: ColumnState) => update(tab.id, { columnState });

  if (meta.isPending) {
    return (
      <div className="flex flex-col gap-2 p-4">
        <Skeleton className="h-5 w-48" />
        <Skeleton className="h-64 w-full" />
      </div>
    );
  }
  if (meta.isError)
    return <EmptyState title="Could not load table" description={meta.error.message} />;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <TableToolbar
        table={meta.data.table}
        columns={meta.data.columns}
        hidden={state.columnState.hidden}
        filtersVisible={state.filtersVisible}
        activeFilters={state.filters.filter((f) => f.enabled).length}
        loading={rows.isFetching}
        onToggleFilters={() => update(tab.id, (s) => ({ filtersVisible: !s.filtersVisible }))}
        onRefresh={refresh}
        onToggleColumn={(name, visible) =>
          update(tab.id, (s) => ({
            columnState: {
              ...s.columnState,
              hidden: visible
                ? s.columnState.hidden.filter((h) => h !== name)
                : [...s.columnState.hidden, name],
            },
          }))
        }
        onShowAllColumns={() =>
          update(tab.id, (s) => ({ columnState: { ...s.columnState, hidden: [] } }))
        }
        onOpenStructure={() => openTable(connectionId, table, "structure")}
        sqlPreview={page?.sql ?? null}
      />
      {state.filtersVisible && (
        <FilterBar
          columns={meta.data.columns}
          rows={
            draft.rows.length > 0 ? draft.rows : [newFilterRow(meta.data.columns[0]?.name ?? "")]
          }
          conjunction={draft.conjunction}
          dirty={draftDirty}
          onChange={(r, c) => setDraft({ rows: r, conjunction: c })}
          onApply={applyFilters}
          onClear={() => {
            setDraft({ rows: [], conjunction: "and" });
            update(tab.id, { filters: [], offset: 0 });
          }}
        />
      )}
      <div className="min-h-0 flex-1">
        {rows.isError ? (
          <EmptyState title="Query failed" description={rows.error.message} />
        ) : (
          <DataGrid
            columns={columns}
            rowCount={viewRows.length}
            getRow={getRow}
            loading={rows.isPending}
            emptyText={state.filters.length > 0 ? "No rows match the filter" : "Table is empty"}
            sort={state.sort}
            onSortChange={onSortChange}
            columnState={state.columnState}
            onColumnStateChange={onColumnStateChange}
            onFocusCell={(cell) =>
              setInspector(
                cell
                  ? {
                      column: cell.column,
                      value: cell.value,
                      rowIndex: cell.pos.r,
                      source: table.name,
                    }
                  : null,
              )
            }
            onOpenFk={openFk}
            copyContext={{
              dialect: DIALECT[driver] ?? "postgres",
              schema: meta.data.table.schema,
              table: meta.data.table.name,
            }}
          />
        )}
      </div>
      <Pagination
        offset={state.offset}
        limit={state.limit}
        pageRows={viewRows.length}
        count={count.data ?? null}
        countPending={count.isPending}
        onChange={(next) => update(tab.id, next)}
        onCountExact={() =>
          void ipc.browse
            .count(connectionId, table, filterGroup, true)
            .then((c) => update(tab.id, { count: c }))
        }
      />
    </div>
  );
}
