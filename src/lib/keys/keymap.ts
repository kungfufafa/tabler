import type { ActionId } from "@/features/palette/actions";

export type Scope = "global" | "sidebar" | "grid" | "grid-edit" | "filter" | "editor" | "dialog";

export interface ShortcutDef {
  id: ActionId;
  keys: string;
  scope: Scope;
  label: string;
  /** Global shortcuts are suppressed inside CodeMirror unless this is set. */
  allowInEditor?: boolean;
  /** Global shortcuts are suppressed while a dialog is open unless this is set. */
  allowInDialog?: boolean;
  /** Global shortcut that must not fire while a cell editor has focus (its edit is not staged yet). */
  notWhileEditing?: boolean;
}

/**
 * Single source of truth for keyboard shortcuts; the Settings → Shortcuts screen renders this list.
 * The defaults follow TablePlus. When an action has several combos, the first one is its primary
 * shortcut (shown in menus, tooltips and the palette); the others are kept as alternatives.
 */
export const KEYMAP: ShortcutDef[] = [
  {
    id: "tab.new-query",
    keys: "Mod+T",
    scope: "global",
    label: "New query tab",
    allowInEditor: true,
  },
  // Inside the editor ⌘E is Explain (editor scope wins).
  { id: "tab.new-query", keys: "Mod+E", scope: "global", label: "Open SQL editor" },
  {
    id: "tab.close",
    keys: "Mod+W",
    scope: "global",
    label: "Close tab",
    allowInEditor: true,
    allowInDialog: true,
  },
  {
    id: "tab.close-all",
    keys: "Mod+Shift+W",
    scope: "global",
    label: "Close all tabs",
    allowInEditor: true,
  },
  {
    id: "tab.reopen",
    keys: "Mod+Shift+T",
    scope: "global",
    label: "Reopen closed tab",
    allowInEditor: true,
  },
  { id: "tab.next", keys: "Mod+]", scope: "global", label: "Next tab", allowInEditor: true },
  { id: "tab.prev", keys: "Mod+[", scope: "global", label: "Previous tab", allowInEditor: true },
  { id: "tab.next", keys: "Ctrl+Tab", scope: "global", label: "Next tab", allowInEditor: true },
  {
    id: "tab.prev",
    keys: "Ctrl+Shift+Tab",
    scope: "global",
    label: "Previous tab",
    allowInEditor: true,
  },
  { id: "tab.next", keys: "Mod+Shift+]", scope: "global", label: "Next tab", allowInEditor: true },
  {
    id: "tab.prev",
    keys: "Mod+Shift+[",
    scope: "global",
    label: "Previous tab",
    allowInEditor: true,
  },
  ...([1, 2, 3, 4, 5, 6, 7, 8, 9] as const).map((n): ShortcutDef => ({
    id: `tab.goto-${n}`,
    keys: `Mod+${n}`,
    scope: "global",
    label: n === 9 ? "Go to last tab" : `Go to tab ${n}`,
    allowInEditor: true,
  })),
  {
    id: "palette.open",
    keys: "Mod+P",
    scope: "global",
    label: "Open anything",
    allowInEditor: true,
  },
  {
    id: "palette.databases",
    keys: "Mod+K",
    scope: "global",
    label: "Switch database",
    allowInEditor: true,
  },
  {
    id: "palette.connections",
    keys: "Mod+Shift+K",
    scope: "global",
    label: "Switch connection",
    allowInEditor: true,
  },
  { id: "settings.open", keys: "Mod+,", scope: "global", label: "Settings", allowInEditor: true },
  {
    id: "ui.toggle-sidebar",
    keys: "Mod+0",
    scope: "global",
    label: "Toggle sidebar",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-sidebar",
    keys: "Mod+B",
    scope: "global",
    label: "Toggle sidebar",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-inspector",
    keys: "Mod+Shift+I",
    scope: "global",
    label: "Toggle row detail panel",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-console",
    keys: "Mod+Shift+C",
    scope: "global",
    label: "Toggle console log",
    allowInEditor: true,
  },
  {
    id: "ui.launcher",
    keys: "Mod+Shift+O",
    scope: "global",
    label: "Connection manager",
    allowInEditor: true,
  },
  {
    id: "sidebar.focus-search",
    keys: "Mod+Shift+F",
    scope: "global",
    label: "Search for item",
    allowInEditor: true,
  },
  {
    id: "sidebar.focus-search",
    keys: "Mod+L",
    scope: "global",
    label: "Search for item",
    allowInEditor: true,
  },
  { id: "view.refresh", keys: "Mod+R", scope: "global", label: "Refresh" },
  { id: "view.refresh", keys: "Mod+Alt+R", scope: "global", label: "Reload current tab" },
  // Switching the view unmounts an open cell editor before it could stage its value.
  {
    id: "view.data",
    keys: "Mod+Ctrl+[",
    scope: "global",
    label: "Show data",
    notWhileEditing: true,
  },
  {
    id: "view.structure",
    keys: "Mod+Ctrl+]",
    scope: "global",
    label: "Show structure",
    notWhileEditing: true,
  },
  {
    id: "connection.new",
    keys: "Mod+N",
    scope: "global",
    label: "New connection",
    allowInEditor: true,
  },
  {
    id: "connection.reconnect",
    keys: "Mod+Shift+R",
    scope: "global",
    label: "Reconnect",
    allowInEditor: true,
  },
  // Staging works from anywhere in the tab (toolbar, filter bar…), not only with the grid focused.
  {
    id: "grid.commit",
    keys: "Mod+S",
    scope: "global",
    label: "Commit changes",
    notWhileEditing: true,
  },
  {
    id: "grid.preview",
    keys: "Mod+Shift+P",
    scope: "global",
    label: "Preview SQL",
    notWhileEditing: true,
  },
  {
    id: "grid.discard",
    keys: "Mod+Shift+Backspace",
    scope: "global",
    label: "Discard all changes",
    notWhileEditing: true,
  },
  { id: "grid.filter", keys: "Mod+F", scope: "grid", label: "Toggle filter bar" },
  { id: "grid.columns", keys: "Mod+Alt+F", scope: "global", label: "Show / hide columns" },
  { id: "grid.new-row", keys: "Mod+I", scope: "grid", label: "New row" },
  { id: "grid.delete-rows", keys: "Mod+Backspace", scope: "grid", label: "Delete selected rows" },
  { id: "grid.duplicate-row", keys: "Mod+D", scope: "grid", label: "Duplicate row" },
  { id: "grid.set-null", keys: "Mod+Shift+N", scope: "grid", label: "Set NULL" },
  { id: "grid.set-default", keys: "Mod+Shift+D", scope: "grid", label: "Set DEFAULT" },
  { id: "grid.open-fk", keys: "Mod+J", scope: "grid", label: "Open referenced row" },
  { id: "grid.undo", keys: "Mod+Z", scope: "grid", label: "Undo staged change" },
  { id: "grid.redo", keys: "Mod+Shift+Z", scope: "grid", label: "Redo staged change" },
  { id: "grid.toggle-detail", keys: "Space", scope: "grid", label: "Toggle row detail" },
  { id: "grid.copy", keys: "Mod+C", scope: "grid", label: "Copy selected rows" },
  { id: "grid.copy-as", keys: "Mod+Alt+C", scope: "grid", label: "Copy rows as…" },
  { id: "grid.paste", keys: "Mod+V", scope: "grid", label: "Paste" },
  // The filter bar above the grid (TablePlus's keys); ↩ in a value applies too.
  { id: "filter.apply", keys: "Mod+Enter", scope: "filter", label: "Apply all filters" },
  { id: "filter.duplicate", keys: "Mod+I", scope: "filter", label: "Duplicate filter" },
  { id: "filter.remove", keys: "Mod+Shift+I", scope: "filter", label: "Remove filter" },
  {
    id: "query.run-current",
    keys: "Mod+Enter",
    scope: "editor",
    label: "Run current statement",
  },
  { id: "query.run", keys: "Mod+Shift+Enter", scope: "editor", label: "Run all" },
  { id: "query.cancel", keys: "Mod+.", scope: "editor", label: "Cancel running query" },
  { id: "query.explain", keys: "Mod+E", scope: "editor", label: "Explain" },
  { id: "query.format", keys: "Mod+I", scope: "editor", label: "Beautify SQL" },
  { id: "query.format", keys: "Alt+Shift+F", scope: "editor", label: "Beautify SQL" },
  { id: "query.minify", keys: "Mod+Shift+I", scope: "editor", label: "Uglify SQL" },
  {
    id: "query.toggle-results",
    keys: "Ctrl+`",
    scope: "global",
    label: "Toggle result pane",
    allowInEditor: true,
  },
  { id: "query.history", keys: "Mod+H", scope: "editor", label: "Query history" },
  { id: "query.save", keys: "Mod+S", scope: "editor", label: "Save query to file" },
  {
    id: "export.open",
    keys: "Mod+Shift+E",
    scope: "global",
    label: "Export…",
    allowInEditor: true,
  },
  { id: "query.open", keys: "Mod+O", scope: "editor", label: "Open .sql file" },
];

const SCOPE_CHAIN: Record<Scope, Scope[]> = {
  global: ["global"],
  sidebar: ["sidebar", "global"],
  grid: ["grid", "global"],
  "grid-edit": ["grid-edit", "global"],
  filter: ["filter", "global"],
  editor: ["editor", "global"],
  dialog: ["dialog", "global"],
};

export function findShortcut(
  combo: string,
  scope: Scope,
  defs: ShortcutDef[] = KEYMAP,
): ShortcutDef | undefined {
  for (const s of SCOPE_CHAIN[scope]) {
    const def = defs.find((d) => d.keys === combo && d.scope === s);
    if (!def) continue;
    if (s === "global" && scope === "editor" && !def.allowInEditor) return undefined;
    if (s === "global" && scope === "dialog" && !def.allowInDialog) return undefined;
    if (s === "global" && scope === "grid-edit" && def.notWhileEditing) return undefined;
    return def;
  }
  return undefined;
}

/** The primary (first) combo of an action. */
export function shortcutFor(id: ActionId): string | undefined {
  return KEYMAP.find((d) => d.id === id)?.keys;
}
