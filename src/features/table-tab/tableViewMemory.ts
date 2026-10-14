import { useCallback, useSyncExternalStore } from "react";
import type { ColumnState } from "@/features/datagrid/types";
import type { SortSpec } from "@/lib/ipc";

/**
 * What TablePlus remembers per table besides its filters (b204, b296, b632): the column layout
 * (positions, widths, hidden columns) and the "Default content order". Keyed like the filters
 * (`filterMemoryKey`: profile + database + table), kept in localStorage so every tab of the table
 * and every later session starts from it, with an in-memory copy when storage is unavailable.
 */
export const TABLE_VIEW_KEY = "tabler.tableView";
/** Most tables remembered; the least recently saved ones are dropped first. */
export const TABLE_VIEW_MAX = 300;

/** Table view settings ▸ Default content order: none, or by primary key ascending / descending. */
export const DEFAULT_ORDERS = ["none", "pkAsc", "pkDesc"] as const;
export type DefaultOrder = (typeof DEFAULT_ORDERS)[number];

export interface TableView {
  columns?: ColumnState;
  order?: DefaultOrder;
  /** Save time (ms), for dropping the oldest entries. */
  at: number;
}

type Store = Record<string, TableView>;

let memory: Store | null = null;
const listeners = new Set<() => void>();

const isObject = (v: unknown): v is Record<string, unknown> =>
  v !== null && typeof v === "object" && !Array.isArray(v);
const isStringArray = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === "string");

function validColumns(v: unknown): v is ColumnState {
  return (
    isObject(v) &&
    isObject(v.widths) &&
    Object.values(v.widths).every((w) => typeof w === "number" && Number.isFinite(w) && w > 0) &&
    isStringArray(v.order) &&
    isStringArray(v.hidden)
  );
}

function validEntry(v: unknown): v is TableView {
  return (
    isObject(v) &&
    typeof v.at === "number" &&
    (v.columns === undefined || validColumns(v.columns)) &&
    (v.order === undefined || DEFAULT_ORDERS.includes(v.order as DefaultOrder))
  );
}

function load(): Store {
  if (memory) return memory;
  const out: Store = {};
  try {
    const raw = globalThis.localStorage?.getItem(TABLE_VIEW_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    if (isObject(parsed))
      for (const [key, entry] of Object.entries(parsed)) if (validEntry(entry)) out[key] = entry;
  } catch {
    // Unreadable storage: start empty.
  }
  memory = out;
  return out;
}

function save(store: Store): void {
  memory = store;
  try {
    globalThis.localStorage?.setItem(TABLE_VIEW_KEY, JSON.stringify(store));
  } catch {
    // Storage unavailable or full: the in-memory copy still serves this session.
  }
  listeners.forEach((l) => l());
}

/** `patch` merged into the table's entry (an entry left with nothing to remember is dropped). */
function remember(key: string, patch: Partial<Omit<TableView, "at">>, now = Date.now()): void {
  const store = { ...load() };
  const next: TableView = { ...store[key], ...patch, at: now };
  if (next.columns === undefined) delete next.columns;
  if (next.order === undefined || next.order === "none") delete next.order;
  if (next.columns === undefined && next.order === undefined) {
    if (!store[key]) return;
    delete store[key];
  } else {
    store[key] = next;
    const keys = Object.keys(store);
    if (keys.length > TABLE_VIEW_MAX)
      keys
        .sort((a, b) => store[a]!.at - store[b]!.at)
        .slice(0, keys.length - TABLE_VIEW_MAX)
        .forEach((k) => delete store[k]);
  }
  save(store);
}

const emptyLayout = (c: ColumnState) =>
  c.order.length === 0 && c.hidden.length === 0 && Object.keys(c.widths).length === 0;

const sameLayout = (a: ColumnState | undefined, b: ColumnState) =>
  !!a && JSON.stringify(a) === JSON.stringify(b);

/** The table's remembered column layout, or null. */
export function recallColumnLayout(key: string | null): ColumnState | null {
  return key ? (load()[key]?.columns ?? null) : null;
}

/**
 * Remembers a table's column layout; the default layout (after "Reset Column Positions and
 * Widths" with nothing hidden) forgets it.
 */
export function rememberColumnLayout(key: string, columns: ColumnState, now = Date.now()): void {
  const prev = load()[key]?.columns;
  if (emptyLayout(columns)) {
    if (prev) remember(key, { columns: undefined }, now);
    return;
  }
  if (sameLayout(prev, columns)) return;
  remember(key, { columns }, now);
}

export function recallDefaultOrder(key: string | null): DefaultOrder {
  return (key ? load()[key]?.order : undefined) ?? "none";
}

export function rememberDefaultOrder(key: string, order: DefaultOrder, now = Date.now()): void {
  if (recallDefaultOrder(key) === order) return;
  remember(key, { order }, now);
}

/** The table's default content order as a sort, used while the user has no sort of their own. */
export function defaultOrderSort(order: DefaultOrder, primaryKey: readonly string[]): SortSpec[] {
  if (order === "none" || primaryKey.length === 0) return [];
  const dir = order === "pkAsc" ? "asc" : "desc";
  return primaryKey.map((column) => ({ column, dir, nulls: null }));
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The table's Default content order, and a setter that remembers it (every tab follows). */
export function useDefaultOrder(key: string | null): [DefaultOrder, (order: DefaultOrder) => void] {
  const order = useSyncExternalStore(subscribe, () => recallDefaultOrder(key));
  const set = useCallback(
    (next: DefaultOrder) => {
      if (key) rememberDefaultOrder(key, next);
    },
    [key],
  );
  return [order, set];
}

/** Test-only: forget everything (memory and storage). */
export function resetTableViewMemoryForTests(): void {
  memory = null;
  try {
    globalThis.localStorage?.removeItem(TABLE_VIEW_KEY);
  } catch {
    // ignore
  }
  listeners.forEach((l) => l());
}
