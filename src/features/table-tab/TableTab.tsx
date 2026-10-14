import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import {
  DataGrid,
  type FocusedCell,
  type GridAction,
  type QuickFilter,
} from "@/features/datagrid/DataGrid";
import type { ReferencedRows } from "@/features/datagrid/quicklook/QuickLook";
import { mysqlLongLat } from "@/features/datagrid/clipboard";
import { nowValue } from "@/features/datagrid/setValue";
import {
  acceptsEmpty,
  buildGridColumns,
  fkPairs,
  fksOf,
  type ColumnState,
  type FkRef,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import { openSqlInNewQueryTab } from "@/features/editor/openSql";
import { openExportForActiveTab } from "@/features/export/store";
import { openImport } from "@/features/import/store";
import { setRowContext } from "@/features/commit/rowContext";
import { openWorkspaceSheet, useCommitGeneration } from "@/features/commit/store";
import { actions, useAction } from "@/features/palette/actions";
import { defaultsFor, mergeRows } from "@/features/staging/merge";
import {
  canRedo,
  deleteRows,
  discardAll,
  insertRow,
  insertRows,
  pendingCount,
  redo,
  revertRow,
  setCell,
  setCells,
  undo,
} from "@/features/staging/model";
import type { CellEdit } from "@/features/staging/types";
import { rowKeyFor } from "@/features/staging/toPayload";
import { ViewSwitch } from "@/features/tabs/ViewSwitch";
import {
  ipc,
  type BrowsePage,
  type Conjunction,
  type Dialect,
  type RowKey,
  type SortSpec,
  type Value,
} from "@/lib/ipc";
import { errorWithHint } from "@/lib/errorText";
import { valueText } from "@/lib/values";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { getPrefs, usePrefs } from "@/stores/prefs";
import { useTabsStore, type Tab } from "@/stores/tabs";
import {
  newFilterRow,
  useTableTab,
  useTableTabsStore,
  type FilterRowSpec,
  type TableTabState,
} from "@/stores/tableTabs";
import { BottomBarFrame } from "./BarParts";
import { TableBottomBar } from "./BottomBar";
import { FilterBar } from "./FilterBar";
import { focusFilterRow } from "./filterFocus";
import { filterMemoryKey, initialFilterState, rememberFilters } from "./filterMemory";
import { joinSql } from "./joinSql";
import { defaultFilterRow, isFilterOp, joinList, lastAppliedRowId, toFilterGroup } from "./filters";
import {
  fkFilterText,
  openExportCurrentPage,
  pickBinaryFile,
  qualifiedName,
  referencedColumns,
  referencedFilter,
} from "./gridActions";
import {
  defaultOrderSort,
  recallColumnLayout,
  rememberColumnLayout,
  useDefaultOrder,
} from "./tableViewMemory";
import { rowKeys, useRowCount, useTableMeta, useTableRows } from "./useTableRows";

const DIALECT: Record<string, Dialect> = { postgres: "postgres", mysql: "mysql", sqlite: "sqlite" };

const NULL: Value = { t: "null" };

/** `valuesEqual` with a fast path for scalars (bulk edit compares whole columns). */
function sameValue(a: Value, b: Value): boolean {
  if (a.t !== b.t) return false;
  if (a.t === "null" || a.t === "default") return true;
  const av = (a as { v: unknown }).v;
  const bv = (b as { v: unknown }).v;
  if (typeof av !== "object" || av === null) return av === bv;
  return JSON.stringify(av) === JSON.stringify(bv);
}

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
  const setView = useTabsStore((s) => s.setView);
  const pin = useTabsStore((s) => s.pin);
  const ensure = useTableTabsStore((s) => s.ensure);
  const update = useTableTabsStore((s) => s.update);
  // Filters are remembered per table (profile + database + table), restored per the filter bar's
  // "Default filter state" setting.
  const profileId = connection?.info.profileId ?? null;
  const database = connection?.info.currentDatabase ?? null;
  const memoryKey = useMemo(
    () => (profileId ? filterMemoryKey(profileId, database, table) : null),
    [profileId, database, table],
  );
  useEffect(() => {
    // grid3: the table's remembered column layout (positions, widths, hidden) too.
    const columnState = recallColumnLayout(memoryKey);
    ensure(tab.id, {
      ...initialFilterState(memoryKey, getPrefs().filterDefaultState, isFilterOp),
      ...(columnState ? { columnState } : {}),
    });
  }, [tab.id, ensure, memoryKey]);
  const state = useTableTab(tab.id);
  // grid3: column layout remembered per table (every tab of it, and the next session).
  useEffect(() => {
    if (!memoryKey) return;
    const slice = useTableTabsStore.getState().slices[tab.id];
    if (slice) rememberColumnLayout(memoryKey, slice.columnState);
  }, [memoryKey, tab.id, state.columnState]);
  // grid3: the table's "Default content order" (⚙ in the bottom bar), used while nothing is sorted.
  const [defaultOrder, setDefaultOrder] = useDefaultOrder(memoryKey);
  useEffect(() => {
    if (!memoryKey) return;
    // The slice itself (not the placeholder shown before it exists).
    const slice = useTableTabsStore.getState().slices[tab.id];
    if (!slice) return;
    rememberFilters(memoryKey, {
      filters: slice.filters,
      conjunction: slice.conjunction,
      visible: slice.filtersVisible,
    });
  }, [memoryKey, tab.id, state.filters, state.conjunction, state.filtersVisible]);
  const staging = state.staging;
  const pending = pendingCount(staging);
  const setInspector = useInspectorStore((s) => s.set);
  const rootRef = useRef<HTMLDivElement>(null);
  const focusGrid = () =>
    rootRef.current
      ?.querySelector<HTMLElement>('[data-scope="grid"]')
      ?.focus({ preventScroll: true });
  /** ⌘↑ on the grid's first row: the filter bar's last row, its value ready to type. */
  const focusFilters = () => focusFilterRow(rootRef.current, "last");

  // A preview tab (italic, replaced by the next single click in the sidebar) is kept once it holds
  // work: staged edits or an applied filter.
  const filtered = state.filters.length > 0;
  useEffect(() => {
    if (tab.preview && (pending > 0 || filtered)) pin(tab.id);
  }, [tab.preview, tab.id, pending, filtered, pin]);

  const meta = useTableMeta(connectionId, table);
  const editable =
    !!meta.data && meta.data.kind === "table" && !(connection?.info.readOnly ?? false);

  const [draft, setDraft] = useState<{
    rows: FilterRowSpec[];
    conjunction: TableTabState["conjunction"];
  }>({ rows: state.filters, conjunction: state.conjunction });
  useEffect(
    () => setDraft({ rows: state.filters, conjunction: state.conjunction }),
    [state.filters, state.conjunction],
  );
  const draftDirty = draft.rows !== state.filters || draft.conjunction !== state.conjunction;
  // The empty bar's starter row keeps its id across renders (a new id would remount its input).
  const defaultColumn = usePrefs((p) => p.filterDefaultColumn);
  const defaultOperator = usePrefs((p) => p.filterDefaultOperator);
  const metaColumns = meta.data?.columns;
  const starterRow = useMemo(
    () => defaultFilterRow(metaColumns ?? [], { column: defaultColumn, operator: defaultOperator }),
    [metaColumns, defaultColumn, defaultOperator],
  );

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
            sort:
              state.sort.length > 0
                ? state.sort
                : defaultOrderSort(defaultOrder, meta.data.primaryKey),
            limit: state.limit,
            offset: state.offset,
          }
        : null,
    [meta.data, filterGroup, state.sort, state.limit, state.offset, defaultOrder],
  );
  // grid3: a new page (or another sort / filter) starts at the grid's top.
  const pageKey = useMemo(
    () => ({ offset: state.offset, sort: state.sort, filters: state.filters }),
    [state.offset, state.sort, state.filters],
  );
  const rows = useTableRows(connectionId, table, request, state.refreshToken, pending > 0);
  const rowNumbers = usePrefs((p) => p.gridRowNumbers);
  const estimateCount = usePrefs((p) => p.gridEstimateCount);
  const estimateAbove = usePrefs((p) => p.gridEstimateThreshold);
  const estimate = useMemo(
    () => ({ above: estimateCount ? estimateAbove : null }),
    [estimateCount, estimateAbove],
  );
  const count = useRowCount(
    connectionId,
    table,
    filterGroup,
    state.refreshToken,
    meta.data !== undefined,
    estimate,
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
  // Grid columns follow the PAGE's columns (row cells are in that order); the metadata may be newer
  // or older for a moment around a refresh.
  const pageColumns = page?.columns;
  const columnSignature = useMemo(
    () => (pageColumns ? pageColumns.map((c) => c.name).join("\u0000") : null),
    [pageColumns],
  );
  // Remember which columns the pending edits were made against (see `stagedOn`).
  const stagedOn = state.stagedOn;
  useEffect(() => {
    if (pending === 0) {
      if (stagedOn !== null) update(tab.id, { stagedOn: null });
    } else if (stagedOn === null && columnSignature !== null) {
      update(tab.id, { stagedOn: columnSignature });
    }
  }, [pending, stagedOn, columnSignature, tab.id, update]);
  const columns = useMemo(
    () =>
      meta.data
        ? buildGridColumns(pageColumns ?? meta.data.columns, meta.data.foreignKeys, editable)
        : [],
    [meta.data, pageColumns, editable],
  );
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

  /** A row's Apply (TablePlus): that filter alone; the other rows stay in the bar, unchecked. */
  const applyFilterRow = useCallback(
    (id: string) =>
      guarded(() =>
        update(tab.id, {
          filters: draft.rows.map((r) => ({ ...r, enabled: r.id === id })),
          conjunction: draft.conjunction,
          offset: 0,
        }),
      ),
    [draft, tab.id, update, guarded],
  );
  /** Applies the bar's checked rows, joined with `conjunction` (Apply All ▾ AND / OR). */
  const applyFilters = useCallback(
    (conjunction?: Conjunction) =>
      guarded(() =>
        update(tab.id, {
          filters: draft.rows,
          conjunction: conjunction ?? draft.conjunction,
          offset: 0,
        }),
      ),
    [draft, tab.id, update, guarded],
  );
  /** ⌘R: drops the backend's cached metadata too, so schema changes (ALTER TABLE…) show up. */
  const refresh = useCallback(
    () =>
      guarded(async () => {
        try {
          await ipc.schema.invalidate(connectionId, table);
        } catch {
          // The reload below still refetches rows; stale metadata is the worst case.
        }
        void qc.invalidateQueries({ queryKey: rowKeys.meta(connectionId, table) });
        update(tab.id, (s) => ({ refreshToken: s.refreshToken + 1, staging: discardAll() }));
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

  // ---- the right sidebar's row details ----------------------------------------
  /** The latest rows, for callbacks handed out earlier (the inspector's `edit`). */
  const live = useRef({ viewRows, page, editable, columns });
  live.current = { viewRows, page, editable, columns };
  /**
   * Stages `value` into source column `col` of the rows `keys` (one undo step), like an inline
   * edit in the grid; the sidebar's Details (one row, or every selected row in a bulk edit).
   */
  const editRows = useCallback(
    (keys: readonly string[], col: number, value: Value) => {
      const { viewRows, page, editable, columns } = live.current;
      if (!editable || !page) return;
      const column = columns[col];
      if (!column?.editable) return;
      // The grid's Set Value guards.
      if (value.t === "null" && !column.nullable) return;
      if (value.t === "default" && !column.hasDefault) return;
      if (value.t === "text" && value.v === "" && !acceptsEmpty(column)) {
        toast.message("EMPTY applies to text columns only");
        return;
      }
      const edits: CellEdit[] = [];
      let sqliteDefaults = 0;
      for (const key of keys) {
        const r = viewRows.findIndex((v) => v.key === key);
        const row = viewRows[r];
        if (!row || row.status === "deleted") continue;
        // SQLite has no `SET col = DEFAULT`; DEFAULT only works when inserting.
        if (value.t === "default" && driver === "sqlite" && row.status !== "inserted") {
          sqliteDefaults++;
          continue;
        }
        edits.push({ rowKey: key, col, value, original: page.rows[r] });
      }
      if (sqliteDefaults > 0)
        toast.message("SQLite can't set an existing value to DEFAULT", {
          description: "DEFAULT works on new rows only.",
        });
      if (edits.length > 0) setStaging((s) => setCells(s, edits));
    },
    [setStaging, driver],
  );
  /** Rows selected in the grid (indexes into `viewRows`): several make the sidebar a bulk edit. */
  const [selectedIdx, setSelectedIdx] = useState<number[]>([]);
  const hiddenColumns = state.columnState.hidden;
  /**
   * The selected rows as one bulk edit, when there are several and `r` is one of them: the first
   * row's values, the columns that differ, and every changed column.
   */
  const bulkFor = useCallback(
    (r: number) => {
      if (selectedIdx.length < 2 || !selectedIdx.includes(r)) return null;
      const rows = selectedIdx.flatMap((i) => (viewRows[i] ? [viewRows[i]] : []));
      const first = rows[0];
      if (!first || rows.length < 2) return null;
      const mixed = new Set<number>();
      const changed = new Set<number>();
      for (let c = 0; c < columns.length; c++) {
        const v0 = first.cells[c] ?? NULL;
        if (rows.some((row) => !sameValue(row.cells[c] ?? NULL, v0))) mixed.add(c);
      }
      for (const row of rows) row.changed?.forEach((c) => changed.add(c));
      const open = rows.filter((row) => row.status !== "deleted");
      return {
        first,
        keys: open.map((row) => row.key),
        changed,
        deleted: open.length === 0,
        bulk: { count: rows.length, mixed, id: rows.map((row) => row.key).join("\u0000") },
      };
    },
    [selectedIdx, viewRows, columns],
  );
  /** Shows row `r` (focused on source column `sourceIndex`) in the right sidebar. */
  const inspectRow = useCallback(
    (r: number, row: ViewRow, sourceIndex: number) => {
      const column = columns[sourceIndex];
      if (!column) return;
      const key = row.status === "inserted" ? undefined : backendKeys.get(row.key);
      const many = bulkFor(r);
      if (many) {
        const canEdit = editable && !!page && !many.deleted;
        setInspector({
          column,
          value: many.first.cells[sourceIndex] ?? NULL,
          rowIndex: r,
          source: table.name,
          row: {
            columns,
            cells: many.first.cells,
            focused: sourceIndex,
            hidden: hiddenColumns,
            changed: many.changed,
            status: many.deleted ? "deleted" : undefined,
            bulk: many.bulk,
            ...(canEdit
              ? { edit: (col: number, value: Value) => editRows(many.keys, col, value) }
              : {}),
          },
          fetch: null,
        });
        return;
      }
      const canEdit = editable && !!page && row.status !== "deleted";
      setInspector({
        column,
        value: row.cells[sourceIndex] ?? NULL,
        rowIndex: r,
        source: table.name,
        row: {
          columns,
          cells: row.cells,
          focused: sourceIndex,
          hidden: hiddenColumns,
          changed: row.changed,
          status: row.status,
          ...(canEdit
            ? { edit: (col: number, value: Value) => editRows([row.key], col, value) }
            : {}),
        },
        fetch: key ? { connectionId, table, key, column: column.name } : null,
      });
    },
    [
      columns,
      backendKeys,
      editable,
      page,
      setInspector,
      table,
      hiddenColumns,
      editRows,
      connectionId,
      bulkFor,
    ],
  );
  /** The grid's focused cell (its row by key: staging and reloads replace the row objects). */
  const focusedRow = useRef<{ key: string; sourceIndex: number } | null>(null);
  /**
   * The column the sidebar is on for row `r`: the one it shows when it already shows that row (its
   * Details can move to another field than the grid's), else `fallback`.
   */
  const sidebarColumn = useCallback(
    (r: number, fallback: number) => {
      const shown = useInspectorStore.getState().cell;
      return shown?.row && shown.source === table.name && shown.rowIndex === r
        ? shown.row.focused
        : fallback;
    },
    [table.name],
  );
  const onFocusCell = (cell: FocusedCell | null) => {
    if (!cell) {
      focusedRow.current = null;
      return setInspector(null);
    }
    const prev = focusedRow.current;
    focusedRow.current = { key: cell.row.key, sourceIndex: cell.sourceIndex };
    // The grid reports its focus again whenever its rows change; that is no reason to move the
    // sidebar off the field it shows.
    const again = prev?.key === cell.row.key && prev.sourceIndex === cell.sourceIndex;
    inspectRow(
      cell.pos.r,
      cell.row,
      again ? sidebarColumn(cell.pos.r, cell.sourceIndex) : cell.sourceIndex,
    );
  };
  // Staged edits (in the grid or from the sidebar itself), undo and reloads replace the focused
  // row: the sidebar follows, so its Details show the staged values.
  useEffect(() => {
    const f = focusedRow.current;
    if (!f || !isActive) return;
    const r = viewRows.findIndex((v) => v.key === f.key);
    const row = viewRows[r];
    if (!row) return;
    const shown = useInspectorStore.getState().cell?.row;
    const many = bulkFor(r);
    if (
      !many &&
      !shown?.bulk &&
      shown?.cells === row.cells &&
      shown.columns === columns &&
      shown.hidden === hiddenColumns &&
      shown.status === row.status &&
      !!shown.edit === (editable && !!page && row.status !== "deleted")
    )
      return;
    inspectRow(r, row, sidebarColumn(r, f.sourceIndex));
  }, [
    viewRows,
    columns,
    hiddenColumns,
    isActive,
    editable,
    page,
    inspectRow,
    sidebarColumn,
    bulkFor,
  ]);
  /** `GridAction` column indexes are SOURCE indexes: positions in `columns` / `page.columns`. */
  const onAction = (a: GridAction) => {
    if (!page) return;
    switch (a.type) {
      case "setNull":
      case "setEmpty":
      case "setDefault": {
        const edits: CellEdit[] = [];
        let sqliteDefaults = 0;
        let notText = 0;
        for (const { r, c } of a.cells) {
          const row = viewRows[r];
          const col = columns[c];
          if (!row || !col || !col.editable || row.status === "deleted") continue;
          if (a.type === "setNull" && !col.nullable) continue;
          if (a.type === "setDefault" && !col.hasDefault) continue;
          if (a.type === "setEmpty" && !acceptsEmpty(col)) {
            notText++;
            continue;
          }
          // SQLite has no `SET col = DEFAULT`; DEFAULT only works when inserting.
          if (a.type === "setDefault" && driver === "sqlite" && row.status !== "inserted") {
            sqliteDefaults++;
            continue;
          }
          edits.push({
            rowKey: row.key,
            col: c,
            value:
              a.type === "setNull"
                ? { t: "null" }
                : a.type === "setEmpty"
                  ? { t: "text", v: "" }
                  : { t: "default" },
            original: originalOf(r),
          });
        }
        if (sqliteDefaults > 0)
          toast.message("SQLite can't set an existing value to DEFAULT", {
            description: "DEFAULT works on new rows only.",
          });
        if (notText > 0 && edits.length === 0) toast.message("EMPTY applies to text columns only");
        if (edits.length > 0) setStaging((s) => setCells(s, edits));
        return;
      }
      case "setNow": {
        // The server's NOW(): staged as the expression, sent unquoted (never a client-side time).
        const edits: CellEdit[] = [];
        for (const { r, c } of a.cells) {
          const row = viewRows[r];
          const col = columns[c];
          const value = col ? nowValue(col.kind) : null;
          if (!row || !col || !value || !col.editable || row.status === "deleted") continue;
          edits.push({ rowKey: row.key, col: c, value, original: originalOf(r) });
        }
        if (edits.length > 0) setStaging((s) => setCells(s, edits));
        return;
      }
      case "setFile": {
        const targets = a.cells.flatMap(({ r, c }) => {
          const row = viewRows[r];
          const col = columns[c];
          return row && col?.editable && col.kind === "bytes" && row.status !== "deleted"
            ? [{ rowKey: row.key, col: c, original: originalOf(r) }]
            : [];
        });
        if (targets.length === 0) return;
        void (async () => {
          try {
            const path = await pickBinaryFile();
            if (!path) return;
            const value = await ipc.files.readBinary(path);
            setStaging((s) =>
              setCells(
                s,
                targets.map((t) => ({ ...t, value })),
              ),
            );
          } catch (e) {
            toast.error("Could not add the file", { description: (e as Error).message });
          }
        })();
        return;
      }
      case "setValues": {
        const edits: CellEdit[] = [];
        for (const { r, c, value } of a.edits) {
          const row = viewRows[r];
          const col = columns[c];
          if (!row || !col || !col.editable || row.status === "deleted") continue;
          edits.push({ rowKey: row.key, col: c, value, original: originalOf(r) });
        }
        if (edits.length > 0) setStaging((s) => setCells(s, edits));
        return;
      }
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
      case "duplicateRows": {
        // One undo step for the whole selection; auto-increment columns get a fresh value.
        const copies = a.rows.flatMap((r) => {
          const row = viewRows[r];
          if (!row) return [];
          const values = row.cells.map((v, i) =>
            page.columns[i]?.isAutoIncrement ? { t: "default" as const } : v,
          );
          return [{ rowKey: `new:${crypto.randomUUID()}`, values }];
        });
        setStaging((s) => insertRows(s, copies));
        return;
      }
      case "newRow":
        setStaging((s) => insertRow(s, `new:${crypto.randomUUID()}`, defaultsFor(page.columns)));
        return;
      case "insertRows": {
        // Pasted rows: each column's default, overwritten by the pasted values.
        const defaults = defaultsFor(page.columns);
        const pasted = a.rows.map((cells) => {
          const values = [...defaults];
          for (const { c, value } of cells) if (c < values.length) values[c] = value;
          return { rowKey: `new:${crypto.randomUUID()}`, values };
        });
        // The whole paste is one undo step.
        setStaging((s) => insertRows(s, pasted));
        return;
      }
      case "undo":
        setStaging((s) => undo(s));
        return;
    }
  };

  // ---- commit / discard ----------------------------------------------------
  // ⌘S / ⇧⌘P / ⇧⌘⌫ commit, preview and discard every pending change of the workspace, this
  // tab's rows with the other tabs' (`features/commit/workspace.ts`); the host answers them. The
  // workspace commit builds this tab's request from what its rows were staged against, which
  // is recorded whenever its page changes (it may be another tab that is shown at commit time).
  useEffect(() => {
    if (!page || columnSignature === null) return;
    setRowContext(tab.id, {
      table,
      columns: page.columns,
      signature: columnSignature,
      identity: page.identity,
      keys: backendKeys,
    });
  }, [tab.id, table, page, columnSignature, backendKeys]);
  // A commit of this tab's rows clears the selection of the edited rows (TablePlus #2979).
  const committedGeneration = useCommitGeneration(tab.id);

  useAction("view.refresh", isActive ? refresh : null);
  // ⌘F (View ▸ Filters): opens the bar, or keeps it open, on the last applied filter's value
  // (TablePlus b190); Esc in the bar closes it.
  const [filterFocus, setFilterFocus] = useState(0);
  const lastApplied = useMemo(
    () => (meta.data ? lastAppliedRowId(state.filters, meta.data.columns) : null),
    [meta.data, state.filters],
  );
  useAction(
    "grid.filter",
    isActive
      ? () => {
          update(tab.id, { filtersVisible: true });
          setFilterFocus((n) => n + 1);
        }
      : null,
  );
  useAction("grid.redo", isActive && canRedo(staging) ? () => setStaging((s) => redo(s)) : null);
  useAction("view.structure", isActive ? () => setView(tab.id, "structure") : null);
  const [columnsOpen, setColumnsOpen] = useState(false);
  /** Rows selected in the grid (the bottom bar's "N of M rows selected"). */
  const [selectedRows, setSelectedRows] = useState(0);
  useAction("grid.columns", isActive ? () => setColumnsOpen((open) => !open) : null);
  // ⌘J (the foreign key of the focused cell) is the grid's own: with several keys it asks which.
  /** ⌘← / ⌘→ in the grid: the previous / next page (like the bottom bar's ‹ ›). */
  const turnPage = (dir: -1 | 1) => {
    const { offset, limit } = state;
    const total = count.data?.count ?? null;
    const hasNext = total !== null ? offset + limit < total : (page?.rows.length ?? 0) === limit;
    if (dir < 0 ? offset === 0 : !hasNext) return;
    guarded(() => update(tab.id, { offset: Math.max(0, offset + dir * limit) }));
  };
  useAction("grid.prev-page", isActive ? () => turnPage(-1) : null);
  useAction("grid.next-page", isActive ? () => turnPage(1) : null);
  /** Quick Look's foreign-key viewer: the referenced row(s), by an `=` filter. */
  const loadReferenced = async (column: GridColumn, value: Value): Promise<ReferencedRows> => {
    const fk = column.fk;
    const filter = referencedFilter(column, value);
    if (!fk || !filter) return { table: fk?.table ?? "", columns: [], rows: [] };
    const target = { schema: fk.schema ?? table.schema, name: fk.table };
    const found = await ipc.browse.rows(connectionId, {
      table: target,
      columns: null,
      filter,
      sort: [],
      limit: 5,
      offset: 0,
    });
    return {
      table: qualifiedName(target),
      columns: referencedColumns(found.columns),
      rows: found.rows,
    };
  };
  /** Quick Look on a blob cut to its preview: the whole stored value. */
  const loadFullValue = async (r: number, c: number): Promise<Value> => {
    const row = viewRows[r];
    const key = row && row.status !== "inserted" ? backendKeys.get(row.key) : undefined;
    const column = columns[c];
    if (!key || !column) throw new Error("This value is not stored yet");
    return ipc.browse.cell(connectionId, table, key, column.name);
  };

  /**
   * FK cell menu ▸ Generate JOIN: `SELECT * FROM this JOIN referenced ON …` in a new SQL tab, over
   * the key the grid picked (`chosen`; the column's own single-column key, `column.fk`, otherwise).
   */
  const generateJoin = (column: GridColumn, chosen?: FkRef) => {
    const fk = chosen ?? column.fk ?? fksOf(column)[0];
    if (!fk) return;
    const pairs = fkPairs(fk, column.name);
    const sql = joinSql(DIALECT[driver] ?? "postgres", {
      table,
      column: pairs[0]?.local ?? column.name,
      references: {
        schema: fk.schema ?? table.schema,
        name: fk.table,
        column: pairs[0]?.ref ?? fk.column,
      },
      pairs:
        pairs.length > 1 ? pairs.map((p) => ({ column: p.local, reference: p.ref })) : undefined,
    });
    openSqlInNewQueryTab(connectionId, sql, driver, `${table.name} JOIN ${fk.table}`);
  };

  /**
   * An FK arrow, ⌘J, the cell menu's Open or Quick Look's Open: the referenced table in a tab of
   * its own, filtered to the key (`=` on each of its columns, AND-ed: a composite key reads its
   * other columns from the row, `rowCells` in SOURCE order, else the grid's focused row). Without
   * a `chosen` key it is the column's own `fk` (what Quick Look shows), never a composite one that
   * merely sorts first. A blob key is typed as hex.
   */
  const openFk = (
    column: GridColumn,
    value: Value,
    chosen?: FkRef,
    rowCells?: readonly Value[],
  ) => {
    const fk = chosen ?? column.fk ?? fksOf(column)[0];
    if (!fk) return;
    const { viewRows, columns } = live.current;
    const focusedKey = focusedRow.current?.key;
    const cells = rowCells ?? viewRows.find((v) => v.key === focusedKey)?.cells;
    const filters: FilterRowSpec[] = [];
    for (const { local, ref } of fkPairs(fk, column.name)) {
      const part =
        local === column.name ? value : cells?.[columns.findIndex((c) => c.name === local)];
      // A NULL part matches nothing: the other parts still find the rows it would sit among.
      if (!part || part.t === "null" || part.t === "default") continue;
      const text = fkFilterText(part);
      if (text === null) {
        toast.message("Only a preview of this value is loaded", {
          description: "Open it in Quick Look to load it in full.",
        });
        return;
      }
      filters.push(newFilterRow(ref, "eq", text));
    }
    if (filters.length === 0) return;
    const target = { schema: fk.schema ?? table.schema, name: fk.table };
    const id = openTable(connectionId, target, "table");
    useTableTabsStore.getState().ensure(id);
    useTableTabsStore.getState().update(id, {
      filters,
      conjunction: "and",
      filtersVisible: true,
      offset: 0,
    });
  };

  /** Cell menu ▸ Quick Filter: the condition replaces the tab's filters and applies at once. */
  const onQuickFilter = ({ column, op, values }: QuickFilter) => {
    const texts = values.flatMap((v) => valueText(v) ?? []);
    // A blank = / ≠ value reads as "not filled in yet"; a quoted one-item list keeps it.
    const blank = (op === "eq" || op === "ne") && (texts[0] ?? "").trim() === "";
    const filterOp = blank ? (op === "eq" ? "in" : "notIn") : op;
    const value = filterOp === "in" || filterOp === "notIn" ? joinList(texts) : (texts[0] ?? "");
    const filters = [newFilterRow(column.name, filterOp, value)];
    guarded(() => update(tab.id, { filters, conjunction: "and", filtersVisible: true, offset: 0 }));
  };
  /** Header menu ▸ Filter with Column: a new condition on that column, its value field focused. */
  const onFilterColumn = (column: GridColumn) => {
    setDraft((d) => ({ ...d, rows: [...d.rows, newFilterRow(column.name)] }));
    update(tab.id, { filtersVisible: true });
  };

  /**
   * Replaces the (estimated) row count with an exact one (`count.data`, then the tab's slice): it
   * goes under the key the shown count was fetched with, estimate setting included.
   */
  const countExactly = () =>
    void ipc.browse
      .count(connectionId, table, filterGroup, true)
      .then((c) =>
        qc.setQueryData(
          rowKeys.count(connectionId, table, filterGroup, state.refreshToken, estimate),
          c,
        ),
      )
      .catch((e: unknown) =>
        toast.error("Could not count the rows", { description: errorWithHint(e) }),
      );
  /** `[+ Row]`: the grid's own "new row" (it focuses the row once it shows up). */
  const addRow = () => {
    focusGrid();
    if (actions.has("grid.new-row")) void actions.run("grid.new-row");
    else onAction({ type: "newRow" });
  };
  const toggleFilters = () => {
    if (!state.filtersVisible) setFilterFocus((n) => n + 1);
    update(tab.id, (s) => ({ filtersVisible: !s.filtersVisible }));
  };
  const onToggleColumn = (name: string, visible: boolean) =>
    update(tab.id, (s) => ({
      columnState: {
        ...s.columnState,
        hidden: visible
          ? s.columnState.hidden.filter((h) => h !== name)
          : [...s.columnState.hidden, name],
      },
    }));

  const onSortChange = (sort: SortSpec[]) => guarded(() => update(tab.id, { sort, offset: 0 }));
  const onColumnStateChange = (columnState: ColumnState) => update(tab.id, { columnState });

  // The view switch stays reachable while the metadata loads or failed to.
  if (meta.isPending || meta.isError) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <div className="min-h-0 flex-1">
          {meta.isPending ? (
            <div className="flex flex-col gap-2 p-4">
              <Skeleton className="h-5 w-48" />
              <Skeleton className="h-64 w-full" />
            </div>
          ) : (
            <EmptyState title="Could not load table" description={errorWithHint(meta.error)} />
          )}
        </div>
        <BottomBarFrame aria-label="Table data">
          <ViewSwitch tabId={tab.id} view="table" />
        </BottomBarFrame>
      </div>
    );
  }
  const noKey = page?.identity === "none";
  /** Conditions the applied filter actually holds (blank and unchecked rows do nothing). */
  const appliedFilters = filterGroup?.items.length ?? 0;

  return (
    <div ref={rootRef} className="flex h-full min-h-0 flex-col">
      {state.filtersVisible && (
        <FilterBar
          columns={meta.data.columns}
          rows={draft.rows.length > 0 ? draft.rows : [starterRow]}
          conjunction={draft.conjunction}
          dirty={draftDirty}
          dialect={DIALECT[driver] ?? "postgres"}
          sql={page?.sql ?? null}
          onChange={(r, c) => setDraft({ rows: r, conjunction: c })}
          onApply={applyFilters}
          onApplyRow={applyFilterRow}
          appliedRows={state.filters}
          table={table.name}
          onClear={() => {
            setDraft({ rows: [], conjunction: "and" });
            guarded(() => update(tab.id, { filters: [], offset: 0 }));
          }}
          onClose={() => {
            update(tab.id, { filtersVisible: false });
            focusGrid();
          }}
          onExport={() => void openExportForActiveTab()}
          focusRequest={filterFocus}
          focusRowId={lastApplied}
          onFocusData={focusGrid}
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
          <EmptyState title="Query failed" description={errorWithHint(rows.error)} />
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
            onFocusCell={onFocusCell}
            onSelectionChange={setSelectedRows}
            onOpenFk={openFk}
            onFocusFilters={state.filtersVisible ? focusFilters : undefined}
            copyContext={{
              dialect: DIALECT[driver] ?? "postgres",
              schema: meta.data.table.schema,
              table: meta.data.table.name,
              mysqlLongLat: mysqlLongLat(
                DIALECT[driver] ?? "postgres",
                connection?.info.serverVersion ?? "",
              ),
            }}
            editable={editable && !!page}
            readOnlyNotice={editable ? undefined : "This table is read-only"}
            active={isActive}
            onEditCommit={onEditCommit}
            onAction={onAction}
            onQuickFilter={onQuickFilter}
            onFilterColumn={onFilterColumn}
            onSelectedRowsChange={setSelectedIdx}
            rowNumbers={rowNumbers}
            onRefresh={refresh}
            onGenerateJoin={generateJoin}
            onImport={editable ? (kind) => openImport({ connectionId, kind, table }) : undefined}
            onExportPage={() => openExportCurrentPage(connectionId, table, tab.id)}
            quickLookKey
            onLoadReferenced={loadReferenced}
            onLoadFullValue={loadFullValue}
            scrollResetKey={pageKey}
            deselectKey={committedGeneration}
          />
        )}
      </div>
      <TableBottomBar
        leading={<ViewSwitch tabId={tab.id} view="table" />}
        canAddRow={editable && !!page}
        onAddRow={addRow}
        offset={state.offset}
        limit={state.limit}
        pageRows={page?.rows.length ?? 0}
        selectedRows={rows.isError ? 0 : selectedRows}
        shownRows={viewRows.length}
        count={count.data ?? null}
        countPending={count.isPending}
        durationMs={page?.durationMs ?? null}
        sql={page?.sql ?? null}
        loading={rows.isFetching}
        onPageChange={(next) => guarded(() => update(tab.id, next))}
        onCountExact={countExactly}
        filtersVisible={state.filtersVisible}
        activeFilters={appliedFilters}
        onToggleFilters={toggleFilters}
        columns={meta.data.columns}
        hidden={hiddenColumns}
        columnsOpen={columnsOpen}
        onColumnsOpenChange={setColumnsOpen}
        onToggleColumn={onToggleColumn}
        onShowAllColumns={() =>
          update(tab.id, (s) => ({ columnState: { ...s.columnState, hidden: [] } }))
        }
        onExport={() => void openExportForActiveTab()}
        defaultOrder={defaultOrder}
        onDefaultOrderChange={memoryKey ? setDefaultOrder : undefined}
        hasPrimaryKey={meta.data.primaryKey.length > 0}
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
                // The Preview SQL sheet of the workspace commit; the interrupted change runs
                // once it went through (and is dropped when the sheet is dismissed).
                const run = guard?.run ?? null;
                setGuard(null);
                openWorkspaceSheet(connectionId, run);
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
