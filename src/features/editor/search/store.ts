import { create } from "zustand";
import { useTabsStore } from "@/stores/tabs";
import type { FindOptions } from "./model";

/** The find bar of a query tab's results (⌘F in the results, or the header's search icon). */
export interface FindState extends FindOptions {
  open: boolean;
  text: string;
  replacement: string;
  /** The replace row is shown. */
  replacing: boolean;
  /** Bumped by every `openFind`: the bar focuses (and selects) its field again. */
  focusSeq: number;
}

export const CLOSED_FIND: Readonly<FindState> = Object.freeze({
  open: false,
  text: "",
  replacement: "",
  replacing: false,
  matchCase: false,
  wholeWord: false,
  regex: false,
  focusSeq: 0,
});

interface FindStore {
  byTab: Record<string, FindState>;
}

export const useFindStore = create<FindStore>()(() => ({ byTab: {} }));

const stateOf = (tabId: string): FindState => useFindStore.getState().byTab[tabId] ?? CLOSED_FIND;

export function setFind(tabId: string, patch: Partial<FindState>): void {
  useFindStore.setState((s) => ({
    byTab: { ...s.byTab, [tabId]: { ...(s.byTab[tabId] ?? CLOSED_FIND), ...patch } },
  }));
}

/** Opens (or refocuses) the tab's find bar, keeping what it searched for last. */
export function openFind(tabId: string, opts: { replace?: boolean } = {}): void {
  const cur = stateOf(tabId);
  setFind(tabId, {
    open: true,
    focusSeq: cur.focusSeq + 1,
    ...(opts.replace ? { replacing: true } : {}),
  });
}

export function closeFind(tabId: string): void {
  if (stateOf(tabId).open) setFind(tabId, { open: false });
}

export function toggleFind(tabId: string): void {
  if (stateOf(tabId).open) closeFind(tabId);
  else openFind(tabId);
}

/** The tab's find bar state (a shared closed state when it never opened). */
export function useFind(tabId: string): FindState {
  return useFindStore((s) => s.byTab[tabId] ?? CLOSED_FIND);
}

/** Forgets the find bars of closed tabs. */
export function dropFind(keep: (tabId: string) => boolean): void {
  const byTab = useFindStore.getState().byTab;
  const gone = Object.keys(byTab).filter((id) => !keep(id));
  if (gone.length === 0) return;
  const next = { ...byTab };
  for (const id of gone) delete next[id];
  useFindStore.setState({ byTab: next });
}

let installed = false;

/** Drops the find bar state of query tabs once they close (once, app-wide). */
export function installFindLifecycle(): void {
  if (installed) return;
  installed = true;
  useTabsStore.subscribe((s, prev) => {
    if (s.tabs === prev.tabs) return;
    const open = new Set(s.tabs.map((t) => t.id));
    dropFind((id) => open.has(id));
  });
}
