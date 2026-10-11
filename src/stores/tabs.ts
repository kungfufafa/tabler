import { create } from "zustand";
import { pendingCount } from "@/features/staging/model";
import { structurePendingCount, useStructureStore } from "@/features/structure/store";
import type { TableRef } from "@/lib/ipc";
import { useTableTabsStore } from "./tableTabs";

export type Tab =
  | { id: string; kind: "table"; connectionId: string; table: TableRef; title: string }
  | { id: string; kind: "structure"; connectionId: string; table: TableRef; title: string }
  | {
      id: string;
      kind: "query";
      connectionId: string;
      title: string;
      /** Set by the SQL editor while the buffer has unsaved edits. */
      dirty?: boolean;
    };

export type TabKind = Tab["kind"];

/** A tab holding work that closing it would throw away. */
export interface DirtyTab {
  id: string;
  kind: TabKind;
  title: string;
  /** Staged row changes for table tabs; 1 for a dirty query tab. */
  count: number;
}

/**
 * A close that is waiting for the user to confirm discarding unsaved work. Rendered by the single
 * `UnsavedChangesDialog` mounted in the tab strip.
 */
export interface CloseRequest {
  /** "tabs": close `ids` on confirm. "window": the native window asked to close. */
  kind: "tabs" | "window";
  ids: string[];
  dirty: DirtyTab[];
  /** Sum of `dirty[].count`. */
  total: number;
}

interface TabsState {
  tabs: Tab[];
  activeId: string | null;
  closed: Tab[];
  queryCounter: number;
  closeRequest: CloseRequest | null;
  open: (tab: Tab) => void;
  openTable: (connectionId: string, table: TableRef, kind?: "table" | "structure") => string;
  newQuery: (connectionId: string) => string;
  /** Opens a fresh query tab on the same connection right after `id` (query tabs only). */
  duplicate: (id: string) => string | null;
  /** Guarded close of one tab (asks first when it holds unsaved work). */
  close: (id: string) => void;
  closeOthers: (id: string) => void;
  closeToRight: (id: string) => void;
  closeAll: () => void;
  /**
   * Every user-initiated close goes through here. Closes immediately and returns true when none of
   * the tabs hold unsaved work; otherwise records a `closeRequest` for the dialog and returns false.
   */
  requestClose: (ids: string[]) => boolean;
  /** Same check for the whole window. True when the window may close right away. */
  requestWindowClose: () => boolean;
  /** Discards the pending request's work; closes its tabs (kind "tabs") and returns the request. */
  confirmClose: () => CloseRequest | null;
  cancelClose: () => void;
  /** Unguarded removal (drops staged edits). Prefer `requestClose`. */
  forceClose: (ids: string[]) => void;
  /** Connection went away: drops its tabs (and their history) without asking. */
  closeForConnection: (connectionId: string) => void;
  /** Points every tab of `oldId` at `newId` (after a reconnect opened a fresh connection). */
  retargetConnection: (oldId: string, newId: string) => void;
  setDirty: (id: string, dirty: boolean) => void;
  activate: (id: string) => void;
  activateIndex: (index: number) => void;
  next: () => void;
  prev: () => void;
  move: (from: number, to: number) => void;
  reopenClosed: () => void;
  rename: (id: string, title: string) => void;
}

const MAX_CLOSED = 20;

const newId = () => crypto.randomUUID();

export function sameTable(a: TableRef, b: TableRef): boolean {
  return a.name === b.name && (a.schema ?? "") === (b.schema ?? "");
}

/**
 * Pending staged edits of a table tab, staged DDL ops of a structure tab, 1 for a dirty query tab.
 */
export function unsavedCount(tab: Tab): number {
  if (tab.kind === "query") return tab.dirty ? 1 : 0;
  if (tab.kind === "structure") return structurePendingCount(tab.id);
  const slice = useTableTabsStore.getState().slices[tab.id];
  return slice ? pendingCount(slice.staging) : 0;
}

/** Drops the per-tab state (staged rows, staged DDL) of tabs that are going away. */
function dropTabState(ids: readonly string[]): void {
  if (ids.length === 0) return;
  const removeSlice = useTableTabsStore.getState().remove;
  for (const id of ids) removeSlice(id);
  useStructureStore.getState().remove(ids);
}

export function dirtyTabs(tabs: readonly Tab[]): DirtyTab[] {
  const out: DirtyTab[] = [];
  for (const t of tabs) {
    const count = unsavedCount(t);
    if (count > 0) out.push({ id: t.id, kind: t.kind, title: t.title, count });
  }
  return out;
}

/** True when any open tab holds unsaved work (used by the window-close guard). */
export function hasUnsaved(): boolean {
  return useTabsStore.getState().tabs.some((t) => unsavedCount(t) > 0);
}

/**
 * The tab to activate after closing `closing`: the active tab if it survives, otherwise the
 * nearest surviving tab to its right, else to its left (TablePlus behaviour).
 */
export function pickNeighbour(
  tabs: readonly Tab[],
  activeId: string | null,
  closing: ReadonlySet<string>,
): string | null {
  const index = tabs.findIndex((t) => t.id === activeId);
  if (index >= 0 && !closing.has(activeId!)) return activeId;
  if (index < 0) return tabs.filter((t) => !closing.has(t.id)).at(-1)?.id ?? null;
  for (let i = index + 1; i < tabs.length; i++) if (!closing.has(tabs[i]!.id)) return tabs[i]!.id;
  for (let i = index - 1; i >= 0; i--) if (!closing.has(tabs[i]!.id)) return tabs[i]!.id;
  return null;
}

function buildRequest(kind: CloseRequest["kind"], targets: readonly Tab[]): CloseRequest | null {
  const dirty = dirtyTabs(targets);
  if (dirty.length === 0) return null;
  return {
    kind,
    ids: targets.map((t) => t.id),
    dirty,
    total: dirty.reduce((n, d) => n + d.count, 0),
  };
}

export const useTabsStore = create<TabsState>()((set, get) => ({
  tabs: [],
  activeId: null,
  closed: [],
  queryCounter: 0,
  closeRequest: null,

  open: (tab) =>
    set((s) => ({
      tabs: s.tabs.some((t) => t.id === tab.id) ? s.tabs : [...s.tabs, tab],
      activeId: tab.id,
    })),

  openTable: (connectionId, table, kind = "table") => {
    const existing = get().tabs.find(
      (t) => t.kind === kind && t.connectionId === connectionId && sameTable(t.table, table),
    );
    if (existing) {
      set({ activeId: existing.id });
      return existing.id;
    }
    const id = newId();
    get().open({ id, kind, connectionId, table, title: table.name });
    return id;
  },

  newQuery: (connectionId) => {
    const id = newId();
    const n = get().queryCounter + 1;
    set({ queryCounter: n });
    get().open({ id, kind: "query", connectionId, title: `Query ${n}` });
    return id;
  },

  duplicate: (id) => {
    const s = get();
    const index = s.tabs.findIndex((t) => t.id === id);
    const source = s.tabs[index];
    if (!source || source.kind !== "query") return null;
    const n = s.queryCounter + 1;
    const tab: Tab = {
      id: newId(),
      kind: "query",
      connectionId: source.connectionId,
      title: `Query ${n}`,
    };
    const tabs = [...s.tabs];
    tabs.splice(index + 1, 0, tab);
    set({ tabs, activeId: tab.id, queryCounter: n });
    return tab.id;
  },

  close: (id) => void get().requestClose([id]),

  closeOthers: (id) => {
    const tabs = get().tabs;
    if (!tabs.some((t) => t.id === id)) return;
    // The kept tab is the only survivor, so it becomes active once the others are gone.
    get().requestClose(tabs.filter((t) => t.id !== id).map((t) => t.id));
  },

  closeToRight: (id) => {
    const tabs = get().tabs;
    const index = tabs.findIndex((t) => t.id === id);
    if (index < 0) return;
    get().requestClose(tabs.slice(index + 1).map((t) => t.id));
  },

  closeAll: () => void get().requestClose(get().tabs.map((t) => t.id)),

  requestClose: (ids) => {
    const s = get();
    if (s.closeRequest) return false;
    const wanted = new Set(ids);
    const targets = s.tabs.filter((t) => wanted.has(t.id));
    if (targets.length === 0) return true;
    const request = buildRequest("tabs", targets);
    if (request) {
      set({ closeRequest: request });
      return false;
    }
    get().forceClose(targets.map((t) => t.id));
    return true;
  },

  requestWindowClose: () => {
    const s = get();
    const request = buildRequest("window", s.tabs);
    if (!request) return true;
    if (s.closeRequest?.kind !== "window") set({ closeRequest: request });
    return false;
  },

  confirmClose: () => {
    const request = get().closeRequest;
    if (!request) return null;
    set({ closeRequest: null });
    if (request.kind === "tabs") get().forceClose(request.ids);
    return request;
  },

  cancelClose: () => set({ closeRequest: null }),

  forceClose: (ids) => {
    const closing = new Set(ids);
    const s = get();
    const removed = s.tabs.filter((t) => closing.has(t.id));
    if (removed.length === 0) return;
    dropTabState(removed.map((t) => t.id));
    set({
      tabs: s.tabs.filter((t) => !closing.has(t.id)),
      activeId: pickNeighbour(s.tabs, s.activeId, closing),
      closed: [...s.closed, ...removed].slice(-MAX_CLOSED),
    });
  },

  closeForConnection: (connectionId) => {
    const s = get();
    const closing = new Set(s.tabs.filter((t) => t.connectionId === connectionId).map((t) => t.id));
    dropTabState([...closing]);
    const request = s.closeRequest;
    const ids = request?.ids.filter((id) => !closing.has(id)) ?? [];
    const dirty = request?.dirty.filter((d) => !closing.has(d.id)) ?? [];
    set({
      tabs: s.tabs.filter((t) => !closing.has(t.id)),
      activeId: pickNeighbour(s.tabs, s.activeId, closing),
      closed: s.closed.filter((t) => t.connectionId !== connectionId),
      closeRequest:
        request && ids.length > 0 && dirty.length > 0
          ? { ...request, ids, dirty, total: dirty.reduce((n, d) => n + d.count, 0) }
          : null,
    });
  },

  retargetConnection: (oldId, newId) => {
    if (oldId === newId) return;
    const retarget = (t: Tab): Tab =>
      t.connectionId === oldId ? { ...t, connectionId: newId } : t;
    set((s) => ({ tabs: s.tabs.map(retarget), closed: s.closed.map(retarget) }));
  },

  setDirty: (id, dirty) =>
    set((s) => {
      const tab = s.tabs.find((t) => t.id === id);
      if (!tab || tab.kind !== "query" || (tab.dirty ?? false) === dirty) return s;
      return { tabs: s.tabs.map((t) => (t.id === id ? { ...tab, dirty } : t)) };
    }),

  activate: (id) => set((s) => (s.tabs.some((t) => t.id === id) ? { activeId: id } : s)),

  activateIndex: (index) =>
    set((s) => {
      if (s.tabs.length === 0) return s;
      const i = index >= 9 ? s.tabs.length - 1 : Math.min(index, s.tabs.length - 1);
      return { activeId: s.tabs[i]?.id ?? s.activeId };
    }),

  next: () =>
    set((s) => {
      if (s.tabs.length === 0) return s;
      const i = s.tabs.findIndex((t) => t.id === s.activeId);
      return { activeId: s.tabs[(i + 1) % s.tabs.length]?.id ?? s.activeId };
    }),

  prev: () =>
    set((s) => {
      if (s.tabs.length === 0) return s;
      const i = s.tabs.findIndex((t) => t.id === s.activeId);
      return { activeId: s.tabs[(i - 1 + s.tabs.length) % s.tabs.length]?.id ?? s.activeId };
    }),

  move: (from, to) =>
    set((s) => {
      if (from === to || from < 0 || to < 0 || from >= s.tabs.length || to >= s.tabs.length)
        return s;
      const tabs = [...s.tabs];
      const [moved] = tabs.splice(from, 1);
      tabs.splice(to, 0, moved!);
      return { tabs };
    }),

  reopenClosed: () =>
    set((s) => {
      const tab = s.closed[s.closed.length - 1];
      if (!tab) return s;
      const restored: Tab = tab.kind === "query" ? { ...tab, dirty: false } : tab;
      return {
        closed: s.closed.slice(0, -1),
        tabs: s.tabs.some((t) => t.id === tab.id) ? s.tabs : [...s.tabs, restored],
        activeId: tab.id,
      };
    }),

  rename: (id, title) =>
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === id ? { ...t, title } : t)) })),
}));

export function useActiveTab(): Tab | null {
  return useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
}
