import { useSyncExternalStore } from "react";
import type { ActionId } from "@/features/palette/actions";
import { getPrefs, setPref, usePrefsStore } from "@/stores/prefs";
import { isMac } from "./combo";
import {
  EMPTY_OVERRIDES,
  migrateKeymapOverrides,
  resolveKeymap,
  type KeymapOverrides,
} from "./overrides";

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
  /**
   * Global shortcut that fires only with the focus outside every scoped area (sidebar, grid, filter
   * bar, cell editor, SQL editor, dialog), which keep the key for themselves: ⌘↩ opens the SQL
   * editor from a table, structure or tool tab.
   */
  unscopedOnly?: boolean;
  /**
   * Run by the focused CodeMirror editor itself (its keymap is built from KEYMAP, see
   * `actionKeymap`), not through the action registry: the key acts on whichever editor has the
   * focus (either pane of a split SQL editor, the SQL Viewer). Remappable like any other binding.
   */
  editorCommand?: boolean;
  /**
   * The binding on Windows and Linux when `keys` holds ⌃ (alone or with ⌘): there the Ctrl key is
   * Mod, so such a combo never fires (see `platformKeymap`). The rule: ⌃⌘X becomes Ctrl+Alt+X and
   * ⌃X becomes Ctrl+X, unless that is taken.
   */
  nonMacKeys?: string;
}

/**
 * Single source of truth for keyboard shortcuts; the Settings → Shortcuts screen renders this list.
 * The defaults follow TablePlus (macOS). When an action has several combos, the first one is its
 * primary shortcut (shown in menus, tooltips and the palette); the others are kept as alternatives.
 */
export const BUILTIN_KEYMAP: readonly ShortcutDef[] = [
  {
    id: "tab.new-query",
    keys: "Mod+T",
    scope: "global",
    label: "New query tab",
    allowInEditor: true,
  },
  // TablePlus: ⌘E opens a SQL editor, from the SQL editor too (Explain is ⌥⌘E, editor scope).
  {
    id: "tab.new-query",
    keys: "Mod+E",
    scope: "global",
    label: "Open SQL editor",
    allowInEditor: true,
  },
  // TablePlus: ⌘↩ from a non-SQL tab opens the SQL editor (in a grid it is Quick Look, in the
  // filter bar Apply, in the editor Run Current; the sidebar's lists open their item with it).
  {
    id: "query.open-editor",
    keys: "Mod+Enter",
    scope: "global",
    label: "Open SQL editor (non-SQL tab)",
    unscopedOnly: true,
  },
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
  {
    id: "tab.next",
    keys: "Ctrl+Tab",
    nonMacKeys: "Mod+Tab",
    scope: "global",
    label: "Next tab",
    allowInEditor: true,
  },
  {
    id: "tab.prev",
    keys: "Ctrl+Shift+Tab",
    nonMacKeys: "Mod+Shift+Tab",
    scope: "global",
    label: "Previous tab",
    allowInEditor: true,
  },
  // TablePlus: ⇧⌘] / ⇧⌘[ select the next / previous pane (Keymap ▸ Navigate).
  {
    id: "ui.next-pane",
    keys: "Mod+Shift+]",
    scope: "global",
    label: "Select next pane",
    allowInEditor: true,
  },
  {
    id: "ui.prev-pane",
    keys: "Mod+Shift+[",
    scope: "global",
    label: "Select previous pane",
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
  // TablePlus: ⇧⌘O is File ▸ New SQL Viewer; the connection manager (File menu) has no key.
  {
    id: "sqlviewer.new",
    keys: "Mod+Shift+O",
    scope: "global",
    label: "New SQL Viewer",
    allowInEditor: true,
  },
  {
    id: "sidebar.focus-search",
    keys: "Mod+Shift+F",
    scope: "global",
    label: "Search for item",
    allowInEditor: true,
  },
  // TablePlus: ⌘R reloads the whole workspace (fixed; from the SQL editor too), ⌥⌘R only the
  // current tab.
  {
    id: "workspace.reload",
    keys: "Mod+R",
    scope: "global",
    label: "Reload workspace",
    allowInEditor: true,
  },
  { id: "view.refresh", keys: "Mod+Alt+R", scope: "global", label: "Reload current tab" },
  // Switching the view unmounts an open cell editor before it could stage its value.
  {
    id: "view.data",
    keys: "Mod+Ctrl+[",
    nonMacKeys: "Mod+Alt+[",
    scope: "global",
    label: "Show data",
    notWhileEditing: true,
  },
  {
    id: "view.structure",
    keys: "Mod+Ctrl+]",
    nonMacKeys: "Mod+Alt+]",
    scope: "global",
    label: "Show structure",
    notWhileEditing: true,
  },
  // TablePlus: ⌘N opens a new window; File ▸ New Connection… has no key.
  {
    id: "window.new",
    keys: "Mod+N",
    scope: "global",
    label: "New window",
    allowInEditor: true,
  },
  {
    id: "connection.reconnect",
    keys: "Mod+Shift+R",
    scope: "global",
    label: "Reconnect",
    allowInEditor: true,
  },
  // Inside the SQL editor ⌘. stays "Cancel running query" (editor scope wins); allowInEditor lets
  // the Connection menu item still open the list while the editor has the focus. Elsewhere in a
  // SQL tab that runs a query (its results grid…) the handler cancels the run instead (AdminHost).
  {
    id: "admin.process-list",
    keys: "Mod+.",
    scope: "global",
    label: "Process list",
    allowInEditor: true,
  },
  // Staging works from anywhere in the tab (toolbar, filter bar…), not only with the grid focused.
  // A cell editor takes ⌘S itself (it stages what was typed first, then commits).
  {
    id: "grid.commit",
    keys: "Mod+S",
    scope: "global",
    label: "Commit changes",
    notWhileEditing: true,
  },
  // Workspace-wide (TablePlus): ⇧⌘P and ⇧⌘⌫ also answer while the SQL editor's text has the focus
  // when the workspace holds staged changes. Their handlers only exist then, so with nothing to
  // preview or discard the key still reaches CodeMirror.
  {
    id: "grid.preview",
    keys: "Mod+Shift+P",
    scope: "global",
    label: "Preview SQL",
    notWhileEditing: true,
    allowInEditor: true,
  },
  {
    id: "grid.discard",
    keys: "Mod+Shift+Backspace",
    scope: "global",
    label: "Discard all changes",
    notWhileEditing: true,
    allowInEditor: true,
  },
  // Opens the filter bar (Esc closes it) on the last applied filter's value.
  { id: "grid.filter", keys: "Mod+F", scope: "grid", label: "Filter rows" },
  { id: "grid.columns", keys: "Mod+Alt+F", scope: "global", label: "Show / hide columns" },
  { id: "grid.new-row", keys: "Mod+I", scope: "grid", label: "New row" },
  { id: "grid.delete-rows", keys: "Mod+Backspace", scope: "grid", label: "Delete selected rows" },
  { id: "grid.duplicate-row", keys: "Mod+D", scope: "grid", label: "Duplicate row" },
  { id: "grid.set-null", keys: "Mod+Shift+N", scope: "grid", label: "Set NULL" },
  // Tabler's own key; ⇧⌘D is TablePlus's Split Right. macOS keeps ⌥⌘D for Show / Hide Dock (turn
  // that off in System Settings ▸ Keyboard ▸ Keyboard Shortcuts, or use the second key).
  { id: "grid.set-default", keys: "Mod+Alt+D", scope: "grid", label: "Set DEFAULT" },
  { id: "grid.set-default", keys: "Mod+Alt+Shift+D", scope: "grid", label: "Set DEFAULT" },
  { id: "grid.open-fk", keys: "Mod+J", scope: "grid", label: "Open referenced row" },
  { id: "grid.undo", keys: "Mod+Z", scope: "grid", label: "Undo staged change" },
  { id: "grid.redo", keys: "Mod+Shift+Z", scope: "grid", label: "Redo staged change" },
  { id: "grid.toggle-detail", keys: "Space", scope: "grid", label: "Toggle row detail" },
  { id: "grid.copy", keys: "Mod+C", scope: "grid", label: "Copy selected rows" },
  { id: "grid.copy-as", keys: "Mod+Alt+C", scope: "grid", label: "Copy rows as…" },
  // ⇧⌘C is two things in TablePlus's own docs: the console log, and Copy Cell Value. Scope settles
  // it: in a grid it copies the focused cell's value, everywhere else it toggles the console.
  { id: "grid.copy-cell", keys: "Mod+Shift+C", scope: "grid", label: "Copy cell value" },
  { id: "grid.paste", keys: "Mod+V", scope: "grid", label: "Paste" },
  // In a grid ⌘↩ is Quick Look (TablePlus); elsewhere it keeps its own meaning.
  { id: "grid.quick-look", keys: "Mod+Enter", scope: "grid", label: "Quick Look editor" },
  { id: "grid.set-value", keys: "Alt+Enter", scope: "grid", label: "Set value…" },
  { id: "grid.prev-page", keys: "Mod+Left", scope: "grid", label: "Previous page" },
  { id: "grid.next-page", keys: "Mod+Right", scope: "grid", label: "Next page" },
  // The filter bar above the grid (TablePlus's keys); ↩ in a value applies too.
  { id: "filter.apply", keys: "Mod+Enter", scope: "filter", label: "Apply all filters" },
  { id: "filter.duplicate", keys: "Mod+I", scope: "filter", label: "Duplicate filter" },
  { id: "filter.remove", keys: "Mod+Shift+I", scope: "filter", label: "Remove filter" },
  {
    id: "filter.apply-or",
    keys: "Mod+Shift+Enter",
    scope: "filter",
    label: "Apply all filters with OR",
  },
  { id: "filter.prev", keys: "Mod+Up", scope: "filter", label: "Previous filter" },
  { id: "filter.next", keys: "Mod+Down", scope: "filter", label: "Next filter" },
  { id: "filter.toggle", keys: "Mod+B", scope: "filter", label: "Check / uncheck filter" },
  { id: "filter.column", keys: "Mod+Left", scope: "filter", label: "Choose filter column" },
  { id: "filter.operator", keys: "Mod+Right", scope: "filter", label: "Choose filter operator" },
  {
    id: "query.run-current",
    keys: "Mod+Enter",
    scope: "editor",
    label: "Run current statement",
  },
  { id: "query.run", keys: "Mod+Shift+Enter", scope: "editor", label: "Run all" },
  { id: "query.cancel", keys: "Mod+.", scope: "editor", label: "Cancel running query" },
  // TablePlus has no default key for Explain; ⌘E stays "Open SQL editor" in the editor too.
  { id: "query.explain", keys: "Mod+Alt+E", scope: "editor", label: "Explain" },
  { id: "query.format", keys: "Mod+I", scope: "editor", label: "Beautify SQL" },
  { id: "query.format", keys: "Alt+Shift+F", scope: "editor", label: "Beautify SQL" },
  { id: "query.minify", keys: "Mod+Shift+I", scope: "editor", label: "Uglify SQL" },
  {
    id: "query.toggle-results",
    keys: "Ctrl+`",
    nonMacKeys: "Mod+`",
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
  { id: "query.add-to-queries", keys: "Mod+Alt+S", scope: "editor", label: "Add to Queries…" },
  {
    id: "query.save-selection",
    keys: "Mod+Shift+S",
    scope: "editor",
    label: "Save selected SQL statements as…",
  },
  { id: "query.font-bigger", keys: "Mod+=", scope: "editor", label: "Increase font size" },
  { id: "query.font-bigger", keys: "Mod+Shift+=", scope: "editor", label: "Increase font size" },
  { id: "query.font-smaller", keys: "Mod+-", scope: "editor", label: "Decrease font size" },
  // Keymap ▸ SQL Editor: the comment toggles run in the focused editor (CodeMirror commands).
  {
    id: "query.comment-line",
    keys: "Mod+/",
    scope: "editor",
    label: "Toggle line comment",
    editorCommand: true,
  },
  {
    id: "query.comment-block",
    keys: "Mod+Alt+/",
    scope: "editor",
    label: "Toggle block comment",
    editorCommand: true,
  },
  { id: "query.split", keys: "Mod+Shift+D", scope: "editor", label: "Split pane horizontally" },
  // Outside the SQL editor (where the editor scope's Split Pane wins) ⇧⌘D splits the workspace,
  // from the grid, the sidebar, the filter bar and the structure tab as well. Not from a cell /
  // inspector editor (grid-edit: splitting remounts the tab body under the open edit).
  {
    id: "workspace.split-right",
    keys: "Mod+Shift+D",
    scope: "global",
    label: "Split right",
    notWhileEditing: true,
  },
  // Workspaces (TablePlus ⇧⌘\ and Keymap ▸ Navigate) and the tab bar's ‹ › history.
  {
    id: "workspace.show",
    keys: "Mod+Shift+\\",
    scope: "global",
    label: "Show workspaces",
    allowInEditor: true,
  },
  {
    id: "workspace.next",
    keys: "Mod+Ctrl+Right",
    nonMacKeys: "Mod+Alt+Right",
    scope: "global",
    label: "Select next workspace",
    allowInEditor: true,
  },
  {
    id: "workspace.prev",
    keys: "Mod+Ctrl+Left",
    nonMacKeys: "Mod+Alt+Left",
    scope: "global",
    label: "Select previous workspace",
    allowInEditor: true,
  },
  // Window ▸ Lock Tabler (the SQL editor and dialogs included: the lock screen covers them).
  {
    id: "app.lock",
    keys: "Mod+Ctrl+L",
    nonMacKeys: "Mod+Alt+L",
    scope: "global",
    label: "Lock Tabler",
    allowInEditor: true,
    allowInDialog: true,
  },
  // Plugins ▸ Manage Plugins… (TablePlus ⌘L; ⇧⌘F stays Search for item) and the built-in SQL
  // Formatter plugin's Plugins ▸ SQL Format (⌃I in the SQL editor).
  {
    id: "plugins.manage",
    keys: "Mod+L",
    scope: "global",
    label: "Manage plugins…",
    allowInEditor: true,
  },
  // Off macOS Ctrl+I is Beautify.
  {
    id: "plugins.sql-format",
    keys: "Ctrl+I",
    nonMacKeys: "Mod+Alt+I",
    scope: "editor",
    label: "SQL Format (plugin)",
  },
  // Off macOS Ctrl+- is Decrease font size: back / forward take the browser's Alt+← / Alt+→ (in
  // the SQL editor too, over CodeMirror's move by syntax).
  {
    id: "tab.back",
    keys: "Ctrl+-",
    nonMacKeys: "Alt+Left",
    scope: "global",
    label: "Go back",
    allowInEditor: true,
  },
  {
    id: "tab.forward",
    keys: "Ctrl+Shift+-",
    nonMacKeys: "Alt+Right",
    scope: "global",
    label: "Go forward",
    allowInEditor: true,
  },
];

/**
 * The built-in keymap of one platform: off macOS every binding with `nonMacKeys` takes it (⌃⌘]
 * Show structure is Ctrl+Alt+] on Windows, ⌃- Go back is Alt+←).
 */
export function platformKeymap(defs: readonly ShortcutDef[], mac: boolean): ShortcutDef[] {
  return defs.map(({ nonMacKeys, ...def }) =>
    mac || !nonMacKeys ? def : { ...def, keys: nonMacKeys },
  );
}

/** The effective keymap: this platform's built-in bindings, then the user's overrides. */
export const KEYMAP: ShortcutDef[] = platformKeymap(BUILTIN_KEYMAP, isMac);

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
    if (s === "global" && scope !== "global" && def.unscopedOnly) return undefined;
    return def;
  }
  return undefined;
}

/** The primary (first) combo of an action. */
export function shortcutFor(id: ActionId): string | undefined {
  return KEYMAP.find((d) => d.id === id)?.keys;
}

// --- user overrides (Settings ▸ Shortcuts) ---------------------------------------------------

/**
 * KEYMAP as shipped. KEYMAP itself is the effective keymap: `applyKeymapOverrides` rewrites it in
 * place, so every reader (the shortcut listener, menus, tooltips, the palette, the native menu
 * guard) sees the user's bindings without knowing about them.
 */
export const DEFAULT_KEYMAP: readonly ShortcutDef[] = Object.freeze(
  KEYMAP.map((d) => Object.freeze({ ...d })),
);

let appliedOverrides: KeymapOverrides = EMPTY_OVERRIDES;
let keymapVersion = 0;
const keymapListeners = new Set<() => void>();

/** Rebuilds KEYMAP from the defaults plus `overrides` (no-op for the overrides already applied). */
export function applyKeymapOverrides(overrides: KeymapOverrides): void {
  if (overrides === appliedOverrides) return;
  appliedOverrides = overrides;
  KEYMAP.splice(0, KEYMAP.length, ...resolveKeymap(DEFAULT_KEYMAP, overrides));
  keymapVersion++;
  keymapListeners.forEach((fn) => fn());
}

/** Calls `fn` after every keymap change. */
export function subscribeKeymap(fn: () => void): () => void {
  keymapListeners.add(fn);
  return () => void keymapListeners.delete(fn);
}

/** Bumped on every keymap change. */
export function getKeymapVersion(): number {
  return keymapVersion;
}

/** The keymap version, reactively: components that show shortcut labels re-render with it. */
export function useKeymapVersion(): number {
  return useSyncExternalStore(subscribeKeymap, getKeymapVersion);
}

/**
 * Applies the preference. Overrides saved by an older keymap are brought up to date first and
 * saved that way (which comes back here once, with nothing left to migrate).
 */
function syncFromPrefs(overrides: KeymapOverrides): void {
  const migrated = migrateKeymapOverrides(overrides);
  applyKeymapOverrides(migrated);
  if (migrated !== overrides) setPref("keymapOverrides", migrated);
}

// The preference is the source of truth; its localStorage mirror is read when prefs.ts loads, so
// even labels computed at import time see the user's bindings.
syncFromPrefs(getPrefs().keymapOverrides);
usePrefsStore.subscribe((s) => syncFromPrefs(s.prefs.keymapOverrides));
