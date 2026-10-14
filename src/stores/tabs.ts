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
    }
  | {
      id: string;
      /**
       * A feature's own tab (Item Overview, metrics…): `tool` names the component registered with
       * `registerToolTab` (src/features/tabs/toolTabs.ts), `params` is that tool's input.
       */
      kind: "tool";
      connectionId: string;
      tool: string;
      title: string;
      params?: unknown;
      /** Tool tabs are never preview tabs. */
      preview?: never;
    };

export type TabKind = Tab["kind"];
export type TableView = "table" | "structure";
/** A table's tab, in either view. */
export type TableTab = Extract<Tab, { kind: "table" | "structure" }>;
export type ToolTab = Extract<Tab, { kind: "tool" }>;

export function isTableTab(tab: Tab | null | undefined): tab is TableTab {
  return tab?.kind === "table" || tab?.kind === "structure";
}

/**
 * A workspace split in two tab groups (Split Right, ⇧⌘D): the tabs of `right` show in the right
 * group, the workspace's other tabs in the left one (each group in strip order). `shown`: the tab
 * each group shows (left, right); the active tab is the focused group's. The split closes by
 * itself once either group has no tab left.
 */
export interface Split {
  right: readonly string[];
  shown: readonly [string | null, string | null];
}

/** 0 = the left (or only) group, 1 = the right group. */
export type TabGroup = 0 | 1;

export interface OpenTableOptions {
  /** Reuse the connection's preview tab (single click in the sidebar). Default: a pinned tab. */
  preview?: boolean;
  /**
   * "Open in New Tab" (sidebar menu, middle-click, ⌘↩ / ⌘-click in Open Anything): always a fresh
   * pinned tab, even when the table is open already (one table can sit in several tabs, each with
   * its own filter / sort / staged edits). A preview tab is never reused for it.
   */
  newTab?: boolean;
}

/** A tab holding work that closing it would throw away. */
export interface DirtyTab {
  id: string;
  kind: TabKind;
  title: string;
  /** Staged row changes for table tabs; 1 for a dirty query tab. */
  count: number;
  /**
   * Set for work that belongs to a connection rather than to a tab (staged sidebar drops and
   * truncates): it survives closing tabs and is lost only on disconnect or quit.
   */
  connectionId?: string;
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
  /** Split workspaces, by connection id (see `Split`). */
  splits: Readonly<Record<string, Split>>;
  open: (tab: Tab) => void;
  /**
   * Shows `table` in `kind` view: activates (and switches the view of) the table's tab when it is
   * open, else replaces the connection's preview tab (`preview`) or opens a new tab. With
   * `newTab` it always opens a new pinned tab (several tabs of one table).
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
  /**
   * Duplicate Tab: a fresh query tab on the same connection right after `id`; for a table tab a
   * new pinned tab of the same table and view that starts with the source's filters, sort and
   * column state (never its staged edits). Tool tabs cannot be duplicated (null).
   */
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
  /**
   * Closes every tab of a connection before it disconnects or switches database: also asks for the
   * connection's own unsaved work (see `registerConnectionWork`), even when it has no tab open.
   */
  requestCloseConnection: (connectionId: string) => boolean;
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
  /**
   * Split Right: a copy of the tab (default: the active one) opens in the right group, which is
   * created when the workspace is not split yet. Table tabs show the same table and view, tool
   * tabs the same tool; a query tab gets a fresh query. Returns the new tab's id.
   */
  splitRight: (id?: string | null) => string | null;
  /**
   * Moves a tab to a group of its workspace (`before`: the tab it goes in front of, default the
   * group's end) and shows it there. Moving the only tab of a workspace to the right does nothing;
   * a group left without tabs closes the split.
   */
  moveToGroup: (id: string, group: TabGroup, before?: string | null) => void;
  /** Puts every tab of a split workspace back in one group. */
  unsplit: (connectionId: string) => void;
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

type GroupState = Pick<TabsState, "tabs" | "activeId" | "splits">;

/** The group a tab is in (0 when its workspace is not split). */
export function groupOf(
  splits: GroupState["splits"],
  tab: Pick<Tab, "id" | "connectionId">,
): TabGroup {
  return splits[tab.connectionId]?.right.includes(tab.id) ? 1 : 0;
}

/** The tabs of one group of a workspace, in strip order (all of them when it is not split). */
export function groupTabs(
  s: Pick<GroupState, "tabs" | "splits">,
  connectionId: string,
  group: TabGroup,
): Tab[] {
  const split = s.splits[connectionId];
  const own = workspaceTabs(s.tabs, connectionId);
  if (!split) return group === 0 ? own : [];
  return own.filter((t) => split.right.includes(t.id) === (group === 1));
}

/** The group the active tab is in, when it belongs to `connectionId` (else the left one). */
export function focusedGroup(s: GroupState, connectionId: string): TabGroup {
  const active = s.tabs.find((t) => t.id === s.activeId);
  return active && active.connectionId === connectionId ? groupOf(s.splits, active) : 0;
}

/**
 * What ⌘1…9, ⌘] / ⌘[ and Close Others / to the Right move within: a split workspace's focused
 * group (the group of `anchor`, default the active tab), else the whole workspace.
 */
function scopeTabs(s: GroupState, connectionId: string | null, anchor?: Tab): Tab[] {
  if (connectionId === null) return [...s.tabs];
  if (!s.splits[connectionId]) return workspaceTabs(s.tabs, connectionId);
  const group = anchor ? groupOf(s.splits, anchor) : focusedGroup(s, connectionId);
  return groupTabs(s, connectionId, group);
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
  if (tab.kind === "tool") return extra;
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

/** One connection's unsaved work kept outside its tabs. */
export interface ConnectionWork {
  connectionId: string;
  title: string;
  count: number;
}

/** Lists of connection-level unsaved work (staged sidebar operations). */
const connectionWorkSources: (() => ConnectionWork[])[] = [];

/** Adds a source of connection-level unsaved work; returns its removal. */
export function registerConnectionWork(list: () => ConnectionWork[]): () => void {
  connectionWorkSources.push(list);
  return () => {
    const i = connectionWorkSources.indexOf(list);
    if (i >= 0) connectionWorkSources.splice(i, 1);
  };
}

/** Connection-level unsaved work as close-dialog entries (all connections when `only` is null). */
export function connectionWork(only: ReadonlySet<string> | null = null): DirtyTab[] {
  const out: DirtyTab[] = [];
  for (const list of connectionWorkSources)
    for (const w of list()) {
      if (w.count <= 0 || (only !== null && !only.has(w.connectionId))) continue;
      out.push({
        id: `connection:${w.connectionId}`,
        kind: "structure",
        title: w.title,
        count: w.count,
        connectionId: w.connectionId,
      });
    }
  return out;
}

/** Unsaved work of the window that belongs to no tab and no connection (the standalone SQL Viewer). */
export interface WindowWork {
  /** Stable key of the work (the close dialog lists it as `window:<id>`). */
  id: string;
  title: string;
  count: number;
}

const windowWorkSources: (() => WindowWork[])[] = [];
const windowWorkListeners = new Set<() => void>();
const notifyWindowWork = () => [...windowWorkListeners].forEach((fn) => fn());

/**
 * Adds a source of window-level unsaved work to every close / quit guard (`hasUnsaved`,
 * `requestWindowClose`); returns its removal. `subscribe` tells when the work changes, so the
 * guard's reporter notices (`subscribeWindowWork`).
 */
export function registerWindowWork(
  list: () => WindowWork[],
  subscribe?: (onChange: () => void) => () => void,
): () => void {
  windowWorkSources.push(list);
  const unsubscribe = subscribe?.(notifyWindowWork);
  notifyWindowWork();
  return () => {
    unsubscribe?.();
    const i = windowWorkSources.indexOf(list);
    if (i >= 0) windowWorkSources.splice(i, 1);
    notifyWindowWork();
  };
}

/** Calls `onChange` whenever window-level unsaved work may have changed. */
export function subscribeWindowWork(onChange: () => void): () => void {
  windowWorkListeners.add(onChange);
  return () => void windowWorkListeners.delete(onChange);
}

/** Window-level unsaved work as close-dialog entries. */
export function windowWork(): DirtyTab[] {
  const out: DirtyTab[] = [];
  for (const list of windowWorkSources)
    for (const w of list()) {
      if (w.count <= 0) continue;
      out.push({ id: `window:${w.id}`, kind: "query", title: w.title, count: w.count });
    }
  return out;
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

/**
 * Duplicate Tab: the new tab starts from the source's filters, sort, paging and column layout
 * (copies, so the two tabs diverge from here on). Staged edits, the loaded page and the row
 * count are not copied.
 */
function copyTableState(fromId: string, toId: string): void {
  const from = useTableTabsStore.getState().slices[fromId];
  if (!from) return;
  useTableTabsStore.getState().update(toId, {
    sort: from.sort.map((x) => ({ ...x })),
    filters: from.filters.map((f) => ({ ...f, id: newId() })),
    conjunction: from.conjunction,
    filtersVisible: from.filtersVisible,
    limit: from.limit,
    offset: from.offset,
    columnState: {
      ...from.columnState,
      widths: { ...from.columnState.widths },
      order: [...from.columnState.order],
      hidden: [...from.columnState.hidden],
    },
  });
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

/**
 * True when any open tab, any connection or the window itself (the standalone SQL Viewer) holds
 * unsaved work (the window-close guard).
 */
export function hasUnsaved(): boolean {
  return (
    useTabsStore.getState().tabs.some((t) => unsavedCount(t) > 0) ||
    connectionWork().length > 0 ||
    windowWork().length > 0
  );
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
  splits: GroupState["splits"] = {},
): string | null {
  const index = tabs.findIndex((t) => t.id === activeId);
  const active = tabs[index];
  if (!active) return null;
  if (!closing.has(active.id)) return active.id;
  // In a split workspace the active tab's own group first, then the other group's shown tab.
  const group = groupOf(splits, active);
  const nearest = (ok: (t: Tab) => boolean) => {
    for (let i = index + 1; i < tabs.length; i++) if (ok(tabs[i]!)) return tabs[i]!.id;
    for (let i = index - 1; i >= 0; i--) if (ok(tabs[i]!)) return tabs[i]!.id;
    return null;
  };
  const survives = (t: Tab) => !closing.has(t.id) && t.connectionId === active.connectionId;
  const split = splits[active.connectionId];
  if (!split) return nearest(survives);
  const sameGroup = nearest((t) => survives(t) && groupOf(splits, t) === group);
  if (sameGroup) return sameGroup;
  const other = split.shown[group === 0 ? 1 : 0];
  if (other && !closing.has(other) && tabs.some((t) => t.id === other)) return other;
  return nearest(survives);
}

/** The surviving tab of `ids` (a group, in strip order) nearest to `previous` in `before`. */
function nearestOf(
  before: readonly Tab[],
  previous: string | null,
  ids: readonly string[],
): string | null {
  if (ids.length === 0) return null;
  const alive = new Set(ids);
  const index = before.findIndex((t) => t.id === previous);
  if (index >= 0) {
    for (let i = index + 1; i < before.length; i++)
      if (alive.has(before[i]!.id)) return before[i]!.id;
    for (let i = index - 1; i >= 0; i--) if (alive.has(before[i]!.id)) return before[i]!.id;
  }
  return ids[0] ?? null;
}

const sameIds = (a: readonly (string | null)[], b: readonly (string | null)[]) =>
  a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * Keeps the splits consistent with a state change: closed tabs leave their group, a new tab that
 * becomes active opens in the group that had the focus, each group shows the active tab when it
 * holds it (else its nearest surviving tab), and a split whose group emptied closes. A patch that
 * sets `splits` itself (Split Right, moving a tab) places its tabs on its own.
 */
function withSplits(s: TabsState, patch: Partial<TabsState>): Partial<TabsState> {
  const explicit = "splits" in patch;
  const source = patch.splits ?? s.splits;
  if (Object.keys(source).length === 0) return patch;
  const tabs = patch.tabs ?? s.tabs;
  const activeId = "activeId" in patch ? (patch.activeId ?? null) : s.activeId;
  const known = new Set(s.tabs.map((t) => t.id));
  const previous = s.tabs.find((t) => t.id === s.activeId);
  let changed = explicit;
  const splits: Record<string, Split> = {};
  for (const [connectionId, split] of Object.entries(source)) {
    const own = workspaceTabs(tabs, connectionId);
    const alive = new Set(own.map((t) => t.id));
    let right = split.right.filter((id) => alive.has(id));
    // A new tab opened while the right group had the focus joins it.
    const joined =
      !explicit &&
      activeId !== null &&
      !known.has(activeId) &&
      alive.has(activeId) &&
      previous?.connectionId === connectionId &&
      right.includes(previous.id);
    if (joined) right = [...right, activeId];
    const rightSet = new Set(right);
    const left = own.filter((t) => !rightSet.has(t.id)).map((t) => t.id);
    const rightOrdered = own.filter((t) => rightSet.has(t.id)).map((t) => t.id);
    if (left.length === 0 || rightOrdered.length === 0) {
      changed = true;
      continue;
    }
    const pick = (ids: string[], shown: string | null): string | null => {
      if (activeId !== null && ids.includes(activeId)) return activeId;
      if (shown !== null && ids.includes(shown)) return shown;
      return nearestOf(s.tabs, shown, ids);
    };
    const shown: [string | null, string | null] = [
      pick(left, split.shown[0]),
      pick(rightOrdered, split.shown[1]),
    ];
    const next: Split = { right: rightOrdered, shown };
    const same = sameIds(next.right, split.right) && sameIds(next.shown, split.shown);
    if (!same) changed = true;
    splits[connectionId] = same ? split : next;
  }
  if (!changed && Object.keys(splits).length === Object.keys(source).length) return patch;
  return { ...patch, splits };
}

function buildRequest(
  kind: CloseRequest["kind"],
  targets: readonly Tab[],
  extra: readonly DirtyTab[] = [],
): CloseRequest | null {
  const dirty = [...dirtyTabs(targets), ...extra];
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
          return patch === s ? s : withHistory(s, withSplits(s, patch));
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
    splits: {},

    open: (tab) =>
      set((s) => ({
        tabs: s.tabs.some((t) => t.id === tab.id) ? s.tabs : [...s.tabs, tab],
        activeId: tab.id,
      })),

    openTable: (connectionId, table, kind = "table", options = {}) => {
      const s = get();
      // "Open in New Tab" never looks for the table's tab, and is never a preview.
      const preview = options.preview === true && options.newTab !== true;
      const existing =
        options.newTab === true
          ? undefined
          : s.tabs.find(
              (t) => isTableTab(t) && t.connectionId === connectionId && sameTable(t.table, table),
            );
      if (isTableTab(existing)) {
        // A single click keeps a preview tab a preview; any other open pins it.
        const updated = { ...existing, kind };
        if (!(preview && existing.preview)) delete updated.preview;
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
        ...(preview ? { preview: true } : {}),
      };
      // A split workspace keeps a preview tab per group: the focused group's is replaced.
      const group = focusedGroup(s, connectionId);
      const previous = preview
        ? s.tabs.find(
            (t) =>
              t.connectionId === connectionId &&
              isTableTab(t) &&
              t.preview &&
              groupOf(s.splits, t) === group,
          )
        : undefined;
      // The preview tab is replaced in place, unless it picked up work since it opened.
      if (previous && unsavedCount(previous) === 0) {
        dropTabState([previous.id]);
        const split = s.splits[connectionId];
        set({
          tabs: s.tabs.map((t) => (t.id === previous.id ? tab : t)),
          activeId: id,
          // It takes the replaced tab's place in its group.
          ...(split?.right.includes(previous.id)
            ? {
                splits: {
                  ...s.splits,
                  [connectionId]: {
                    ...split,
                    right: split.right.map((r) => (r === previous.id ? id : r)),
                  },
                },
              }
            : {}),
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
        if (!isTableTab(tab) || tab.kind === kind) return s;
        return { tabs: s.tabs.map((t) => (t.id === id ? { ...tab, kind } : t)) };
      }),

    pin: (id) =>
      set((s) => {
        const tab = s.tabs.find((t) => t.id === id);
        if (!isTableTab(tab) || !tab.preview) return s;
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
      if (!source || (source.kind !== "query" && !isTableTab(source))) return null;
      let tab: Tab;
      let queryCounter = s.queryCounter;
      if (source.kind === "query") {
        queryCounter += 1;
        tab = {
          id: newId(),
          kind: "query",
          connectionId: source.connectionId,
          title: `Query ${queryCounter}`,
        };
      } else {
        tab = {
          id: newId(),
          kind: source.kind,
          connectionId: source.connectionId,
          table: { ...source.table },
          title: source.title,
        };
        copyTableState(source.id, tab.id);
      }
      const tabs = [...s.tabs];
      tabs.splice(index + 1, 0, tab);
      set({ tabs, activeId: tab.id, queryCounter });
      return tab.id;
    },

    close: (id) => void get().requestClose([id]),

    closeOthers: (id) => {
      const kept = get().tabs.find((t) => t.id === id);
      if (!kept) return;
      // The kept tab is the only survivor of its workspace, so it becomes active once the others are
      // gone (now, or when the unsaved-changes dialog is confirmed). `pickNeighbour` alone would
      // not pick it when another workspace's tab was active.
      // In a split workspace: the other tabs of the kept tab's group.
      get().requestClose(
        scopeTabs(get(), kept.connectionId, kept)
          .filter((t) => t.id !== id)
          .map((t) => t.id),
        id,
      );
    },

    closeToRight: (id) => {
      const tab = get().tabs.find((t) => t.id === id);
      if (!tab) return;
      const tabs = scopeTabs(get(), tab.connectionId, tab);
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

    requestCloseConnection: (connectionId) => {
      const s = get();
      if (s.closeRequest) return false;
      const targets = s.tabs.filter((t) => t.connectionId === connectionId);
      const request = buildRequest("tabs", targets, connectionWork(new Set([connectionId])));
      if (request) {
        set({ closeRequest: request });
        return false;
      }
      if (targets.length > 0) get().forceClose(targets.map((t) => t.id));
      return true;
    },

    requestWindowClose: () => {
      const s = get();
      const hooks = windowCloseHooks;
      const guarded = hooks ? s.tabs.filter((t) => !hooks.survivesClose(t, s.tabs)) : s.tabs;
      const request = buildRequest("window", guarded, [...connectionWork(), ...windowWork()]);
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
        activeId: pickNeighbour(s.tabs, s.activeId, closing, s.splits),
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
      set((s) => {
        const split = s.splits[oldId];
        const splits = { ...s.splits };
        delete splits[oldId];
        if (split) splits[newId] = split;
        return {
          tabs: s.tabs.map(retarget),
          closed: s.closed.map(retarget),
          ...(split ? { splits } : {}),
        };
      });
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
        const tabs = scopeTabs(s, scopeOf(s, connectionId));
        if (tabs.length === 0) return s;
        const i = index >= 9 ? tabs.length - 1 : Math.min(index, tabs.length - 1);
        return { activeId: tabs[i]?.id ?? s.activeId };
      }),

    next: (connectionId) =>
      set((s) => {
        const tabs = scopeTabs(s, scopeOf(s, connectionId));
        if (tabs.length === 0) return s;
        const i = tabs.findIndex((t) => t.id === s.activeId);
        return { activeId: tabs[(i + 1) % tabs.length]?.id ?? s.activeId };
      }),

    prev: (connectionId) =>
      set((s) => {
        const tabs = scopeTabs(s, scopeOf(s, connectionId));
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
        else if (isTableTab(tab)) {
          const { preview: _preview, ...rest } = tab;
          restored = rest;
        } else restored = tab;
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

    splitRight: (id) => {
      const s = get();
      const source = s.tabs.find((t) => t.id === (id ?? s.activeId));
      if (!source) return null;
      const copyId = newId();
      let copy: Tab;
      let queryCounter = s.queryCounter;
      if (source.kind === "query") {
        queryCounter += 1;
        copy = {
          id: copyId,
          kind: "query",
          connectionId: source.connectionId,
          title: `Query ${queryCounter}`,
        };
      } else if (isTableTab(source)) {
        copy = {
          id: copyId,
          kind: source.kind,
          connectionId: source.connectionId,
          table: { ...source.table },
          title: source.title,
        };
      } else copy = { ...source, id: copyId };
      const split = s.splits[source.connectionId];
      const left = split?.shown[0] ?? (groupOf(s.splits, source) === 0 ? source.id : null);
      set({
        tabs: [...s.tabs, copy],
        activeId: copyId,
        queryCounter,
        splits: {
          ...s.splits,
          [source.connectionId]: {
            right: [...(split?.right ?? []), copyId],
            shown: [left, copyId],
          },
        },
      });
      return copyId;
    },

    moveToGroup: (id, group, before) => {
      const s = get();
      const tab = s.tabs.find((t) => t.id === id);
      if (!tab) return;
      const connectionId = tab.connectionId;
      const split = s.splits[connectionId] ?? { right: [], shown: [null, null] };
      const own = workspaceTabs(s.tabs, connectionId);
      if (group === 1 && own.length < 2) return;
      const right =
        group === 1
          ? [...split.right.filter((r) => r !== id), id]
          : split.right.filter((r) => r !== id);
      // Strip position: in front of `before`, else after the target group's last tab.
      const rest = s.tabs.filter((t) => t.id !== id);
      const inTarget = (t: Tab) =>
        t.connectionId === connectionId && right.includes(t.id) === (group === 1) && t.id !== id;
      let at: number;
      const beforeIndex = before ? rest.findIndex((t) => t.id === before) : -1;
      if (beforeIndex >= 0) at = beforeIndex;
      else {
        let last = -1;
        rest.forEach((t, i) => {
          if (inTarget(t)) last = i;
        });
        at = last >= 0 ? last + 1 : rest.length;
      }
      const tabs = [...rest.slice(0, at), tab, ...rest.slice(at)];
      // Leaving a workspace that was not split: the left group keeps showing the active tab.
      const shown: [string | null, string | null] = [
        split.shown[0] ?? (s.activeId !== id ? s.activeId : null),
        split.shown[1],
      ];
      shown[group] = id;
      set({
        tabs,
        activeId: id,
        splits: { ...s.splits, [connectionId]: { right, shown } },
      });
    },

    unsplit: (connectionId) =>
      set((s) => {
        if (!s.splits[connectionId]) return s;
        const splits = { ...s.splits };
        delete splits[connectionId];
        return { splits };
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
