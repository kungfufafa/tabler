import { create } from "zustand";
import type { TableRef } from "@/lib/ipc";

export type Tab =
  | { id: string; kind: "table"; connectionId: string; table: TableRef; title: string }
  | { id: string; kind: "structure"; connectionId: string; table: TableRef; title: string }
  | { id: string; kind: "query"; connectionId: string; title: string };

export type TabKind = Tab["kind"];

interface TabsState {
  tabs: Tab[];
  activeId: string | null;
  closed: Tab[];
  queryCounter: number;
  open: (tab: Tab) => void;
  openTable: (connectionId: string, table: TableRef, kind?: "table" | "structure") => string;
  newQuery: (connectionId: string) => string;
  close: (id: string) => void;
  closeAll: () => void;
  closeForConnection: (connectionId: string) => void;
  activate: (id: string) => void;
  activateIndex: (index: number) => void;
  next: () => void;
  prev: () => void;
  move: (from: number, to: number) => void;
  reopenClosed: () => void;
  rename: (id: string, title: string) => void;
}

const newId = () => crypto.randomUUID();

export function sameTable(a: TableRef, b: TableRef): boolean {
  return a.name === b.name && (a.schema ?? "") === (b.schema ?? "");
}

export const useTabsStore = create<TabsState>()((set, get) => ({
  tabs: [],
  activeId: null,
  closed: [],
  queryCounter: 0,

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

  close: (id) =>
    set((s) => {
      const index = s.tabs.findIndex((t) => t.id === id);
      if (index < 0) return s;
      const tab = s.tabs[index]!;
      const tabs = s.tabs.filter((t) => t.id !== id);
      let activeId = s.activeId;
      if (activeId === id) {
        const neighbour = tabs[Math.min(index, tabs.length - 1)];
        activeId = neighbour?.id ?? null;
      }
      return { tabs, activeId, closed: [...s.closed.slice(-19), tab] };
    }),

  closeAll: () =>
    set((s) => ({ tabs: [], activeId: null, closed: [...s.closed, ...s.tabs].slice(-20) })),

  closeForConnection: (connectionId) =>
    set((s) => {
      const tabs = s.tabs.filter((t) => t.connectionId !== connectionId);
      const activeId = tabs.some((t) => t.id === s.activeId)
        ? s.activeId
        : (tabs[tabs.length - 1]?.id ?? null);
      return { tabs, activeId };
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
      return { closed: s.closed.slice(0, -1), tabs: [...s.tabs, tab], activeId: tab.id };
    }),

  rename: (id, title) =>
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === id ? { ...t, title } : t)) })),
}));

export function useActiveTab(): Tab | null {
  return useTabsStore((s) => s.tabs.find((t) => t.id === s.activeId) ?? null);
}
