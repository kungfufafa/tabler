/**
 * Multi-selection of the sidebar's items tree (TablePlus: ⌘-click toggles, ⇧-click selects a
 * range, ⌘A selects every listed item). Keys are tree row keys; `order` is the item rows' keys in
 * tree order (headers excluded), so a range never includes a section header.
 */

export interface TreeSelection {
  keys: ReadonlySet<string>;
  /** Where a ⇧-click range starts: the last plainly clicked or ⌘-clicked row. */
  anchor: string | null;
}

export const EMPTY_SELECTION: TreeSelection = { keys: new Set(), anchor: null };

export function selectOnly(key: string): TreeSelection {
  return { keys: new Set([key]), anchor: key };
}

/** ⌘-click: adds `key` to the selection or takes it out. */
export function toggleSelected(sel: TreeSelection, key: string): TreeSelection {
  const keys = new Set(sel.keys);
  if (keys.has(key)) keys.delete(key);
  else keys.add(key);
  return { keys, anchor: key };
}

/** ⇧-click: every item between the anchor and `key` (just `key` without an anchor). */
export function selectRange(
  sel: TreeSelection,
  order: readonly string[],
  key: string,
): TreeSelection {
  const to = order.indexOf(key);
  const from = sel.anchor === null ? -1 : order.indexOf(sel.anchor);
  if (to < 0) return sel;
  if (from < 0) return selectOnly(key);
  const [a, b] = from <= to ? [from, to] : [to, from];
  return { keys: new Set(order.slice(a, b + 1)), anchor: sel.anchor };
}

/** ⌘A. */
export function selectAll(order: readonly string[]): TreeSelection {
  return { keys: new Set(order), anchor: order[0] ?? null };
}

/** The selection after a click with these modifiers (`meta`: ⌘ / Ctrl, `shift`: ⇧). */
export function clickSelection(
  sel: TreeSelection,
  order: readonly string[],
  key: string,
  mods: { meta: boolean; shift: boolean },
): TreeSelection {
  if (mods.shift) return selectRange(sel, order, key);
  if (mods.meta) return toggleSelected(sel, key);
  return selectOnly(key);
}

/** Drops keys no longer listed (a search narrowed the tree, a table was dropped). */
export function pruneSelection(sel: TreeSelection, order: readonly string[]): TreeSelection {
  const listed = new Set(order);
  if ([...sel.keys].every((k) => listed.has(k)) && (sel.anchor === null || listed.has(sel.anchor)))
    return sel;
  const keys = new Set([...sel.keys].filter((k) => listed.has(k)));
  return { keys, anchor: sel.anchor !== null && listed.has(sel.anchor) ? sel.anchor : null };
}

/**
 * The rows a menu or key acts on: the whole selection when `key` is part of it, else only `key`
 * (right-clicking an unselected row selects it alone, as on macOS).
 */
export function targetKeys(sel: TreeSelection, key: string): string[] {
  return sel.keys.has(key) ? [...sel.keys] : [key];
}
