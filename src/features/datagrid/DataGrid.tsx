import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  ArrowUp,
  ClipboardPaste,
  Copy,
  ExternalLink,
  Eye,
  EyeOff,
  Filter,
  KeyRound,
  MoveHorizontal,
  PanelRight,
  Plus,
  RotateCcw,
  Trash2,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { toast } from "sonner";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAction, type ActionId } from "@/features/palette/actions";
import type { FilterOp, SortSpec, Value } from "@/lib/ipc";
import { comboFromEvent, formatCombo, isMac } from "@/lib/keys/combo";
import { findShortcut, shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { cellDisplay, displayValue, editText, valueText } from "@/lib/values";
import { toggleRowDetail, useInspectorStore } from "@/stores/inspector";
import { useUiStore } from "@/stores/ui";
import { CellEditor, type EditorExit } from "./CellEditor";
import {
  formatCells,
  parseTsv,
  readClipboardText,
  writeClipboard,
  type CopyContext,
  type CopyFormat,
} from "./clipboard";
import {
  fitWidth,
  hiddenCount,
  hideColumn,
  idAfter,
  moveColumn,
  resetOrder,
  showAll,
} from "./columnState";
import { GridCell } from "./GridCell";
import { holdsWholeRows, pastedRows, pastedValue, planPaste, type PastedRow } from "./paste";
import {
  EMPTY_SELECTION,
  isCellSelected,
  isRowSelected,
  mergeSpans,
  selectedColumns,
  selectedRowCount,
  selectedRows,
  selectionReducer,
  type SelectionAction,
  type SelectionState,
} from "./selection";
import { nextSort } from "./sort";
import {
  acceptsEmpty,
  columnWidth,
  EMPTY_COLUMN_STATE,
  gutterWidth,
  HEADER_H,
  layoutColumns,
  MAX_COL_W,
  MIN_COL_W,
  ROW_H,
  type CellPos,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "./types";

/**
 * Row/table actions the grid asks its owner to perform. Column indexes here (`cells[].c`,
 * `edits[].c`) are SOURCE indexes — positions in the `columns` prop, i.e. in `ViewRow.cells` —
 * never visual positions (columns can be hidden or reordered). `r` is the row index.
 */
export type GridAction =
  /** Set Value ▸ NULL / EMPTY ('' in text columns) / DEFAULT. */
  | { type: "setNull" | "setEmpty" | "setDefault"; cells: CellPos[] }
  /** Delete, Duplicate and Discard Changes on the selected rows. */
  | { type: "deleteRows" | "duplicateRows" | "revertRows"; rows: number[] }
  | { type: "newRow" }
  /** ⌘V of whole rows: new rows with these cells set (`c` SOURCE index), the rest at their defaults. */
  | { type: "insertRows"; rows: PastedRow[] }
  | { type: "undo" }
  /** A paste: every cell's typed value, applied as one batch (one undo step). */
  | { type: "setValues"; edits: Array<{ r: number; c: number; value: Value }> };

/** A "Quick Filter" picked from the cell menu (TablePlus): the owner replaces its filters with it. */
export interface QuickFilter {
  column: GridColumn;
  op: Extract<FilterOp, "eq" | "ne" | "in" | "notIn" | "isNull" | "isNotNull">;
  /** The focused value for = / ≠, the distinct selected values for IN / NOT IN, none for IS NULL. */
  values: Value[];
}

export interface FocusedCell {
  /** VISUAL position (row, visible column). */
  pos: CellPos;
  /** Index of the focused column in the `columns` prop / `row.cells`. */
  sourceIndex: number;
  column: GridColumn;
  value: Value;
  row: ViewRow;
}

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
  onFocusCell?: (cell: FocusedCell | null) => void;
  /** Called with the number of selected rows whenever it changes (the bottom bar's "N of M"). */
  onSelectionChange?: (rows: number) => void;
  onOpenFk?: (column: GridColumn, value: Value) => void;
  /** Dialect/table used by "Copy as INSERT". */
  copyContext?: CopyContext;
  /** Enables inline editing and row actions. */
  editable?: boolean;
  /** Shown when pasting into a grid that cannot be edited (e.g. "This table is read-only"). */
  readOnlyNotice?: string;
  /** When true the grid owns the grid-scoped keyboard actions (one active grid at a time). */
  active?: boolean;
  /** `c` is a SOURCE column index (see `GridAction`). */
  onEditCommit?: (r: number, c: number, value: Value) => void;
  onAction?: (action: GridAction) => void;
  /** Enables the cell menu's "Quick Filter" submenu. */
  onQuickFilter?: (filter: QuickFilter) => void;
  /** Enables the header menu's "Filter with Column" (a new filter condition on that column). */
  onFilterColumn?: (column: GridColumn) => void;
  className?: string;
}

const NULL: Value = { t: "null" };

function cellFromEvent(target: EventTarget | null): CellPos | null {
  const el = (target as Element | null)?.closest?.<HTMLElement>("[data-cell]");
  if (!el) return null;
  return { r: Number(el.dataset.r), c: Number(el.dataset.c) };
}

/** Most cells a Set Value on whole columns touches. */
const MAX_ACTION_CELLS = 20_000;

interface CopyChoice {
  label: string;
  /** Name used in the "Copied N rows as …" toast. */
  short: string;
  format: CopyFormat;
  headers?: boolean;
}

const COPY_CHOICES: CopyChoice[] = [
  { label: "TSV with header", short: "TSV", format: "tsv", headers: true },
  { label: "CSV", short: "CSV", format: "csv" },
  { label: "JSON", short: "JSON", format: "json" },
  { label: "INSERT statements", short: "INSERT statements", format: "insert" },
  { label: "Markdown table", short: "Markdown", format: "markdown" },
];

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Most distinct values an IN / NOT IN quick filter collects from the selection. */
const MAX_IN_VALUES = 1000;

/** Display form of an action's primary shortcut ("" when it has none). */
function keysFor(id: ActionId): string {
  const keys = shortcutFor(id);
  return keys ? formatCombo(keys) : "";
}

function MenuKeys({ id }: { id: ActionId }) {
  const keys = keysFor(id);
  return keys ? <ContextMenuShortcut>{keys}</ContextMenuShortcut> : null;
}

/** Short single-line form of a value for menu labels. */
function shortValue(v: Value): string {
  const text = displayValue(v).text;
  return text.length > 24 ? `${text.slice(0, 23)}…` : text;
}

/** "Quick Filter" items for the focused value; the IN list is collected when the submenu opens. */
function QuickFilterItems({
  column,
  value,
  collect,
  onPick,
}: {
  column: GridColumn;
  value: Value;
  collect: () => Value[];
  onPick: (filter: QuickFilter) => void;
}) {
  const [values] = useState(collect);
  const name = column.name;
  const pick = (op: QuickFilter["op"], vals: Value[] = []) => onPick({ column, op, values: vals });
  return (
    <>
      {value.t === "null" ? (
        <>
          <ContextMenuItem onSelect={() => pick("isNull")}>{name} IS NULL</ContextMenuItem>
          <ContextMenuItem onSelect={() => pick("isNotNull")}>{name} IS NOT NULL</ContextMenuItem>
        </>
      ) : (
        <>
          <ContextMenuItem onSelect={() => pick("eq", [value])}>
            {name} = {shortValue(value)}
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => pick("ne", [value])}>
            {name} ≠ {shortValue(value)}
          </ContextMenuItem>
        </>
      )}
      {values.length > 1 && (
        <>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => pick("in", values)}>
            {name} IN ({plural(values.length, "selected value")})
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => pick("notIn", values)}>
            {name} NOT IN ({plural(values.length, "selected value")})
          </ContextMenuItem>
        </>
      )}
    </>
  );
}

/** "Discard Changes", shown only when a selected row has staged changes (checked as the menu opens). */
function DiscardChangesItem({
  hasChanges,
  onSelect,
}: {
  hasChanges: () => boolean;
  onSelect: () => void;
}) {
  const [show] = useState(hasChanges);
  if (!show) return null;
  return (
    <ContextMenuItem onSelect={onSelect}>
      <RotateCcw /> Discard Changes
    </ContextMenuItem>
  );
}

/** Context menu ▸ Open Row Details: the right sidebar on the focused row (kept open if it is). */
function openRowDetail(): void {
  if (useUiStore.getState().inspectorOpen) useInspectorStore.getState().setMode("row");
  else toggleRowDetail();
}

type MenuTarget = { kind: "cell" } | { kind: "header"; id: string };

/** Where the ⌥-click Set Value menu opens, in content coordinates. */
interface QuickMenu {
  x: number;
  y: number;
}

interface HeaderPress {
  id: string;
  pointerId: number;
  x: number;
  y: number;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  dragging: boolean;
}

interface Reorder {
  id: string;
  targetId: string;
  before: boolean;
  /** Drop indicator position in content coordinates. */
  x: number;
}

/** Zebra stripe on every other row: a faint foreground tint, so it works in both themes. */
const ZEBRA = "bg-foreground/[2.5%]";

const layer = (color: string) => `linear-gradient(${color}, ${color})`;

/**
 * Tints layered over a row number's OPAQUE background (it is sticky: cells scroll under it, so the
 * translucent dark-theme staged colours must not replace its background): the selection, then
 * the row's error or staged state.
 */
function gutterTint(row: ViewRow | undefined, selected: boolean): string | undefined {
  const layers: string[] = [];
  if (selected) layers.push(layer("var(--grid-selection)"));
  if (row?.error) layers.push(layer("color-mix(in oklab, var(--error) 25%, transparent)"));
  else if (row?.status === "deleted") layers.push(layer("var(--deleted)"));
  else if (row?.status === "inserted") layers.push(layer("var(--inserted)"));
  else if (row?.status === "updated") layers.push(layer("var(--changed)"));
  return layers.length > 0 ? layers.join(", ") : undefined;
}

const DRAG_THRESHOLD = 4;
const EDGE = 24;

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
  onSelectionChange,
  onOpenFk,
  copyContext,
  editable = false,
  readOnlyNotice,
  active = false,
  onEditCommit,
  onAction,
  onQuickFilter,
  onFilterColumn,
  className,
}: DataGridProps) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [localColumnState, setLocalColumnState] = useState<ColumnState>(EMPTY_COLUMN_STATE);
  const columnState = columnStateProp ?? localColumnState;
  const setColumnState = useCallback(
    (next: ColumnState) => {
      setLocalColumnState(next);
      onColumnStateChange?.(next);
    },
    [onColumnStateChange],
  );
  // `cols` are the visible columns in display order; `src[c]` maps a visual column to its index in
  // `columns` / `row.cells`. Selection and virtualization use visual indexes; data access uses src.
  const { cols, src } = useMemo(() => layoutColumns(columns, columnState), [columns, columnState]);
  const widths = useMemo(() => cols.map((c) => columnWidth(c, columnState)), [cols, columnState]);
  const offsets = useMemo(() => {
    const out: number[] = new Array(widths.length + 1);
    out[0] = 0;
    for (let i = 0; i < widths.length; i++) out[i + 1] = out[i]! + widths[i]!;
    return out;
  }, [widths]);
  const totalWidth = offsets[offsets.length - 1] ?? 0;
  /** Row-number gutter: as narrow as the largest row number allows. */
  const gutterW = gutterWidth(rowCount);
  const colIndexById = useMemo(() => new Map(cols.map((c, i) => [c.id, i])), [cols]);
  const valueAt = useCallback(
    (row: ViewRow, c: number): Value => row.cells[src[c] ?? -1] ?? NULL,
    [src],
  );

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

  const [editing, setEditing] = useState<{
    pos: CellPos;
    source: number;
    initialText: string;
  } | null>(null);
  const [copyMenu, setCopyMenu] = useState(false);
  const [quickMenu, setQuickMenu] = useState<QuickMenu | null>(null);
  const [menuTarget, setMenuTarget] = useState<MenuTarget>({ kind: "cell" });
  const [reorder, setReorder] = useState<Reorder | null>(null);

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
    paddingStart: gutterW,
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
    const sourceIndex = src[f.c];
    if (row && column && sourceIndex !== undefined)
      onFocusCell?.({ pos: f, sourceIndex, column, value: valueAt(row, f.c), row });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusKey, getRow, cols, src]);

  const selectedCount = selectedRowCount(selection);
  useEffect(() => {
    onSelectionChange?.(selectedCount);
  }, [selectedCount, onSelectionChange]);

  const focusGrid = useCallback(() => scrollerRef.current?.focus({ preventScroll: true }), []);

  // ---- editing -------------------------------------------------------------
  const canEditCell = useCallback(
    (pos: CellPos): boolean => {
      if (!editable) return false;
      const col = cols[pos.c];
      const row = getRow(pos.r);
      return !!col && !!row && col.editable && col.kind !== "bytes" && row.status !== "deleted";
    },
    [editable, cols, getRow],
  );
  /** Explains why Enter/double-click does nothing on a blob cell. */
  const noteBytes = useCallback(
    (pos: CellPos): boolean => {
      const col = cols[pos.c];
      if (!editable || !col?.editable || col.kind !== "bytes") return false;
      toast.message("Binary values can't be edited inline");
      return true;
    },
    [editable, cols],
  );

  /** Opens the editor on a cell with its current value (typing never starts an edit). */
  const startEdit = useCallback(
    (pos: CellPos) => {
      if (!canEditCell(pos)) return;
      const row = getRow(pos.r)!;
      setEditing({ pos, source: src[pos.c]!, initialText: editText(valueAt(row, pos.c)) });
    },
    [canEditCell, getRow, src, valueAt],
  );

  const finishEdit = (value: Value | null, exit: EditorExit) => {
    const current = editing;
    setEditing(null);
    focusGrid();
    // Dialog/portal editors drop focus as they unmount (Radix settles it in a timeout); take it
    // back unless the user has moved on to something else.
    const refocus = () => {
      const a = document.activeElement;
      if (!a || a === document.body) focusGrid();
    };
    window.setTimeout(refocus, 0);
    window.setTimeout(refocus, 50);
    if (!current) return;
    if (value) onEditCommit?.(current.pos.r, current.source, value);
    if (exit === "enter") dispatch({ type: "move", dr: 1, dc: 0, shift: false, jump: false });
    else if (exit === "tab") dispatch({ type: "move", dr: 0, dc: 1, shift: false, jump: false });
    else if (exit === "shiftTab")
      dispatch({ type: "move", dr: 0, dc: -1, shift: false, jump: false });
  };

  // ---- grid-scoped actions (only the active grid registers them) -----------
  const act = useCallback(
    (make: (s: SelectionState) => GridAction | null) => {
      const a = make(selectionRef.current);
      if (a) onAction?.(a);
    },
    [onAction],
  );
  /**
   * The cells Set Value works on, as SOURCE positions (what `GridAction` carries): the focused
   * column of every selected row (just the focused cell when no row is selected), or every cell of
   * the selected columns.
   */
  const sourceCells = useCallback(
    (s: SelectionState): CellPos[] => {
      const out: CellPos[] = [];
      if (s.cols.length > 0) {
        const sources = selectedColumns(s).flatMap((c) => src[c] ?? []);
        for (let r = 0; r < rowCount && out.length < MAX_ACTION_CELLS; r++)
          for (const c of sources) out.push({ r, c });
        return out.slice(0, MAX_ACTION_CELLS);
      }
      const f = s.focus;
      const source = f ? src[f.c] : undefined;
      if (!f || source === undefined) return out;
      const rows = selectedRows(s);
      return (rows.length > 0 ? rows : [f.r]).map((r) => ({ r, c: source }));
    },
    [src, rowCount],
  );
  /**
   * Rows asked for (a new row, pasted rows): once they show up (as the grid's last rows) they are
   * selected and the first one focused, so new rows on a long page are scrolled into view.
   * `before` is the row count when they were asked for.
   */
  const pendingRows = useRef<{ before: number; count: number } | null>(null);
  const newRow = useCallback(() => {
    if (!onAction) return;
    pendingRows.current = { before: rowCount, count: 1 };
    onAction({ type: "newRow" });
  }, [onAction, rowCount]);
  useEffect(() => {
    const pending = pendingRows.current;
    if (pending === null || rowCount === pending.before) return;
    pendingRows.current = null;
    if (rowCount === pending.before + pending.count)
      dispatch({
        type: "selectRows",
        r0: pending.before,
        r1: rowCount - 1,
        focus: { r: pending.before, c: 0 },
      });
  }, [rowCount, dispatch]);

  const enabled = active && editable && !!onAction;
  const setNull = useCallback(
    () => act((s) => ({ type: "setNull", cells: sourceCells(s) })),
    [act, sourceCells],
  );
  const setEmpty = useCallback(
    () => act((s) => ({ type: "setEmpty", cells: sourceCells(s) })),
    [act, sourceCells],
  );
  const setDefault = useCallback(
    () => act((s) => ({ type: "setDefault", cells: sourceCells(s) })),
    [act, sourceCells],
  );
  useAction("grid.set-null", enabled ? setNull : null);
  useAction("grid.set-empty", enabled ? setEmpty : null);
  useAction("grid.set-default", enabled ? setDefault : null);
  /** A row action on the selected rows (nothing when none are, e.g. while columns are selected). */
  const rowAction = useCallback(
    (type: "deleteRows" | "duplicateRows" | "revertRows") =>
      act((s) => {
        const rows = selectedRows(s);
        return rows.length > 0 ? { type, rows } : null;
      }),
    [act],
  );
  const deleteRows = useCallback(() => rowAction("deleteRows"), [rowAction]);
  const duplicateRows = useCallback(() => rowAction("duplicateRows"), [rowAction]);
  useAction("grid.delete-rows", enabled ? deleteRows : null);
  useAction("grid.duplicate-row", enabled ? duplicateRows : null);
  useAction("grid.new-row", enabled ? newRow : null);
  useAction("grid.undo", enabled ? () => act(() => ({ type: "undo" })) : null);
  // Read-only grids (query results) have a row detail too.
  useAction("grid.toggle-detail", active ? toggleRowDetail : null);

  // ---- copy ----------------------------------------------------------------
  /**
   * What ⌘C / Copy Rows As copy: the selected rows over the visible columns in display order, or
   * every row of the selected columns.
   */
  const collectSelected = useCallback((): { columns: GridColumn[]; rows: Value[][] } | null => {
    const s = selectionRef.current;
    const byColumn = s.cols.length > 0;
    const vcs = byColumn ? selectedColumns(s) : cols.map((_, c) => c);
    const indexes = byColumn ? Array.from({ length: rowCount }, (_, r) => r) : selectedRows(s);
    if (indexes.length === 0 || vcs.length === 0) return null;
    const rows: Value[][] = [];
    for (const r of indexes) {
      const row = getRow(r);
      if (row) rows.push(vcs.map((c) => valueAt(row, c)));
    }
    return { columns: vcs.map((c) => cols[c]!), rows };
  }, [cols, getRow, valueAt, rowCount]);

  const copy = useCallback(async () => {
    const block = collectSelected();
    if (!block) return;
    try {
      await writeClipboard(formatCells("tsv", block.columns, block.rows));
    } catch (e) {
      toast.error("Could not copy to the clipboard", { description: (e as Error).message });
    }
  }, [collectSelected]);

  const copyText = useCallback(async (text: string, done?: string) => {
    try {
      await writeClipboard(text);
    } catch (e) {
      toast.error("Could not copy to the clipboard", { description: (e as Error).message });
      return;
    }
    if (done) toast.success(done);
  }, []);
  /** Context menu ▸ Copy Cell Value: the focused cell's text (NULL as empty). */
  const copyCellValue = useCallback(() => {
    const f = selectionRef.current.focus;
    const row = f ? getRow(f.r) : undefined;
    if (f && row) void copyText(valueText(valueAt(row, f.c)) ?? "");
  }, [getRow, valueAt, copyText]);
  const copyColumnName = useCallback(
    (col: GridColumn) => void copyText(col.name, "Column name copied"),
    [copyText],
  );

  const copyAs = useCallback(
    async (choice: CopyChoice) => {
      const block = collectSelected();
      if (!block) return;
      try {
        await writeClipboard(
          formatCells(choice.format, block.columns, block.rows, {
            ...copyContext,
            headers: choice.headers ?? false,
          }),
        );
      } catch (e) {
        toast.error("Could not copy to the clipboard", { description: (e as Error).message });
        return;
      }
      toast.success(`Copied ${plural(block.rows.length, "row")} as ${choice.short}`);
    },
    [collectSelected, copyContext],
  );

  const openCopyMenu = useCallback(() => {
    if (selectionRef.current.focus) setCopyMenu(true);
  }, []);
  // Highlight the first format once the menu has mounted (and focused itself) so ↩ copies at once.
  const copyMenuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!copyMenu) return;
    const id = window.setTimeout(
      () =>
        copyMenuRef.current
          ?.querySelector<HTMLElement>('[role="menuitem"]')
          ?.focus({ preventScroll: true }),
      0,
    );
    return () => window.clearTimeout(id);
  }, [copyMenu]);

  // ---- paste ---------------------------------------------------------------
  const canPaste = editable && !!onAction;
  const pasteText = useCallback(
    (text: string) => {
      if (!canPaste) {
        if (readOnlyNotice) toast.message(readOnlyNotice);
        return;
      }
      // "" parses to one empty field: a copied NULL / empty cell pastes as NULL (or '' for text).
      const block = parseTsv(text);
      if (block.length === 0) return;
      const s = selectionRef.current;
      // TablePlus: copied rows (one field per visible column) paste as new rows.
      if (holdsWholeRows(block, cols.length)) {
        const { rows, skipped } = pastedRows(block, cols, src);
        pendingRows.current = { before: rowCount, count: rows.length };
        onAction?.({ type: "insertRows", rows });
        let msg = `Pasted ${plural(rows.length, "row")}`;
        if (skipped > 0) msg += ` · ${plural(skipped, "value")} skipped`;
        toast.success(msg);
        return;
      }
      if (!s.focus) {
        toast.message("Select a cell to paste into");
        return;
      }
      // A value goes into the focused cell; a block starts in its column, on the first row of the
      // selected rows around it. With whole columns selected, a value fills them and a block starts
      // at their top.
      const columnRects = mergeSpans(s.cols).map(([c0, c1]) => ({
        r0: 0,
        r1: rowCount - 1,
        c0,
        c1,
      }));
      const f = s.focus;
      const value = block.length === 1 && block[0]!.length === 1;
      const run = mergeSpans(s.rows).find(([r0, r1]) => f.r >= r0 && f.r <= r1);
      const anchor = { r: value ? f.r : (run?.[0] ?? f.r), c: f.c };
      const plan = planPaste(block, columnRects, anchor, rowCount, cols.length);
      const edits: Array<{ r: number; c: number; value: Value }> = [];
      let skipped = 0;
      for (const t of plan.targets) {
        const col = cols[t.c];
        const row = getRow(t.r);
        const source = src[t.c];
        if (
          !col ||
          !row ||
          source === undefined ||
          !col.editable ||
          col.kind === "bytes" ||
          row.status === "deleted"
        ) {
          skipped++;
          continue;
        }
        const value = pastedValue(col, t.text, row.cells[source] ?? NULL);
        if (!value) {
          skipped++;
          continue;
        }
        edits.push({ r: t.r, c: source, value });
      }
      if (edits.length > 0) onAction?.({ type: "setValues", edits });
      let msg = `Pasted ${plural(edits.length, "cell")}`;
      if (skipped > 0) msg += ` · ${skipped} skipped`;
      if (plan.clippedRows > 0) msg += ` · ${plural(plan.clippedRows, "row")} didn't fit`;
      if (plan.clippedCols > 0) msg += ` · ${plural(plan.clippedCols, "column")} didn't fit`;
      if (edits.length > 0) toast.success(msg);
      else toast.message(msg);
    },
    [canPaste, readOnlyNotice, rowCount, cols, getRow, src, onAction],
  );
  const paste = useCallback(async () => {
    if (!canPaste) {
      if (readOnlyNotice) toast.message(readOnlyNotice);
      return;
    }
    let text: string;
    try {
      text = await readClipboardText();
    } catch (e) {
      toast.error("Could not read the clipboard", { description: (e as Error).message });
      return;
    }
    pasteText(text);
  }, [canPaste, readOnlyNotice, pasteText]);

  useAction("grid.copy", active ? () => void copy() : null);
  useAction("grid.copy-as", active ? openCopyMenu : null);
  useAction("grid.paste", enabled ? () => void paste() : null);

  // ---- column header -------------------------------------------------------
  const fitColumn = useCallback(
    (id: string) => {
      const vi = colIndexById.get(id);
      const col = vi === undefined ? undefined : cols[vi];
      if (vi === undefined || !col) return;
      const texts: string[] = [];
      for (const vr of rowVirtualizer.getVirtualItems()) {
        const row = getRow(vr.index);
        if (row) texts.push(cellDisplay(valueAt(row, vi)).text);
      }
      const w = fitWidth(col, texts) + (col.fk ? 16 : 0);
      setColumnState({
        ...columnState,
        widths: { ...columnState.widths, [id]: Math.min(MAX_COL_W, w) },
      });
    },
    [colIndexById, cols, rowVirtualizer, getRow, valueAt, setColumnState, columnState],
  );

  const commitReorder = (r: Reorder) => {
    const allIds = columns.map((c) => c.id);
    const beforeId = r.before ? r.targetId : idAfter(allIds, columnState, r.targetId);
    const next = moveColumn(allIds, columnState, r.id, beforeId);
    if (next !== columnState) setColumnState(next);
  };

  // ---- mouse ---------------------------------------------------------------
  const dragging = useRef(false);
  const resizing = useRef<{ id: string; startX: number; startW: number } | null>(null);
  const headerPress = useRef<HeaderPress | null>(null);
  /** Cell under the last primary-button press (null when it landed on a gutter, header or blank). */
  const pressedCell = useRef<CellPos | null>(null);
  /** An ⌥-press on a cell: its Set Value menu opens on release (after the press moved the focus). */
  const quickPress = useRef<QuickMenu | null>(null);
  /**
   * Runs once the context menu has closed instead of handing the focus back to the grid (for items
   * that move the focus elsewhere: the menu's focus trap would pull it back while it animates out).
   */
  const afterMenuClose = useRef<(() => void) | null>(null);

  const cancelHeaderDrag = () => {
    const press = headerPress.current;
    headerPress.current = null;
    setReorder(null);
    const el = scrollerRef.current;
    if (press && el?.hasPointerCapture(press.pointerId)) el.releasePointerCapture(press.pointerId);
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    // Events from portaled children (editors, menus) bubble here through React; ignore them.
    if (!e.currentTarget.contains(e.target as Node)) return;
    const target = e.target as HTMLElement;
    if (editing) return;
    const contextClick = e.button === 2 || (e.button === 0 && e.ctrlKey && isMac);
    if (e.button === 0 && !contextClick) pressedCell.current = null;
    quickPress.current = null;
    const resizeId = target.closest<HTMLElement>("[data-resize]")?.dataset.resize;
    if (resizeId !== undefined && !contextClick) {
      e.preventDefault();
      e.stopPropagation();
      const i = colIndexById.get(resizeId) ?? 0;
      resizing.current = { id: resizeId, startX: e.clientX, startW: widths[i] ?? 120 };
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }
    if (target.closest("[data-fk]") && !contextClick) {
      const cell = cellFromEvent(target);
      const row = cell ? getRow(cell.r) : undefined;
      const column = cell ? cols[cell.c] : undefined;
      if (cell && row && column && onOpenFk) onOpenFk(column, valueAt(row, cell.c));
      return;
    }
    const header = target.closest<HTMLElement>("[data-header]");
    if (header) {
      const id = header.dataset.header!;
      focusGrid();
      if (contextClick) {
        setMenuTarget(id === "__corner" ? { kind: "cell" } : { kind: "header", id });
        return;
      }
      if (id === "__corner") {
        dispatch({ type: "selectAll" });
        return;
      }
      if (e.button !== 0) return;
      // Sorting/selecting happens on pointerup when the pointer barely moved; a drag reorders.
      headerPress.current = {
        id,
        pointerId: e.pointerId,
        x: e.clientX,
        y: e.clientY,
        shift: e.shiftKey,
        alt: e.altKey,
        meta: e.metaKey,
        dragging: false,
      };
      e.currentTarget.setPointerCapture(e.pointerId);
      return;
    }
    // ⌘-click (Ctrl-click off macOS) toggles a row in and out of the selection.
    const toggle = isMac ? e.metaKey : e.ctrlKey;
    const gutter = target.closest<HTMLElement>("[data-gutter]");
    if (gutter) {
      const r = Number(gutter.dataset.gutter);
      focusGrid();
      if (contextClick) {
        setMenuTarget({ kind: "cell" });
        if (!isRowSelected(selection, r))
          dispatch({ type: "selectRow", r, shift: false, meta: false });
        return;
      }
      if (e.button !== 0) return;
      dispatch({ type: "selectRow", r, shift: e.shiftKey, meta: toggle });
      // Dragging down the row numbers selects a range of rows too.
      if (!toggle) {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
      }
      return;
    }
    const cell = cellFromEvent(target);
    if (!cell) return;
    focusGrid();
    if (contextClick) {
      // A right-click inside the selection keeps it (the menu works on every selected row) and
      // focuses the clicked cell (Copy Cell Value, Quick Filter); outside, it selects that row.
      setMenuTarget({ kind: "cell" });
      dispatch({ type: isCellSelected(selection, cell.r, cell.c) ? "focus" : "set", pos: cell });
      return;
    }
    if (e.button !== 0) return;
    if (e.altKey && editable && onAction) {
      // TablePlus: ⌥-click a cell for its Set Value menu (NULL / EMPTY / DEFAULT). A press inside
      // the selection keeps it, so the menu applies to the clicked column of every selected row.
      dispatch({ type: isCellSelected(selection, cell.r, cell.c) ? "focus" : "set", pos: cell });
      const box = contentRef.current?.getBoundingClientRect();
      quickPress.current = { x: e.clientX - (box?.left ?? 0), y: e.clientY - (box?.top ?? 0) };
      return;
    }
    pressedCell.current = cell;
    dispatch({ type: "click", pos: cell, shift: e.shiftKey, meta: toggle });
    // A drag from the press selects a range of rows (not after a ⌘-click toggle).
    if (!toggle) {
      dragging.current = true;
      e.currentTarget.setPointerCapture(e.pointerId);
    }
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (resizing.current) {
      const { id, startX, startW } = resizing.current;
      const w = Math.max(MIN_COL_W, Math.min(MAX_COL_W, startW + (e.clientX - startX)));
      setColumnState({ ...columnState, widths: { ...columnState.widths, [id]: w } });
      return;
    }
    const scroller = scrollerRef.current;
    const press = headerPress.current;
    if (press) {
      if (!press.dragging) {
        if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_THRESHOLD) return;
        press.dragging = true;
      }
      if (!scroller) return;
      const rect = scroller.getBoundingClientRect();
      const x = Math.max(rect.left + gutterW + 1, Math.min(e.clientX, rect.right - 1));
      const el = document
        .elementFromPoint(x, rect.top + HEADER_H / 2)
        ?.closest<HTMLElement>("[data-header]");
      const targetId = el?.dataset.header;
      const vi = targetId === undefined ? undefined : colIndexById.get(targetId);
      if (el && targetId !== undefined && vi !== undefined) {
        const r = el.getBoundingClientRect();
        const before = x < r.left + r.width / 2;
        const lineX = gutterW + (before ? offsets[vi]! : offsets[vi + 1]!);
        setReorder((cur) =>
          cur && cur.targetId === targetId && cur.before === before && cur.x === lineX
            ? cur
            : { id: press.id, targetId, before, x: lineX },
        );
      }
      if (e.clientX > rect.right - EDGE) scroller.scrollLeft += 12;
      else if (e.clientX < rect.left + gutterW + EDGE) scroller.scrollLeft -= 12;
      return;
    }
    if (!dragging.current) return;
    const hit = document.elementFromPoint(e.clientX, e.clientY);
    const cell = cellFromEvent(hit);
    const gutterRow = hit?.closest<HTMLElement>("[data-gutter]")?.dataset.gutter;
    if (cell) dispatch({ type: "dragTo", pos: cell });
    else if (gutterRow !== undefined)
      dispatch({
        type: "dragTo",
        pos: { r: Number(gutterRow), c: selectionRef.current.focus?.c ?? 0 },
      });
    if (scroller) {
      const rect = scroller.getBoundingClientRect();
      if (e.clientY > rect.bottom - EDGE) scroller.scrollTop += 12;
      else if (e.clientY < rect.top + HEADER_H + EDGE) scroller.scrollTop -= 12;
      if (e.clientX > rect.right - EDGE) scroller.scrollLeft += 12;
      else if (e.clientX < rect.left + gutterW + EDGE) scroller.scrollLeft -= 12;
    }
  };

  const endPointer = (e: ReactPointerEvent<HTMLDivElement>) => {
    const quick = quickPress.current;
    quickPress.current = null;
    if (quick && e.type === "pointerup") setQuickMenu(quick);
    const press = headerPress.current;
    headerPress.current = null;
    if (press && e.type === "pointerup") {
      if (press.dragging) {
        if (reorder) commitReorder(reorder);
      } else if (press.alt) {
        const c = colIndexById.get(press.id);
        if (c !== undefined)
          dispatch({ type: "selectColumn", c, shift: press.shift, meta: press.meta });
      } else if (onSortChange) {
        onSortChange(nextSort(sort, press.id, press.shift));
      }
    }
    setReorder(null);
    dragging.current = false;
    resizing.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId))
      e.currentTarget.releasePointerCapture(e.pointerId);
  };

  const onDoubleClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (editing || !e.currentTarget.contains(e.target as Node)) return;
    // Pointer capture (set on pointerdown for drag-select) retargets click/dblclick to the scroller,
    // so hit-test the point instead of trusting e.target.
    const hit = document.elementFromPoint(e.clientX, e.clientY);
    const resizeId = hit?.closest<HTMLElement>("[data-resize]")?.dataset.resize;
    if (resizeId !== undefined) {
      fitColumn(resizeId);
      return;
    }
    if (hit?.closest("[data-header]") || hit?.closest("[data-gutter]")) return;
    // Only a double-click ON a cell edits: a row number or the blank area below / beside the rows
    // must not open the editor on whatever cell happens to be focused. When the point no longer
    // resolves to a cell (the content moved under the pointer), use the cell both presses hit.
    const cell = cellFromEvent(hit) ?? pressedCell.current;
    if (cell) {
      if (!noteBytes(cell)) startEdit(cell);
      return;
    }
    // TablePlus: double-clicking the blank area below the last row inserts a row.
    const scroller = scrollerRef.current;
    if (!scroller || !editable || !onAction || loading) return;
    const y = e.clientY - scroller.getBoundingClientRect().top + scroller.scrollTop;
    if (y > HEADER_H + rowCount * ROW_H) newRow();
  };

  const onContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
    // Right-clicks inside portaled editors/menus must not open the grid menu.
    if (!e.currentTarget.contains(e.target as Node)) {
      e.preventDefault();
      return;
    }
    const header = (e.target as Element).closest?.<HTMLElement>("[data-header]");
    const id = header?.dataset.header;
    setMenuTarget(id && id !== "__corner" ? { kind: "header", id } : { kind: "cell" });
  };

  // ---- keyboard ------------------------------------------------------------
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (editing || copyMenu || quickMenu || !e.currentTarget.contains(e.target as Node)) return;
    if (e.key === "Escape" && headerPress.current?.dragging) {
      e.preventDefault();
      cancelHeaderDrag();
      return;
    }
    // Grid keys for grids that do not own the global actions (e.g. query results); an active grid
    // registered them, so the keyboard provider ran them before this handler.
    switch (findShortcut(comboFromEvent(e.nativeEvent), "grid")?.id) {
      case "grid.copy":
        e.preventDefault();
        void copy();
        return;
      case "grid.copy-as":
        e.preventDefault();
        openCopyMenu();
        return;
      case "grid.paste":
        e.preventDefault();
        void paste();
        return;
      case "grid.toggle-detail":
        // Space shows the row detail; it never starts an edit with a space.
        e.preventDefault();
        toggleRowDetail();
        return;
    }
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
        // ⇥ / ⇧⇥ move the focused cell within its row.
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
        if (!selection.focus) return;
        if (canEditCell(selection.focus)) {
          e.preventDefault();
          startEdit(selection.focus);
        } else if (noteBytes(selection.focus)) e.preventDefault();
        return;
      case "Backspace":
      case "Delete":
        // TablePlus: ⌫ / ⌦ (and ⌘⌫) stage the selected rows for deletion; they never edit a cell.
        // (⇧⌘⌫ is Discard all changes: not a delete even when there is nothing to discard.)
        if (e.altKey || e.shiftKey) return;
        e.preventDefault();
        if (editable && onAction) deleteRows();
        else if (readOnlyNotice) toast.message(readOnlyNotice);
        return;
      case "a":
      case "A":
        if (mod) {
          e.preventDefault();
          dispatch({ type: "selectAll" });
        }
        return;
      // Typing does not edit (TablePlus): ↩, F2 or a double-click do, so a stray key changes nothing.
    }
  };

  // Native Edit → Copy / Paste (menu bar) arrive as DOM clipboard events.
  const onCopy = (e: React.ClipboardEvent<HTMLDivElement>) => {
    if (editing || !e.currentTarget.contains(e.target as Node)) return;
    const block = collectSelected();
    if (!block) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", formatCells("tsv", block.columns, block.rows));
  };
  const onPaste = (e: React.ClipboardEvent<HTMLDivElement>) => {
    if (editing || !e.currentTarget.contains(e.target as Node)) return;
    e.preventDefault();
    // No text on the clipboard at all (nothing copied, or only an image): nothing to paste. An
    // empty text item is different — it is a copied NULL / empty cell, pasted as one empty field.
    const types = e.clipboardData.types as ArrayLike<string> | undefined;
    if (types && !Array.from(types).includes("text/plain")) return;
    pasteText(e.clipboardData.getData("text/plain"));
  };

  // ---- render --------------------------------------------------------------
  const virtualRows = rowVirtualizer.getVirtualItems();
  const virtualCols = colVirtualizer.getVirtualItems();
  const totalHeight = rowVirtualizer.getTotalSize();
  const focusedCol = selection.focus ? cols[selection.focus.c] : undefined;
  const focusedRow = selection.focus ? getRow(selection.focus.r) : undefined;
  const focusedValue =
    focusedRow && selection.focus ? valueAt(focusedRow, selection.focus.c) : undefined;
  const byColumn = selection.cols.length > 0;
  /** Rows Copy / Copy Rows As work on (every row while whole columns are selected). */
  const copyRows = byColumn ? rowCount : selectedCount;

  const headerId = menuTarget.kind === "header" ? menuTarget.id : null;
  const headerVi = headerId === null ? undefined : colIndexById.get(headerId);
  // Looked up among ALL columns: after "Hide Column" the menu still shows header items while closing.
  const headerCol = headerId === null ? undefined : columns.find((c) => c.id === headerId);
  const headerSorted = headerId !== null && sort.some((s) => s.column === headerId);
  const hiddenN = hiddenCount(columns, columnState);

  // Set Value items that would change nothing for the target column(s) are disabled.
  const settable = { null: false, empty: false, default: false };
  if (editable) {
    const targets = byColumn
      ? selectedColumns(selection)
      : selection.focus
        ? [selection.focus.c]
        : [];
    for (const c of targets) {
      const col = cols[c];
      if (!col?.editable) continue;
      settable.null ||= col.nullable;
      settable.empty ||= acceptsEmpty(col);
      settable.default ||= col.hasDefault;
    }
  }
  const canQuickFilter =
    !!onQuickFilter &&
    !!focusedCol &&
    !!focusedValue &&
    focusedCol.kind !== "bytes" &&
    focusedValue.t !== "default";
  /** Distinct non-NULL values of the focused column in the selected rows (for IN / NOT IN). */
  const collectInValues = (): Value[] => {
    const s = selectionRef.current;
    const f = s.focus;
    if (!f) return [];
    const seen = new Set<string>();
    const out: Value[] = [];
    const rows =
      s.cols.length > 0 ? Array.from({ length: rowCount }, (_, r) => r) : selectedRows(s);
    for (const r of rows) {
      const row = getRow(r);
      const v = row ? valueAt(row, f.c) : NULL;
      const text = valueText(v);
      if (text === null || seen.has(text)) continue;
      seen.add(text);
      out.push(v);
      if (out.length >= MAX_IN_VALUES) break;
    }
    return out;
  };

  /** Whether a selected row has staged changes (Discard Changes), checked when the menu opens. */
  const selectionHasChanges = (): boolean =>
    selectedRows(selectionRef.current).some((r) => !!getRow(r)?.status);

  // Selected rows get a full-row accent tint (row number included), selected columns a full-height
  // one. Drawn above the cells, so staged colours show through.
  const rowSelected = (r: number) => !byColumn && isRowSelected(selection, r);
  const overlay = byColumn
    ? mergeSpans(selection.cols).map(([c0, c1]) => (
        <div
          key={`c${c0}`}
          data-selection="columns"
          className="pointer-events-none absolute z-[1] bg-(--grid-selection)"
          style={{
            left: gutterW + (offsets[c0] ?? 0),
            top: HEADER_H,
            width: (offsets[c1 + 1] ?? 0) - (offsets[c0] ?? 0),
            height: rowCount * ROW_H,
          }}
        />
      ))
    : mergeSpans(selection.rows).map(([r0, r1]) => (
        <div
          key={`r${r0}`}
          data-selection="rows"
          className="pointer-events-none absolute z-[1] bg-(--grid-selection)"
          style={{
            left: gutterW,
            top: HEADER_H + r0 * ROW_H,
            width: totalWidth,
            height: (r1 - r0 + 1) * ROW_H,
          }}
        />
      ));

  const editorRect = editing
    ? {
        left: gutterW + (offsets[editing.pos.c] ?? 0),
        top: HEADER_H + editing.pos.r * ROW_H,
        width: widths[editing.pos.c] ?? 120,
        height: ROW_H,
      }
    : null;
  const editingCol = editing ? columns[editing.source] : undefined;
  const editingRow = editing ? getRow(editing.pos.r) : undefined;

  const altClick = isMac ? "⌥Click" : "Alt+Click";
  /** Row and value actions (Set Value, Insert / Duplicate / Delete, Discard Changes). */
  const canAct = editable && !!onAction;
  const openFkItem = focusedCol?.fk && focusedValue && focusedValue.t !== "null" && (
    <ContextMenuItem onSelect={() => onOpenFk?.(focusedCol, focusedValue)}>
      <ExternalLink /> Open {focusedCol.fk.table} · {focusedCol.fk.column}
      <MenuKeys id="grid.open-fk" />
    </ContextMenuItem>
  );

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={scrollerRef}
          tabIndex={0}
          data-scope="grid"
          className={cn(
            "bg-surface focus-visible:ring-brand/40 relative h-full w-full overflow-auto outline-none select-none focus-visible:ring-1 focus-visible:ring-inset",
            // Selected rows (macOS table style): the accent while the grid has the focus, grey
            // otherwise.
            "[--grid-selection:color-mix(in_oklab,var(--foreground)_8%,transparent)] focus-within:[--grid-selection:color-mix(in_oklab,var(--brand)_26%,transparent)] dark:focus-within:[--grid-selection:color-mix(in_oklab,var(--brand)_38%,transparent)]",
            className,
          )}
          style={{ contain: "strict", overscrollBehavior: "contain" }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endPointer}
          onPointerCancel={endPointer}
          onDoubleClick={onDoubleClick}
          onContextMenu={onContextMenu}
          onKeyDown={onKeyDown}
          onCopy={onCopy}
          onPaste={onPaste}
        >
          <div
            ref={contentRef}
            className="relative"
            style={{ width: gutterW + totalWidth, height: totalHeight }}
          >
            <div
              className="sticky top-0 z-[3]"
              style={{ height: HEADER_H, width: gutterW + totalWidth }}
            >
              <div
                data-header="__corner"
                className="border-border bg-surface-2 sticky left-0 z-[4] float-left h-full border-r border-b"
                style={{ width: gutterW }}
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
                    className={cn(
                      // Name centred (TablePlus); a sort badge reserves the same room on both
                      // sides so the name stays centred and never runs under it.
                      "border-border bg-surface-2 text-foreground hover:bg-accent absolute top-0 flex h-full items-center justify-center border-r border-b text-[12px] font-semibold",
                      s ? (sortIndex ? "px-7" : "px-5") : "px-2",
                      reorder?.id === col.id && "opacity-50",
                    )}
                    style={{ left: vc.start, width: vc.size }}
                  >
                    <span className="flex min-w-0 items-center gap-1">
                      {col.isPk && (
                        <KeyRound
                          data-pk=""
                          aria-label="Primary key"
                          className="text-warning size-3 shrink-0"
                        />
                      )}
                      <span className="truncate">{col.name}</span>
                    </span>
                    {s && (
                      <span
                        data-sort={s.dir}
                        className="text-brand absolute right-2 flex items-center"
                      >
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
                      title="Drag to resize · double-click to fit"
                      className="hover:bg-brand/50 absolute top-0 right-0 h-full w-1.5 cursor-col-resize"
                    />
                  </div>
                );
              })}
              {reorder && (
                <div
                  aria-hidden
                  className="bg-brand pointer-events-none absolute top-0 z-[5] w-0.5"
                  style={{ left: reorder.x - 1, height: HEADER_H }}
                />
              )}
            </div>

            {virtualRows.map((vr) => {
              const row = getRow(vr.index);
              const isFocusRow = selection.focus?.r === vr.index;
              const selected = rowSelected(vr.index);
              return (
                <div
                  key={vr.key}
                  data-row={vr.index}
                  data-selected={selected ? "" : undefined}
                  className={cn(
                    "group/row hover:bg-row-hover absolute left-0",
                    // Subtle zebra striping; staged cell colours and the selection paint over it (a
                    // selected row shows the accent tint alone).
                    !selected && vr.index % 2 === 1 && ZEBRA,
                  )}
                  style={{ top: vr.start, height: vr.size, width: gutterW + totalWidth }}
                >
                  <div
                    data-gutter={vr.index}
                    className={cn(
                      "border-border bg-surface-2 text-muted-foreground sticky left-0 z-[2] float-left flex h-full items-center justify-end border-r border-b pr-1.5 text-[11px] tabular-nums",
                      (isFocusRow || selected) && "text-foreground",
                      row?.error && "text-error",
                    )}
                    style={{ width: gutterW, backgroundImage: gutterTint(row, selected) }}
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
                    ? virtualCols.map((vc) => {
                        const source = src[vc.index] ?? -1;
                        return (
                          <GridCell
                            key={vc.key}
                            r={vr.index}
                            c={vc.index}
                            value={row.cells[source] ?? NULL}
                            col={cols[vc.index]!}
                            left={vc.start}
                            width={vc.size}
                            rowStatus={row.status}
                            changed={row.changed?.has(source)}
                          />
                        );
                      })
                    : virtualCols.map((vc) => (
                        <div
                          key={vc.key}
                          className="border-gridline absolute top-0 flex h-full items-center border-r border-b px-2"
                          style={{ left: vc.start, width: vc.size }}
                        >
                          <div className="bg-muted h-2.5 w-2/3 animate-pulse rounded" />
                        </div>
                      ))}
                </div>
              );
            })}

            {overlay}
            {selection.focus && !editing && (
              <div
                data-focus-ring=""
                className="ring-brand pointer-events-none absolute z-[1] ring-1 ring-inset"
                style={{
                  left: gutterW + (offsets[selection.focus.c] ?? 0),
                  top: HEADER_H + selection.focus.r * ROW_H,
                  width: widths[selection.focus.c] ?? 0,
                  height: ROW_H,
                }}
              />
            )}
            {editing && editorRect && editingCol && (
              <CellEditor
                key={`${editing.pos.r}:${editing.source}`}
                col={editingCol}
                initialText={editing.initialText}
                previous={editingRow?.cells[editing.source] ?? NULL}
                {...editorRect}
                onCommit={(v, exit) => finishEdit(v, exit)}
                onCancel={() => finishEdit(null, "blur")}
              />
            )}
            {copyMenu && selection.focus && (
              <DropdownMenu open onOpenChange={(open) => !open && setCopyMenu(false)}>
                <DropdownMenuTrigger asChild>
                  <div
                    aria-hidden
                    className="pointer-events-none absolute size-0"
                    style={{
                      left: gutterW + (offsets[selection.focus.c] ?? 0),
                      top: HEADER_H + (selection.focus.r + 1) * ROW_H,
                    }}
                  />
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="start"
                  sideOffset={2}
                  className="w-52"
                  ref={copyMenuRef}
                  onCloseAutoFocus={(e) => {
                    e.preventDefault();
                    focusGrid();
                  }}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    const n = Number(e.key);
                    const choice = Number.isInteger(n) ? COPY_CHOICES[n - 1] : undefined;
                    if (choice && !e.metaKey && !e.ctrlKey && !e.altKey) {
                      e.preventDefault();
                      setCopyMenu(false);
                      void copyAs(choice);
                    }
                  }}
                >
                  <DropdownMenuLabel className="text-muted-foreground py-1 text-[11px] font-medium">
                    Copy {plural(copyRows, "row")} as
                  </DropdownMenuLabel>
                  {COPY_CHOICES.map((choice, i) => (
                    <DropdownMenuItem
                      key={choice.label}
                      className="text-[12.5px]"
                      onSelect={() => void copyAs(choice)}
                    >
                      {choice.label}
                      <DropdownMenuShortcut>{i + 1}</DropdownMenuShortcut>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
            {quickMenu && selection.focus && (
              <DropdownMenu open onOpenChange={(open) => !open && setQuickMenu(null)}>
                <DropdownMenuTrigger asChild>
                  <div
                    aria-hidden
                    className="pointer-events-none absolute size-0"
                    style={{ left: quickMenu.x, top: quickMenu.y }}
                  />
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="start"
                  sideOffset={2}
                  className="w-44"
                  onCloseAutoFocus={(e) => {
                    e.preventDefault();
                    focusGrid();
                  }}
                >
                  <DropdownMenuLabel className="text-muted-foreground py-1 text-[11px] font-medium">
                    Set Value
                  </DropdownMenuLabel>
                  <DropdownMenuItem disabled={!settable.null} onSelect={setNull}>
                    NULL
                    <DropdownMenuShortcut>{keysFor("grid.set-null")}</DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={!settable.empty} onSelect={setEmpty}>
                    EMPTY
                    <DropdownMenuShortcut>{keysFor("grid.set-empty")}</DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuItem disabled={!settable.default} onSelect={setDefault}>
                    DEFAULT
                    <DropdownMenuShortcut>{keysFor("grid.set-default")}</DropdownMenuShortcut>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </div>

          {rowCount === 0 && !loading && (
            <div
              className="text-muted-foreground pointer-events-none absolute inset-0 flex items-center justify-center text-xs"
              style={{ paddingTop: HEADER_H }}
            >
              {emptyText}
            </div>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent
        className="w-60"
        onCloseAutoFocus={(e) => {
          const run = afterMenuClose.current;
          afterMenuClose.current = null;
          if (!run) return;
          e.preventDefault();
          run();
        }}
      >
        {headerId !== null && headerCol ? (
          <>
            {onSortChange && (
              <>
                <ContextMenuItem
                  onSelect={() => onSortChange([{ column: headerCol.id, dir: "asc", nulls: null }])}
                >
                  <ArrowUp /> Sort Ascending
                </ContextMenuItem>
                <ContextMenuItem
                  onSelect={() =>
                    onSortChange([{ column: headerCol.id, dir: "desc", nulls: null }])
                  }
                >
                  <ArrowDown /> Sort Descending
                </ContextMenuItem>
                {headerSorted && (
                  <ContextMenuItem
                    inset
                    onSelect={() => onSortChange(sort.filter((s) => s.column !== headerCol.id))}
                  >
                    Remove from Sort
                  </ContextMenuItem>
                )}
                <ContextMenuSeparator />
              </>
            )}
            {onFilterColumn && (
              <>
                <ContextMenuItem
                  onSelect={() => {
                    afterMenuClose.current = () => onFilterColumn(headerCol);
                  }}
                >
                  <Filter /> Filter with Column
                </ContextMenuItem>
                <ContextMenuSeparator />
              </>
            )}
            <ContextMenuItem onSelect={() => copyColumnName(headerCol)}>
              <Copy /> Copy Column Name
            </ContextMenuItem>
            <ContextMenuItem
              inset
              disabled={headerVi === undefined}
              onSelect={() =>
                headerVi !== undefined &&
                dispatch({ type: "selectColumn", c: headerVi, shift: false, meta: false })
              }
            >
              Select Column
              <ContextMenuShortcut>{altClick}</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={cols.length <= 1 || headerVi === undefined}
              onSelect={() => setColumnState(hideColumn(columnState, headerCol.id))}
            >
              <EyeOff /> Hide Column
            </ContextMenuItem>
            <ContextMenuItem
              disabled={hiddenN === 0}
              onSelect={() => setColumnState(showAll(columnState))}
            >
              <Eye /> Show All Columns{hiddenN > 0 ? ` (${hiddenN} hidden)` : ""}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => fitColumn(headerCol.id)}>
              <MoveHorizontal /> Size to Fit
            </ContextMenuItem>
            {columnState.order.length > 0 && (
              <ContextMenuItem inset onSelect={() => setColumnState(resetOrder(columnState))}>
                Reset Column Order
              </ContextMenuItem>
            )}
          </>
        ) : (
          <>
            <ContextMenuItem disabled={!selection.focus} onSelect={openRowDetail}>
              <PanelRight /> Open Row Details
              <MenuKeys id="grid.toggle-detail" />
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem disabled={copyRows === 0} onSelect={() => void copy()}>
              <Copy /> Copy
              <MenuKeys id="grid.copy" />
            </ContextMenuItem>
            <ContextMenuSub>
              <ContextMenuSubTrigger inset disabled={copyRows === 0}>
                Copy Rows As
                <MenuKeys id="grid.copy-as" />
              </ContextMenuSubTrigger>
              <ContextMenuSubContent className="w-48">
                {COPY_CHOICES.map((choice) => (
                  <ContextMenuItem key={choice.label} onSelect={() => void copyAs(choice)}>
                    {choice.label}
                  </ContextMenuItem>
                ))}
              </ContextMenuSubContent>
            </ContextMenuSub>
            <ContextMenuItem inset disabled={!focusedValue} onSelect={copyCellValue}>
              Copy Cell Value
            </ContextMenuItem>
            <ContextMenuItem
              inset
              disabled={!focusedCol}
              onSelect={() => focusedCol && copyColumnName(focusedCol)}
            >
              Copy Column Name
            </ContextMenuItem>
            {canPaste && (
              <ContextMenuItem onSelect={() => void paste()}>
                <ClipboardPaste /> Paste
                <MenuKeys id="grid.paste" />
              </ContextMenuItem>
            )}
            {(canAct || canQuickFilter || openFkItem) && <ContextMenuSeparator />}
            {canAct && (
              <ContextMenuSub>
                <ContextMenuSubTrigger inset disabled={!selection.focus}>
                  Set Value
                  <ContextMenuShortcut>{altClick}</ContextMenuShortcut>
                </ContextMenuSubTrigger>
                <ContextMenuSubContent className="w-44">
                  <ContextMenuItem disabled={!settable.null} onSelect={setNull}>
                    NULL
                    <MenuKeys id="grid.set-null" />
                  </ContextMenuItem>
                  <ContextMenuItem disabled={!settable.empty} onSelect={setEmpty}>
                    EMPTY
                    <MenuKeys id="grid.set-empty" />
                  </ContextMenuItem>
                  <ContextMenuItem disabled={!settable.default} onSelect={setDefault}>
                    DEFAULT
                    <MenuKeys id="grid.set-default" />
                  </ContextMenuItem>
                </ContextMenuSubContent>
              </ContextMenuSub>
            )}
            {canQuickFilter && (
              <ContextMenuSub>
                <ContextMenuSubTrigger>
                  <Filter /> Quick Filter
                </ContextMenuSubTrigger>
                <ContextMenuSubContent className="max-w-80 min-w-48">
                  <QuickFilterItems
                    column={focusedCol}
                    value={focusedValue}
                    collect={collectInValues}
                    onPick={onQuickFilter}
                  />
                </ContextMenuSubContent>
              </ContextMenuSub>
            )}
            {openFkItem}
            {canAct && (
              <>
                <ContextMenuSeparator />
                <ContextMenuItem onSelect={newRow}>
                  <Plus /> Insert Row
                  <MenuKeys id="grid.new-row" />
                </ContextMenuItem>
                <ContextMenuItem inset disabled={selectedCount === 0} onSelect={duplicateRows}>
                  Duplicate
                  <MenuKeys id="grid.duplicate-row" />
                </ContextMenuItem>
                <ContextMenuItem
                  variant="destructive"
                  disabled={selectedCount === 0}
                  onSelect={deleteRows}
                >
                  <Trash2 /> Delete
                  <ContextMenuShortcut>{formatCombo("Backspace")}</ContextMenuShortcut>
                </ContextMenuItem>
                <DiscardChangesItem
                  hasChanges={selectionHasChanges}
                  onSelect={() => rowAction("revertRows")}
                />
              </>
            )}
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
