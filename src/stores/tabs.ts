import { create, type StateCreator } from "zustand";
import { pendingCount } from "@/features/staging/model";
import { structurePendingCount, useStructureStore } from "@/features/structure/store";
import type { TableRef } from "@/lib/ipc";
import { useTableTabsStore } from "./tableTabs";

/**
 * A table's tab shows its rows ("table") or its structure ("structure"); the two views share the
 * tab and switch in place (`setView`). `preview`: the replaceable tab a single click in the sidebar
 * opens (italic title, TablePlus/VS Code style); editing, filtering or a double-click pins it.
 */
export type Tab =
  | {
      id: string;
      kind: "table";
      connectionId: string;
      table: TableRef;
      title: string;
      preview?: boolean;
    }
  | {
      id: string;
      kind: "structure";
      connectionId: string;
      table: TableRef;
      title: string;
      preview?: boolean;
    }
  | {
      id: string;
      kind: "query";
      connectionId: string;
      title: string;
      /** Set by the SQL editor while the buffer has unsaved edits. */
      dirty?: boolean;
    };

export type TabKind = Tab["kind"];
export type TableView = "table" | "structure";

export interface OpenTableOptions {
  /** Reuse the connection's preview tab (single click in the sidebar). Default: a pinned tab. */
  preview?: boolean;
}

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
 * `UnsavedChangesDialog` mounted by the app shell.
 */
export interface CloseRequest {
  /** "tabs": close `ids` on confirm. "window": the native window asked to close. */
  kind: "tabs" | "window";
  ids: string[];
  dirty: DirtyTab[];
  /** Sum of `dirty[].count`. */
  total: number;
  /** Tab to activate once the close went through ("Close Others" keeps and shows this one). */
  activateAfter?: string;
}

interface TabsState {
  tabs: Tab[];
  activeId: string | null;
  closed: Tab[];
  queryCounter: number;
  closeRequest: CloseRequest | null;
  /**
   * Tabs in the order they were activated, for the tab bar's ‹ › buttons (browser-style: a new
   * activation drops the forward entries). Closed tabs leave it; `historyIndex` is the current entry.
   */
  history: string[];
  historyIndex: number;
  open: (tab: Tab) => void;
  /**
   * Shows `table` in `kind` view: activates (and switches the view of) the table's tab when it is
   * open, else replaces the connection's preview tab (`preview`) or opens a new tab.
   */
  openTable: (
    connectionId: string,
    table: TableRef,
    kind?: TableView,
    options?: OpenTableOptions,
  ) => string;
  /** Switches a table tab between its rows and its structure. */
  setView: (id: string, kind: TableView) => void;
  /** Turns a preview tab into a regular one. */
  pin: (id: string) => void;
  newQuery: (connectionId: string) => string;
  /** Opens a fresh query tab on the same connection right after `id` (query tabs only). */
  duplicate: (id: string) => string | null;
  /** Guarded close of one tab (asks first when it holds unsaved work). */
  close: (id: string) => void;
  /** Closes the other tabs of `id`'s workspace (its connection). */
  closeOthers: (id: string) => void;
  /** Closes the tabs after `id` in its workspace. */
  closeToRight: (id: string) => void;
  /** Closes every tab of a workspace (default: the active tab's), or all when there is none. */
  closeAll: (connectionId?: string | null) => void;
  /**
   * Every user-initiated close goes through here. Closes immediately and returns true when none of
   * the tabs hold unsaved work; otherwise records a `closeRequest` for the dialog and returns false.
   * `activateAfter`: a surviving tab to activate once the close went through.
   */
  requestClose: (ids: string[], activateAfter?: string) => boolean;
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
  /** No tab active (the active connection has none open); the workspace shows its empty state. */
  clearActive: () => void;
  /**
   * ⌘1…⌘9, ⌘] / ⌘[: move within one workspace — the tabs of `connectionId` (default: the active
   * tab's connection), like TablePlus's per-connection workspaces.
   */
  activateIndex: (index: number, connectionId?: string | null) => void;
  next: (connectionId?: string | null) => void;
  prev: (connectionId?: string | null) => void;
  move: (from: number, to: number) => void;
  reopenClosed: () => void;
  rename: (id: string, title: string) => void;
  /** Shows the previously activated tab (‹ in the tab bar). */
  back: () => void;
  /** Undoes `back` (› in the tab bar). */
  forward: () => void;
  /**
   * Appends tabs rebuilt from the saved session (none of them active) and moves the "Query N"
   * counter past their titles.
   */
  restoreTabs: (tabs: readonly Tab[]) => void;
}

/**
 * What the window-close guard asks the session layer (installed by it): which tabs' unsaved work
 * the session keeps anyway (auto-saved SQL query tabs, restored on the next start), so closing the
 * window does not ask about them, and how to close once that work is written.
 */
export interface WindowCloseHooks {
  survivesClose: (tab: Tab, tabs: readonly Tab[]) => boolean;
  /** Starts writing the session now (the close dialog may close the window right after). */
  save: () => void;
  /** Writes the session, then closes the window (the guard was told not to close yet). */
  closeAfterSave: () => void;
}

let windowCloseHooks: WindowCloseHooks | null = null;

/** Installs the session's close hooks; returns the uninstall function. */
export function setWindowCloseHooks(hooks: WindowCloseHooks): () => void {
  windowCloseHooks = hooks;
  return () => {
    if (windowCloseHooks === hooks) windowCloseHooks = null;
  };
}

/** Highest N of the "Query N" titles in `tabs` (0 when there is none). */
export function maxQueryNumber(tabs: readonly Pick<Tab, "kind" | "title">[]): number {
  let n = 0;
  for (const t of tabs) {
    const m = t.kind === "query" ? /^Query (\d+)$/.exec(t.title) : null;
    if (m) n = Math.max(n, Number(m[1]));
  }
  return n;
}

const MAX_CLOSED = 20;
const MAX_HISTORY = 50;

const newId = () => crypto.randomUUID();

/**
 * The tabs of one workspace: a connection's tabs in strip order. A null connection means every tab
 * (nothing open yet, or a caller that does not scope).
 */
export function workspaceTabs(tabs: readonly Tab[], connectionId: string | null): Tab[] {
  return connectionId === null ? [...tabs] : tabs.filter((t) => t.connectionId === connectionId);
}

/** The workspace a tab action applies to: the given connection, else the active tab's. */
function scopeOf(
  s: { tabs: readonly Tab[]; activeId: string | null },
  connectionId: string | null | undefined,
): string | null {
  if (connectionId !== undefined) return connectionId;
  return s.tabs.find((t) => t.id === s.activeId)?.connectionId ?? null;
}

export function sameTable(a: TableRef, b: TableRef): boolean {
  return a.name === b.name && (a.schema ?? "") === (b.schema ?? "");
}

/**
 * Pending staged row edits plus staged DDL ops of a table tab (both views keep their work while the
 * other one shows), 1 for a dirty query tab.
 */
export function unsavedCount(tab: Tab): number {
  const extra = unsavedSources.reduce((n, count) => n + count(tab), 0);
  if (tab.kind === "query") return (tab.dirty ? 1 : 0) + extra;
  const slice = useTableTabsStore.getState().slices[tab.id];
  return (slice ? pendingCount(slice.staging) : 0) + structurePendingCount(tab.id) + extra;
}

/** Unsaved work counted by features this store cannot import (staged edits of query results). */
const unsavedSources: ((tab: Tab) => number)[] = [];

/** Adds a counter to `unsavedCount` (and so to every close / quit guard); returns its removal. */
export function registerUnsavedSource(count: (tab: Tab) => number): () => void {
  unsavedSources.push(count);
  return () => {
    const i = unsavedSources.indexOf(count);
    if (i >= 0) unsavedSources.splice(i, 1);
  };
}

/** Told about every close the user confirmed (Discard), before its tabs go. */
const confirmedCloseHooks: ((request: CloseRequest) => void)[] = [];

/** Adds a hook to `confirmClose` (drops work kept outside the tabs); returns its removal. */
export function onConfirmedClose(hook: (request: CloseRequest) => void): () => void {
  confirmedCloseHooks.push(hook);
  return () => {
    const i = confirmedCloseHooks.indexOf(hook);
    if (i >= 0) confirmedCloseHooks.splice(i, 1);
  };
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
 * nearest surviving tab of the same connection to its right, else to its left (TablePlus
 * behaviour). Null when that connection has no tab left (or nothing was active): the workspace
 * then shows the connection's empty state instead of jumping to another connection's tab.
 */
export function pickNeighbour(
  tabs: readonly Tab[],
  activeId: string | null,
  closing: ReadonlySet<string>,
): string | null {
  const index = tabs.findIndex((t) => t.id === activeId);
  const active = tabs[index];
  if (!active) return null;
  if (!closing.has(active.id)) return active.id;
  const survives = (t: Tab) => !closing.has(t.id) && t.connectionId === active.connectionId;
  for (let i = index + 1; i < tabs.length; i++) if (survives(tabs[i]!)) return tabs[i]!.id;
  for (let i = index - 1; i >= 0; i--) if (survives(tabs[i]!)) return tabs[i]!.id;
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

type HistoryState = Pick<TabsState, "tabs" | "activeId" | "history" | "historyIndex">;

/**
 * Drops closed tabs from the history (and the repeats that leaves behind). The current entry
 * becomes the nearest surviving one at or before it.
 */
function pruneHistory(
  history: readonly string[],
  index: number,
  tabs: readonly Tab[],
): { history: string[]; historyIndex: number } {
  const alive = new Set(tabs.map((t) => t.id));
  const out: string[] = [];
  let historyIndex = -1;
  history.forEach((id, i) => {
    if (!alive.has(id)) return;
    if (out[out.length - 1] !== id) out.push(id);
    if (i <= index) historyIndex = out.length - 1;
  });
  return { history: out, historyIndex };
}

/**
 * Records a state change in the back/forward history: closed tabs leave it, and a newly active
 * tab becomes its current entry. `back` / `forward` move through it themselves (they set
 * `historyIndex`), so they are passed through untouched.
 */
function withHistory(s: TabsState, patch: Partial<TabsState>): Partial<TabsState> {
  if ("historyIndex" in patch) return patch;
  let history = s.history;
  let historyIndex = s.historyIndex;
  if (patch.tabs && patch.tabs !== s.tabs)
    ({ history, historyIndex } = pruneHistory(history, historyIndex, patch.tabs));
  const activeId = patch.activeId;
  if (activeId && activeId !== history[historyIndex]) {
    history = [...history.slice(0, historyIndex + 1), activeId].slice(-MAX_HISTORY);
    historyIndex = history.length - 1;
  }
  if (history === s.history && historyIndex === s.historyIndex) return patch;
  return { ...patch, history, historyIndex };
}

/** Where `back` goes: the nearest earlier entry that is still open and not the visible tab. */
function backTarget(s: HistoryState): number {
  const tabs = new Set(s.tabs.map((t) => t.id));
  // With no tab showing (its last one closed), the current entry itself is "back".
  const from = s.history[s.historyIndex] === s.activeId ? s.historyIndex - 1 : s.historyIndex;
  for (let i = Math.min(from, s.history.length - 1); i >= 0; i--) {
    const id = s.history[i]!;
    if (tabs.has(id) && id !== s.activeId) return i;
  }
  return -1;
}

/** Where `forward` goes: the nearest later entry that is still open and not the visible tab. */
function forwardTarget(s: HistoryState): number {
  const tabs = new Set(s.tabs.map((t) => t.id));
  for (let i = Math.max(s.historyIndex + 1, 0); i < s.history.length; i++) {
    const id = s.history[i]!;
    if (tabs.has(id) && id !== s.activeId) return i;
  }
  return -1;
}

export const canGoBack = (s: HistoryState): boolean => backTarget(s) >= 0;
export const canGoForward = (s: HistoryState): boolean => forwardTarget(s) >= 0;

type SetTabs = (update: Partial<TabsState> | ((s: TabsState) => Partial<TabsState>)) => void;

/** Routes every change the store's own actions make through the history bookkeeping. */
const tracked =
  (creator: (set: SetTabs, get: () => TabsState) => TabsState): StateCreator<TabsState> =>
  (set, get) =>
    creator(
      (update) =>
        set((s) => {
          const patch = typeof update === "function" ? update(s) : update;
          return patch === s ? s : withHistory(s, patch);
        }),
      get,
    );

export const useTabsStore = create<TabsState>()(
  tracked((set, get) => ({
    tabs: [],
    activeId: null,
    closed: [],
    queryCounter: 0,
    closeRequest: null,
    history: [],
    historyIndex: -1,

    open: (tab) =>
      set((s) => ({
        tabs: s.tabs.some((t) => t.id === tab.id) ? s.tabs : [...s.tabs, tab],
        activeId: tab.id,
      })),

    openTable: (connectionId, table, kind = "table", options = {}) => {
      const s = get();
      const existing = s.tabs.find(
        (t) => t.kind !== "query" && t.connectionId === connectionId && sameTable(t.table, table),
      );
      if (existing && existing.kind !== "query") {
        // A single click keeps a preview tab a preview; any other open pins it.
        const updated = { ...existing, kind };
        if (!(options.preview && existing.preview)) delete updated.preview;
        set({
          tabs: s.tabs.map((t) => (t.id === existing.id ? updated : t)),
          activeId: existing.id,
        });
        return existing.id;
      }
      const id = newId();
      const tab: Tab = {
        id,
        kind,
        connectionId,
        table,
        title: table.name,
        ...(options.preview ? { preview: true } : {}),
      };
      const previous = options.preview
        ? s.tabs.find((t) => t.connectionId === connectionId && t.kind !== "query" && t.preview)
        : undefined;
      // The preview tab is replaced in place, unless it picked up work since it opened.
      if (previous && unsavedCount(previous) === 0) {
        dropTabState([previous.id]);
        set({
          tabs: s.tabs.map((t) => (t.id === previous.id ? tab : t)),
          activeId: id,
        });
        return id;
      }
      if (previous) get().pin(previous.id);
      get().open(tab);
      return id;
    },

    setView: (id, kind) =>
      set((s) => {
        const tab = s.tabs.find((t) => t.id === id);
        if (!tab || tab.kind === "query" || tab.kind === kind) return s;
        return { tabs: s.tabs.map((t) => (t.id === id ? { ...tab, kind } : t)) };
      }),

    pin: (id) =>
      set((s) => {
        const tab = s.tabs.find((t) => t.id === id);
        if (!tab || tab.kind === "query" || !tab.preview) return s;
        const pinned = { ...tab };
        delete pinned.preview;
        return { tabs: s.tabs.map((t) => (t.id === id ? pinned : t)) };
      }),

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
      const kept = get().tabs.find((t) => t.id === id);
      if (!kept) return;
      // The kept tab is the only survivor of its workspace, so it becomes active once the others are
      // gone (now, or when the unsaved-changes dialog is confirmed). `pickNeighbour` alone would
      // not pick it when another workspace's tab was active.
      get().requestClose(
        workspaceTabs(get().tabs, kept.connectionId)
          .filter((t) => t.id !== id)
          .map((t) => t.id),
        id,
      );
    },

    closeToRight: (id) => {
      const tab = get().tabs.find((t) => t.id === id);
      if (!tab) return;
      const tabs = workspaceTabs(get().tabs, tab.connectionId);
      get().requestClose(tabs.slice(tabs.indexOf(tab) + 1).map((t) => t.id));
    },

    closeAll: (connectionId) => {
      const s = get();
      void get().requestClose(workspaceTabs(s.tabs, scopeOf(s, connectionId)).map((t) => t.id));
    },

    requestClose: (ids, activateAfter) => {
      const s = get();
      if (s.closeRequest) return false;
      const wanted = new Set(ids);
      const targets = s.tabs.filter((t) => wanted.has(t.id));
      if (targets.length > 0) {
        const request = buildRequest("tabs", targets);
        if (request) {
          set({ closeRequest: activateAfter ? { ...request, activateAfter } : request });
          return false;
        }
        get().forceClose(targets.map((t) => t.id));
      }
      if (activateAfter) get().activate(activateAfter);
      return true;
    },

    requestWindowClose: () => {
      const s = get();
      const hooks = windowCloseHooks;
      const guarded = hooks ? s.tabs.filter((t) => !hooks.survivesClose(t, s.tabs)) : s.tabs;
      const request = buildRequest("window", guarded);
      if (!request) {
        // Only auto-saved query text is unsaved: write the session first, then close.
        if (hooks && guarded.length < s.tabs.length && dirtyTabs(s.tabs).length > 0) {
          hooks.closeAfterSave();
          return false;
        }
        return true;
      }
      hooks?.save();
      if (s.closeRequest?.kind !== "window") set({ closeRequest: request });
      return false;
    },

    confirmClose: () => {
      const request = get().closeRequest;
      if (!request) return null;
      set({ closeRequest: null });
      for (const hook of [...confirmedCloseHooks]) hook(request);
      if (request.kind === "tabs") {
        get().forceClose(request.ids);
        if (request.activateAfter) get().activate(request.activateAfter);
      }
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
      const closing = new Set(
        s.tabs.filter((t) => t.connectionId === connectionId).map((t) => t.id),
      );
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

    clearActive: () => set((s) => (s.activeId === null ? s : { activeId: null })),

    activateIndex: (index, connectionId) =>
      set((s) => {
        const tabs = workspaceTabs(s.tabs, scopeOf(s, connectionId));
        if (tabs.length === 0) return s;
        const i = index >= 9 ? tabs.length - 1 : Math.min(index, tabs.length - 1);
        return { activeId: tabs[i]?.id ?? s.activeId };
      }),

    next: (connectionId) =>
      set((s) => {
        const tabs = workspaceTabs(s.tabs, scopeOf(s, connectionId));
        if (tabs.length === 0) return s;
        const i = tabs.findIndex((t) => t.id === s.activeId);
        return { activeId: tabs[(i + 1) % tabs.length]?.id ?? s.activeId };
      }),

    prev: (connectionId) =>
      set((s) => {
        const tabs = workspaceTabs(s.tabs, scopeOf(s, connectionId));
        if (tabs.length === 0) return s;
        const i = tabs.findIndex((t) => t.id === s.activeId);
        return { activeId: tabs[(i - 1 + tabs.length) % tabs.length]?.id ?? s.activeId };
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
        let restored: Tab;
        if (tab.kind === "query") restored = { ...tab, dirty: false };
        else {
          const { preview: _preview, ...rest } = tab;
          restored = rest;
        }
        return {
          closed: s.closed.slice(0, -1),
          tabs: s.tabs.some((t) => t.id === tab.id) ? s.tabs : [...s.tabs, restored],
          activeId: tab.id,
        };
      }),

    rename: (id, title) =>
      set((s) => {
        const name = title.trim();
        const tab = s.tabs.find((t) => t.id === id);
        if (!tab || !name || tab.title === name) return s;
        return { tabs: s.tabs.map((t) => (t.id === id ? { ...t, title: name } : t)) };
      }),

    back: () =>
      set((s) => {
        const i = backTarget(s);
        return i < 0 ? s : { activeId: s.history[i]!, historyIndex: i };
      }),

    forward: () =>
      set((s) => {
        const i = forwardTarget(s);
        return i < 0 ? s : { activeId: s.history[i]!, historyIndex: i };
      }),

    restoreTabs: (restored) =>
      set((s) => {
        const known = new Set(s.tabs.map((t) => t.id));
        const added = restored.filter((t) => !known.has(t.id));
        if (added.length === 0) return s;
        return {
          tabs: [...s.tabs, ...added],
          queryCounter: Math.max(s.queryCounter, maxQueryNumber(added)),
        };
      }),
  })),
);

export function useActiveTab(): Tab | null {
  return useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
}
