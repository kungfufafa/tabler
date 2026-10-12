import type { ColumnMeta, ForeignKeyMeta, Value, ValueKind } from "@/lib/ipc";

export const ROW_H = 28;
export const HEADER_H = 28;
export const GUTTER_W = 48;
export const MIN_COL_W = 48;
export const MAX_COL_W = 1200;

export interface FkRef {
  schema: string | null;
  table: string;
  column: string;
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
  fk?: FkRef;
  enumValues?: string[] | null;
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

export function buildGridColumns(
  columns: ColumnMeta[],
  foreignKeys: ForeignKeyMeta[] = [],
  editable = true,
): GridColumn[] {
  return columns.map((c) => {
    const fk = foreignKeys.find((f) => f.columns.length === 1 && f.columns[0] === c.name);
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
