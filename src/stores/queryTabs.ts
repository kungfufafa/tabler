import { create } from "zustand";
import type { RunState } from "@/features/editor/runModel";
import type { ErrorPayload, ExplainMode, ExplainResult } from "@/lib/ipc";

/** The plan shown in a query tab's "Plan" pane (from the `explain` command). */
export interface PlanState {
  /** queryId passed to `explain` (lets Stop cancel it). */
  id: string;
  mode: ExplainMode;
  sql: string;
  /** UTF-16 offset of `sql` in the document. */
  baseOffset: number;
  status: "running" | "done" | "error" | "cancelled";
  result: ExplainResult | null;
  error: ErrorPayload | null;
  startedAt: number;
  finishedAt: number | null;
}

/** Results pane selection: a result buffer id, the messages list or the plan. */
export type ResultPane = "messages" | "plan" | (string & {});

export interface QueryTabState {
  run: RunState | null;
  plan: PlanState | null;
  pane: ResultPane;
  /** File the buffer was opened from / saved to; the tab is titled after it. */
  filePath: string | null;
  historyOpen: boolean;
  /** Editor / results split (panel id → percentage), remembered across tab switches. */
  layout: Record<string, number> | null;
}

export const EMPTY_QUERY_TAB: Readonly<QueryTabState> = Object.freeze({
  run: null,
  plan: null,
  pane: "messages",
  filePath: null,
  historyOpen: false,
  layout: null,
});

type Patch = Partial<QueryTabState> | ((s: QueryTabState) => Partial<QueryTabState>);

interface QueryTabsStore {
  slices: Record<string, QueryTabState>;
  update: (tabId: string, patch: Patch) => void;
  remove: (tabId: string) => void;
}

export const useQueryTabsStore = create<QueryTabsStore>()((set) => ({
  slices: {},
  update: (tabId, patch) =>
    set((st) => {
      const cur = st.slices[tabId] ?? EMPTY_QUERY_TAB;
      const next = typeof patch === "function" ? patch(cur) : patch;
      return { slices: { ...st.slices, [tabId]: { ...cur, ...next } } };
    }),
  remove: (tabId) =>
    set((st) => {
      if (!(tabId in st.slices)) return st;
      const slices = { ...st.slices };
      delete slices[tabId];
      return { slices };
    }),
}));

/** Current state of a query tab outside React. */
export function getQueryTab(tabId: string): Readonly<QueryTabState> {
  return useQueryTabsStore.getState().slices[tabId] ?? EMPTY_QUERY_TAB;
}

export function updateQueryTab(tabId: string, patch: Patch): void {
  useQueryTabsStore.getState().update(tabId, patch);
}

/** Subscribes to one field of a query tab (returns the stored reference, never a fresh object). */
export function useQueryTabField<K extends keyof QueryTabState>(
  tabId: string,
  key: K,
): QueryTabState[K] {
  return useQueryTabsStore((s) => (s.slices[tabId] ?? EMPTY_QUERY_TAB)[key]);
}

/** True while a statement or an explain is executing in the tab. */
export function isBusy(s: Readonly<QueryTabState>): boolean {
  return s.run?.status === "running" || s.plan?.status === "running";
}

export function useQueryTabBusy(tabId: string): boolean {
  return useQueryTabsStore((s) => isBusy(s.slices[tabId] ?? EMPTY_QUERY_TAB));
}
