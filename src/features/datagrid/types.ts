import type { ColumnMeta, ForeignKeyMeta, Value, ValueKind } from "@/lib/ipc";

/**
 * Row height (TablePlus-compact). The ONE place it is defined: virtualization, hit-testing, the
 * selection overlay, the editor and "blank area below the rows" all derive from it.
 */
export const DEFAULT_ROW_H = 22;
/**
 * A live binding: Settings ▸ Appearance ▸ Data table (font, line height, padding) changes it with
 * `setRowHeight`, and the grids re-measure (`subscribeRowHeight`). Read it at use, never cache it.
 */
export let ROW_H = DEFAULT_ROW_H;
/** The column header is a little taller than a row (24px for the default 22px row). */
export const HEADER_EXTRA_H = 2;
/**
 * The column header's height: the row height plus `HEADER_EXTRA_H`, so it scales with the grid's
 * font size, line height and row padding like the rows do. A live binding like `ROW_H`.
 */
export let HEADER_H = DEFAULT_ROW_H + HEADER_EXTRA_H;

const rowHeightListeners = new Set<() => void>();

/** Sets the row height (whole px, 14–64) and the header height with it; true when it changed. */
export function setRowHeight(px: number): boolean {
  const next = Math.min(64, Math.max(14, Math.round(px)));
  if (next === ROW_H) return false;
  ROW_H = next;
  HEADER_H = next + HEADER_EXTRA_H;
  rowHeightListeners.forEach((l) => l());
  return true;
}

export function subscribeRowHeight(listener: () => void): () => void {
  rowHeightListeners.add(listener);
  return () => rowHeightListeners.delete(listener);
}

export const getRowHeight = (): number => ROW_H;
/** Narrowest row-number gutter (enough for three digits). */
export const MIN_GUTTER_W = 32;
/** The gutter without row numbers: a slim strip for the staged / error tint and row selection. */
export const SLIM_GUTTER_W = 8;
export const MIN_COL_W = 48;

/** Row-number gutter width: just wide enough for the largest row number (11px tabular digits). */
export function gutterWidth(rowCount: number): number {
  const digits = String(Math.max(1, Math.floor(rowCount))).length;
  return Math.max(MIN_GUTTER_W, digits * 7 + 14);
}
export const MAX_COL_W = 1200;

export interface FkRef {
  schema: string | null;
  table: string;
  /** The referenced column (of a composite key: the one the column it sits on maps to). */
  column: string;
  /** The constraint's name (`GridColumn.fks` entries). */
  name?: string;
  /** Every column of the key on this table, in key order (`GridColumn.fks` entries). */
  localColumns?: string[];
  /** The referenced columns, parallel to `localColumns`. */
  refColumns?: string[];
}

export interface GridColumn {
  id: string;
  name: string;
  kind: ValueKind;
  dataType: string;
  nullable: boolean;
  isPk: boolean;
  hasDefault: boolean;
  isAutoInc: boolean;
  editable: boolean;
  align: "left" | "right";
  /** The first foreign key that is this column alone (Quick Look's FK viewer, Generate JOIN). */
  fk?: FkRef;
  /**
   * Every foreign key the column is part of, composite keys included: one arrow jumps straight
   * to its target, several make it a menu. Absent without any.
   */
  fks?: FkRef[];
  enumValues?: string[] | null;
}

/**
 * Whether Set Value ▸ EMPTY ('') applies: plain string columns only (enum / set values, JSON, UUIDs
 * and the other typed kinds have no meaningful empty string).
 */
export function acceptsEmpty(col: Pick<GridColumn, "kind">): boolean {
  return col.kind === "text";
}

export type RowStatus = "updated" | "inserted" | "deleted";

export interface ViewRow {
  key: string;
  cells: Value[];
  status?: RowStatus;
  /** Column indexes with a staged edit. */
  changed?: ReadonlySet<number>;
  error?: string;
}

/** A cell position. Whether `c` is a visual or a source column index depends on the API (see each use). */
export interface CellPos {
  r: number;
  c: number;
}

export interface ColumnState {
  widths: Record<string, number>;
  order: string[];
  hidden: string[];
}

export const EMPTY_COLUMN_STATE: ColumnState = { widths: {}, order: [], hidden: [] };

const NUMERIC: ReadonlySet<ValueKind> = new Set(["int", "bigInt", "decimal", "float"]);

export function defaultWidth(kind: ValueKind, name: string): number {
  const byName = Math.min(360, Math.max(80, name.length * 8 + 36));
  switch (kind) {
    case "bool":
      return Math.max(72, byName);
    case "int":
    case "float":
      return Math.max(90, byName);
    case "bigInt":
    case "decimal":
      return Math.max(120, byName);
    case "uuid":
      return 300;
    case "date":
      return Math.max(110, byName);
    case "time":
      return Math.max(100, byName);
    case "timestamp":
    case "timestampTz":
      return Math.max(190, byName);
    case "json":
    case "array":
      return 240;
    case "bytes":
      return Math.max(110, byName);
    case "text":
      return Math.max(180, byName);
    default:
      return Math.max(140, byName);
  }
}

/** The foreign keys of `col` (`fks`; a hand-made column may carry only `fk`). */
export function fksOf(col: Pick<GridColumn, "fk" | "fks">): readonly FkRef[] {
  return col.fks ?? (col.fk ? [col.fk] : []);
}

/** Whether a foreign key spans several columns. */
export function isCompositeFk(fk: FkRef): boolean {
  return (fk.localColumns?.length ?? 1) > 1;
}

/**
 * The column pairs a jump through `fk` filters by: this table's column and the referenced one it
 * must equal. `column` (the clicked one) stands for a key without `localColumns`.
 */
export function fkPairs(fk: FkRef, column: string): Array<{ local: string; ref: string }> {
  const local = fk.localColumns ?? [column];
  const ref = fk.refColumns ?? [fk.column];
  return local.map((l, i) => ({ local: l, ref: ref[i] ?? fk.column }));
}

/** The referenced columns of a foreign key as text: `id`, `a, b` for a composite one. */
export function fkColumnsText(fk: FkRef): string {
  const refs = fk.refColumns && fk.refColumns.length > 0 ? fk.refColumns : [fk.column];
  return refs.join(", ");
}

/** A foreign key's label in a menu: `table (column)`, `table (a, b)` for a composite one. */
export function fkLabel(fk: FkRef): string {
  return `${fk.table} (${fkColumnsText(fk)})`;
}

function toFkRef(f: ForeignKeyMeta, column: string): FkRef {
  const at = Math.max(0, f.columns.indexOf(column));
  return {
    schema: f.refTable.schema,
    table: f.refTable.name,
    column: f.refColumns[at] ?? f.refColumns[0] ?? "",
    name: f.name,
    localColumns: [...f.columns],
    refColumns: [...f.refColumns],
  };
}

export function buildGridColumns(
  columns: ColumnMeta[],
  foreignKeys: ForeignKeyMeta[] = [],
  editable = true,
): GridColumn[] {
  return columns.map((c) => {
    const fk = foreignKeys.find((f) => f.columns.length === 1 && f.columns[0] === c.name);
    const fks = foreignKeys
      .filter((f) => f.columns.includes(c.name))
      .map((f) => toFkRef(f, c.name));
    return {
      id: c.name,
      name: c.name,
      kind: c.kind,
      dataType: c.dataType,
      nullable: c.nullable,
      isPk: c.isPk,
      hasDefault: c.defaultExpr !== null || c.isAutoIncrement,
      isAutoInc: c.isAutoIncrement,
      editable: editable && !c.isGenerated,
      align: NUMERIC.has(c.kind) ? "right" : "left",
      fk: fk
        ? { schema: fk.refTable.schema, table: fk.refTable.name, column: fk.refColumns[0] ?? "" }
        : undefined,
      fks: fks.length > 0 ? fks : undefined,
      enumValues: c.enumValues,
    };
  });
}

/**
 * The visible columns in display order (`cols`) plus, for each one, its index in the source column
 * list (`src[i]` is the position of `cols[i]` in `columns`). Row cells, staged patches and grid
 * actions all use SOURCE indexes; selection and rendering use VISUAL indexes (positions in `cols`).
 */
export interface ColumnLayout {
  cols: GridColumn[];
  src: number[];
}

/** Applies order/hidden to the column list. Unknown ids in `order` are ignored; new columns go last. */
export function layoutColumns(columns: GridColumn[], state: ColumnState): ColumnLayout {
  const indexById = new Map<string, number>();
  columns.forEach((c, i) => {
    if (!indexById.has(c.id)) indexById.set(c.id, i);
  });
  const hidden = new Set(state.hidden);
  const placed = new Set<number>();
  const cols: GridColumn[] = [];
  const src: number[] = [];
  const place = (i: number) => {
    const c = columns[i];
    if (!c || placed.has(i)) return;
    placed.add(i);
    if (hidden.has(c.id)) return;
    cols.push(c);
    src.push(i);
  };
  for (const id of state.order) {
    const i = indexById.get(id);
    if (i !== undefined) place(i);
  }
  for (let i = 0; i < columns.length; i++) place(i);
  return { cols, src };
}

/** Visible columns in display order (see `layoutColumns` for the source-index mapping). */
export function visibleColumns(columns: GridColumn[], state: ColumnState): GridColumn[] {
  return layoutColumns(columns, state).cols;
}

export function columnWidth(col: GridColumn, state: ColumnState): number {
  return state.widths[col.id] ?? defaultWidth(col.kind, col.name);
}
