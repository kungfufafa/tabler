import { create } from "zustand";
import { onSqlLog, type SqlLogCategory, type SqlLogEvent } from "@/lib/ipc";
import { useConnectionsStore } from "@/stores/connections";

/** Entries kept; the oldest fall off once the log is full. */
export const CONSOLE_MAX_ENTRIES = 1000;

export type ConsoleCategory = SqlLogCategory;
export type ConsoleFilter = "all" | ConsoleCategory;

/** One line of the console log: a statement the app ran (see `SqlLogEvent`). */
export interface ConsoleEntry extends SqlLogEvent {
  id: number;
  /** When it was logged (ms since the epoch). */
  at: number;
  /** The connection's name and current database when it was logged (null: not open then). */
  connectionName: string | null;
  database: string | null;
}

interface ConsoleState {
  /** Oldest first. Replaced (never mutated) on every change, so it is a stable selector result. */
  entries: ConsoleEntry[];
  filter: ConsoleFilter;
  log: (event: SqlLogEvent, at?: number) => void;
  clear: () => void;
  setFilter: (filter: ConsoleFilter) => void;
}

/** `entries` plus `entry`, dropping the oldest ones beyond `max`. */
export function appendCapped<T>(entries: readonly T[], entry: T, max = CONSOLE_MAX_ENTRIES): T[] {
  const keep = entries.length >= max ? entries.slice(entries.length - max + 1) : entries;
  return [...keep, entry];
}

/** The entries the filter shows (the same array for "all"). */
export function filterEntries(entries: ConsoleEntry[], filter: ConsoleFilter): ConsoleEntry[] {
  return filter === "all" ? entries : entries.filter((e) => e.category === filter);
}

let nextId = 1;

export const useConsoleStore = create<ConsoleState>()((set) => ({
  entries: [],
  filter: "all",
  log: (event, at = Date.now()) => {
    const info = useConnectionsStore.getState().byId[event.connectionId]?.info;
    const entry: ConsoleEntry = {
      ...event,
      id: nextId++,
      at,
      connectionName: info?.name ?? null,
      database: info?.currentDatabase ?? null,
    };
    set((s) => ({ entries: appendCapped(s.entries, entry) }));
  },
  clear: () => set({ entries: [] }),
  setFilter: (filter) => set({ filter }),
}));

// Everything the IPC facade runs is logged, whether or not the panel is open.
const unsubscribe = onSqlLog((event) => useConsoleStore.getState().log(event));
import.meta.hot?.dispose(unsubscribe);
