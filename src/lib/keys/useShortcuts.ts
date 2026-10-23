import { useEffect, type ReactNode } from "react";
import { actions, type ActionId } from "@/features/palette/actions";
import { comboFromEvent } from "./combo";
import { findShortcut, type Scope, type ShortcutDef } from "./keymap";

/**
 * Open dialogs only. A Radix dialog that is closing stays mounted with `data-state="closed"` (and
 * the focus on its button) until its exit animation ends; it must not keep the app's shortcuts:
 * Esc then ⌘S right away has to commit.
 */
const DIALOGS = ":is([role='dialog'], [role='alertdialog']):not([data-state='closed'])";

/**
 * Resolves the keyboard scope from the focused element (`data-scope` attributes, CodeMirror,
 * dialogs). A CodeMirror editor inside a dialog (Bootstrap commands, a trigger's body) is the
 * dialog's: the SQL editor's keys (⌘↩ Run, ⌘O, ⌘W…) must not reach the tab behind it, and the
 * editor's own keymap (⌘↩ / ⌘S save) must get them. A dialog that shields the app's shortcuts
 * itself (Quick Look) opts in to the editor scope with `data-shortcut-scope="editor"`. A
 * CodeMirror that is a form field, not a SQL editor (the Details sidebar's), names its own scope
 * with `data-cm-scope` (`grid-edit` while it can be edited, like the native fields beside it).
 */
export function resolveScope(target: Element | null): Scope {
  if (!target) return "global";
  const dialog = target.closest(DIALOGS);
  if (target.closest(".cm-editor")) {
    const own = target.closest<HTMLElement>("[data-cm-scope]")?.dataset.cmScope as
      Scope | undefined;
    if (own) return own;
    if (!dialog) return "editor";
    const optIn = target.closest("[data-shortcut-scope='editor']");
    return optIn && dialog.contains(optIn) ? "editor" : "dialog";
  }
  if (dialog) return "dialog";
  const scoped = target.closest<HTMLElement>("[data-scope]");
  const scope = scoped?.dataset.scope as Scope | undefined;
  return scope ?? "global";
}

/** Text fields, selects and contenteditable regions: where a plain key types instead. */
function isTextEntry(el: Element | null): boolean {
  if (!el) return false;
  if (el instanceof HTMLElement && el.isContentEditable) return true;
  return el.matches("input, textarea, select");
}

let capturing = 0;

/**
 * Settings ▸ Shortcuts is recording a new shortcut: every key goes to the recorder, none runs its
 * action. Returns the function that ends it.
 */
export function beginShortcutCapture(): () => void {
  capturing++;
  let ended = false;
  return () => {
    if (ended) return;
    ended = true;
    capturing--;
  };
}

export function isCapturingShortcut(): boolean {
  return capturing > 0;
}

/**
 * Grid keys that stay with the grid's container when the grid has no handler: ⌘↩ in a SQL tab's
 * results is Run Current unless the grid binds Quick Look to it (`DataGrid` `quickLookKey`).
 */
const GRID_PASSTHROUGH: ReadonlySet<ActionId> = new Set(["grid.quick-look"]);

/**
 * Grid keys that share their chord with a global binding: with no grid handler to run, the global
 * action answers. ⇧⌘C copies the cell value in a grid, but a grid-scoped area with nothing to copy
 * (the structure tab) is still where the console log toggles.
 */
const FALLS_BACK_TO_GLOBAL: ReadonlySet<ActionId> = new Set(["grid.copy-cell"]);

/** ⌘R / ⌥⌘R: the webview would reload the page (and lose every buffer) if nothing stopped it. */
const RELOAD_KEYS: ReadonlySet<ActionId> = new Set(["workspace.reload", "view.refresh"]);

/** Whether an unhandled shortcut is still kept from the menu bar and the page (see the listener). */
export function blocksUnhandled(def: ShortcutDef, scope: Scope): boolean {
  return scope === "grid" && def.scope === "grid" && !GRID_PASSTHROUGH.has(def.id);
}

let dispatching = 0;

/**
 * Runs `id` as its keyboard shortcut. Its handler is called synchronously inside, so it can tell
 * (`isShortcutDispatch`) a key from the menu bar or the palette: ⌘. cancels the running query,
 * Connection ▸ Process List… always opens the list.
 */
export function runShortcut(id: ActionId): void {
  dispatching++;
  try {
    void actions.run(id);
  } finally {
    dispatching--;
  }
}

/** Whether the action handler running right now was started by its keyboard shortcut. */
export function isShortcutDispatch(): boolean {
  return dispatching > 0;
}

/** One capture-phase keydown listener for the whole app; dispatches into the action registry. */
export function KeyboardProvider({ children }: { children: ReactNode }) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.defaultPrevented || capturing > 0) return;
      const combo = comboFromEvent(e);
      const target = e.target instanceof Element ? e.target : document.activeElement;
      // Shortcuts without a modifier (Space for the row detail…) never steal typing.
      if (!e.metaKey && !e.ctrlKey && !e.altKey && isTextEntry(target)) return;
      const scope = resolveScope(target);
      const def = findShortcut(combo, scope);
      if (!def) return;
      // The focused CodeMirror editor runs it itself (Toggle Line / Block Comment): its keymap is
      // built from KEYMAP, so a remapped key still reaches the right editor pane.
      if (def.editorCommand && scope === "editor") return;
      if (!actions.has(def.id)) {
        if (FALLS_BACK_TO_GLOBAL.has(def.id)) {
          const global = findShortcut(combo, "global");
          if (global && actions.has(global.id)) {
            e.preventDefault();
            e.stopPropagation();
            runShortcut(global.id);
            return;
          }
        }
        // Nothing handles it yet; still stop the webview from doing something odd (⌘R reload, ⌘P print).
        if (RELOAD_KEYS.has(def.id) || def.id === "palette.open") e.preventDefault();
        // A grid key the focused grid refuses (⌘I New Row, ⌥⌘D Set DEFAULT in a read-only result)
        // is spent: neither the menu's key equivalent (Beautify, Split Pane) nor the query tab's
        // forwarding of editor keys may run it in the editor behind the grid.
        if (blocksUnhandled(def, scope)) e.preventDefault();
        actions.notify(def.id);
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      runShortcut(def.id);
    };
    window.addEventListener("keydown", handler, { capture: true });
    return () => window.removeEventListener("keydown", handler, { capture: true });
  }, []);
  return children;
}
