import { useEffect } from "react";
import { actions, type ActionId } from "@/features/palette/actions";
import { listenEvent } from "@/lib/ipc/events";
import { KEYMAP, type Scope } from "@/lib/keys/keymap";
import { resolveScope } from "@/lib/keys/useShortcuts";

/**
 * Action ids the native menu bar can send (`menu:action`). Mirrors `menu_action` in
 * src-tauri/src/lib.rs; anything else is ignored.
 */
export const MENU_ACTIONS = [
  "settings.open",
  "connection.new",
  "tab.new-query",
  "query.open",
  "tab.close",
  "tab.close-all",
  "tab.reopen",
  "backup.open",
  "restore.open",
  "export.open",
  "ui.launcher",
  "palette.open",
  "palette.databases",
  "palette.connections",
  "view.data",
  "view.structure",
  "ui.toggle-sidebar",
  "ui.toggle-inspector",
  "ui.toggle-console",
  "view.refresh",
  "connection.reconnect",
  "workspace.show",
  "workspace.next",
  "workspace.prev",
  "admin.process-list",
  "admin.users",
  "connection.disconnect",
  "import.csv",
  "import.sql",
  // File: the SQL editor's save items.
  "query.save-as",
  "query.save-selection",
  "query.add-to-queries",
  // Edit: the SQL editor's items, then the data grid's.
  "query.format",
  "query.minify",
  "query.comment-line",
  "query.comment-block",
  "query.split",
  "grid.new-row",
  "grid.duplicate-row",
  "grid.delete-rows",
  "grid.set-null",
  "grid.set-empty",
  "grid.set-default",
  "grid.set-now",
  "grid.copy-as",
  "grid.quick-look",
  // View.
  "grid.columns",
  "grid.filter",
  // Navigate.
  "tab.back",
  "tab.forward",
  "tab.prev",
  "tab.next",
  "tab.goto-1",
  "tab.goto-2",
  "tab.goto-3",
  "tab.goto-4",
  "tab.goto-5",
  "tab.goto-6",
  "tab.goto-7",
  "tab.goto-8",
  "tab.goto-9",
  "ui.prev-pane",
  "ui.next-pane",
] as const satisfies readonly ActionId[];

/**
 * Menu items shown without their keys: their shortcut is context-dependent and the same keys run
 * another menu item (⌘I Beautify / Insert Row, ⌘D duplicate row / the editor's next occurrence,
 * ⇧⌘D Split Pane / Set DEFAULT). The key goes through the page's scope resolution instead (the
 * grid's keymap scope wins while the grid has the focus); the menu item only answers clicks. Their
 * accelerators are never synced (`installMenuAcceleratorSync`). Mirrors `PLAIN_MENU_ACTIONS`.
 */
export const MENU_UNBOUND_ACTIONS: readonly MenuActionId[] = [
  "grid.new-row",
  "grid.duplicate-row",
  "grid.set-default",
];

export type MenuActionId = (typeof MENU_ACTIONS)[number];

export function isMenuAction(id: unknown): id is MenuActionId {
  return typeof id === "string" && (MENU_ACTIONS as readonly string[]).includes(id);
}

/** Waits (a few frames at most) for a handler of `id` to register, e.g. in a tab that just opened. */
function whenRegistered(id: ActionId, attempts = 30): Promise<boolean> {
  return new Promise((resolve) => {
    const check = (left: number) => {
      if (actions.has(id)) resolve(true);
      else if (left <= 0) resolve(false);
      else setTimeout(() => check(left - 1), 16);
    };
    check(attempts);
  });
}

/**
 * Whether a menu item may run its action while focus is in `scope`, by the same rules as its
 * keyboard shortcut (`findShortcut`): global shortcuts are inert inside a dialog unless
 * `allowInDialog`, and inside CodeMirror unless `allowInEditor`; editor-scoped ones (Open SQL
 * File…) do not reach into a dialog either, not even from an editor inside one (`resolveScope`
 * gives it the dialog's scope); `notWhileEditing` ones (Data / Structure) wait while a cell editor
 * has the focus.
 *
 * This matters because the page's shortcut listener leaves a key it suppresses unhandled, and
 * WKWebView then hands that key equivalent to the menu: without this check ⌘T in the connection
 * form would open a query tab (unmounting the form), ⌘R would refresh the grid behind the Preview
 * SQL sheet, and so on. A mouse click on the menu while a dialog is open is ignored the same way.
 */
export function menuActionAllowed(id: MenuActionId, scope: Scope): boolean {
  const defs = KEYMAP.filter((d) => d.id === id);
  // The grid's items (Delete Rows ⌘⌫, Set Value ▸ …) act on its selection, like TablePlus's menu
  // on the focused view: not while a cell editor, the filter bar, the sidebar or the SQL editor
  // has the focus, where their keys edit text or mean something else (⌘⌫ drops sidebar tables).
  const gridOnly = id.startsWith("grid.") && defs.every((d) => d.scope === "grid");
  if (gridOnly && NOT_FOR_GRID_ITEMS.has(scope)) return false;
  // The Edit menu's SQL editor items (Beautify ⌘I, Split Pane ⇧⌘D…) change the editor: not from
  // a grid, the filter bar, the sidebar or a dialog, where their keys mean something else (⌘I
  // New Row, ⇧⌘D Set DEFAULT) and reach the menu only when the focused view refused them.
  if (EDITOR_EDIT_ITEMS.has(id) && NOT_FOR_EDITOR_ITEMS.has(scope)) return false;
  // Menu-only items (User Management…, Disconnect) have no key the page could have suppressed.
  if (defs.length === 0) return scope !== "dialog";
  if (scope === "dialog") return defs.some((d) => d.allowInDialog);
  if (scope === "editor") return defs.some((d) => d.scope === "editor" || d.allowInEditor);
  if (scope === "grid-edit") return !defs.every((d) => d.notWhileEditing);
  return true;
}

const NOT_FOR_GRID_ITEMS: ReadonlySet<Scope> = new Set([
  "grid-edit",
  "filter",
  "sidebar",
  "editor",
]);

/** Edit menu items that edit the SQL editor's text or layout (File ▸ Open / Save… do not). */
const EDITOR_EDIT_ITEMS: ReadonlySet<MenuActionId> = new Set([
  "query.format",
  "query.minify",
  "query.comment-line",
  "query.comment-block",
  "query.split",
]);

const NOT_FOR_EDITOR_ITEMS: ReadonlySet<Scope> = new Set([
  "grid",
  "grid-edit",
  "filter",
  "sidebar",
  "dialog",
]);

/**
 * Runs the action a menu item stands for, unless the focused dialog or editor suppresses it
 * (`menuActionAllowed`). "Open SQL File…" outside a query tab first opens a new query tab (on the
 * active connection) and then asks it for the file. Returns whether something handled it.
 */
export async function dispatchMenuAction(id: unknown): Promise<boolean> {
  if (!isMenuAction(id)) return false;
  if (!menuActionAllowed(id, resolveScope(document.activeElement))) return false;
  if (id === "query.open" && !actions.has(id)) {
    if (!(await actions.run("tab.new-query"))) return false;
    if (!(await whenRegistered(id))) return false;
  }
  return actions.run(id);
}

/**
 * Routes the native menu bar (App / File / Edit / View / Connection / Navigate / Window items with
 * their accelerators) into the action registry. A menu accelerator only reaches here when the
 * page did not handle the key itself (WKWebView offers key equivalents to the page first), which
 * includes keys the page suppresses in dialogs and editors, so `dispatchMenuAction` applies those
 * rules again. Mount once.
 */
export function useNativeMenu(): void {
  // Edit ▸ Toggle Line / Block Comment: their keys are the SQL editor's own keymap, so only the
  // menu items need a handler (loaded on first use: the editor controller is heavy).
  useEffect(() => {
    const comment = (kind: "line" | "block") => async () => {
      const { toggleCommentInActiveTab } = await import("@/features/editor/controller");
      toggleCommentInActiveTab(kind);
    };
    const off = [
      actions.register("query.comment-line", comment("line")),
      actions.register("query.comment-block", comment("block")),
    ];
    return () => off.forEach((u) => u());
  }, []);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenEvent("menu:action", (id) => void dispatchMenuAction(id)).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
