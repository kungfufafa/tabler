/** Most-recently-used palette items, newest first, persisted per machine in localStorage. */

export const MRU_STORAGE_KEY = "tabler.palette.mru";
export const MRU_MAX = 30;

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    // Accessing localStorage throws when site data is blocked.
    return null;
  }
}

export function readMru(storage: StorageLike | null = defaultStorage()): string[] {
  try {
    const raw = storage?.getItem(MRU_STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((k): k is string => typeof k === "string").slice(0, MRU_MAX);
  } catch {
    return [];
  }
}

/** Moves `key` to the front of `list` (pure). */
export function pushMru(list: readonly string[], key: string, max: number = MRU_MAX): string[] {
  return [key, ...list.filter((k) => k !== key)].slice(0, max);
}

/** Records a use of `key` and returns the updated list. Storage failures are ignored. */
export function recordMru(key: string, storage: StorageLike | null = defaultStorage()): string[] {
  const next = pushMru(readMru(storage), key);
  try {
    storage?.setItem(MRU_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Quota exceeded / storage disabled: the palette still works, just without history.
  }
  return next;
}
