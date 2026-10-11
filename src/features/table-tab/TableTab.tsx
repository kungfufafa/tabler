import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { EmptyState } from "@/components/EmptyState";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { DataGrid, type GridAction } from "@/features/datagrid/DataGrid";
import { buildGridColumns, type ColumnState, type GridColumn } from "@/features/datagrid/types";
import { useAction } from "@/features/palette/actions";
import { defaultsFor, mergeRows } from "@/features/staging/merge";
import {
  deleteRows,
  discardAll,
  insertRow,
  pendingCount,
  revertCell,
  revertRow,
  setCell,
  setRowError,
  undo,
} from "@/features/staging/model";
import { rowKeyFor, toStagedChanges } from "@/features/staging/toPayload";
import {
  ipc,
  type BrowsePage,
  type Dialect,
  type RowKey,
  type SortSpec,
  type Value,
} from "@/lib/ipc";
import { valueText } from "@/lib/values";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  newFilterRow,
  useTableTab,
  useTableTabsStore,
  type FilterRowSpec,
  type TableTabState,
} from "@/stores/tableTabs";
import { FilterBar } from "./FilterBar";
import { toFilterGroup } from "./filters";
import { Pagination } from "./Pagination";
import { PREVIEW_CLOSED, type PreviewState } from "./previewState";
import { PreviewSqlSheet } from "./PreviewSqlSheet";
import { TableToolbar } from "./TableToolbar";
import { rowKeys, useRowCount, useTableMeta, useTableRows } from "./useTableRows";

const DIALECT: Record<string, Dialect> = { postgres: "postgres", mysql: "mysql", sqlite: "sqlite" };

function rowKeyString(page: BrowsePage, index: number): string {
  const key = rowKeyFor(page, index);
  if (key.kind === "rowid") return `rid:${key.value}`;
  const vals = Object.values(key.value).map((v) => valueText(v) ?? "NULL");
  return `${key.kind === "pk" ? "pk" : "all"}:${vals.join("|")}`;
}

export function TableTab({ tab }: { tab: Extract<Tab, { kind: "table" }> }) {
  const { connectionId, table } = tab;
  const qc = useQueryClient();
  const connection = useConnectionsStore((s) => s.byId[connectionId]);
  const driver = connection?.info.driver ?? "postgres";
  const isActive = useTabsStore((s) => s.activeId === tab.id);
  const openTable = useTabsStore((s) => s.openTable);
  const ensure = useTableTabsStore((s) => s.ensure);
  const update = useTableTabsStore((s) => s.update);
  useEffect(() => {
    ensure(tab.id);
  }, [tab.id, ensure]);
  const state = useTableTab(tab.id);
  const staging = state.staging;
  const pending = pendingCount(staging);
  const setInspector = useInspectorStore((s) => s.set);

  const meta = useTableMeta(connectionId, table);
  const editable =
    !!meta.data && meta.data.kind === "table" && !(connection?.info.readOnly ?? false);
  const columns = useMemo(
    () => (meta.data ? buildGridColumns(meta.data.columns, meta.data.foreignKeys, editable) : []),
    [meta.data, editable],
  );

  const [draft, setDraft] = useState<{
    rows: FilterRowSpec[];
    conjunction: TableTabState["conjunction"];
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
  const keys = useMemo(() => (page ? page.rows.map((_, i) => rowKeyString(page, i)) : []), [page]);
  const backendKeys = useMemo(() => {
    const m = new Map<string, RowKey>();
    if (page) page.rows.forEach((_, i) => m.set(keys[i]!, rowKeyFor(page, i)));
    return m;
  }, [page, keys]);
  const viewRows = useMemo(
    () => (page ? mergeRows(page, keys, staging) : []),
    [page, keys, staging],
  );
  const getRow = useCallback((i: number) => viewRows[i], [viewRows]);
  const setStaging = useCallback(
    (fn: (s: TableTabState["staging"]) => TableTabState["staging"]) =>
      update(tab.id, (s) => ({ staging: fn(s.staging) })),
    [tab.id, update],
  );

  // ---- guard: anything that replaces the page asks about pending edits ----
  const [guard, setGuard] = useState<null | { run: () => void }>(null);
  const guarded = useCallback(
    (run: () => void) => {
      if (pending > 0) setGuard({ run });
      else run();
    },
    [pending],
  );

  const applyFilters = useCallback(
    () =>
      guarded(() =>
        update(tab.id, { filters: draft.rows, conjunction: draft.conjunction, offset: 0 }),
      ),
    [draft, tab.id, update, guarded],
  );
  const refresh = useCallback(
    () =>
      guarded(() => {
        update(tab.id, (s) => ({ refreshToken: s.refreshToken + 1, staging: discardAll() }));
        void qc.invalidateQueries({ queryKey: rowKeys.meta(connectionId, table) });
      }),
    [tab.id, update, qc, connectionId, table, guarded],
  );

  // ---- editing -------------------------------------------------------------
  const originalOf = (r: number): Value[] | undefined => page?.rows[r];
  const onEditCommit = (r: number, c: number, value: Value) => {
    const key = viewRows[r]?.key;
    if (!key) return;
    setStaging((s) => setCell(s, key, c, value, originalOf(r)));
  };
  const onAction = (a: GridAction) => {
    if (!page) return;
    switch (a.type) {
      case "setNull":
      case "setDefault":
        setStaging((s) => {
          let next = s;
          for (const { r, c } of a.cells) {
            const row = viewRows[r];
            const col = columns[c];
            if (!row || !col || !col.editable || row.status === "deleted") continue;
            if (a.type === "setNull" && !col.nullable) continue;
            if (a.type === "setDefault" && !col.hasDefault) continue;
            next = setCell(
              next,
              row.key,
              c,
              a.type === "setNull" ? { t: "null" } : { t: "default" },
              originalOf(r),
            );
          }
          return next;
        });
        return;
      case "revertCells":
        setStaging((s) =>
          a.cells.reduce(
            (acc, { r, c }) => (viewRows[r] ? revertCell(acc, viewRows[r]!.key, c) : acc),
            s,
          ),
        );
        return;
      case "revertRows":
        setStaging((s) =>
          a.rows.reduce((acc, r) => (viewRows[r] ? revertRow(acc, viewRows[r]!.key) : acc), s),
        );
        return;
      case "deleteRows":
        setStaging((s) =>
          deleteRows(
            s,
            a.rows.flatMap((r) =>
              viewRows[r] ? [{ rowKey: viewRows[r]!.key, original: originalOf(r) }] : [],
            ),
          ),
        );
        return;
      case "duplicateRows":
        setStaging((s) =>
          a.rows.reduce((acc, r) => {
            const row = viewRows[r];
            if (!row) return acc;
            const cells = row.cells.map((v, i) =>
              page.columns[i]?.isAutoIncrement ? { t: "default" as const } : v,
            );
            return insertRow(acc, `new:${crypto.randomUUID()}`, cells);
          }, s),
        );
        return;
      case "newRow":
        setStaging((s) => insertRow(s, `new:${crypto.randomUUID()}`, defaultsFor(page.columns)));
        return;
      case "undo":
        setStaging((s) => undo(s));
        return;
    }
  };

  // ---- commit / discard ----------------------------------------------------
  const [preview, setPreview] = useState<PreviewState>(PREVIEW_CLOSED);
  const buildRequest = () =>
    page
      ? {
          table,
          changes: toStagedChanges(staging, page.columns, (k) => backendKeys.get(k) ?? null),
          returning: true,
        }
      : null;

  const openPreview = async () => {
    const req = buildRequest();
    if (!req || req.changes.length === 0) return;
    setPreview({ ...PREVIEW_CLOSED, open: true, loading: true });
    try {
      const statements = await ipc.mutate.preview(connectionId, req);
      setPreview((p) => ({ ...p, statements, loading: false }));
    } catch (e) {
      setPreview((p) => ({ ...p, loading: false, error: (e as Error).message }));
    }
  };

  const applyPreview = async () => {
    const req = buildRequest();
    if (!req) return;
    setPreview((p) => ({ ...p, applying: true, error: null, failedIndex: null }));
    try {
      const result = await ipc.mutate.apply(connectionId, req);
      if (result.success) {
        setPreview(PREVIEW_CLOSED);
        update(tab.id, (s) => ({ staging: discardAll(), refreshToken: s.refreshToken + 1 }));
        toast.success(
          `Applied ${result.results.length} change${result.results.length === 1 ? "" : "s"} · ${result.durationMs} ms`,
        );
      } else {
        const failed = result.results.findIndex((r) => r.error);
        setPreview((p) => ({
          ...p,
          applying: false,
          error: result.error?.message ?? "Failed",
          failedIndex: failed >= 0 ? failed : null,
        }));
        setStaging((s) =>
          result.results.reduce(
            (acc, r) => (r.error ? setRowError(acc, r.rowId, r.error.message) : acc),
            s,
          ),
        );
      }
    } catch (e) {
      setPreview((p) => ({ ...p, applying: false, error: (e as Error).message }));
    }
  };

  const discard = () => {
    if (pending === 0) return;
    setStaging(() => discardAll());
    toast.message("Changes discarded");
  };

  useAction("view.refresh", isActive ? refresh : null);
  useAction(
    "grid.filter",
    isActive ? () => update(tab.id, (s) => ({ filtersVisible: !s.filtersVisible })) : null,
  );
  useAction("grid.commit", isActive && pending > 0 ? () => void openPreview() : null);
  useAction("grid.discard", isActive && pending > 0 ? discard : null);
  useAction(
    "grid.open-fk",
    isActive
      ? () => {
          const cell = useInspectorStore.getState().cell;
          if (cell?.column.fk && cell.value.t !== "null") openFk(cell.column, cell.value);
        }
      : null,
  );
  useEffect(() => {
    if (!preview.open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !preview.applying && !preview.loading) {
        e.preventDefault();
        void applyPreview();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preview.open, preview.applying, preview.loading, staging]);

  const openFk = (column: GridColumn, value: Value) => {
    const fk = column.fk!;
    const target = { schema: fk.schema ?? table.schema, name: fk.table };
    const id = openTable(connectionId, target, "table");
    useTableTabsStore.getState().ensure(id);
    useTableTabsStore.getState().update(id, {
      filters: [newFilterRow(fk.column, "eq", valueText(value) ?? "")],
      conjunction: "and",
      filtersVisible: true,
      offset: 0,
    });
  };

  const onSortChange = (sort: SortSpec[]) => guarded(() => update(tab.id, { sort, offset: 0 }));
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
  const noKey = page?.identity === "none";

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
            guarded(() => update(tab.id, { filters: [], offset: 0 }));
          }}
        />
      )}
      {noKey && editable && (
        <div className="border-warning/40 bg-warning/10 text-warning shrink-0 border-b px-3 py-1 text-[11px]">
          This table has no primary key: edits match every column of the original row and affect at
          most one row.
        </div>
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
            editable={editable && !!page}
            active={isActive}
            onEditCommit={onEditCommit}
            onAction={onAction}
          />
        )}
      </div>
      <Pagination
        offset={state.offset}
        limit={state.limit}
        pageRows={page?.rows.length ?? 0}
        count={count.data ?? null}
        countPending={count.isPending}
        onChange={(next) => guarded(() => update(tab.id, next))}
        onCountExact={() =>
          void ipc.browse
            .count(connectionId, table, filterGroup, true)
            .then((c) => update(tab.id, { count: c }))
        }
      />
      <PreviewSqlSheet
        state={preview}
        onOpenChange={(open) => setPreview((p) => (open ? p : PREVIEW_CLOSED))}
        onApply={() => void applyPreview()}
      />
      <AlertDialog open={guard !== null} onOpenChange={(open) => !open && setGuard(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {pending} pending change{pending === 1 ? "" : "s"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              Reloading the rows will drop your unsaved edits. Commit them first, or discard them.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <AlertDialogAction
              variant="outline"
              onClick={() => {
                const run = guard?.run;
                setGuard(null);
                setStaging(() => discardAll());
                run?.();
              }}
            >
              Discard and continue
            </AlertDialogAction>
            <AlertDialogAction
              onClick={() => {
                setGuard(null);
                void openPreview();
              }}
            >
              Commit…
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
