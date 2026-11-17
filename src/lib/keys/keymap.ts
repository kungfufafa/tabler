import { useSyncExternalStore } from "react";
import type { ActionId } from "@/features/palette/actions";
import { isWindows } from "@/lib/platform";
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
    label: "New Query Tab",
    allowInEditor: true,
  },
  // TablePlus: ⌘E opens a SQL editor, from the SQL editor too (Explain is ⌥⌘E, editor scope).
  {
    id: "tab.new-query",
    keys: "Mod+E",
    scope: "global",
    label: "Open SQL Editor",
    allowInEditor: true,
  },
  // TablePlus: ⌘↩ from a non-SQL tab opens the SQL editor (in a grid it is Quick Look, in the
  // filter bar Apply, in the editor Run Current; the sidebar's lists open their item with it).
  {
    id: "query.open-editor",
    keys: "Mod+Enter",
    scope: "global",
    label: "Open SQL Editor (non-SQL tab)",
    unscopedOnly: true,
  },
  {
    id: "tab.close",
    keys: "Mod+W",
    scope: "global",
    label: "Close Tab",
    allowInEditor: true,
    allowInDialog: true,
  },
  {
    id: "tab.close-all",
    keys: "Mod+Shift+W",
    scope: "global",
    label: "Close All Tabs",
    allowInEditor: true,
  },
  {
    id: "tab.reopen",
    keys: "Mod+Shift+T",
    scope: "global",
    label: "Reopen Closed Tab",
    allowInEditor: true,
  },
  { id: "tab.next", keys: "Mod+]", scope: "global", label: "Next Tab", allowInEditor: true },
  { id: "tab.prev", keys: "Mod+[", scope: "global", label: "Previous Tab", allowInEditor: true },
  {
    id: "tab.next",
    keys: "Ctrl+Tab",
    nonMacKeys: "Mod+Tab",
    scope: "global",
    label: "Next Tab",
    allowInEditor: true,
  },
  {
    id: "tab.prev",
    keys: "Ctrl+Shift+Tab",
    nonMacKeys: "Mod+Shift+Tab",
    scope: "global",
    label: "Previous Tab",
    allowInEditor: true,
  },
  // TablePlus: ⇧⌘] / ⇧⌘[ select the next / previous pane (Keymap ▸ Navigate).
  {
    id: "ui.next-pane",
    keys: "Mod+Shift+]",
    scope: "global",
    label: "Select Next Pane",
    allowInEditor: true,
  },
  {
    id: "ui.prev-pane",
    keys: "Mod+Shift+[",
    scope: "global",
    label: "Select Previous Pane",
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
    label: "Open Anything",
    allowInEditor: true,
  },
  {
    id: "palette.databases",
    keys: "Mod+K",
    scope: "global",
    label: "Switch Database",
    allowInEditor: true,
  },
  {
    id: "palette.connections",
    keys: "Mod+Shift+K",
    scope: "global",
    label: "Switch Connection",
    allowInEditor: true,
  },
  { id: "settings.open", keys: "Mod+,", scope: "global", label: "Settings", allowInEditor: true },
  {
    id: "ui.toggle-sidebar",
    keys: "Mod+0",
    scope: "global",
    label: "Toggle Sidebar",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-sidebar",
    keys: "Mod+B",
    scope: "global",
    label: "Toggle Sidebar",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-inspector",
    keys: "Mod+Shift+I",
    scope: "global",
    label: "Toggle Row Detail Panel",
    allowInEditor: true,
  },
  {
    id: "ui.toggle-console",
    keys: "Mod+Shift+C",
    scope: "global",
    label: "Toggle Console Log",
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
    label: "Search for Item",
    allowInEditor: true,
  },
  // TablePlus: ⌘R reloads the whole workspace (fixed; from the SQL editor too), ⌥⌘R only the
  // current tab.
  {
    id: "workspace.reload",
    keys: "Mod+R",
    scope: "global",
    label: "Reload Workspace",
    allowInEditor: true,
  },
  { id: "view.refresh", keys: "Mod+Alt+R", scope: "global", label: "Reload Current Tab" },
  // Switching the view unmounts an open cell editor before it could stage its value.
  {
    id: "view.data",
    keys: "Mod+Ctrl+[",
    nonMacKeys: "Mod+Alt+[",
    scope: "global",
    label: "Show Data",
    notWhileEditing: true,
  },
  {
    id: "view.structure",
    keys: "Mod+Ctrl+]",
    nonMacKeys: "Mod+Alt+]",
    scope: "global",
    label: "Show Structure",
    notWhileEditing: true,
  },
  // TablePlus: ⌘N opens a new window; File ▸ New Connection… has no key.
  {
    id: "window.new",
    keys: "Mod+N",
    scope: "global",
    label: "New Window",
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
    label: "Process List",
    allowInEditor: true,
  },
  // Staging works from anywhere in the tab (toolbar, filter bar…), not only with the grid focused.
  // A cell editor takes ⌘S itself (it stages what was typed first, then commits).
  {
    id: "grid.commit",
    keys: "Mod+S",
    scope: "global",
    label: "Commit Changes",
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
    label: "Discard All Changes",
    notWhileEditing: true,
    allowInEditor: true,
  },
  // Opens the filter bar (Esc closes it) on the last applied filter's value.
  { id: "grid.filter", keys: "Mod+F", scope: "grid", label: "Filter Rows" },
  { id: "grid.columns", keys: "Mod+Alt+F", scope: "global", label: "Show / Hide Columns" },
  { id: "grid.new-row", keys: "Mod+I", scope: "grid", label: "New Row" },
  { id: "grid.delete-rows", keys: "Mod+Backspace", scope: "grid", label: "Delete Selected Rows" },
  { id: "grid.duplicate-row", keys: "Mod+D", scope: "grid", label: "Duplicate Row" },
  { id: "grid.set-null", keys: "Mod+Shift+N", scope: "grid", label: "Set NULL" },
  // Tabler's own key; ⇧⌘D is TablePlus's Split Right. macOS keeps ⌥⌘D for Show / Hide Dock (turn
  // that off in System Settings ▸ Keyboard ▸ Keyboard Shortcuts, or use the second key).
  { id: "grid.set-default", keys: "Mod+Alt+D", scope: "grid", label: "Set DEFAULT" },
  { id: "grid.set-default", keys: "Mod+Alt+Shift+D", scope: "grid", label: "Set DEFAULT" },
  { id: "grid.open-fk", keys: "Mod+J", scope: "grid", label: "Open Referenced Row" },
  { id: "grid.undo", keys: "Mod+Z", scope: "grid", label: "Undo Staged Change" },
  { id: "grid.redo", keys: "Mod+Shift+Z", scope: "grid", label: "Redo Staged Change" },
  { id: "grid.toggle-detail", keys: "Space", scope: "grid", label: "Toggle Row Detail" },
  { id: "grid.copy", keys: "Mod+C", scope: "grid", label: "Copy Selected Rows" },
  { id: "grid.copy-as", keys: "Mod+Alt+C", scope: "grid", label: "Copy Rows As…" },
  // ⇧⌘C is two things in TablePlus's own docs: the console log, and Copy Cell Value. Scope settles
  // it: in a grid it copies the focused cell's value, everywhere else it toggles the console.
  { id: "grid.copy-cell", keys: "Mod+Shift+C", scope: "grid", label: "Copy Cell Value" },
  { id: "grid.paste", keys: "Mod+V", scope: "grid", label: "Paste" },
  // In a grid ⌘↩ is Quick Look (TablePlus); elsewhere it keeps its own meaning.
  { id: "grid.quick-look", keys: "Mod+Enter", scope: "grid", label: "Quick Look Editor" },
  { id: "grid.set-value", keys: "Alt+Enter", scope: "grid", label: "Set Value…" },
  { id: "grid.prev-page", keys: "Mod+Left", scope: "grid", label: "Previous Page" },
  { id: "grid.next-page", keys: "Mod+Right", scope: "grid", label: "Next Page" },
  // The filter bar above the grid (TablePlus's keys); ↩ in a value applies too.
  { id: "filter.apply", keys: "Mod+Enter", scope: "filter", label: "Apply All Filters" },
  { id: "filter.duplicate", keys: "Mod+I", scope: "filter", label: "Duplicate Filter" },
  { id: "filter.remove", keys: "Mod+Shift+I", scope: "filter", label: "Remove Filter" },
  {
    id: "filter.apply-or",
    keys: "Mod+Shift+Enter",
    scope: "filter",
    label: "Apply All Filters with OR",
  },
  { id: "filter.prev", keys: "Mod+Up", scope: "filter", label: "Previous Filter" },
  { id: "filter.next", keys: "Mod+Down", scope: "filter", label: "Next Filter" },
  { id: "filter.toggle", keys: "Mod+B", scope: "filter", label: "Check / Uncheck Filter" },
  { id: "filter.column", keys: "Mod+Left", scope: "filter", label: "Choose Filter Column" },
  { id: "filter.operator", keys: "Mod+Right", scope: "filter", label: "Choose Filter Operator" },
  {
    id: "query.run-current",
    keys: "Mod+Enter",
    scope: "editor",
    label: "Run Current Statement",
  },
  { id: "query.run", keys: "Mod+Shift+Enter", scope: "editor", label: "Run All" },
  { id: "query.cancel", keys: "Mod+.", scope: "editor", label: "Cancel Running Query" },
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
    label: "Toggle Result Pane",
    allowInEditor: true,
  },
  { id: "query.history", keys: "Mod+H", scope: "editor", label: "Query History" },
  { id: "query.save", keys: "Mod+S", scope: "editor", label: "Save Query to File" },
  {
    id: "export.open",
    keys: "Mod+Shift+E",
    scope: "global",
    label: "Export…",
    allowInEditor: true,
  },
  { id: "query.open", keys: "Mod+O", scope: "editor", label: "Open .sql File" },
  { id: "query.add-to-queries", keys: "Mod+Alt+S", scope: "editor", label: "Add to Queries…" },
  {
    id: "query.save-selection",
    keys: "Mod+Shift+S",
    scope: "editor",
    label: "Save Selected SQL Statements As…",
  },
  { id: "query.font-bigger", keys: "Mod+=", scope: "editor", label: "Increase Font Size" },
  { id: "query.font-bigger", keys: "Mod+Shift+=", scope: "editor", label: "Increase Font Size" },
  { id: "query.font-smaller", keys: "Mod+-", scope: "editor", label: "Decrease Font Size" },
  // Keymap ▸ SQL Editor: the comment toggles run in the focused editor (CodeMirror commands).
  {
    id: "query.comment-line",
    keys: "Mod+/",
    scope: "editor",
    label: "Toggle Line Comment",
    editorCommand: true,
  },
  {
    id: "query.comment-block",
    keys: "Mod+Alt+/",
    scope: "editor",
    label: "Toggle Block Comment",
    editorCommand: true,
  },
  { id: "query.split", keys: "Mod+Shift+D", scope: "editor", label: "Split Pane Horizontally" },
  // Outside the SQL editor (where the editor scope's Split Pane wins) ⇧⌘D splits the workspace,
  // from the grid, the sidebar, the filter bar and the structure tab as well. Not from a cell /
  // inspector editor (grid-edit: splitting remounts the tab body under the open edit).
  {
    id: "workspace.split-right",
    keys: "Mod+Shift+D",
    scope: "global",
    label: "Split Right",
    notWhileEditing: true,
  },
  // Workspaces (TablePlus ⇧⌘\ and Keymap ▸ Navigate) and the tab bar's ‹ › history.
  {
    id: "workspace.show",
    keys: "Mod+Shift+\\",
    scope: "global",
    label: "Show Workspaces",
    allowInEditor: true,
  },
  {
    id: "workspace.next",
    keys: "Mod+Ctrl+Right",
    nonMacKeys: "Mod+Alt+Right",
    scope: "global",
    label: "Select Next Workspace",
    allowInEditor: true,
  },
  {
    id: "workspace.prev",
    keys: "Mod+Ctrl+Left",
    nonMacKeys: "Mod+Alt+Left",
    scope: "global",
    label: "Select Previous Workspace",
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
    label: "Manage Plugins…",
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
    label: "Go Back",
    allowInEditor: true,
  },
  {
    id: "tab.forward",
    keys: "Ctrl+Shift+-",
    nonMacKeys: "Alt+Right",
    scope: "global",
    label: "Go Forward",
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

/**
 * Bindings only Windows has. Its windows have no menu bar (the ⋮ button holds File, Edit,
 * View…), so F10, the Windows key for "go to the menu", pops that menu. Not a bare Alt: an Alt
 * key-up in a webview clashes with Alt+key shortcuts, Alt+click and AltGr on many layouts.
 */
export const WINDOWS_KEYMAP: readonly ShortcutDef[] = [
  {
    id: "ui.app-menu",
    keys: "F10",
    scope: "global",
    label: "Show App Menu",
    allowInEditor: true,
  },
];

/** The effective keymap: this platform's built-in bindings, then the user's overrides. */
export const KEYMAP: ShortcutDef[] = [
  ...platformKeymap(BUILTIN_KEYMAP, isMac),
  ...(isWindows ? WINDOWS_KEYMAP : []),
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
