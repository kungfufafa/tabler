import { createElement, Fragment, type ReactNode } from "react";
import type { Value } from "@/lib/ipc";
import type { CopyContext } from "./clipboard";
import type { GridColumn } from "./types";

/**
 * Extension point of the grid's cell menu (Plugins ▸ …) and of the active grid (a plugin command
 * run from the menu bar works on its rows). Kept to what a grid knows; indexes are SOURCE indexes.
 */
export interface GridMenuContext {
  /** Every column, in source order. */
  columns: GridColumn[];
  /** The table / dialect the grid shows (Copy as INSERT's), when it shows one. */
  copyContext?: CopyContext;
  /** The focused row's cells, null without a focused row. */
  focusedRow: Value[] | null;
  /** Source index of the focused column. */
  focusedColumn: number | null;
  /** The selected rows' cells (the focused row when no row is selected). */
  selectedRows: () => Value[][];
  /** Stages `value` into the focused cell, on editable grids. */
  stageFocused?: (value: Value) => void;
}

export type GridMenuExtension = (context: () => GridMenuContext) => ReactNode;

const extensions = new Set<GridMenuExtension>();

/** Adds entries at the end of every grid's cell menu. Returns the function that removes them. */
export function registerGridMenuExtension(extension: GridMenuExtension): () => void {
  extensions.add(extension);
  return () => void extensions.delete(extension);
}

/** What the extensions add to a cell menu (rendered while it is open). */
export function renderGridMenuExtensions(context: () => GridMenuContext): ReactNode {
  return [...extensions].map((ext, i) => createElement(Fragment, { key: i }, ext(context)));
}

let active: (() => GridMenuContext) | null = null;

/** The grid of the active tab reports itself (one at a time). Returns the unregister function. */
export function setActiveGridMenuContext(context: () => GridMenuContext): () => void {
  active = context;
  return () => {
    if (active === context) active = null;
  };
}

/** The active grid's rows and focus, or null. */
export function activeGridMenuContext(): GridMenuContext | null {
  return active?.() ?? null;
}
