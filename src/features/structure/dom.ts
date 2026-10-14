import type { KeyboardEvent, MouseEvent } from "react";

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
  // (React also delivers the events of popovers and menus rendered elsewhere in the page)
  if (!e.currentTarget.contains(t)) return;
  if (t.closest("input, textarea, select, [contenteditable='true'], [role='menu']")) return;
  e.preventDefault();
  drop();
}

/** Controls inside a row: a click on them is theirs, not a click on the row. */
const INTERACTIVE =
  "input, textarea, select, button, a, label, [role='combobox'], [role='menuitem'], [contenteditable='true']";

/**
 * The control a click landed on, or null when it landed on the row itself (its padding, a cell's
 * margin). The row's `#` button (`data-row-select`) counts as the row.
 */
export function controlHit(target: EventTarget | null): Element | null {
  if (!(target instanceof Element)) return null;
  const hit = target.closest(INTERACTIVE);
  return hit && !hit.hasAttribute("data-row-select") ? hit : null;
}

/**
 * Whether a mouse event on a row landed on the row itself: inside it in the page (not in a popover
 * or menu rendered elsewhere, whose events React still delivers) and not on one of its controls.
 */
export function onRowItself(e: MouseEvent<HTMLElement>): boolean {
  return e.currentTarget.contains(e.target as Node) && controlHit(e.target) === null;
}

/**
 * Gives a grid row the focus when it is pressed outside its controls (and on its `#` button,
 * which WebKit does not focus), so the Delete key has a focused row to act on.
 */
export function focusRowOnMouseDown(e: MouseEvent<HTMLElement>): void {
  if (!onRowItself(e)) return;
  // the `#` button would take the focus (Chrome) or leave it elsewhere (WebKit): the row keeps it
  if ((e.target as Element).closest("[data-row-select]")) e.preventDefault();
  e.currentTarget.focus({ preventScroll: true });
}
