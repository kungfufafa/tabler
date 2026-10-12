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
  "tab.new-query",
  "query.open",
  "tab.close",
  "tab.reopen",
  "export.open",
  "ui.launcher",
  "palette.open",
  "ui.toggle-sidebar",
  "ui.toggle-inspector",
  "view.refresh",
] as const satisfies readonly ActionId[];

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
 * File…) do not reach into a dialog either.
 *
 * This matters because the page's shortcut listener leaves a key it suppresses unhandled, and
 * WKWebView then hands that key equivalent to the menu: without this check ⌘T in the connection
 * form would open a query tab (unmounting the form), ⌘R would refresh the grid behind the Preview
 * SQL sheet, and so on. A mouse click on the menu while a dialog is open is ignored the same way.
 */
export function menuActionAllowed(id: MenuActionId, scope: Scope): boolean {
  const defs = KEYMAP.filter((d) => d.id === id);
  if (scope === "dialog") return defs.some((d) => d.allowInDialog);
  if (scope === "editor") return defs.some((d) => d.scope === "editor" || d.allowInEditor);
  return true;
}

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
 * Routes the native menu bar (File / View / App items with their accelerators) into the action
 * registry. A menu accelerator only reaches here when the page did not handle the key itself
 * (WKWebView offers key equivalents to the page first), which includes keys the page suppresses
 * in dialogs and editors, so `dispatchMenuAction` applies those rules again. Mount once.
 */
export function useNativeMenu(): void {
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
