import { formatCombo, isMac } from "@/lib/keys/combo";
import { findShortcut, type Scope, type ShortcutDef } from "@/lib/keys/keymap";

export interface ShortcutRow {
  /** The action id for keymap rows; `ref:<keys>` for reference rows (built-in keys). */
  id: string;
  label: string;
  /** Every combo bound to this action+label in the scope, in keymap order. */
  keys: string[];
}

export interface ShortcutGroup {
  scope: Scope;
  title: string;
  rows: ShortcutRow[];
}

/**
 * A key the app handles itself rather than through KEYMAP and the action registry: grid
 * navigation (DataGrid), the cell editors, CodeMirror's own keymaps. Listed so the shortcut sheet
 * is complete; combos are spelled like KEYMAP's (`Up`, `PageDown`, `Mod+Alt+G`…).
 */
export interface ReferenceShortcut {
  scope: Scope;
  keys: string;
  label: string;
}

/** Rows with the same label merge, so alternatives (Enter / F2) share one line. */
export const REFERENCE_SHORTCUTS: ReadonlyArray<ReferenceShortcut> = [
  // Table grid (DataGrid's keydown handler and clicks). The grid selects rows (TablePlus).
  { scope: "grid", keys: "Up", label: "Select the row above / below" },
  { scope: "grid", keys: "Down", label: "Select the row above / below" },
  { scope: "grid", keys: "Shift+Up", label: "Extend the row selection" },
  { scope: "grid", keys: "Shift+Down", label: "Extend the row selection" },
  { scope: "grid", keys: "Shift+Click", label: "Extend the row selection" },
  { scope: "grid", keys: "Mod+Click", label: "Add / remove a row" },
  { scope: "grid", keys: "Left", label: "Previous / next cell in the row" },
  { scope: "grid", keys: "Right", label: "Previous / next cell in the row" },
  { scope: "grid", keys: "Tab", label: "Previous / next cell in the row" },
  { scope: "grid", keys: "Shift+Tab", label: "Previous / next cell in the row" },
  { scope: "grid", keys: "Mod+Up", label: "First / last row" },
  { scope: "grid", keys: "Mod+Down", label: "First / last row" },
  { scope: "grid", keys: "Mod+Home", label: "First / last row" },
  { scope: "grid", keys: "Mod+End", label: "First / last row" },
  { scope: "grid", keys: "Home", label: "First / last column" },
  { scope: "grid", keys: "End", label: "First / last column" },
  { scope: "grid", keys: "PageUp", label: "Page up / down" },
  { scope: "grid", keys: "PageDown", label: "Page up / down" },
  { scope: "grid", keys: "Enter", label: "Edit cell (typing does not edit)" },
  { scope: "grid", keys: "F2", label: "Edit cell (typing does not edit)" },
  { scope: "grid", keys: "Backspace", label: "Delete selected rows" },
  { scope: "grid", keys: "Delete", label: "Delete selected rows" },
  { scope: "grid", keys: "Escape", label: "Select only the focused row" },
  { scope: "grid", keys: "Mod+A", label: "Select all rows" },
  { scope: "grid", keys: "Alt+Click", label: "Select a column (on its header)" },
  // Cell editors (inline input, multi-line panel, JSON editor).
  { scope: "grid-edit", keys: "Enter", label: "Save and move down" },
  { scope: "grid-edit", keys: "Tab", label: "Save and move to the next / previous cell" },
  { scope: "grid-edit", keys: "Shift+Tab", label: "Save and move to the next / previous cell" },
  { scope: "grid-edit", keys: "Escape", label: "Cancel the edit" },
  { scope: "grid-edit", keys: "Alt+Enter", label: "New line (opens the multi-line editor)" },
  { scope: "grid-edit", keys: "Mod+Enter", label: "Save (JSON / multi-line editor)" },
  // The editors take ⌘S themselves: they stage what was typed, then commit the tab.
  { scope: "grid-edit", keys: "Mod+S", label: "Stage the edit and commit changes" },
  // SQL editor (CodeMirror's default, search and completion keymaps).
  { scope: "editor", keys: "Mod+F", label: "Find and replace" },
  { scope: "editor", keys: "F3", label: "Find next" },
  { scope: "editor", keys: "Mod+G", label: "Find next" },
  { scope: "editor", keys: "Shift+F3", label: "Find previous" },
  { scope: "editor", keys: "Mod+Shift+G", label: "Find previous" },
  { scope: "editor", keys: "Mod+D", label: "Select next occurrence" },
  { scope: "editor", keys: "Mod+Alt+G", label: "Go to line" },
  { scope: "editor", keys: "Alt+Up", label: "Move line up / down" },
  { scope: "editor", keys: "Alt+Down", label: "Move line up / down" },
  { scope: "editor", keys: "Ctrl+Space", label: "Show completions" },
  // SQL editor: TablePlus's multiple carets (cmExtensions.multiCaretKeymap). The comment toggles
  // (⌘/, ⌥⌘/) are KEYMAP entries now (Keymap ▸ SQL Editor).
  { scope: "editor", keys: "Mod+Click", label: "Add a caret" },
  { scope: "editor", keys: "Ctrl+Shift+Up", label: "Add a caret above / below" },
  { scope: "editor", keys: "Ctrl+Shift+Down", label: "Add a caret above / below" },
  { scope: "editor", keys: "Mod+Ctrl+G", label: "Select all occurrences" },
  { scope: "editor", keys: "Escape", label: "Back to a single caret" },
  // Sidebar items tree (SchemaTree's keydown handler and clicks).
  { scope: "sidebar", keys: "Mod+Click", label: "Add / remove an item" },
  { scope: "sidebar", keys: "Shift+Click", label: "Select a range of items" },
  { scope: "sidebar", keys: "Shift+Up", label: "Select a range of items" },
  { scope: "sidebar", keys: "Shift+Down", label: "Select a range of items" },
  { scope: "sidebar", keys: "Mod+A", label: "Select all items" },
  { scope: "sidebar", keys: "Mod+C", label: "Copy the selected names" },
  { scope: "sidebar", keys: "Backspace", label: "Delete the selected items" },
  { scope: "sidebar", keys: "Alt+Backspace", label: "Truncate the selected tables" },
  { scope: "sidebar", keys: "Shift+Backspace", label: "Delete with CASCADE (PostgreSQL)" },
  {
    scope: "sidebar",
    keys: "Mod+Backspace",
    label: "Delete ignoring foreign key checks (MySQL)",
  },
];

export const SCOPE_TITLES: Record<Scope, string> = {
  global: "Global",
  sidebar: "Sidebar",
  grid: "Table grid",
  "grid-edit": "Cell editor",
  filter: "Filter bar",
  editor: "SQL editor",
  dialog: "Dialogs",
};

export const SCOPE_ORDER: Scope[] = [
  "global",
  "sidebar",
  "grid",
  "grid-edit",
  "filter",
  "editor",
  "dialog",
];

export const isReference = (row: Pick<ShortcutRow, "id">) => row.id.startsWith("ref:");

/** Lower-cased text a row can be found by: label, action id, scope and every spelling of its keys. */
export function haystack(row: ShortcutRow, scope: Scope, mac: boolean): string {
  const keys = row.keys.flatMap((k) => [
    k,
    k.replace(/\bMod\b/g, mac ? "Cmd" : "Ctrl"),
    formatCombo(k, mac),
  ]);
  const id = isReference(row) ? [] : [row.id];
  return [row.label, ...id, SCOPE_TITLES[scope], ...keys].join(" ").toLowerCase();
}

/** The lower-cased, whitespace-separated terms of a search (every one must match). */
export function queryTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/** One binding on its way into a row. */
interface Entry {
  scope: Scope;
  keys: string;
  label: string;
  id: string;
  /** Bindings with the same key in a scope are listed once. */
  bindingKey: string;
  /** Bindings with the same key in a scope share a row. */
  rowKey: string;
}

/**
 * Turns the keymap plus the reference keys into the read-only Settings → Shortcuts table: grouped
 * by scope (fixed order, empty groups dropped), duplicate bindings removed (a reference key the
 * keymap takes in that scope, e.g. a global key allowed in the editor, is left out: it never
 * reaches CodeMirror or the grid), and alternative combos for the same action and label merged
 * into one row, primary combo first. Every whitespace-separated term of `query` must match.
 */
export function groupShortcuts(
  defs: readonly ShortcutDef[],
  query = "",
  mac: boolean = isMac,
  reference: readonly ReferenceShortcut[] = REFERENCE_SHORTCUTS,
): ShortcutGroup[] {
  const defList = [...defs];
  const entries: Entry[] = [
    ...defs.map((d) => ({
      scope: d.scope,
      keys: d.keys,
      label: d.label,
      id: d.id,
      bindingKey: `${d.id}\u0000${d.keys}`,
      rowKey: `${d.id}\u0000${d.label}`,
    })),
    ...reference
      .filter((r) => findShortcut(r.keys, r.scope, defList) === undefined)
      .map((r) => ({
        scope: r.scope,
        keys: r.keys,
        label: r.label,
        id: `ref:${r.keys}`,
        bindingKey: `ref\u0000${r.scope}\u0000${r.keys}`,
        rowKey: `ref\u0000${r.label}`,
      })),
  ];

  const seen = new Set<string>();
  const byScope = new Map<Scope, Map<string, ShortcutRow>>();
  for (const e of entries) {
    if (seen.has(e.bindingKey)) continue;
    seen.add(e.bindingKey);
    let rows = byScope.get(e.scope);
    if (!rows) {
      rows = new Map();
      byScope.set(e.scope, rows);
    }
    const row = rows.get(e.rowKey);
    if (row) row.keys.push(e.keys);
    else rows.set(e.rowKey, { id: e.id, label: e.label, keys: [e.keys] });
  }

  const terms = queryTerms(query);
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
