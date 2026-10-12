import { create } from "zustand";
import { tableKey, type TableRef } from "@/lib/ipc";

/** localStorage key holding the sidebar's recently opened tables. */
export const RECENT_STORAGE_KEY = "tabler.sidebar.recent";
/** TablePlus lists the last 5 opened items. */
export const MAX_RECENT = 5;

/** Recently opened tables (`tableKey`s, most recent first) per scope (`pinScope`). */
export type RecentMap = Record<string, string[]>;

export const EMPTY_RECENT: readonly string[] = Object.freeze([]);

/** Keeps the well-formed part of a stored value: string lists under string keys, no blanks. */
export function parseRecent(value: unknown): RecentMap {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: RecentMap = {};
  for (const [scope, list] of Object.entries(value as Record<string, unknown>)) {
    if (!Array.isArray(list)) continue;
    const keys = [
      ...new Set(list.filter((k): k is string => typeof k === "string" && k !== "")),
    ].slice(0, MAX_RECENT);
    if (keys.length > 0) out[scope] = keys;
  }
  return out;
}

/** The stored lists; empty when storage is unavailable (private window, blocked site data). */
function readStored(): RecentMap {
  try {
    const raw = localStorage.getItem(RECENT_STORAGE_KEY);
    return raw ? parseRecent(JSON.parse(raw)) : {};
  } catch {
    return {};
  }
}

function writeStored(recent: RecentMap): void {
  try {
    localStorage.setItem(RECENT_STORAGE_KEY, JSON.stringify(recent));
  } catch {
    // Not persisted; the in-memory list still works for this session.
  }
}

interface RecentState {
  recent: RecentMap;
}

export const useRecentStore = create<RecentState>()(() => ({ recent: readStored() }));

/** Moves `table` to the front of `scope`'s recent list (keeping at most `MAX_RECENT`). */
export function recordRecent(scope: string, table: TableRef): void {
  const key = tableKey(table);
  const current = useRecentStore.getState().recent[scope] ?? EMPTY_RECENT;
  if (current[0] === key) return;
  const next = [key, ...current.filter((k) => k !== key)].slice(0, MAX_RECENT);
  const recent = { ...useRecentStore.getState().recent, [scope]: next };
  useRecentStore.setState({ recent });
  writeStored(recent);
}

/** The recent table keys of `scope` (a stable reference until that scope changes). */
export function useRecent(scope: string | null): readonly string[] {
  return useRecentStore((s) => (scope !== null ? s.recent[scope] : undefined) ?? EMPTY_RECENT);
}

/** Test helper: reads the stored lists again. */
export function reloadRecent(): void {
  useRecentStore.setState({ recent: readStored() });
}
