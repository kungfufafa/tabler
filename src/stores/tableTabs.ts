import { create } from "zustand";
import type { ColumnState } from "@/features/datagrid/types";
import { emptyStaging } from "@/features/staging/model";
import type { StagingState } from "@/features/staging/types";
import type { Conjunction, FilterOp, RowCount, SortSpec } from "@/lib/ipc";
import { DEFAULT_PREFS, getPrefs } from "./prefs";

export interface FilterRowSpec {
  id: string;
  column: string;
  op: FilterOp;
  value: string;
  value2: string;
  enabled: boolean;
}

export interface PageInfo {
  rows: number;
  durationMs: number;
  sql: string;
}

export interface TableTabState {
  sort: SortSpec[];
  filters: FilterRowSpec[];
  conjunction: Conjunction;
  filtersVisible: boolean;
  limit: number;
  offset: number;
  columnState: ColumnState;
  /** Set after a page loads; shown in the status bar. */
  page: PageInfo | null;
  count: RowCount | null;
  /** Bumped by "Refresh" to refetch without changing the request. */
  refreshToken: number;
  staging: StagingState;
}

/** Built-in page size; new tabs use the "default page size" preference (`getPrefs().pageSize`). */
export const DEFAULT_LIMIT: number = DEFAULT_PREFS.pageSize;

export function newFilterRow(column: string, op: FilterOp = "eq", value = ""): FilterRowSpec {
  return { id: crypto.randomUUID(), column, op, value, value2: "", enabled: true };
}

function initial(init?: Partial<TableTabState>): TableTabState {
  return {
    sort: [],
    filters: [],
    conjunction: "and",
    filtersVisible: false,
    limit: getPrefs().pageSize,
    offset: 0,
    columnState: { widths: {}, order: [], hidden: [] },
    page: null,
    count: null,
    refreshToken: 0,
    staging: emptyStaging(),
    ...init,
  };
}

interface TableTabsStore {
  slices: Record<string, TableTabState>;
  ensure: (tabId: string, init?: Partial<TableTabState>) => TableTabState;
  update: (
    tabId: string,
    patch: Partial<TableTabState> | ((s: TableTabState) => Partial<TableTabState>),
  ) => void;
  remove: (tabId: string) => void;
}

export const useTableTabsStore = create<TableTabsStore>()((set, get) => ({
  slices: {},
  ensure: (tabId, init) => {
    const existing = get().slices[tabId];
    if (existing) return existing;
    const s = initial(init);
    set((st) => ({ slices: { ...st.slices, [tabId]: s } }));
    return s;
  },
  update: (tabId, patch) =>
    set((st) => {
      const cur = st.slices[tabId] ?? initial();
      const next = typeof patch === "function" ? patch(cur) : patch;
      return { slices: { ...st.slices, [tabId]: { ...cur, ...next } } };
    }),
  remove: (tabId) =>
    set((st) => {
      const slices = { ...st.slices };
      delete slices[tabId];
      return { slices };
    }),
}));

/** Stable placeholder per page size (selectors must not return a fresh object every render). */
const fallbacks = new Map<number, TableTabState>();
function fallback(): TableTabState {
  const limit = getPrefs().pageSize;
  let s = fallbacks.get(limit);
  if (!s) {
    s = initial({ limit });
    fallbacks.set(limit, s);
  }
  return s;
}

export function useTableTab(tabId: string): TableTabState {
  return useTableTabsStore((s) => s.slices[tabId] ?? fallback());
}
