import type { Conjunction, FilterOp, TableRef } from "@/lib/ipc";
import type { FilterDefaultState } from "@/stores/prefs";
import type { FilterRowSpec, TableTabState } from "@/stores/tableTabs";

/**
 * Filters are remembered per table (TablePlus): the applied rows, their conjunction and whether
 * the bar was shown, keyed by connection profile + database + table. Kept in localStorage (per
 * viewer, survives restarts) with an in-memory copy for when storage is unavailable.
 */
export const FILTER_MEMORY_KEY = "tabler.filters";
/** Most tables remembered; the least recently saved ones are dropped first. */
export const FILTER_MEMORY_MAX = 200;

export interface RememberedFilters {
  filters: FilterRowSpec[];
  conjunction: Conjunction;
  visible: boolean;
  /** Save time (ms), for dropping the oldest entries. */
  at: number;
}

type Store = Record<string, RememberedFilters>;

let memory: Store | null = null;

export function filterMemoryKey(
  profileId: string,
  database: string | null | undefined,
  table: TableRef,
): string {
  return [profileId, database ?? "", table.schema ?? "", table.name].join("\u0000");
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  v !== null && typeof v === "object" && !Array.isArray(v);

function validRow(v: unknown): v is FilterRowSpec {
  return (
    isObject(v) &&
    typeof v.id === "string" &&
    typeof v.column === "string" &&
    typeof v.op === "string" &&
    typeof v.value === "string" &&
    typeof v.value2 === "string" &&
    typeof v.enabled === "boolean"
  );
}

function validEntry(v: unknown): v is RememberedFilters {
  return (
    isObject(v) &&
    Array.isArray(v.filters) &&
    v.filters.every(validRow) &&
    (v.conjunction === "and" || v.conjunction === "or") &&
    typeof v.visible === "boolean" &&
    typeof v.at === "number"
  );
}

function load(): Store {
  if (memory) return memory;
  const out: Store = {};
  try {
    const raw = globalThis.localStorage?.getItem(FILTER_MEMORY_KEY);
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
  try {
    globalThis.localStorage?.setItem(FILTER_MEMORY_KEY, JSON.stringify(store));
  } catch {
    // Storage unavailable or full: the in-memory copy still serves this session.
  }
}

/** The remembered filters of a table, or null. */
export function recallFilters(key: string): RememberedFilters | null {
  return load()[key] ?? null;
}

/** Remembers a table's filters (nothing to remember — no rows, bar hidden — forgets it). */
export function rememberFilters(
  key: string,
  state: { filters: FilterRowSpec[]; conjunction: Conjunction; visible: boolean },
  now: number = Date.now(),
): void {
  const store = { ...load() };
  const prev = store[key];
  if (state.filters.length === 0 && !state.visible) {
    if (!prev) return;
    delete store[key];
  } else {
    if (
      prev &&
      prev.visible === state.visible &&
      prev.conjunction === state.conjunction &&
      JSON.stringify(prev.filters) === JSON.stringify(state.filters)
    )
      return;
    store[key] = { ...state, at: now };
    const keys = Object.keys(store);
    if (keys.length > FILTER_MEMORY_MAX) {
      keys
        .sort((a, b) => store[a]!.at - store[b]!.at)
        .slice(0, keys.length - FILTER_MEMORY_MAX)
        .forEach((k) => delete store[k]);
    }
  }
  memory = store;
  save(store);
}

/**
 * A new table tab's filter state for the "Default filter state" setting: the table's last state
 * (filters applied, bar shown or hidden as it was), or an empty bar always shown / hidden.
 * Remembered rows whose operator is unknown (an older version) are dropped.
 */
export function initialFilterState(
  key: string | null,
  mode: FilterDefaultState,
  knownOp: (op: string) => op is FilterOp,
): Partial<TableTabState> {
  if (mode === "show") return { filtersVisible: true };
  if (mode === "hide") return { filtersVisible: false };
  const remembered = key ? recallFilters(key) : null;
  if (!remembered) return {};
  return {
    filters: remembered.filters.filter((r) => knownOp(r.op)),
    conjunction: remembered.conjunction,
    filtersVisible: remembered.visible,
  };
}

/** Test-only: forget everything (memory and storage). */
export function resetFilterMemoryForTests(): void {
  memory = null;
  try {
    globalThis.localStorage?.removeItem(FILTER_MEMORY_KEY);
  } catch {
    // ignore
  }
}
