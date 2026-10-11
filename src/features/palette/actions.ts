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
  | "palette.open"
  | "settings.open"
  | "ui.toggle-sidebar"
  | "ui.toggle-inspector"
  | "ui.launcher"
  | "sidebar.focus-search"
  | "view.refresh"
  | "grid.commit"
  /** Always opens the Preview SQL sheet, whatever "Confirm before commit" says (not in KEYMAP). */
  | "grid.preview"
  | "grid.discard"
  | "grid.filter"
  | "grid.new-row"
  | "grid.delete-rows"
  | "grid.duplicate-row"
  | "grid.set-null"
  | "grid.set-default"
  | "grid.open-fk"
  | "grid.undo"
  | "query.run"
  | "query.run-current"
  | "query.cancel"
  | "query.explain"
  | "query.format"
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
