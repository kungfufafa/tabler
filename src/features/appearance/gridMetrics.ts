import { useLayoutEffect, useRef, useSyncExternalStore } from "react";
import { getRowHeight, subscribeRowHeight } from "@/features/datagrid/types";
import {
  DEFAULT_GRID_FONT_SIZE,
  DEFAULT_GRID_LINE_HEIGHT,
  DEFAULT_GRID_ROW_PADDING,
  GRID_ROW_PADDING_PRESETS,
} from "./appearancePrefs";

/** The grid's text line in px: the font size times the line-height ratio (at least 10). */
export function gridLineHeightPx(fontSize: number, ratio: number): number {
  return Math.max(10, Math.round(fontSize * ratio));
}

/** A grid row: one text line plus the padding above and below it. */
export function gridRowHeight(fontSize: number, ratio: number, padding: number): number {
  return gridLineHeightPx(fontSize, ratio) + 2 * padding;
}

/** The Compact / Default / Comfortable preset a padding is, or null for a custom number. */
export function paddingPreset(
  padding: number,
): (typeof GRID_ROW_PADDING_PRESETS)[number]["id"] | null {
  return GRID_ROW_PADDING_PRESETS.find((p) => p.padding === padding)?.id ?? null;
}

/** The row height the defaults give: what the grid had before the preferences existed. */
export const DEFAULT_GRID_ROW_HEIGHT = gridRowHeight(
  DEFAULT_GRID_FONT_SIZE,
  DEFAULT_GRID_LINE_HEIGHT,
  DEFAULT_GRID_ROW_PADDING,
);

/**
 * Re-renders the calling grid when the row height changes (Settings ▸ Appearance) and runs
 * `onChange` once the new height is in place, e.g. to re-measure the row virtualizer.
 */
export function useRowHeightChange(onChange: () => void): number {
  const rowHeight = useSyncExternalStore(subscribeRowHeight, getRowHeight);
  const latest = useRef(onChange);
  latest.current = onChange;
  const first = useRef(true);
  useLayoutEffect(() => {
    if (first.current) first.current = false;
    else latest.current();
  }, [rowHeight]);
  return rowHeight;
}
