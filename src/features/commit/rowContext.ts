import type { BrowsePage, RowKey, TableRef } from "@/lib/ipc";

/**
 * What a table tab's staged rows were staged against: the page's columns and every fetched row's
 * backend key. Only the visible tab is mounted, while ⌘S commits the staged rows of EVERY tab
 * of the workspace: the tab records this whenever its page changes, and the workspace commit
 * builds the other tabs' requests from it.
 */
export interface RowContext {
  table: TableRef;
  columns: BrowsePage["columns"];
  /** The page's column names joined like the slice's `stagedOn`. */
  signature: string;
  identity: BrowsePage["identity"];
  /** Grid row key → backend row key (by PK, SQLite rowid, or every original column). */
  keys: ReadonlyMap<string, RowKey>;
}

const contexts = new Map<string, RowContext>();

export function setRowContext(tabId: string, ctx: RowContext): void {
  contexts.set(tabId, ctx);
}

export function rowContextOf(tabId: string): RowContext | undefined {
  return contexts.get(tabId);
}

/** Forgets the contexts of tabs that are gone (`keep`: the tab ids still around). */
export function retainRowContexts(keep: ReadonlySet<string>): void {
  for (const id of [...contexts.keys()]) if (!keep.has(id)) contexts.delete(id);
}

/** Test helper. */
export function resetRowContexts(): void {
  contexts.clear();
}
