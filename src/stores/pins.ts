import { useEffect } from "react";
import { create } from "zustand";
import { ipc, tableKey, type TableRef } from "@/lib/ipc";

/** App-DB setting holding the sidebar pins (`get_settings` / `set_setting`). */
export const PINS_SETTING_KEY = "sidebarPins";
/** Pins are written this long after the last change. */
export const PINS_SAVE_DELAY_MS = 300;

/** Pinned tables (`tableKey`: `schema.name`) per scope (`pinScope`). */
export type PinMap = Record<string, string[]>;

export const EMPTY_PINS: ReadonlySet<string> = new Set();

/** Pins belong to a saved connection and the database it is on: `profileId|database`. */
export function pinScope(profileId: string, database: string | null | undefined): string {
  return `${profileId}|${database ?? ""}`;
}

/** Keeps the well-formed part of a stored value: string lists under string keys, no blanks. */
export function parsePins(value: unknown): PinMap {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: PinMap = {};
  for (const [scope, list] of Object.entries(value as Record<string, unknown>)) {
    if (!Array.isArray(list)) continue;
    const keys = [...new Set(list.filter((k): k is string => typeof k === "string" && k !== ""))];
    if (keys.length > 0) out[scope] = keys;
  }
  return out;
}

interface PinsState {
  pins: PinMap;
  /** `pins` as sets; a scope's set is only replaced when that scope changes. */
  sets: Record<string, ReadonlySet<string>>;
  loaded: boolean;
}

export const usePinsStore = create<PinsState>()(() => ({ pins: {}, sets: {}, loaded: false }));

let loading: Promise<void> | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
/** Scopes changed before the stored pins arrived: the local state wins for them. */
const touched = new Set<string>();

/** Reads the stored pins once (later calls share the first read). */
export function loadPins(): Promise<void> {
  loading ??= ipc.settings.get().then(
    (all) => {
      const stored = parsePins(all[PINS_SETTING_KEY]);
      usePinsStore.setState((s) => {
        const pins = { ...stored };
        const sets: Record<string, ReadonlySet<string>> = {};
        for (const [scope, keys] of Object.entries(stored)) sets[scope] = new Set(keys);
        for (const scope of touched) {
          delete pins[scope];
          delete sets[scope];
          const local = s.pins[scope];
          const localSet = s.sets[scope];
          if (local && localSet) {
            pins[scope] = local;
            sets[scope] = localSet;
          }
        }
        touched.clear();
        return { pins, sets, loaded: true };
      });
    },
    () => {
      // Unreadable settings: keep working in memory (saves still go through).
      usePinsStore.setState({ loaded: true });
    },
  );
  return loading;
}

function scheduleSave(): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    // Never write before the stored pins were merged in, or other scopes would be lost.
    void loadPins()
      .then(() => ipc.settings.set(PINS_SETTING_KEY, usePinsStore.getState().pins))
      .catch(() => undefined);
  }, PINS_SAVE_DELAY_MS);
}

/** Pins `table` in `scope`, or unpins it when it is pinned. */
export function togglePin(scope: string, table: TableRef): void {
  const key = tableKey(table);
  if (!usePinsStore.getState().loaded) touched.add(scope);
  usePinsStore.setState((s) => {
    const current = s.pins[scope] ?? [];
    const next = current.includes(key) ? current.filter((k) => k !== key) : [...current, key];
    const pins = { ...s.pins };
    const sets = { ...s.sets };
    if (next.length === 0) {
      delete pins[scope];
      delete sets[scope];
    } else {
      pins[scope] = next;
      sets[scope] = new Set(next);
    }
    return { pins, sets };
  });
  scheduleSave();
}

export function isPinned(scope: string | null, table: TableRef): boolean {
  return scope !== null && (usePinsStore.getState().sets[scope]?.has(tableKey(table)) ?? false);
}

/** The pinned table keys of `scope` (a stable reference until that scope changes). */
export function usePinned(scope: string | null): ReadonlySet<string> {
  useEffect(() => void loadPins(), []);
  return usePinsStore((s) => (scope !== null ? s.sets[scope] : undefined) ?? EMPTY_PINS);
}

/** Test helper: forget loaded pins and pending saves. */
export function resetPins(): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = null;
  loading = null;
  touched.clear();
  usePinsStore.setState({ pins: {}, sets: {}, loaded: false });
}
