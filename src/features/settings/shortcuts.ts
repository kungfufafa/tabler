import type { ActionId } from "@/features/palette/actions";
import { formatCombo, isMac } from "@/lib/keys/combo";
import type { Scope, ShortcutDef } from "@/lib/keys/keymap";

export interface ShortcutRow {
  id: ActionId;
  label: string;
  /** Every combo bound to this action+label in the scope, in keymap order. */
  keys: string[];
}

export interface ShortcutGroup {
  scope: Scope;
  title: string;
  rows: ShortcutRow[];
}

export const SCOPE_TITLES: Record<Scope, string> = {
  global: "Global",
  sidebar: "Sidebar",
  grid: "Table grid",
  "grid-edit": "Cell editor",
  editor: "SQL editor",
  dialog: "Dialogs",
};

const SCOPE_ORDER: Scope[] = ["global", "sidebar", "grid", "grid-edit", "editor", "dialog"];

/** Lower-cased text a row can be found by: label, action id, scope and every spelling of its keys. */
function haystack(row: ShortcutRow, scope: Scope, mac: boolean): string {
  const keys = row.keys.flatMap((k) => [
    k,
    k.replace(/\bMod\b/g, mac ? "Cmd" : "Ctrl"),
    formatCombo(k, mac),
  ]);
  return [row.label, row.id, SCOPE_TITLES[scope], ...keys].join(" ").toLowerCase();
}

/**
 * Turns the keymap into the read-only Settings → Shortcuts table: grouped by scope (fixed order,
 * empty groups dropped), duplicate id+keys bindings removed, and alternative combos for the same
 * action and label merged into one row. Every whitespace-separated term of `query` must match.
 */
export function groupShortcuts(
  defs: readonly ShortcutDef[],
  query = "",
  mac: boolean = isMac,
): ShortcutGroup[] {
  const seen = new Set<string>();
  const byScope = new Map<Scope, Map<string, ShortcutRow>>();
  for (const def of defs) {
    const bindingKey = `${def.id}\u0000${def.keys}`;
    if (seen.has(bindingKey)) continue;
    seen.add(bindingKey);
    let rows = byScope.get(def.scope);
    if (!rows) {
      rows = new Map();
      byScope.set(def.scope, rows);
    }
    const rowKey = `${def.id}\u0000${def.label}`;
    const row = rows.get(rowKey);
    if (row) row.keys.push(def.keys);
    else rows.set(rowKey, { id: def.id, label: def.label, keys: [def.keys] });
  }

  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  return SCOPE_ORDER.flatMap((scope) => {
    const all = [...(byScope.get(scope)?.values() ?? [])];
    const rows =
      terms.length === 0
        ? all
        : all.filter((row) => {
            const text = haystack(row, scope, mac);
            return terms.every((t) => text.includes(t));
          });
    return rows.length > 0 ? [{ scope, title: SCOPE_TITLES[scope], rows }] : [];
  });
}
