import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowDown,
  ArrowUp,
  ClipboardPaste,
  Copy,
  ExternalLink,
  Eye,
  EyeOff,
  KeyRound,
  MoveHorizontal,
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAction } from "@/features/palette/actions";
import type { SortSpec, Value } from "@/lib/ipc";
import { comboFromEvent, formatCombo, isMac } from "@/lib/keys/combo";
import { shortcutFor } from "@/lib/keys/keymap";
import { cn } from "@/lib/utils";
import { displayValue, editText, parseEditText } from "@/lib/values";
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
import { planPaste } from "./paste";
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
  | { type: "setNull" | "setDefault" | "revertCells"; cells: CellPos[] }
  | { type: "deleteRows" | "duplicateRows" | "revertRows"; rows: number[] }
  | { type: "newRow" }
  | { type: "undo" }
  /** A paste: every cell's typed value, applied as one batch (one undo step). */
  | { type: "setValues"; edits: Array<{ r: number; c: number; value: Value }> };

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
  className?: string;
}

const NULL: Value = { t: "null" };

function cellFromEvent(target: EventTarget | null): CellPos | null {
  const el = (target as Element | null)?.closest?.<HTMLElement>("[data-cell]");
  if (!el) return null;
  return { r: Number(el.dataset.r), c: Number(el.dataset.c) };
}

const MAX_ACTION_CELLS = 20_000;

/** Visual cells covered by the selection (deduplicated, capped). */
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

const TEXT_LIKE: ReadonlySet<GridColumn["kind"]> = new Set(["text", "enum", "set"]);

/** Typed value for pasted text, or null when it does not fit the column. */
function pastedValue(col: GridColumn, text: string, previous: Value): Value | null {
  if (text === "" && col.nullable && !TEXT_LIKE.has(col.kind)) return NULL;
  if (col.kind === "enum" && (col.enumValues?.length ?? 0) > 0 && !col.enumValues!.includes(text))
    return null;
  const parsed = parseEditText(col.kind, text, previous);
  return parsed.ok ? parsed.value : null;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

type MenuTarget = { kind: "cell" } | { kind: "header"; id: string };

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
  onOpenFk,
  copyContext,
  editable = false,
  readOnlyNotice,
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
    typed: boolean;
  } | null>(null);
  const [copyMenu, setCopyMenu] = useState(false);
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
    const sourceIndex = src[f.c];
    if (row && column && sourceIndex !== undefined)
      onFocusCell?.({ pos: f, sourceIndex, column, value: valueAt(row, f.c), row });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusKey, getRow, cols, src]);

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

  const startEdit = useCallback(
    (pos: CellPos, initialText?: string) => {
      if (!canEditCell(pos)) return;
      const row = getRow(pos.r)!;
      setEditing({
        pos,
        source: src[pos.c]!,
        initialText: initialText ?? editText(valueAt(row, pos.c)),
        typed: initialText !== undefined,
      });
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
  /** Selected cells as SOURCE positions (what `GridAction` carries). */
  const sourceCells = useCallback(
    (s: SelectionState): CellPos[] =>
      selectedCells(s).flatMap(({ r, c }) => {
        const source = src[c];
        return source === undefined ? [] : [{ r, c: source }];
      }),
    [src],
  );
  const enabled = active && editable && !!onAction;
  useAction(
    "grid.set-null",
    enabled ? () => act((s) => ({ type: "setNull", cells: sourceCells(s) })) : null,
  );
  useAction(
    "grid.set-default",
    enabled ? () => act((s) => ({ type: "setDefault", cells: sourceCells(s) })) : null,
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

  // ---- copy ----------------------------------------------------------------
  /** The selection's bounding block: visible columns in display order. */
  const collectSelected = useCallback((): { columns: GridColumn[]; rows: Value[][] } | null => {
    const rect = selectionBounds(selectionRef.current);
    if (!rect) return null;
    const subCols = cols.slice(rect.c0, rect.c1 + 1);
    const rows: Value[][] = [];
    for (let r = rect.r0; r <= rect.r1; r++) {
      const row = getRow(r);
      if (!row) continue;
      rows.push(subCols.map((_, i) => valueAt(row, rect.c0 + i)));
    }
    return { columns: subCols, rows };
  }, [cols, getRow, valueAt]);

  const copy = useCallback(async () => {
    const block = collectSelected();
    if (!block) return;
    try {
      await writeClipboard(formatCells("tsv", block.columns, block.rows));
    } catch (e) {
      toast.error("Could not copy to the clipboard", { description: (e as Error).message });
    }
  }, [collectSelected]);

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
      if (!s.focus) {
        toast.message("Select a cell to paste into");
        return;
      }
      const plan = planPaste(block, s.ranges.map(normalize), s.focus, rowCount, cols.length);
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
        if (row) texts.push(displayValue(valueAt(row, vi)).text);
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
    const gutter = target.closest<HTMLElement>("[data-gutter]");
    if (gutter) {
      if (contextClick) setMenuTarget({ kind: "cell" });
      dispatch({
        type: "selectRow",
        r: Number(gutter.dataset.gutter),
        shift: e.shiftKey,
        meta: e.metaKey,
      });
      focusGrid();
      return;
    }
    const cell = cellFromEvent(target);
    if (!cell) return;
    focusGrid();
    if (contextClick) {
      setMenuTarget({ kind: "cell" });
      const inside = selection.ranges.some((rg) => {
        const n = normalize(rg);
        return cell.r >= n.r0 && cell.r <= n.r1 && cell.c >= n.c0 && cell.c <= n.c1;
      });
      if (!inside) dispatch({ type: "set", pos: cell });
      return;
    }
    if (e.button !== 0) return;
    pressedCell.current = cell;
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
    const scroller = scrollerRef.current;
    const press = headerPress.current;
    if (press) {
      if (!press.dragging) {
        if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < DRAG_THRESHOLD) return;
        press.dragging = true;
      }
      if (!scroller) return;
      const rect = scroller.getBoundingClientRect();
      const x = Math.max(rect.left + GUTTER_W + 1, Math.min(e.clientX, rect.right - 1));
      const el = document
        .elementFromPoint(x, rect.top + HEADER_H / 2)
        ?.closest<HTMLElement>("[data-header]");
      const targetId = el?.dataset.header;
      const vi = targetId === undefined ? undefined : colIndexById.get(targetId);
      if (el && targetId !== undefined && vi !== undefined) {
        const r = el.getBoundingClientRect();
        const before = x < r.left + r.width / 2;
        const lineX = GUTTER_W + (before ? offsets[vi]! : offsets[vi + 1]!);
        setReorder((cur) =>
          cur && cur.targetId === targetId && cur.before === before && cur.x === lineX
            ? cur
            : { id: press.id, targetId, before, x: lineX },
        );
      }
      if (e.clientX > rect.right - EDGE) scroller.scrollLeft += 12;
      else if (e.clientX < rect.left + GUTTER_W + EDGE) scroller.scrollLeft -= 12;
      return;
    }
    if (!dragging.current) return;
    const cell = cellFromEvent(document.elementFromPoint(e.clientX, e.clientY));
    if (cell) dispatch({ type: "dragTo", pos: cell });
    if (scroller) {
      const rect = scroller.getBoundingClientRect();
      if (e.clientY > rect.bottom - EDGE) scroller.scrollTop += 12;
      else if (e.clientY < rect.top + HEADER_H + EDGE) scroller.scrollTop -= 12;
      if (e.clientX > rect.right - EDGE) scroller.scrollLeft += 12;
      else if (e.clientX < rect.left + GUTTER_W + EDGE) scroller.scrollLeft -= 12;
    }
  };

  const endPointer = (e: ReactPointerEvent<HTMLDivElement>) => {
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
    if (cell && !noteBytes(cell)) startEdit(cell);
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
    if (editing || copyMenu || !e.currentTarget.contains(e.target as Node)) return;
    if (e.key === "Escape" && headerPress.current?.dragging) {
      e.preventDefault();
      cancelHeaderDrag();
      return;
    }
    // Clipboard keys for grids that do not own the global actions (e.g. query results).
    switch (comboFromEvent(e.nativeEvent)) {
      case "Mod+C":
        e.preventDefault();
        void copy();
        return;
      case "Mod+Shift+C":
        e.preventDefault();
        openCopyMenu();
        return;
      case "Mod+V":
        e.preventDefault();
        void paste();
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
  // Cheap counts (select-all on a big result must not walk every cell on each render).
  const firstRange = selection.ranges.length === 1 ? normalize(selection.ranges[0]!) : null;
  const rowsSelected = firstRange
    ? firstRange.r1 - firstRange.r0 + 1
    : selection.ranges.length === 0
      ? 0
      : selectedRows(selection).length;
  const multiCell =
    selection.ranges.length > 1 ||
    (!!firstRange && (firstRange.r1 > firstRange.r0 || firstRange.c1 > firstRange.c0));

  const headerId = menuTarget.kind === "header" ? menuTarget.id : null;
  const headerVi = headerId === null ? undefined : colIndexById.get(headerId);
  // Looked up among ALL columns: after "Hide Column" the menu still shows header items while closing.
  const headerCol = headerId === null ? undefined : columns.find((c) => c.id === headerId);
  const headerSorted = headerId !== null && sort.some((s) => s.column === headerId);
  const hiddenN = hiddenCount(columns, columnState);

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
  const editingCol = editing ? columns[editing.source] : undefined;
  const editingRow = editing ? getRow(editing.pos.r) : undefined;

  const copyCombo = formatCombo(shortcutFor("grid.copy") ?? "Mod+C");
  const copyAsCombo = formatCombo(shortcutFor("grid.copy-as") ?? "Mod+Shift+C");
  const pasteCombo = formatCombo(shortcutFor("grid.paste") ?? "Mod+V");

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
          onContextMenu={onContextMenu}
          onKeyDown={onKeyDown}
          onCopy={onCopy}
          onPaste={onPaste}
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
                    className={cn(
                      "border-border bg-surface-2 text-muted-foreground hover:bg-accent absolute top-0 flex h-full items-center gap-1 border-r border-b px-2 text-xs font-semibold",
                      reorder?.id === col.id && "opacity-50",
                    )}
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
            {editing && editorRect && editingCol && (
              <CellEditor
                key={`${editing.pos.r}:${editing.source}`}
                col={editingCol}
                initialText={editing.initialText}
                typed={editing.typed}
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
                      left: GUTTER_W + (offsets[selection.focus.c] ?? 0),
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
                    Copy {plural(rowsSelected, "row")} as
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
          </div>

          {rowCount === 0 && !loading && (
            <div className="text-muted-foreground pointer-events-none absolute inset-0 flex items-center justify-center pt-7 text-xs">
              {emptyText}
            </div>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="w-60">
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
            <ContextMenuItem
              onSelect={() =>
                void writeClipboard(headerCol.name).then(
                  () => toast.success("Column name copied"),
                  (e: unknown) =>
                    toast.error("Could not copy to the clipboard", {
                      description: (e as Error).message,
                    }),
                )
              }
            >
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
              <ContextMenuShortcut>{isMac ? "⌥Click" : "Alt+Click"}</ContextMenuShortcut>
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
            <ContextMenuItem onSelect={() => void copy()} disabled={!selection.focus}>
              <Copy /> Copy
              <ContextMenuShortcut>{copyCombo}</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuSub>
              <ContextMenuSubTrigger inset disabled={!selection.focus}>
                Copy as
                <ContextMenuShortcut>{copyAsCombo}</ContextMenuShortcut>
              </ContextMenuSubTrigger>
              <ContextMenuSubContent className="w-48">
                {COPY_CHOICES.map((choice) => (
                  <ContextMenuItem key={choice.label} onSelect={() => void copyAs(choice)}>
                    {choice.label}
                  </ContextMenuItem>
                ))}
              </ContextMenuSubContent>
            </ContextMenuSub>
            {canPaste && (
              <ContextMenuItem disabled={!selection.focus} onSelect={() => void paste()}>
                <ClipboardPaste /> Paste
                <ContextMenuShortcut>{pasteCombo}</ContextMenuShortcut>
              </ContextMenuItem>
            )}
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
                  inset
                  disabled={!selection.focus}
                  onSelect={() => {
                    const f = selection.focus;
                    if (f && !noteBytes(f)) startEdit(f);
                  }}
                >
                  Edit cell
                  <ContextMenuShortcut>↩</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  inset
                  disabled={!selection.focus}
                  onSelect={() => act((s) => ({ type: "setNull", cells: sourceCells(s) }))}
                >
                  Set NULL
                  <ContextMenuShortcut>⌘⇧N</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  inset
                  disabled={!selection.focus}
                  onSelect={() => act((s) => ({ type: "setDefault", cells: sourceCells(s) }))}
                >
                  Set DEFAULT
                  <ContextMenuShortcut>⌘⇧D</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  inset
                  disabled={!selection.focus}
                  onSelect={() => act((s) => ({ type: "revertCells", cells: sourceCells(s) }))}
                >
                  Revert cell{multiCell ? "s" : ""}
                </ContextMenuItem>
                <ContextMenuSeparator />
                <ContextMenuItem inset onSelect={() => act(() => ({ type: "newRow" }))}>
                  New row
                  <ContextMenuShortcut>⌘N</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  inset
                  disabled={rowsSelected === 0}
                  onSelect={() => act((s) => ({ type: "duplicateRows", rows: selectedRows(s) }))}
                >
                  Duplicate row{rowsSelected > 1 ? "s" : ""}
                  <ContextMenuShortcut>⌘D</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  inset
                  disabled={rowsSelected === 0}
                  onSelect={() => act((s) => ({ type: "revertRows", rows: selectedRows(s) }))}
                >
                  Revert row{rowsSelected > 1 ? "s" : ""}
                </ContextMenuItem>
                <ContextMenuItem
                  inset
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
            <ContextMenuItem inset onSelect={() => dispatch({ type: "selectAll" })}>
              Select all
              <ContextMenuShortcut>⌘A</ContextMenuShortcut>
            </ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  );
}
