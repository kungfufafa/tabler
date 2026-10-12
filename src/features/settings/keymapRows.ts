import type { ActionId } from "@/features/palette/actions";
import type { Scope, ShortcutDef } from "@/lib/keys/keymap";
import {
  conflictsWith,
  isFixedBinding,
  slotKeys,
  type KeymapOverrides,
} from "@/lib/keys/overrides";

/** One built-in binding of an action as Settings ▸ Shortcuts edits it. */
export interface KeymapSlot {
  /** Index among the action's built-in bindings (the override slot). */
  slot: number;
  /** Current combo; "" when the user removed it. */
  keys: string;
  defaultKeys: string;
  /** ⌘R / ⌘0 cannot be remapped (TablePlus). */
  fixed: boolean;
}

/** An editable row: an action's bindings that share a label (alternatives on one line). */
export interface KeymapRow {
  id: ActionId;
  label: string;
  scope: Scope;
  slots: KeymapSlot[];
  /** Some slot differs from its default. */
  modified: boolean;
}

/** Row identity, as the read-only listing merges rows: action id + label. */
export const rowKey = (id: string, label: string): string => `${id}\u0000${label}`;

/**
 * The editable keymap: one row per action and label (like `groupShortcuts`), in keymap order, each
 * with its bindings' current combos. Rows stay listed when every binding was removed, so they can
 * be bound again.
 */
export function keymapRows(
  defaults: readonly ShortcutDef[],
  overrides: KeymapOverrides,
): KeymapRow[] {
  const rows = new Map<string, KeymapRow>();
  const keysById = new Map<string, string[]>();
  const seen = new Map<string, number>();
  for (const def of defaults) {
    const slot = seen.get(def.id) ?? 0;
    seen.set(def.id, slot + 1);
    let keys = keysById.get(def.id);
    if (!keys) {
      keys = slotKeys(defaults, overrides, def.id);
      keysById.set(def.id, keys);
    }
    const current = keys[slot] ?? def.keys;
    const key = rowKey(def.id, def.label);
    let row = rows.get(key);
    if (!row) {
      row = { id: def.id, label: def.label, scope: def.scope, slots: [], modified: false };
      rows.set(key, row);
    }
    // The same combo listed twice for an action (an exact duplicate binding) shows once.
    if (row.slots.some((s) => s.defaultKeys === def.keys)) continue;
    row.slots.push({ slot, keys: current, defaultKeys: def.keys, fixed: isFixedBinding(def) });
    if (current !== def.keys) row.modified = true;
  }
  return [...rows.values()];
}

/** Labels of the other actions in the row's scope that share one of its combos. */
export function rowConflicts(row: KeymapRow, keymap: readonly ShortcutDef[]): string[] {
  const labels = new Set<string>();
  for (const s of row.slots) {
    if (!s.keys) continue;
    for (const d of conflictsWith(keymap, row.scope, s.keys, row.id)) labels.add(d.label);
  }
  return [...labels];
}
