import type { Rect } from "./selection";
import type { CellPos } from "./types";

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
