import type { Value } from "@/lib/ipc";
import { parseEditText } from "@/lib/values";
import type { Rect } from "./selection";
import type { CellPos, GridColumn } from "./types";

export interface PasteTarget {
  r: number;
  /** VISUAL column index (position among the visible columns). */
  c: number;
  text: string;
}

export interface PastePlan {
  targets: PasteTarget[];
  /** Block rows that fell below the last row. */
  clippedRows: number;
  /** Block columns that fell past the last visible column. */
  clippedCols: number;
}

const EMPTY: PastePlan = { targets: [], clippedRows: 0, clippedCols: 0 };

/**
 * Where each pasted field goes. A single value pasted over a multi-cell selection fills every selected
 * cell; otherwise the block is anchored at the top-left of the first range (or the focus) and clipped
 * to `rowCount` × `colCount`.
 */
export function planPaste(
  block: readonly (readonly string[])[],
  ranges: readonly Rect[],
  focus: CellPos | null,
  rowCount: number,
  colCount: number,
): PastePlan {
  if (block.length === 0 || rowCount <= 0 || colCount <= 0) return EMPTY;
  const single = block.length === 1 && block[0]!.length === 1 ? block[0]![0]! : null;

  if (single !== null) {
    const seen = new Set<string>();
    const targets: PasteTarget[] = [];
    for (const rg of ranges) {
      for (let r = Math.max(0, rg.r0); r <= Math.min(rg.r1, rowCount - 1); r++) {
        for (let c = Math.max(0, rg.c0); c <= Math.min(rg.c1, colCount - 1); c++) {
          const k = `${r}:${c}`;
          if (seen.has(k)) continue;
          seen.add(k);
          targets.push({ r, c, text: single });
        }
      }
    }
    if (targets.length > 1) return { targets, clippedRows: 0, clippedCols: 0 };
  }

  const first = ranges[0];
  const anchor = first ? { r: first.r0, c: first.c0 } : focus;
  if (!anchor) return EMPTY;
  const targets: PasteTarget[] = [];
  let clippedRows = 0;
  let widest = 0;
  block.forEach((fields, i) => {
    const r = anchor.r + i;
    if (r >= rowCount) {
      clippedRows++;
      return;
    }
    widest = Math.max(widest, fields.length);
    fields.forEach((text, j) => {
      const c = anchor.c + j;
      if (c < colCount) targets.push({ r, c, text });
    });
  });
  const clippedCols = Math.max(0, widest - Math.max(0, colCount - anchor.c));
  return { targets, clippedRows, clippedCols };
}

/**
 * Whether the clipboard holds whole rows (TablePlus): every line has one field per visible column,
 * so ⌘V adds them as new rows. A one-column grid cannot tell a row from a value: it never does.
 */
export function holdsWholeRows(block: readonly (readonly string[])[], colCount: number): boolean {
  return colCount > 1 && block.length > 0 && block.every((fields) => fields.length === colCount);
}

const NULL: Value = { t: "null" };
const TEXT_LIKE: ReadonlySet<GridColumn["kind"]> = new Set(["text", "enum", "set"]);

/**
 * Typed value for pasted text, or null when it does not fit the column. An empty field is NULL in
 * a nullable non-text column (what copying a NULL writes) and '' in a text column.
 */
export function pastedValue(col: GridColumn, text: string, previous: Value = NULL): Value | null {
  if (text === "" && col.nullable && !TEXT_LIKE.has(col.kind)) return NULL;
  if (col.kind === "enum" && (col.enumValues?.length ?? 0) > 0 && !col.enumValues!.includes(text))
    return null;
  const parsed = parseEditText(col.kind, text, previous);
  return parsed.ok ? parsed.value : null;
}

/** One pasted row: the cells it sets, by SOURCE column index (the others keep their defaults). */
export type PastedRow = Array<{ c: number; value: Value }>;

/**
 * Whole pasted rows (`holdsWholeRows`) typed per visible column. `src[i]` is the source index of
 * visible column `cols[i]`. Read-only, binary and auto-increment columns are left to their
 * defaults (like Duplicate: a copied id would clash); values that do not fit are skipped.
 */
export function pastedRows(
  block: readonly (readonly string[])[],
  cols: readonly GridColumn[],
  src: readonly number[],
): { rows: PastedRow[]; skipped: number } {
  let skipped = 0;
  const rows = block.map((fields) => {
    const cells: PastedRow = [];
    fields.forEach((text, i) => {
      const col = cols[i];
      const c = src[i];
      if (!col || c === undefined || !col.editable || col.kind === "bytes" || col.isAutoInc) return;
      const value = pastedValue(col, text);
      if (value) cells.push({ c, value });
      else skipped++;
    });
    return cells;
  });
  return { rows, skipped };
}
