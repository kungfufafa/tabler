import { useEffect } from "react";

export type TabGoto = `tab.goto-${1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9}`;

export type ActionId =
  | "tab.new-query"
  | "tab.close"
  | "tab.close-all"
  | "tab.reopen"
  | "tab.next"
  | "tab.prev"
  /** The tab bar's ‹ › (tab history). */
  | "tab.back"
  | "tab.forward"
  /** Show Workspaces (⇧⌘\): the open workspaces, to switch to one. */
  | "workspace.show"
  | "workspace.next"
  | "workspace.prev"
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
  /** The Quick Look Editor on the focused cell (⌘↩ in a grid, middle-click). */
  | "grid.quick-look"
  /** The Set Value menu on the focused cell (⌥↩). */
  | "grid.set-value"
  /** Table grids: the previous / next page (⌘← / ⌘→). */
  | "grid.prev-page"
  | "grid.next-page"
  /** Filter bar: apply every checked filter, duplicate / remove the focused one. */
  | "filter.apply"
  | "filter.duplicate"
  | "filter.remove"
  /** Filter bar: apply every checked filter joined with OR (⇧⌘↩). */
  | "filter.apply-or"
  /** Filter bar: focus the previous / next filter row (⌘↑ / ⌘↓). */
  | "filter.prev"
  | "filter.next"
  /** Filter bar: check / uncheck the focused row (⌘B). */
  | "filter.toggle"
  /** Filter bar: open the focused row's column / operator popup (⌘← / ⌘→). */
  | "filter.column"
  | "filter.operator"
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
  /** "Add to Queries…": saves the selection / current statement as a saved query (⌥⌘S). */
  | "query.add-to-queries"
  /** "Save SQL As…": the editor text to a new file (the tab follows it). */
  | "query.save-as"
  /** "Save Selected SQL Statements As…" (⇧⌘S): the selection to a new file. */
  | "query.save-selection"
  /** SQL editor font size up / down (⌘+ / ⌘−). */
  | "query.font-bigger"
  | "query.font-smaller"
  /** Splits the query editor in two (⇧⌘D), or closes the split. */
  | "query.split"
  /** Connection ▸ Process List… (⌘.; inside the SQL editor ⌘. stays Cancel). */
  | "admin.process-list"
  /** Connection ▸ User Management… */
  | "admin.users"
  /** ⌘K database list: New Database… / Drop Database…; PostgreSQL New Schema… / Drop Schema…. */
  | "admin.new-database"
  | "admin.drop-database"
  | "admin.new-schema"
  | "admin.drop-schema"
  /** Connection ▸ Disconnect (asks first when tabs hold unsaved work). */
  | "connection.disconnect"
  /** File ▸ Backup… / Restore…: the backup window (pg_dump / mysqldump and friends). */
  | "backup.open"
  | "restore.open"
  | "export.open"
  /** File ▸ Import ▸ From CSV… / From SQL Dump… into the active connection. */
  | "import.csv"
  | "import.sql"
  /** Generate Diagram…: the ER diagram of the active connection's current schema. */
  | "diagram.open"
  /** Edit ▸ Set Value ▸ Now (local time) on the selected cells. */
  | "grid.set-now"
  /** Edit ▸ Toggle Line Comment (⌘/) / Toggle Block Comment (⌥⌘/) in the SQL editor. */
  | "query.comment-line"
  | "query.comment-block"
  /** Navigate ▸ Select Next / Previous Pane (⇧⌘] / ⇧⌘[): sidebar → tab → row detail → console. */
  | "ui.next-pane"
  | "ui.prev-pane";

export type ActionHandler = () => void | Promise<void>;

/**
 * Every user-visible behaviour is an action. Shortcuts, the command palette, context menus and the
 * native menu all dispatch here. Handlers stack: the most recently registered one wins (so the
 * focused tab can override a global default).
 */
class ActionRegistry {
  private handlers = new Map<ActionId, ActionHandler[]>();
  private observers = new Map<ActionId, Set<() => void>>();

  /**
   * Calls `fn` whenever `id` is dispatched (shortcut, toolbar, palette, native menu), before and
   * independently of its handler, e.g. the sidebar reloading its lists on `view.refresh`.
   */
  observe(id: ActionId, fn: () => void): () => void {
    const set = this.observers.get(id) ?? new Set();
    set.add(fn);
    this.observers.set(id, set);
    return () => {
      set.delete(fn);
      if (set.size === 0) this.observers.delete(id);
    };
  }

  /** Tells the observers of `id` it was dispatched (also when nothing handles it). */
  notify(id: ActionId): void {
    for (const fn of this.observers.get(id) ?? []) fn();
  }

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

  /**
   * Registers `handler` under every handler of `id`: a default that any other handler (the
   * focused tab's) overrides, whenever that one was registered.
   */
  registerFallback(id: ActionId, handler: ActionHandler): () => void {
    const list = this.handlers.get(id) ?? [];
    list.unshift(handler);
    this.handlers.set(id, list);
    return () => {
      const current = this.handlers.get(id);
      if (!current) return;
      const i = current.indexOf(handler);
      if (i >= 0) current.splice(i, 1);
      if (current.length === 0) this.handlers.delete(id);
    };
  }

  has(id: ActionId): boolean {
    return (this.handlers.get(id)?.length ?? 0) > 0;
  }

  async run(id: ActionId): Promise<boolean> {
    this.notify(id);
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
