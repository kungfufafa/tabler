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
import {
  acceptsEmpty,
  buildGridColumns,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import { openExportForActiveTab } from "@/features/export/store";
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
  setRowError,
  undo,
} from "@/features/staging/model";
import type { CellEdit } from "@/features/staging/types";
import { rowKeyFor, toStagedChanges } from "@/features/staging/toPayload";
import { ViewSwitch } from "@/features/tabs/ViewSwitch";
import {
  ipc,
  type BrowsePage,
  type Dialect,
  type RowKey,
  type SortSpec,
  type Value,
} from "@/lib/ipc";
import { formatCombo } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { valueText } from "@/lib/values";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore } from "@/stores/inspector";
import { getPrefs } from "@/stores/prefs";
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
import { joinList, toFilterGroup } from "./filters";
import { PREVIEW_CLOSED, type PreviewState } from "./previewState";
import { PreviewSqlSheet } from "./PreviewSqlSheet";
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
  const setView = useTabsStore((s) => s.setView);
  const pin = useTabsStore((s) => s.pin);
  const ensure = useTableTabsStore((s) => s.ensure);
  const update = useTableTabsStore((s) => s.update);
  useEffect(() => {
    ensure(tab.id);
  }, [tab.id, ensure]);
  const state = useTableTab(tab.id);
  const staging = state.staging;
  const pending = pendingCount(staging);
  const setInspector = useInspectorStore((s) => s.set);
  const rootRef = useRef<HTMLDivElement>(null);
  const focusGrid = () =>
    rootRef.current
      ?.querySelector<HTMLElement>('[data-scope="grid"]')
      ?.focus({ preventScroll: true });

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
  const firstColumn = meta.data?.columns[0]?.name ?? "";
  const starterRow = useMemo(() => newFilterRow(firstColumn), [firstColumn]);

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
  const rows = useTableRows(connectionId, table, request, state.refreshToken, pending > 0);
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

  const applyFilters = useCallback(
    () =>
      guarded(() =>
        update(tab.id, { filters: draft.rows, conjunction: draft.conjunction, offset: 0 }),
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
  /** Stages `value` into source column `col` of the row `key`, like an inline edit in the grid. */
  const editRow = useCallback(
    (key: string, col: number, value: Value) => {
      const { viewRows, page, editable, columns } = live.current;
      if (!editable || !page) return;
      const r = viewRows.findIndex((v) => v.key === key);
      const row = viewRows[r];
      const column = columns[col];
      if (!row || row.status === "deleted" || !column?.editable) return;
      // The grid's Set Value guards.
      if (value.t === "null" && !column.nullable) return;
      if (value.t === "default") {
        if (!column.hasDefault) return;
        // SQLite has no `SET col = DEFAULT`; DEFAULT only works when inserting.
        if (driver === "sqlite" && row.status !== "inserted") {
          toast.message("SQLite can't set an existing value to DEFAULT", {
            description: "DEFAULT works on new rows only.",
          });
          return;
        }
      }
      if (value.t === "text" && value.v === "" && !acceptsEmpty(column)) {
        toast.message("EMPTY applies to text columns only");
        return;
      }
      setStaging((s) => setCell(s, key, col, value, page.rows[r]));
    },
    [setStaging, driver],
  );
  const hiddenColumns = state.columnState.hidden;
  /** Shows row `r` (focused on source column `sourceIndex`) in the right sidebar. */
  const inspectRow = useCallback(
    (r: number, row: ViewRow, sourceIndex: number) => {
      const column = columns[sourceIndex];
      if (!column) return;
      const key = row.status === "inserted" ? undefined : backendKeys.get(row.key);
      const canEdit = editable && !!page && row.status !== "deleted";
      setInspector({
        column,
        value: row.cells[sourceIndex] ?? { t: "null" },
        rowIndex: r,
        source: table.name,
        row: {
          columns,
          cells: row.cells,
          focused: sourceIndex,
          hidden: hiddenColumns,
          changed: row.changed,
          status: row.status,
          ...(canEdit ? { edit: (col: number, value: Value) => editRow(row.key, col, value) } : {}),
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
      editRow,
      connectionId,
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
    if (
      shown?.cells === row.cells &&
      shown.columns === columns &&
      shown.hidden === hiddenColumns &&
      shown.status === row.status &&
      !!shown.edit === (editable && !!page && row.status !== "deleted")
    )
      return;
    inspectRow(r, row, sidebarColumn(r, f.sourceIndex));
  }, [viewRows, columns, hiddenColumns, isActive, editable, page, inspectRow, sidebarColumn]);
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
  const [preview, setPreview] = useState<PreviewState>(PREVIEW_CLOSED);
  const buildRequest = () => {
    if (!page) return null;
    if (stagedOn !== null && stagedOn !== columnSignature) {
      // The page was reloaded with other columns since the edits were staged: committing would
      // write values into the wrong columns.
      toast.error("The table's columns changed since these edits were made", {
        description: `Discard the pending changes (${formatCombo(shortcutFor("grid.discard") ?? "")}), then edit the rows again.`,
      });
      return null;
    }
    return {
      table,
      changes: toStagedChanges(staging, page.columns, (k) => backendKeys.get(k) ?? null),
      returning: true,
    };
  };

  const openPreview = async () => {
    const req = buildRequest();
    if (!req || req.changes.length === 0) {
      afterApply.current = null;
      return;
    }
    setPreview({ ...PREVIEW_CLOSED, open: true, loading: true });
    try {
      const statements = await ipc.mutate.preview(connectionId, req);
      setPreview((p) => ({ ...p, statements, loading: false }));
    } catch (e) {
      setPreview((p) => ({ ...p, loading: false, error: (e as Error).message }));
    }
  };

  /**
   * What the unsaved-changes dialog's "Commit…" interrupted (a page change, a reload…): it runs once
   * after the next successful apply, and is dropped when the apply fails or the sheet is dismissed.
   */
  const afterApply = useRef<(() => void) | null>(null);

  /** Applies `req`; on success clears the staging and reloads, on failure marks the failed rows. */
  const apply = async (req: NonNullable<ReturnType<typeof buildRequest>>) => {
    const result = await ipc.mutate.apply(connectionId, req);
    if (result.success) {
      setPreview(PREVIEW_CLOSED);
      update(tab.id, (s) => ({ staging: discardAll(), refreshToken: s.refreshToken + 1 }));
      toast.success(
        `Applied ${result.results.length} change${result.results.length === 1 ? "" : "s"} · ${result.durationMs} ms`,
      );
      const next = afterApply.current;
      afterApply.current = null;
      next?.();
      return;
    }
    afterApply.current = null;
    setStaging((s) =>
      result.results.reduce(
        (acc, r) => (r.error ? setRowError(acc, r.rowId, r.error.message) : acc),
        s,
      ),
    );
    const failed = result.results.findIndex((r) => r.error);
    // Show what failed in the Preview SQL sheet (opened here when ⌘S skipped it).
    setPreview((p) => ({
      ...p,
      open: true,
      statements: p.statements.length > 0 ? p.statements : result.sql,
      loading: false,
      applying: false,
      error: result.error?.message ?? "Failed",
      failedIndex: failed >= 0 ? failed : null,
    }));
  };

  const applyPreview = async () => {
    const req = buildRequest();
    if (!req) return;
    setPreview((p) => ({ ...p, applying: true, error: null, failedIndex: null }));
    try {
      await apply(req);
    } catch (e) {
      afterApply.current = null;
      setPreview((p) => ({ ...p, applying: false, error: (e as Error).message }));
    }
  };

  /**
   * ⌘S: opens the Preview SQL sheet, or applies right away when "Confirm before commit" is off.
   * Tables without a key always preview: their UPDATE/DELETE match rows by every original value.
   */
  const [committing, setCommitting] = useState(false);
  const commit = async () => {
    if (getPrefs().confirmCommit || page?.identity === "none") return openPreview();
    const req = buildRequest();
    if (!req || req.changes.length === 0 || committing) return;
    setCommitting(true);
    try {
      await apply(req);
    } catch (e) {
      afterApply.current = null;
      toast.error("Could not apply changes", { description: (e as Error).message });
    } finally {
      setCommitting(false);
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
  useAction("grid.commit", isActive && pending > 0 ? () => void commit() : null);
  useAction("grid.preview", isActive && pending > 0 ? () => void openPreview() : null);
  useAction("grid.discard", isActive && pending > 0 ? discard : null);
  useAction("grid.redo", isActive && canRedo(staging) ? () => setStaging((s) => redo(s)) : null);
  useAction("view.structure", isActive ? () => setView(tab.id, "structure") : null);
  const [columnsOpen, setColumnsOpen] = useState(false);
  /** Rows selected in the grid (the bottom bar's "N of M rows selected"). */
  const [selectedRows, setSelectedRows] = useState(0);
  useAction("grid.columns", isActive ? () => setColumnsOpen((open) => !open) : null);
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

  /** Replaces the (estimated) row count with an exact one (`count.data`, then the tab's slice). */
  const countExactly = () =>
    void ipc.browse
      .count(connectionId, table, filterGroup, true)
      .then((c) =>
        qc.setQueryData(rowKeys.count(connectionId, table, filterGroup, state.refreshToken), c),
      )
      .catch((e: unknown) =>
        toast.error("Could not count the rows", { description: (e as Error).message }),
      );
  /** `[+ Row]`: the grid's own "new row" (it focuses the row once it shows up). */
  const addRow = () => {
    focusGrid();
    if (actions.has("grid.new-row")) void actions.run("grid.new-row");
    else onAction({ type: "newRow" });
  };
  const toggleFilters = () => update(tab.id, (s) => ({ filtersVisible: !s.filtersVisible }));
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
            <EmptyState title="Could not load table" description={meta.error.message} />
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
          sql={page?.sql ?? null}
          onChange={(r, c) => setDraft({ rows: r, conjunction: c })}
          onApply={applyFilters}
          onClear={() => {
            setDraft({ rows: [], conjunction: "and" });
            guarded(() => update(tab.id, { filters: [], offset: 0 }));
          }}
          onClose={() => {
            update(tab.id, { filtersVisible: false });
            focusGrid();
          }}
          onExport={() => void openExportForActiveTab()}
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
            onFocusCell={onFocusCell}
            onSelectionChange={setSelectedRows}
            onOpenFk={openFk}
            copyContext={{
              dialect: DIALECT[driver] ?? "postgres",
              schema: meta.data.table.schema,
              table: meta.data.table.name,
            }}
            editable={editable && !!page}
            readOnlyNotice={editable ? undefined : "This table is read-only"}
            active={isActive}
            onEditCommit={onEditCommit}
            onAction={onAction}
            onQuickFilter={onQuickFilter}
            onFilterColumn={onFilterColumn}
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
      />
      <PreviewSqlSheet
        state={preview}
        warning={
          noKey
            ? "No primary key: each UPDATE/DELETE matches every original column value and must affect exactly one row."
            : undefined
        }
        onOpenChange={(open) => {
          if (!open) afterApply.current = null;
          setPreview((p) => (open ? p : PREVIEW_CLOSED));
        }}
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
                afterApply.current = guard?.run ?? null;
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
