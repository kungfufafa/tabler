import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp, Copy, ExternalLink, KeyRound } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { toast } from "sonner";
import { ValueKindIcon } from "@/components/TypeIcon";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { useAction } from "@/features/palette/actions";
import type { SortSpec, Value } from "@/lib/ipc";
import { cn } from "@/lib/utils";
import { editText } from "@/lib/values";
import { CellEditor, type EditorExit } from "./CellEditor";
import { formatCells, writeClipboard, type CopyContext, type CopyFormat } from "./clipboard";
import { GridCell } from "./GridCell";
import {
  EMPTY_SELECTION,
  normalize,
  selectedRows,
  selectionBounds,
  selectionReducer,
  type SelectionAction,
  type SelectionState,
} from "./selection";
import { nextSort } from "./sort";
import {
  columnWidth,
  EMPTY_COLUMN_STATE,
  GUTTER_W,
  HEADER_H,
  MAX_COL_W,
  MIN_COL_W,
  ROW_H,
  visibleColumns,
  type CellPos,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "./types";

export type GridAction =
  | { type: "setNull" | "setDefault" | "revertCells"; cells: CellPos[] }
  | { type: "deleteRows" | "duplicateRows" | "revertRows"; rows: number[] }
  | { type: "newRow" }
  | { type: "undo" };

export interface DataGridProps {
  columns: GridColumn[];
  rowCount: number;
  getRow: (index: number) => ViewRow | undefined;
  loading?: boolean;
  emptyText?: string;
  sort?: SortSpec[];
  onSortChange?: (sort: SortSpec[]) => void;
  columnState?: ColumnState;
  onColumnStateChange?: (state: ColumnState) => void;
  /** Called when the focused cell changes (for the inspector). */
  onFocusCell?: (
    cell: { pos: CellPos; column: GridColumn; value: Value; row: ViewRow } | null,
  ) => void;
  onOpenFk?: (column: GridColumn, value: Value) => void;
  /** Dialect/table used by "Copy as INSERT". */
  copyContext?: CopyContext;
  /** Enables inline editing and row actions. */
  editable?: boolean;
  /** When true the grid owns the grid-scoped keyboard actions (one active grid at a time). */
  active?: boolean;
  onEditCommit?: (r: number, c: number, value: Value) => void;
  onAction?: (action: GridAction) => void;
  className?: string;
}

function cellFromEvent(target: EventTarget | null): CellPos | null {
  const el = (target as Element | null)?.closest<HTMLElement>("[data-cell]");
  if (!el) return null;
  return { r: Number(el.dataset.r), c: Number(el.dataset.c) };
}

const MAX_ACTION_CELLS = 20_000;

function selectedCells(s: SelectionState): CellPos[] {
  const seen = new Set<string>();
  const out: CellPos[] = [];
  for (const range of s.ranges) {
    const n = normalize(range);
    for (let r = n.r0; r <= n.r1; r++) {
      for (let c = n.c0; c <= n.c1; c++) {
        const k = `${r}:${c}`;
        if (seen.has(k)) continue;
        seen.add(k);
        out.push({ r, c });
        if (out.length >= MAX_ACTION_CELLS) return out;
      }
    }
  }
  return out;
}

export function DataGrid({
  columns,
  rowCount,
  getRow,
  loading,
  emptyText = "No rows",
  sort = [],
  onSortChange,
  columnState: columnStateProp,
  onColumnStateChange,
  onFocusCell,
  onOpenFk,
  copyContext,
  editable = false,
  active = false,
  onEditCommit,
  onAction,
  className,
}: DataGridProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const [localColumnState, setLocalColumnState] = useState<ColumnState>(EMPTY_COLUMN_STATE);
  const columnState = columnStateProp ?? localColumnState;
  const setColumnState = useCallback(
    (next: ColumnState) => {
      setLocalColumnState(next);
      onColumnStateChange?.(next);
    },
    [onColumnStateChange],
  );
  const cols = useMemo(() => visibleColumns(columns, columnState), [columns, columnState]);
  const widths = useMemo(() => cols.map((c) => columnWidth(c, columnState)), [cols, columnState]);
  const offsets = useMemo(() => {
    const out: number[] = new Array(widths.length + 1);
    out[0] = 0;
    for (let i = 0; i < widths.length; i++) out[i + 1] = out[i]! + widths[i]!;
    return out;
  }, [widths]);
  const totalWidth = offsets[offsets.length - 1] ?? 0;
  const colIndexById = useMemo(() => new Map(cols.map((c, i) => [c.id, i])), [cols]);

  const [selection, setSelection] = useState<SelectionState>(EMPTY_SELECTION);
  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const bounds = useMemo(() => ({ rows: rowCount, cols: cols.length }), [rowCount, cols.length]);
  const dispatch = useCallback(
    (action: SelectionAction) => setSelection((s) => selectionReducer(s, action, bounds)),
    [bounds],
  );
  useEffect(() => {
    dispatch({ type: "clamp" });
  }, [bounds, dispatch]);

  const [editing, setEditing] = useState<{ pos: CellPos; initialText: string } | null>(null);

  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollerRef.current,
    estimateSize: () => ROW_H,
    paddingStart: HEADER_H,
    overscan: 8,
  });
  const colVirtualizer = useVirtualizer({
    horizontal: true,
    count: cols.length,
    getScrollElement: () => scrollerRef.current,
    estimateSize: (i) => widths[i] ?? 120,
    paddingStart: GUTTER_W,
    overscan: 3,
    getItemKey: (i) => cols[i]?.id ?? i,
  });
  useEffect(() => {
    colVirtualizer.measure();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [widths]);

  const focusKey = selection.focus ? `${selection.focus.r}:${selection.focus.c}` : "";
  useEffect(() => {
    const f = selection.focus;
    if (!f) {
      onFocusCell?.(null);
      return;
    }
    rowVirtualizer.scrollToIndex(f.r, { align: "auto" });
    colVirtualizer.scrollToIndex(f.c, { align: "auto" });
    const row = getRow(f.r);
    const column = cols[f.c];
    if (row && column)
      onFocusCell?.({ pos: f, column, value: row.cells[f.c] ?? { t: "null" }, row });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusKey, getRow, cols]);

  // ---- editing -------------------------------------------------------------
  const canEditCell = useCallback(
    (pos: CellPos): boolean => {
      if (!editable) return false;
      const col = cols[pos.c];
      const row = getRow(pos.r);
      return !!col && !!row && col.editable && row.status !== "deleted";
    },
    [editable, cols, getRow],
  );

  const startEdit = useCallback(
    (pos: CellPos, initialText?: string) => {
      if (!canEditCell(pos)) return;
      const row = getRow(pos.r)!;
      setEditing({ pos, initialText: initialText ?? editText(row.cells[pos.c] ?? { t: "null" }) });
    },
    [canEditCell, getRow],
  );

  const finishEdit = (value: Value | null, exit: EditorExit) => {
    const pos = editing?.pos;
    setEditing(null);
    scrollerRef.current?.focus({ preventScroll: true });
    if (!pos) return;
    if (value) onEditCommit?.(pos.r, pos.c, value);
    if (exit === "enter") dispatch({ type: "move", dr: 1, dc: 0, shift: false, jump: false });
    else if (exit === "tab") dispatch({ type: "move", dr: 0, dc: 1, shift: false, jump: false });
    else if (exit === "shiftTab")
      dispatch({ type: "move", dr: 0, dc: -1, shift: false, jump: false });
  };

  // ---- grid-scoped actions (only the active, editable grid registers them) --
  const act = useCallback(
    (make: (s: SelectionState) => GridAction | null) => {
      const a = make(selectionRef.current);
      if (a) onAction?.(a);
    },
    [onAction],
  );
  const enabled = active && editable && !!onAction;
  useAction(
    "grid.set-null",
    enabled ? () => act((s) => ({ type: "setNull", cells: selectedCells(s) })) : null,
  );
  useAction(
    "grid.set-default",
    enabled ? () => act((s) => ({ type: "setDefault", cells: selectedCells(s) })) : null,
  );
  useAction(
    "grid.delete-rows",
    enabled ? () => act((s) => ({ type: "deleteRows", rows: selectedRows(s) })) : null,
  );
  useAction(
    "grid.duplicate-row",
    enabled ? () => act((s) => ({ type: "duplicateRows", rows: selectedRows(s) })) : null,
  );
  useAction("grid.new-row", enabled ? () => act(() => ({ type: "newRow" })) : null);
  useAction("grid.undo", enabled ? () => act(() => ({ type: "undo" })) : null);

  // ---- mouse ---------------------------------------------------------------
  const dragging = useRef(false);
  const resizing = useRef<{ id: string; startX: number; startW: number } | null>(null);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    const target = e.target as HTMLElement;
    if (editing) return;
    const resizeId = target.closest<HTMLElement>("[data-resize]")?.dataset.resize;
    if (resizeId !== undefined) {
      e.preventDefault();
      e.stopPropagation();
      const i = colIndexById.get(resizeId) ?? 0;
      resizing.current = { id: resizeId, startX: e.clientX, startW: widths[i] ?? 120 };
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }
    if (target.closest("[data-fk]")) {
      const cell = cellFromEvent(target);
      const row = cell ? getRow(cell.r) : undefined;
      const column = cell ? cols[cell.c] : undefined;
      if (row && column && onOpenFk) onOpenFk(column, row.cells[cell!.c] ?? { t: "null" });
      return;
    }
    const header = target.closest<HTMLElement>("[data-header]");
    if (header) {
      const id = header.dataset.header!;
      if (id === "__corner") dispatch({ type: "selectAll" });
      else if (e.altKey)
        dispatch({
          type: "selectColumn",
          c: colIndexById.get(id) ?? 0,
          shift: e.shiftKey,
          meta: e.metaKey,
        });
      else if (onSortChange && e.button === 0) onSortChange(nextSort(sort, id, e.shiftKey));
      scrollerRef.current?.focus({ preventScroll: true });
      return;
    }
    const gutter = target.closest<HTMLElement>("[data-gutter]");
    if (gutter) {
      dispatch({
        type: "selectRow",
        r: Number(gutter.dataset.gutter),
        shift: e.shiftKey,
        meta: e.metaKey,
      });
      scrollerRef.current?.focus({ preventScroll: true });
      return;
    }
    const cell = cellFromEvent(target);
    if (!cell) return;
    scrollerRef.current?.focus({ preventScroll: true });
    if (e.button === 2) {
      const inside = selection.ranges.some((rg) => {
        const n = normalize(rg);
        return cell.r >= n.r0 && cell.r <= n.r1 && cell.c >= n.c0 && cell.c <= n.c1;
      });
      if (!inside) dispatch({ type: "set", pos: cell });
      return;
    }
    if (e.button !== 0) return;
    dispatch({ type: "click", pos: cell, shift: e.shiftKey, meta: e.metaKey });
    dragging.current = true;
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (resizing.current) {
      const { id, startX, startW } = resizing.current;
      const w = Math.max(MIN_COL_W, Math.min(MAX_COL_W, startW + (e.clientX - startX)));
      setColumnState({ ...columnState, widths: { ...columnState.widths, [id]: w } });
      return;
    }
    if (!dragging.current) return;
    const cell = cellFromEvent(document.elementFromPoint(e.clientX, e.clientY));
    if (cell) dispatch({ type: "dragTo", pos: cell });
    const scroller = scrollerRef.current;
    if (scroller) {
      const rect = scroller.getBoundingClientRect();
      if (e.clientY > rect.bottom - 24) scroller.scrollTop += 12;
      else if (e.clientY < rect.top + HEADER_H + 24) scroller.scrollTop -= 12;
      if (e.clientX > rect.right - 24) scroller.scrollLeft += 12;
      else if (e.clientX < rect.left + GUTTER_W + 24) scroller.scrollLeft -= 12;
    }
  };

  const endPointer = (e: ReactPointerEvent<HTMLDivElement>) => {
    dragging.current = false;
    resizing.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId))
      e.currentTarget.releasePointerCapture(e.pointerId);
  };

  const onDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    // Pointer capture (set on pointerdown for drag-select) retargets click/dblclick to the scroller,
    // so hit-test the point instead of trusting e.target.
    const cell =
      cellFromEvent(document.elementFromPoint(e.clientX, e.clientY)) ?? selectionRef.current.focus;
    if (cell) startEdit(cell);
  };

  // ---- keyboard ------------------------------------------------------------
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (editing) return;
    const mod = e.metaKey || e.ctrlKey;
    const move = (dr: number, dc: number) => {
      e.preventDefault();
      dispatch({ type: "move", dr, dc, shift: e.shiftKey, jump: mod });
    };
    switch (e.key) {
      case "ArrowUp":
        return move(-1, 0);
      case "ArrowDown":
        return move(1, 0);
      case "ArrowLeft":
        return move(0, -1);
      case "ArrowRight":
        return move(0, 1);
      case "Home":
        e.preventDefault();
        return dispatch({ type: "move", dr: mod ? -1 : 0, dc: -1, shift: e.shiftKey, jump: true });
      case "End":
        e.preventDefault();
        return dispatch({ type: "move", dr: mod ? 1 : 0, dc: 1, shift: e.shiftKey, jump: true });
      case "PageDown":
      case "PageUp": {
        e.preventDefault();
        const page = Math.max(
          1,
          Math.floor(((scrollerRef.current?.clientHeight ?? 400) - HEADER_H) / ROW_H) - 1,
        );
        return dispatch({
          type: "move",
          dr: e.key === "PageDown" ? page : -page,
          dc: 0,
          shift: e.shiftKey,
          jump: false,
        });
      }
      case "Tab":
        e.preventDefault();
        return dispatch({
          type: "move",
          dr: 0,
          dc: e.shiftKey ? -1 : 1,
          shift: false,
          jump: false,
        });
      case "Escape":
        return dispatch({ type: "collapse" });
      case "Enter":
      case "F2":
        if (selection.focus && canEditCell(selection.focus)) {
          e.preventDefault();
          startEdit(selection.focus);
        }
        return;
      case "Backspace":
      case "Delete":
        if (selection.focus && canEditCell(selection.focus) && !mod) {
          e.preventDefault();
          startEdit(selection.focus, "");
        }
        return;
      case "a":
      case "A":
        if (mod) {
          e.preventDefault();
          dispatch({ type: "selectAll" });
        }
        return;
      default:
        if (
          !mod &&
          !e.altKey &&
          e.key.length === 1 &&
          selection.focus &&
          canEditCell(selection.focus)
        ) {
          e.preventDefault();
          startEdit(selection.focus, e.key);
        }
    }
  };

  // ---- copy ----------------------------------------------------------------
  const collectSelected = useCallback((): { columns: GridColumn[]; rows: Value[][] } | null => {
    const rect = selectionBounds(selection);
    if (!rect) return null;
    const subCols = cols.slice(rect.c0, rect.c1 + 1);
    const rows: Value[][] = [];
    for (let r = rect.r0; r <= rect.r1; r++) {
      const row = getRow(r);
      if (!row) continue;
      rows.push(subCols.map((_, i) => row.cells[rect.c0 + i] ?? { t: "null" }));
    }
    return { columns: subCols, rows };
  }, [selection, cols, getRow]);

  const copyAs = useCallback(
    async (format: CopyFormat) => {
      const block = collectSelected();
      if (!block) return;
      await writeClipboard(
        formatCells(format, block.columns, block.rows, { ...copyContext, headers: false }),
      );
      toast.success(
        `Copied ${block.rows.length} row${block.rows.length === 1 ? "" : "s"} as ${format.toUpperCase()}`,
      );
    },
    [collectSelected, copyContext],
  );

  const onCopy = (e: React.ClipboardEvent<HTMLDivElement>) => {
    if (editing) return;
    const block = collectSelected();
    if (!block) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", formatCells("tsv", block.columns, block.rows));
  };

  // ---- render --------------------------------------------------------------
  const virtualRows = rowVirtualizer.getVirtualItems();
  const virtualCols = colVirtualizer.getVirtualItems();
  const totalHeight = rowVirtualizer.getTotalSize();
  const focusedCol = selection.focus ? cols[selection.focus.c] : undefined;
  const focusedRow = selection.focus ? getRow(selection.focus.r) : undefined;
  const focusedValue =
    focusedRow && selection.focus ? focusedRow.cells[selection.focus.c] : undefined;
  const rowsSelected = selectedRows(selection).length;

  const overlay = selection.ranges.map((range, i) => {
    const n = normalize(range);
    const left = GUTTER_W + (offsets[n.c0] ?? 0);
    const width = (offsets[n.c1 + 1] ?? 0) - (offsets[n.c0] ?? 0);
    return (
      <div
        key={i}
        className="bg-brand-soft ring-brand/60 pointer-events-none absolute z-[1] ring-1 ring-inset"
        style={{ left, top: HEADER_H + n.r0 * ROW_H, width, height: (n.r1 - n.r0 + 1) * ROW_H }}
      />
    );
  });

  const editorRect = editing
    ? {
        left: GUTTER_W + (offsets[editing.pos.c] ?? 0),
        top: HEADER_H + editing.pos.r * ROW_H,
        width: widths[editing.pos.c] ?? 120,
        height: ROW_H,
      }
    : null;

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={scrollerRef}
          tabIndex={0}
          data-scope="grid"
          className={cn(
            "bg-surface focus-visible:ring-brand/40 relative h-full w-full overflow-auto outline-none select-none focus-visible:ring-1 focus-visible:ring-inset",
            className,
          )}
          style={{ contain: "strict", overscrollBehavior: "contain" }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPointer}
          onPointerCancel={endPointer}
          onDoubleClick={onDoubleClick}
          onKeyDown={onKeyDown}
          onCopy={onCopy}
        >
          <div className="relative" style={{ width: GUTTER_W + totalWidth, height: totalHeight }}>
            <div
              className="sticky top-0 z-[3]"
              style={{ height: HEADER_H, width: GUTTER_W + totalWidth }}
            >
              <div
                data-header="__corner"
                className="border-border bg-surface-2 sticky left-0 z-[4] float-left h-full border-r border-b"
                style={{ width: GUTTER_W }}
              />
              {virtualCols.map((vc) => {
                const col = cols[vc.index]!;
                const s = sort.find((x) => x.column === col.id);
                const sortIndex = sort.length > 1 && s ? sort.indexOf(s) + 1 : null;
                return (
                  <div
                    key={vc.key}
                    data-header={col.id}
                    title={`${col.name} · ${col.dataType}${col.nullable ? "" : " · not null"}${col.editable ? "" : " · read-only"}`}
                    className="border-border bg-surface-2 text-muted-foreground hover:bg-accent absolute top-0 flex h-full items-center gap-1 border-r border-b px-2 text-xs font-semibold"
                    style={{ left: vc.start, width: vc.size }}
                  >
                    {col.isPk ? (
                      <KeyRound className="text-warning size-3 shrink-0" />
                    ) : (
                      <ValueKindIcon kind={col.kind} />
                    )}
                    <span className="text-foreground/90 truncate">{col.name}</span>
                    {s && (
                      <span className="text-brand ml-auto flex shrink-0 items-center">
                        {s.dir === "asc" ? (
                          <ArrowUp className="size-3" />
                        ) : (
                          <ArrowDown className="size-3" />
                        )}
                        {sortIndex && <span className="text-[9px]">{sortIndex}</span>}
                      </span>
                    )}
                    <div
                      data-resize={col.id}
                      className="hover:bg-brand/50 absolute top-0 right-0 h-full w-1.5 cursor-col-resize"
                    />
                  </div>
                );
              })}
            </div>

            {virtualRows.map((vr) => {
              const row = getRow(vr.index);
              const isFocusRow = selection.focus?.r === vr.index;
              return (
                <div
                  key={vr.key}
                  className={cn(
                    "group/row hover:bg-row-hover absolute left-0",
                    isFocusRow && "bg-row-hover/60",
                  )}
                  style={{ top: vr.start, height: vr.size, width: GUTTER_W + totalWidth }}
                >
                  <div
                    data-gutter={vr.index}
                    className={cn(
                      "border-border bg-surface-2 text-muted-foreground sticky left-0 z-[2] float-left flex h-full items-center justify-end border-r border-b pr-2 font-mono text-[10px] tabular-nums",
                      isFocusRow && "text-foreground",
                      row?.status === "deleted" && "bg-deleted",
                      row?.status === "inserted" && "bg-inserted",
                      row?.status === "updated" && "bg-changed",
                      row?.error && "bg-error/25 text-error",
                    )}
                    style={{ width: GUTTER_W }}
                    title={
                      row?.error ??
                      (row?.status === "inserted"
                        ? "New row"
                        : row?.status === "deleted"
                          ? "Marked for deletion"
                          : undefined)
                    }
                  >
                    {row?.status === "inserted" ? "+" : vr.index + 1}
                  </div>
                  {row
                    ? virtualCols.map((vc) => (
                        <GridCell
                          key={vc.key}
                          r={vr.index}
                          c={vc.index}
                          value={row.cells[vc.index] ?? { t: "null" }}
                          col={cols[vc.index]!}
                          left={vc.start}
                          width={vc.size}
                          rowStatus={row.status}
                          changed={row.changed?.has(vc.index)}
                        />
                      ))
                    : virtualCols.map((vc) => (
                        <div
                          key={vc.key}
                          className="border-gridline absolute top-0 flex h-full items-center border-r border-b px-2"
                          style={{ left: vc.start, width: vc.size }}
                        >
                          <div className="bg-muted h-3 w-2/3 animate-pulse rounded" />
                        </div>
                      ))}
                </div>
              );
            })}

            {overlay}
            {selection.focus && !editing && (
              <div
                className="ring-brand pointer-events-none absolute z-[2] ring-2 ring-inset"
                style={{
                  left: GUTTER_W + (offsets[selection.focus.c] ?? 0),
                  top: HEADER_H + selection.focus.r * ROW_H,
                  width: widths[selection.focus.c] ?? 0,
                  height: ROW_H,
                }}
              />
            )}
            {editing && editorRect && (
              <CellEditor
                key={`${editing.pos.r}:${editing.pos.c}`}
                col={cols[editing.pos.c]!}
                initialText={editing.initialText}
                previous={getRow(editing.pos.r)?.cells[editing.pos.c] ?? { t: "null" }}
                {...editorRect}
                onCommit={(v, exit) => finishEdit(v, exit)}
                onCancel={() => finishEdit(null, "blur")}
              />
            )}
          </div>

          {rowCount === 0 && !loading && (
            <div className="text-muted-foreground pointer-events-none absolute inset-0 flex items-center justify-center pt-7 text-xs">
              {emptyText}
            </div>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-60">
        <ContextMenuItem onSelect={() => void copyAs("tsv")} disabled={!selection.focus}>
          <Copy /> Copy
          <ContextMenuShortcut>⌘C</ContextMenuShortcut>
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger disabled={!selection.focus}>Copy as</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem onSelect={() => void copyAs("json")}>JSON</ContextMenuItem>
            <ContextMenuItem onSelect={() => void copyAs("csv")}>CSV (with header)</ContextMenuItem>
            <ContextMenuItem onSelect={() => void copyAs("insert")}>
              INSERT statements
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => void copyAs("markdown")}>
              Markdown table
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        {focusedCol?.fk && focusedValue && focusedValue.t !== "null" && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => onOpenFk?.(focusedCol, focusedValue)}>
              <ExternalLink /> Open {focusedCol.fk.table} · {focusedCol.fk.column}
              <ContextMenuShortcut>⌘⇧F</ContextMenuShortcut>
            </ContextMenuItem>
          </>
        )}
        {editable && onAction && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={!selection.focus}
              onSelect={() => selection.focus && startEdit(selection.focus)}
            >
              Edit cell
              <ContextMenuShortcut>↩</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!selection.focus}
              onSelect={() => act((s) => ({ type: "setNull", cells: selectedCells(s) }))}
            >
              Set NULL
              <ContextMenuShortcut>⌘⇧N</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!selection.focus}
              onSelect={() => act((s) => ({ type: "setDefault", cells: selectedCells(s) }))}
            >
              Set DEFAULT
              <ContextMenuShortcut>⌘⇧D</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!selection.focus}
              onSelect={() => act((s) => ({ type: "revertCells", cells: selectedCells(s) }))}
            >
              Revert cell{selectedCells(selection).length > 1 ? "s" : ""}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={() => act(() => ({ type: "newRow" }))}>
              New row
              <ContextMenuShortcut>⌘N</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={rowsSelected === 0}
              onSelect={() => act((s) => ({ type: "duplicateRows", rows: selectedRows(s) }))}
            >
              Duplicate row{rowsSelected > 1 ? "s" : ""}
              <ContextMenuShortcut>⌘D</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={rowsSelected === 0}
              onSelect={() => act((s) => ({ type: "revertRows", rows: selectedRows(s) }))}
            >
              Revert row{rowsSelected > 1 ? "s" : ""}
            </ContextMenuItem>
            <ContextMenuItem
              variant="destructive"
              disabled={rowsSelected === 0}
              onSelect={() => act((s) => ({ type: "deleteRows", rows: selectedRows(s) }))}
            >
              Delete row{rowsSelected > 1 ? "s" : ""}
              <ContextMenuShortcut>⌘⌫</ContextMenuShortcut>
            </ContextMenuItem>
          </>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => dispatch({ type: "selectAll" })}>
          Select all
          <ContextMenuShortcut>⌘A</ContextMenuShortcut>
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}
