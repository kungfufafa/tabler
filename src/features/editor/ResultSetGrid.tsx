import { useQueries, type UseQueryResult } from "@tanstack/react-query";
import { Hash, MessageSquareText, Pin, PinOff } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { DataGrid, type FocusedCell, type GridAction } from "@/features/datagrid/DataGrid";
import {
  acceptsEmpty,
  EMPTY_COLUMN_STATE,
  layoutColumns,
  type CellPos,
  type ColumnState,
  type ViewRow,
} from "@/features/datagrid/types";
import {
  bufferIndex,
  duplicateCells,
  insertedKeys,
  keyOf,
  newRowCells,
  resultRow,
} from "@/features/results/changes";
import {
  READ_ONLY_RESULT,
  TARGET_CHANGED,
  originTables,
  resultEditability,
  resultGridColumns,
  sameTable,
  type ResultEditability,
} from "@/features/results/editability";
import { autoLimitLabel, durationLabel } from "@/features/results/footer";
import {
  canStageAgainst,
  dirtyResultOf,
  setResultEditability,
  setResultStaging,
  stagingOf,
  useResultsStore,
} from "@/features/results/store";
import {
  deleteRows,
  insertRow,
  insertRows,
  revertRow,
  setCell,
  setCells,
  undo,
} from "@/features/staging/model";
import type { CellEdit, StagingState } from "@/features/staging/types";
import {
  BarIconButton,
  BarSeparator,
  BarTooltip,
  BottomBarFrame,
} from "@/features/table-tab/BarParts";
import { rowKeys } from "@/features/table-tab/useTableRows";
import {
  ipc,
  type Dialect,
  type ResultColumn,
  type SortSpec,
  type TableMeta,
  type Value,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "@/stores/connections";
import { useInspectorStore, type CellFetch } from "@/stores/inspector";
import { setPref, usePrefs } from "@/stores/prefs";
import { resultData, useResultRowCount } from "@/stores/resultData";
import { rowsLabel } from "./labels";
import { resultGridColumns as readOnlyGridColumns, sortedOrder } from "./resultColumns";
import type { ResultSetInfo } from "./runModel";
import { FindBar } from "./search/FindBar";
import type { ReplaceEdit } from "./search/model";
import { useFind } from "./search/store";

const NULL: Value = { t: "null" };
const NO_ROWS: Value[][] = [];
const PENDING: ResultEditability = { kind: "pending" };

/**
 * `useQueries` combine: each table's metadata, null when it failed (also a failed reload, whose
 * older data is kept: the result is then read-only rather than edited against a structure that
 * may be stale), undefined while loading.
 */
const metaData = (rs: UseQueryResult<TableMeta>[]) => rs.map((r) => (r.isError ? null : r.data));

/**
 * Whether the result can be edited (`resultEditability`), once its statement finished: the
 * metadata of the tables its columns come from is loaded (shared with the table tabs' cache), and
 * the statement `sql` reads them in a way an edit can be written back. Published to the results
 * store, where ⌘S / Commit read the edit target.
 */
function useResultEditability(
  connectionId: string | null,
  info: ResultSetInfo,
  sql: string | null,
  dialect: Dialect,
): ResultEditability {
  const tables = useMemo(() => originTables(info.columns), [info.columns]);
  const readOnly = useConnectionsStore((s) =>
    connectionId ? (s.byId[connectionId]?.info.readOnly ?? false) : false,
  );
  const metas = useQueries({
    queries: tables.map((table) => ({
      queryKey: rowKeys.meta(connectionId ?? "", table),
      queryFn: () => ipc.schema.table(connectionId!, table),
      staleTime: 5 * 60_000,
      retry: false,
      enabled: info.done && connectionId !== null,
    })),
    combine: metaData,
  });
  const editability = useMemo(
    () =>
      !info.done || !connectionId
        ? PENDING
        : resultEditability(info.columns, (t) => metas[tables.findIndex((x) => sameTable(x, t))], {
            sql,
            dialect,
            readOnlyConnection: readOnly,
          }),
    [info.done, info.columns, connectionId, metas, tables, readOnly, sql, dialect],
  );
  useEffect(() => setResultEditability(info.id, editability), [info.id, editability]);
  return editability;
}

/** The subtle notice of an edit attempt on a read-only result. */
function notifyReadOnly(editability: ResultEditability): void {
  if (editability.kind !== "readOnly") return;
  toast.message(READ_ONLY_RESULT, { id: "read-only-result", description: editability.reason });
}

export interface ResultGridProps {
  tabId: string;
  connectionId: string | null;
  /** The result set (`resultData` buffer `info.id`). */
  info: ResultSetInfo;
  /** "Result 2": the inspector's source and the export title. */
  label: string;
  /** The statement that produced the result (null when unknown: the result is read-only). */
  sql: string | null;
  dialect: Dialect;
  /** The grid owns the grid shortcuts (the visible tab's selected result). */
  active: boolean;
  /** Fixed height (stacked results); the grid fills its parent otherwise. */
  height?: number;
  /** The footer's controls (rows, time, Show Message, Pin, row numbers, Export…). */
  footer?: ReactNode;
}

/**
 * One streamed result set. Editable like a table grid when its columns come from one table whose
 * primary key is in the result (see `features/results`): double-click / ↩ edits, Set Value, ⌘I,
 * ⌘D, ⌫, undo / redo; staged edits live in the results store until ⌘S commits them. Otherwise an
 * edit attempt says the result is read-only.
 */
export function ResultGrid({
  tabId,
  connectionId,
  info,
  label,
  sql,
  dialect,
  active,
  height,
  footer,
}: ResultGridProps) {
  const rowCount = useResultRowCount(info.id);
  // Streamed rows are appended to this array in place (`rowCount` re-renders); a commit replaces
  // it, so its identity changes then.
  const rows = resultData.get(info.id)?.rows ?? NO_ROWS;
  const editability = useResultEditability(connectionId, info, sql, dialect);
  const target = editability.kind === "editable" ? editability.target : null;
  const staging = useResultsStore((s) => stagingOf(s, info.id));
  const driver = useConnectionsStore((s) =>
    connectionId ? s.byId[connectionId]?.info.driver : undefined,
  );
  const rowNumbers = usePrefs((p) => p.resultRowNumbers);
  const columns = useMemo(() => resultGridColumns(info.columns, target), [info.columns, target]);
  const [sort, setSort] = useState<SortSpec[]>([]);
  const [columnState, setColumnState] = useState<ColumnState>(EMPTY_COLUMN_STATE);
  // Client-side sort; recomputed as rows stream in or a commit changes them.
  const order = useMemo(
    () => (sort.length > 0 && rowCount > 0 ? sortedOrder(rows, sort) : null),
    [sort, rowCount, rows],
  );
  const inserted = useMemo(() => insertedKeys(staging), [staging]);
  const width = info.columns.length;
  const getRow = useCallback(
    (i: number): ViewRow | undefined => resultRow(rows, order, staging, i, inserted, width),
    [rows, order, staging, inserted, width],
  );
  const editable = target !== null && info.done;

  // ---- staging ---------------------------------------------------------------------------
  const originalOf = useCallback(
    (key: string): Value[] | undefined => {
      const idx = bufferIndex(key);
      return idx === null ? undefined : rows[idx];
    },
    [rows],
  );
  /**
   * Stages `fn` unless another result of the tab already holds changes (one at a time), or the
   * result's changes were staged against another edit target than the current one. Returns
   * whether it staged (a refusal says why in a toast).
   */
  const stage = useCallback(
    (fn: (s: StagingState) => StagingState): boolean => {
      if (!editable || !target) {
        notifyReadOnly(editability);
        return false;
      }
      const st = useResultsStore.getState();
      const other = dirtyResultOf(st, tabId);
      if (other && other !== info.id) {
        toast.message("Commit or discard the other result's changes first", {
          id: "result-edits-elsewhere",
          description: "Only one result at a time can hold uncommitted changes.",
        });
        return false;
      }
      if (!canStageAgainst(st, info.id, target)) {
        toast.error(TARGET_CHANGED, { id: "result-target-changed" });
        return false;
      }
      setResultStaging(tabId, info.id, fn, target);
      return true;
    },
    [editable, editability, target, tabId, info.id],
  );

  const onEditCommit = (r: number, c: number, value: Value) => {
    const row = getRow(r);
    if (!row) return;
    stage((s) => setCell(s, row.key, c, value, originalOf(row.key)));
  };

  /** Stages `value` into column `col` of row `key` from the right sidebar (the grid's guards). */
  const editFromSidebar = useCallback(
    (key: string, col: number, value: Value) => {
      const column = columns[col];
      if (!column?.editable) return;
      if (value.t === "null" && !column.nullable) return;
      if (value.t === "default") {
        if (!column.hasDefault) return;
        if (driver === "sqlite" && !key.startsWith("new:")) {
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
      stage((s) => setCell(s, key, col, value, originalOf(key)));
    },
    [columns, driver, stage, originalOf],
  );

  /** Stages cell values (Set Value, Find's Replace) as one undo step; false when none was. */
  const stageValues = (values: readonly ReplaceEdit[]): boolean => {
    const edits: CellEdit[] = [];
    for (const { r, c, value } of values) {
      const row = getRow(r);
      if (!row || !columns[c]?.editable || row.status === "deleted") continue;
      edits.push({ rowKey: row.key, col: c, value, original: originalOf(row.key) });
    }
    return edits.length > 0 && stage((s) => setCells(s, edits));
  };

  const onAction = (a: GridAction) => {
    if (!target) return notifyReadOnly(editability);
    switch (a.type) {
      case "setNull":
      case "setEmpty":
      case "setDefault": {
        const edits: CellEdit[] = [];
        let sqliteDefaults = 0;
        let notText = 0;
        for (const { r, c } of a.cells) {
          const row = getRow(r);
          const col = columns[c];
          if (!row || !col?.editable || row.status === "deleted") continue;
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
          const value: Value =
            a.type === "setNull"
              ? { t: "null" }
              : a.type === "setEmpty"
                ? { t: "text", v: "" }
                : { t: "default" };
          edits.push({ rowKey: row.key, col: c, value, original: originalOf(row.key) });
        }
        if (sqliteDefaults > 0)
          toast.message("SQLite can't set an existing value to DEFAULT", {
            description: "DEFAULT works on new rows only.",
          });
        if (notText > 0 && edits.length === 0) toast.message("EMPTY applies to text columns only");
        if (edits.length > 0) stage((s) => setCells(s, edits));
        return;
      }
      case "setValues":
        stageValues(a.edits);
        return;
      case "revertRows": {
        const keys = a.rows.flatMap((r) => getRow(r)?.key ?? []);
        stage((s) => keys.reduce((acc, k) => revertRow(acc, k), s));
        return;
      }
      case "deleteRows": {
        const rows = a.rows.flatMap((r) => {
          const row = getRow(r);
          return row ? [{ rowKey: row.key, original: originalOf(row.key) }] : [];
        });
        stage((s) => deleteRows(s, rows));
        return;
      }
      case "duplicateRows": {
        // One undo step for the whole selection; auto-increment columns get a fresh value.
        const copies = a.rows.flatMap((r) => {
          const row = getRow(r);
          return row
            ? [{ rowKey: `new:${crypto.randomUUID()}`, values: duplicateCells(target, row.cells) }]
            : [];
        });
        stage((s) => insertRows(s, copies));
        return;
      }
      case "newRow":
        stage((s) => insertRow(s, `new:${crypto.randomUUID()}`, newRowCells(target)));
        return;
      case "insertRows": {
        // Pasted rows: the table's defaults, overwritten by the pasted values of its columns.
        const pasted = a.rows.map((cells) => {
          const values = newRowCells(target);
          for (const { c, value } of cells) if (target.columns[c]) values[c] = value;
          return { rowKey: `new:${crypto.randomUUID()}`, values };
        });
        stage((s) => insertRows(s, pasted));
        return;
      }
      case "undo":
        setResultStaging(tabId, info.id, (s) => undo(s));
        return;
    }
  };

  // ---- the right sidebar's row details -----------------------------------------------------
  const setInspector = useInspectorStore((s) => s.set);
  const focused = useRef<{ key: string; sourceIndex: number } | null>(null);
  const onFocusCell = (cell: FocusedCell | null) => {
    if (!cell) {
      focused.current = null;
      return setInspector(null);
    }
    const prev = focused.current;
    focused.current = { key: cell.row.key, sourceIndex: cell.sourceIndex };
    // Reported again because the rows changed (an edit): keep the field the sidebar shows.
    const shown = useInspectorStore.getState().cell;
    const again = prev?.key === cell.row.key && prev.sourceIndex === cell.sourceIndex;
    const col =
      again && shown?.row && shown.source === label && shown.rowIndex === cell.pos.r
        ? shown.row.focused
        : cell.sourceIndex;
    const column = columns[col];
    if (!column) return;
    const row = cell.row;
    const original = originalOf(row.key);
    // A saved row of the target table: the full (blob) value of a column of that table (by its
    // name there, not the result's alias) can be fetched by key; other columns have none.
    const fetchAt = (c: number): CellFetch | null => {
      const origin = target?.columns[c];
      return target && origin && original && connectionId && row.status !== "inserted"
        ? { connectionId, table: target.table, key: keyOf(target, original), column: origin.name }
        : null;
    };
    setInspector({
      column,
      value: row.cells[col] ?? NULL,
      rowIndex: cell.pos.r,
      source: label,
      row: {
        columns,
        cells: row.cells,
        focused: col,
        changed: row.changed,
        status: row.status,
        ...(editable && row.status !== "deleted"
          ? { edit: (c: number, value: Value) => editFromSidebar(row.key, c, value) }
          : {}),
      },
      fetch: fetchAt(col),
      fetchAt,
    });
  };

  // ---- find & replace (⌘F) -----------------------------------------------------------------
  const find = useFind(tabId);
  const showFind = active && find.open && info.columns.length > 0;
  const [reveal, setReveal] = useState<CellPos | null>(null);
  const onReveal = useCallback((cell: CellPos) => setReveal({ r: cell.r, c: cell.c }), []);
  const gridBox = useRef<HTMLDivElement>(null);
  const readOnlyReason = editable
    ? null
    : !info.done
      ? "The query is still running."
      : editability.kind === "readOnly"
        ? editability.reason || READ_ONLY_RESULT
        : READ_ONLY_RESULT;
  // The columns the grid shows (Hide Column), in display order: Find searches those only.
  const visibleColumns = useMemo(
    () => layoutColumns(columns, columnState).src,
    [columns, columnState],
  );
  const focusGrid = () =>
    gridBox.current
      ?.querySelector<HTMLElement>("[data-scope='grid']")
      ?.focus({ preventScroll: true });

  // An edit attempt on a read-only result says why (the grid itself ignores it).
  const onDoubleClick = (e: MouseEvent<HTMLDivElement>) => {
    if (editable || editability.kind !== "readOnly") return;
    const hit = document.elementFromPoint(e.clientX, e.clientY);
    if (hit?.closest("[data-cell]")) notifyReadOnly(editability);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (editable || editability.kind !== "readOnly" || e.defaultPrevented) return;
    if ((e.key === "Enter" || e.key === "F2") && !e.metaKey && !e.ctrlKey && !e.altKey)
      notifyReadOnly(editability);
  };

  return (
    <div
      className="flex min-h-0 flex-col"
      style={height === undefined ? { height: "100%" } : { height }}
    >
      {showFind && (
        <FindBar
          tabId={tabId}
          rowCount={rowCount + inserted.length}
          getRow={getRow}
          columns={columns}
          visibleColumns={visibleColumns}
          readOnlyReason={readOnlyReason}
          onReveal={onReveal}
          onReplace={stageValues}
          onClose={focusGrid}
        />
      )}
      <div
        ref={gridBox}
        className="min-h-0 flex-1"
        onDoubleClick={onDoubleClick}
        onKeyDown={onKeyDown}
      >
        <DataGrid
          columns={columns}
          rowCount={rowCount + inserted.length}
          getRow={getRow}
          loading={!info.done && rowCount === 0}
          emptyText={info.columns.length === 0 ? "No columns" : "No rows"}
          sort={sort}
          onSortChange={setSort}
          columnState={columnState}
          onColumnStateChange={setColumnState}
          copyContext={{
            dialect,
            schema: target?.table.schema ?? null,
            table: target?.table.name ?? info.columns.find((c) => c.table)?.table ?? "result",
          }}
          editable={editable}
          readOnlyNotice={editability.kind === "readOnly" ? READ_ONLY_RESULT : undefined}
          active={active}
          rowNumbers={rowNumbers}
          onEditCommit={onEditCommit}
          onAction={onAction}
          onFocusCell={onFocusCell}
          revealCell={showFind ? reveal : null}
        />
      </div>
      {footer}
    </div>
  );
}

/** TablePlus's results footer: row count, time, Show Message, Pin, row numbers, Export…. */
export function ResultFooter({
  info,
  rows,
  pinned,
  onShowMessage,
  onTogglePin,
  onExport,
}: {
  info: ResultSetInfo;
  rows: number;
  pinned: boolean;
  onShowMessage: (() => void) | null;
  onTogglePin: () => void;
  onExport: () => void;
}) {
  const rowNumbers = usePrefs((p) => p.resultRowNumbers);
  const duration = durationLabel(info.durationMs);
  return (
    <BottomBarFrame aria-label="Result" className="h-7 gap-1.5">
      <span className="truncate tabular-nums" data-slot="result-rows">
        {info.truncated ? (
          <BarTooltip label="The row limit stopped reading more rows (the editor bar's limit).">
            <span className="text-warning">{autoLimitLabel(rows)}</span>
          </BarTooltip>
        ) : (
          rowsLabel(rows)
        )}
      </span>
      {duration && (
        <>
          <BarSeparator />
          <span className="shrink-0 tabular-nums" data-slot="result-duration">
            {duration}
          </span>
        </>
      )}
      <span className="flex-1" />
      {onShowMessage && (
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground hover:text-foreground h-5 gap-1 px-1.5 text-[11px]"
          onClick={onShowMessage}
        >
          <MessageSquareText className="size-3" />
          <span className="@max-[420px]:hidden">Show Message</span>
        </Button>
      )}
      <BarIconButton
        label={pinned ? "Unpin Result" : "Pin Result"}
        tooltip={pinned ? "Unpin Result" : "Pin Result: keep it when the query runs again"}
        aria-pressed={pinned}
        className={cn(pinned && "text-brand hover:text-brand")}
        onClick={onTogglePin}
      >
        {pinned ? <PinOff /> : <Pin />}
      </BarIconButton>
      <BarIconButton
        label={rowNumbers ? "Hide Row Numbers" : "Show Row Numbers"}
        aria-pressed={rowNumbers}
        className={cn(rowNumbers && "text-foreground")}
        onClick={() => setPref("resultRowNumbers", !rowNumbers)}
      >
        <Hash />
      </BarIconButton>
      <BarSeparator />
      <Button
        variant="ghost"
        size="xs"
        className="text-muted-foreground hover:text-foreground h-5 px-1.5 text-[11px]"
        onClick={onExport}
      >
        Export…
      </Button>
    </BottomBarFrame>
  );
}

/** Read-only grid over rows that arrived in one piece (an EXPLAIN result). */
export function StaticGrid({
  columns: resultColumns,
  rows,
  label,
  dialect,
}: {
  columns: readonly ResultColumn[];
  rows: readonly Value[][];
  label: string;
  dialect: Dialect;
}) {
  const columns = useMemo(() => readOnlyGridColumns(resultColumns), [resultColumns]);
  const [sort, setSort] = useState<SortSpec[]>([]);
  const order = useMemo(
    () => (sort.length > 0 && rows.length > 0 ? sortedOrder(rows, sort) : null),
    [sort, rows],
  );
  const getRow = useCallback(
    (i: number): ViewRow | undefined => {
      const idx = order ? order[i] : i;
      const cells = idx === undefined ? undefined : rows[idx];
      return cells ? { key: String(idx), cells } : undefined;
    },
    [order, rows],
  );
  const setInspector = useInspectorStore((s) => s.set);
  return (
    <DataGrid
      columns={columns}
      rowCount={rows.length}
      getRow={getRow}
      emptyText={resultColumns.length === 0 ? "No columns" : "No rows"}
      sort={sort}
      onSortChange={setSort}
      copyContext={{ dialect, schema: null, table: "result" }}
      editable={false}
      onFocusCell={(cell) =>
        setInspector(
          cell
            ? {
                column: cell.column,
                value: cell.value,
                rowIndex: cell.pos.r,
                source: label,
                row: { columns, cells: cell.row.cells, focused: cell.sourceIndex },
                fetch: null,
              }
            : null,
        )
      }
    />
  );
}
