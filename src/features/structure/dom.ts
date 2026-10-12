import type { KeyboardEvent } from "react";

/** The grid control whose `data-cell-key` is `key` (keys may hold any character). */
export function cellByKey(key: string, root: ParentNode = document): HTMLElement | null {
  for (const el of root.querySelectorAll<HTMLElement>("[data-cell-key]"))
    if (el.dataset.cellKey === key) return el;
  return null;
}

/** Focuses (and selects, for text fields) the grid control `key`. */
export function focusCell(key: string, root: ParentNode = document): boolean {
  const el = cellByKey(key, root);
  if (!el) return false;
  el.scrollIntoView?.({ block: "nearest" });
  el.focus();
  if (el instanceof HTMLInputElement) el.select();
  return true;
}

/** Delete / Backspace on a row (outside its text fields and menus) runs `drop`. */
export function rowDeleteKey(e: KeyboardEvent<HTMLElement>, drop: () => void): void {
  if (e.key !== "Delete" && e.key !== "Backspace") return;
  if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
  const t = e.target as HTMLElement;
  if (t.closest("input, textarea, select, [contenteditable='true'], [role='menu']")) return;
  e.preventDefault();
  drop();
}
