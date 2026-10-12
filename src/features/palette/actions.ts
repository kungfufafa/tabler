import { useEffect } from "react";

export type TabGoto = `tab.goto-${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9}`;

export type ActionId =
  | "tab.new-query"
  | "tab.close"
  | "tab.close-all"
  | "tab.reopen"
  | "tab.next"
  | "tab.prev"
  | TabGoto
  /** Open Anything (⌘P): tables, tabs, schemas, databases, connections and actions. */
  | "palette.open"
  /** The palette narrowed to the databases (and schemas) of the active connection (⌘K). */
  | "palette.databases"
  /** The palette narrowed to the saved connections (⇧⌘K). */
  | "palette.connections"
  | "settings.open"
  | "ui.toggle-sidebar"
  | "ui.toggle-inspector"
  | "ui.toggle-console"
  | "ui.launcher"
  | "sidebar.focus-search"
  | "view.refresh"
  /** Table tabs: show the rows (⌃⌘[) or the structure (⌃⌘]) in the same tab. */
  | "view.data"
  | "view.structure"
  | "connection.reconnect"
  /** Shows the welcome screen with the new-connection form (⌘N). */
  | "connection.new"
  | "grid.commit"
  /** Always opens the Preview SQL sheet, whatever "Confirm before commit" says. */
  | "grid.preview"
  | "grid.discard"
  | "grid.filter"
  /** Opens the toolbar's column visibility menu. */
  | "grid.columns"
  | "grid.new-row"
  | "grid.delete-rows"
  | "grid.duplicate-row"
  | "grid.set-null"
  | "grid.set-empty"
  | "grid.set-default"
  | "grid.open-fk"
  | "grid.undo"
  | "grid.redo"
  /** Shows / hides the row detail panel (the inspector on its Row view). */
  | "grid.toggle-detail"
  | "grid.copy"
  | "grid.copy-as"
  | "grid.paste"
  /** Filter bar: apply every checked filter, duplicate / remove the focused one. */
  | "filter.apply"
  | "filter.duplicate"
  | "filter.remove"
  | "query.run"
  | "query.run-current"
  | "query.cancel"
  | "query.explain"
  | "query.format"
  | "query.minify"
  | "query.toggle-results"
  | "query.history"
  | "query.save"
  | "query.open"
  | "export.open";

export type ActionHandler = () => void | Promise<void>;

/**
 * Every user-visible behaviour is an action. Shortcuts, the command palette, context menus and the
 * native menu all dispatch here. Handlers stack: the most recently registered one wins (so the
 * focused tab can override a global default).
 */
class ActionRegistry {
  private handlers = new Map<ActionId, ActionHandler[]>();

  register(id: ActionId, handler: ActionHandler): () => void {
    const list = this.handlers.get(id) ?? [];
    list.push(handler);
    this.handlers.set(id, list);
    return () => {
      const current = this.handlers.get(id);
      if (!current) return;
      const i = current.lastIndexOf(handler);
      if (i >= 0) current.splice(i, 1);
      if (current.length === 0) this.handlers.delete(id);
    };
  }

  has(id: ActionId): boolean {
    return (this.handlers.get(id)?.length ?? 0) > 0;
  }

  async run(id: ActionId): Promise<boolean> {
    const list = this.handlers.get(id);
    const handler = list?.[list.length - 1];
    if (!handler) return false;
    await handler();
    return true;
  }
}

export const actions = new ActionRegistry();

/** Registers a handler for the lifetime of the component (re-registers when `handler` changes). */
export function useAction(id: ActionId, handler: ActionHandler | null | undefined): void {
  useEffect(() => {
    if (!handler) return;
    return actions.register(id, handler);
  }, [id, handler]);
}
