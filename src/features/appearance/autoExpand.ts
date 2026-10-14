import { useEffect, useRef } from "react";
import {
  columnWidth,
  EMPTY_COLUMN_STATE,
  type ColumnState,
  type GridColumn,
  type ViewRow,
} from "@/features/datagrid/types";
import type { Value } from "@/lib/ipc";
import { cellDisplay } from "@/lib/values";
import { usePrefs } from "@/stores/prefs";

const NULL: Value = { t: "null" };

/** General ▸ Table Data ▸ Auto expand text column: the widest a column opens to. */
export const AUTO_EXPAND_MAX_W = 480;
/** Longest text measured per cell: past this a column is at its maximum anyway. */
const MAX_MEASURED_CHARS = 100;
/** Rows sampled per table. */
export const AUTO_EXPAND_SAMPLE_ROWS = 60;
/** Average glyph width of the grid's 12px font and a cell's horizontal padding (see `fitWidth`). */
const CHAR_W = 7.5;
const CELL_PADDING = 24;

export interface AutoExpandColumn {
  id: string;
  kind: string;
  /** The width the column opens with: sized for its header (`defaultWidth`). */
  width: number;
  /** The user already sized it (a remembered layout): left alone. */
  sized: boolean;
  /** Room for the foreign-key arrow. */
  extra?: number;
}

/**
 * The widths text columns grow to: wide enough for their longest sampled value, at most
 * `AUTO_EXPAND_MAX_W`. Only text columns whose header-sized width is narrower than their content
 * change (never shrunk), and only columns the user has not sized. `fontSize` scales the glyph
 * width (the grid's font is 12px by default).
 */
export function autoExpandWidths(
  columns: readonly AutoExpandColumn[],
  samples: ReadonlyMap<string, readonly string[]>,
  fontSize = 12,
): Record<string, number> {
  const glyph = CHAR_W * (fontSize / 12);
  const out: Record<string, number> = {};
  for (const col of columns) {
    if (col.kind !== "text" || col.sized) continue;
    let chars = 0;
    for (const text of samples.get(col.id) ?? []) chars = Math.max(chars, text.length);
    if (chars === 0) continue;
    const content = Math.min(chars, MAX_MEASURED_CHARS) * glyph + CELL_PADDING + (col.extra ?? 0);
    const width = Math.round(Math.min(AUTO_EXPAND_MAX_W, content));
    if (width > col.width) out[col.id] = width;
  }
  return out;
}

export interface AutoExpandArgs {
  columns: readonly GridColumn[];
  columnState: ColumnState;
  setColumnState: (next: ColumnState) => void;
  rowCount: number;
  loading?: boolean;
  getRow: (index: number) => ViewRow | undefined;
}

/**
 * General ▸ Table Data ▸ Auto expand text column (off by default): expands text columns once per
 * table, when its first rows are in. The grid calls this with its columns and rows; the new widths
 * go through `setColumnState`, so they are remembered like any widths the user dragged.
 */
export function useAutoExpandColumns(args: AutoExpandArgs): void {
  const enabled = usePrefs((p) => p.gridAutoExpandText);
  const fontSize = usePrefs((p) => p.gridFontSize);
  const done = useRef<string | null>(null);
  const latest = useRef(args);
  latest.current = args;
  const signature = args.columns.map((c) => c.id).join("\u0001");
  const ready = enabled && args.rowCount > 0 && !args.loading;
  useEffect(() => {
    if (!ready || done.current === signature) return;
    const a = latest.current;
    const samples = new Map<string, string[]>();
    let sampled = 0;
    for (let r = 0; r < Math.min(a.rowCount, AUTO_EXPAND_SAMPLE_ROWS); r++) {
      const row = a.getRow(r);
      if (!row) continue;
      sampled++;
      a.columns.forEach((col, i) => {
        const text = cellDisplay(row.cells[i] ?? NULL);
        if (text.placeholder) return;
        const list = samples.get(col.id);
        if (list) list.push(text.text);
        else samples.set(col.id, [text.text]);
      });
    }
    // Rows that are not loaded yet: try again when they are.
    if (sampled === 0) return;
    done.current = signature;
    const widths = autoExpandWidths(
      a.columns.map((col) => ({
        id: col.id,
        kind: col.kind,
        // No remembered width yet: the width the column opens with.
        width: columnWidth(col, EMPTY_COLUMN_STATE),
        sized: a.columnState.widths[col.id] !== undefined,
        extra: col.fk ? 16 : 0,
      })),
      samples,
      fontSize,
    );
    if (Object.keys(widths).length > 0)
      a.setColumnState({ ...a.columnState, widths: { ...a.columnState.widths, ...widths } });
  }, [ready, signature, args.rowCount, fontSize]);
}
