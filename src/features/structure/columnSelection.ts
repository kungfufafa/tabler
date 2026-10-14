import { create } from "zustand";
import type { Dialect } from "@/lib/ipc";
import type { ColumnField } from "./columnEdits";
import type { ColumnRow } from "./model";

/**
 * The columns selected in a structure view, as the right sidebar shows them (TablePlus: the
 * sidebar edits the attributes of the selected columns, "Multiple values" where they differ).
 * Published by the active structure tab, cleared when it goes.
 */
export interface ColumnSelection {
  tabId: string;
  /** The table's name. */
  table: string;
  dialect: Dialect;
  /** The selected rows (live and dropped), in grid order. */
  rows: ColumnRow[];
  editable: boolean;
  /** Stages `raw` as `field` on every selected column (one undo step). */
  onEdit: (field: ColumnField, raw: string | boolean) => void;
}

interface SelectionState {
  selection: ColumnSelection | null;
  publish: (selection: ColumnSelection | null) => void;
  /** Clears the selection of `tabId` (a tab that goes away or loses focus). */
  clear: (tabId: string) => void;
}

export const useColumnSelectionStore = create<SelectionState>()((set) => ({
  selection: null,
  publish: (selection) => set({ selection }),
  clear: (tabId) => set((s) => (s.selection?.tabId === tabId ? { selection: null } : s)),
}));

/** What a click on a column's `#` does to the selection. */
export interface SelectModifiers {
  meta: boolean;
  shift: boolean;
}

/**
 * The selection after clicking `key`: alone, ⌘ toggles it, ⇧ extends from `anchor` (the last
 * plain / ⌘ click) over `order` (the keys in grid order).
 */
export function nextSelection(
  selected: readonly string[],
  anchor: string | null,
  order: readonly string[],
  key: string,
  mods: SelectModifiers,
): { selected: string[]; anchor: string } {
  if (mods.shift && anchor !== null) {
    const a = order.indexOf(anchor);
    const b = order.indexOf(key);
    if (a >= 0 && b >= 0) {
      const range = order.slice(Math.min(a, b), Math.max(a, b) + 1);
      // ⌘⇧ adds the range to what is selected.
      const kept = mods.meta ? selected.filter((k) => !range.includes(k)) : [];
      return { selected: [...kept, ...range], anchor };
    }
  }
  if (mods.meta)
    return {
      selected: selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key],
      anchor: key,
    };
  return { selected: [key], anchor: key };
}

/** The value `rows` share for `pick`, or `MIXED` when they differ. */
export const MIXED = Symbol("multiple values");

export function commonValue<T>(
  rows: readonly ColumnRow[],
  pick: (r: ColumnRow) => T,
): T | typeof MIXED {
  const first = rows[0];
  if (!first) return MIXED;
  const v = pick(first);
  return rows.every((r) => pick(r) === v) ? v : MIXED;
}
