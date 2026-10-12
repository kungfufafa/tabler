import { toast } from "sonner";
import { create } from "zustand";
import { ipc } from "@/lib/ipc";

/**
 * User preferences. One JSON object stored under the `prefs` key of the app DB settings table,
 * mirrored to localStorage so the first paint already uses them. Theme lives in the UI store.
 */

/** App DB settings key holding the whole preferences object. */
export const PREFS_KEY = "prefs";
/** localStorage mirror for instant startup (the app DB copy wins once loaded). */
export const PREFS_LOCAL_KEY = "tabler.prefs";
export const SAVE_DEBOUNCE_MS = 300;

/** Interface text size in px; the root font-size is scaled so rem-based UI follows. */
export const UI_FONT_SIZES = { small: 12, default: 13, large: 14 } as const;
export type UiFontSize = keyof typeof UI_FONT_SIZES;

export const PAGE_SIZES = [100, 200, 300, 500, 1000] as const;
export type PageSize = (typeof PAGE_SIZES)[number];

export const TAB_SIZES = [2, 4] as const;
export type TabSize = (typeof TAB_SIZES)[number];

export const EDITOR_FONT_SIZE_RANGE = { min: 11, max: 18 } as const;
export const CONNECT_TIMEOUT_RANGE = { min: 1, max: 300 } as const;

export interface Prefs {
  /** Interface text size (Small 12px / Default 13px / Large 14px). */
  uiFontSize: UiFontSize;
  /** Rows per page for newly opened table tabs. */
  pageSize: PageSize;
  /**
   * Show the Preview SQL sheet on ⌘S; when false (the default, as in TablePlus) staged changes are
   * applied immediately and ⇧⌘P previews them.
   */
  confirmCommit: boolean;
  /** SQL editor font size in px (11–18). */
  editorFontSize: number;
  editorTabSize: TabSize;
  editorLineWrap: boolean;
  /** Uppercase keywords when formatting SQL. */
  editorUppercaseKeywords: boolean;
  /** Default connect timeout for new connections, in seconds. */
  connectTimeout: number;
}

export const DEFAULT_PREFS: Readonly<Prefs> = Object.freeze({
  uiFontSize: "default",
  pageSize: 300,
  confirmCommit: false,
  editorFontSize: 13,
  editorTabSize: 2,
  editorLineWrap: false,
  editorUppercaseKeywords: true,
  connectTimeout: 15,
});

const PREF_KEYS = Object.keys(DEFAULT_PREFS) as (keyof Prefs)[];

const oneOf =
  (values: readonly unknown[]) =>
  (v: unknown): boolean =>
    values.includes(v);
const intIn =
  (min: number, max: number) =>
  (v: unknown): boolean =>
    typeof v === "number" && Number.isInteger(v) && v >= min && v <= max;
const isBool = (v: unknown): boolean => typeof v === "boolean";

const VALID: { [K in keyof Prefs]: (v: unknown) => boolean } = {
  uiFontSize: oneOf(Object.keys(UI_FONT_SIZES)),
  pageSize: oneOf(PAGE_SIZES),
  confirmCommit: isBool,
  editorFontSize: intIn(EDITOR_FONT_SIZE_RANGE.min, EDITOR_FONT_SIZE_RANGE.max),
  editorTabSize: oneOf(TAB_SIZES),
  editorLineWrap: isBool,
  editorUppercaseKeywords: isBool,
  connectTimeout: intIn(CONNECT_TIMEOUT_RANGE.min, CONNECT_TIMEOUT_RANGE.max),
};

function assign<K extends keyof Prefs>(target: Prefs, key: K, value: unknown): void {
  target[key] = value as Prefs[K];
}

/**
 * Overlays the valid known keys of `raw` onto `base`. Unknown keys are dropped and invalid values
 * (wrong type, out of range) keep the `base` value, so stale or hand-edited data never breaks the UI.
 */
export function sanitizePrefs(raw: unknown, base: Readonly<Prefs> = DEFAULT_PREFS): Prefs {
  const out: Prefs = { ...base };
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return out;
  const src = raw as Record<string, unknown>;
  for (const key of PREF_KEYS) {
    if (Object.hasOwn(src, key) && VALID[key](src[key])) assign(out, key, src[key]);
  }
  return out;
}

export function samePrefs(a: Readonly<Prefs>, b: Readonly<Prefs>): boolean {
  return PREF_KEYS.every((k) => a[k] === b[k]);
}

function readLocal(): unknown {
  try {
    const raw = globalThis.localStorage?.getItem(PREFS_LOCAL_KEY);
    return raw ? (JSON.parse(raw) as unknown) : null;
  } catch {
    return null;
  }
}

function writeLocal(prefs: Prefs): void {
  try {
    globalThis.localStorage?.setItem(PREFS_LOCAL_KEY, JSON.stringify(prefs));
  } catch {
    // Storage unavailable or full: the app DB copy is the source of truth anyway.
  }
}

// --- persistence --------------------------------------------------------------

let saveTimer: ReturnType<typeof setTimeout> | null = null;
let loadPromise: Promise<void> | null = null;
let editedBeforeLoad = false;

async function saveNow(): Promise<void> {
  try {
    await ipc.settings.set(PREFS_KEY, usePrefsStore.getState().prefs);
  } catch (err) {
    console.warn("[prefs] save failed", err);
    toast.error("Could not save preferences", {
      id: "prefs-save",
      description: err instanceof Error ? err.message : String(err),
    });
  }
}

function scheduleSave(): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void saveNow();
  }, SAVE_DEBOUNCE_MS);
}

/** Writes a pending (debounced) save right away, e.g. when the window is going away. */
export function flushPrefs(): Promise<void> {
  if (saveTimer === null) return Promise.resolve();
  clearTimeout(saveTimer);
  saveTimer = null;
  return saveNow();
}

/**
 * Reads the app DB copy once per session (later calls return the same promise). Edits made
 * before it resolves win over the stored copy; a localStorage-only copy (first run, or the
 * in-memory mock backend) is pushed to the app DB.
 */
export function loadPrefs(): Promise<void> {
  loadPromise ??= (async () => {
    let stored: unknown;
    try {
      stored = (await ipc.settings.get())[PREFS_KEY];
    } catch (err) {
      console.warn("[prefs] load failed; using local copy", err);
      usePrefsStore.setState({ loaded: true });
      return;
    }
    const current = usePrefsStore.getState().prefs;
    if (editedBeforeLoad || stored === undefined || stored === null) {
      usePrefsStore.setState({ loaded: true });
      if (!editedBeforeLoad && !samePrefs(current, DEFAULT_PREFS)) scheduleSave();
      return;
    }
    const prefs = sanitizePrefs(stored);
    usePrefsStore.setState({ prefs: samePrefs(prefs, current) ? current : prefs, loaded: true });
    writeLocal(prefs);
  })();
  return loadPromise;
}

// --- store --------------------------------------------------------------------

interface PrefsState {
  prefs: Prefs;
  /** True once the app DB copy has been read (or failed to load). */
  loaded: boolean;
  setPref: <K extends keyof Prefs>(key: K, value: Prefs[K]) => void;
  /** Applies the valid keys of `patch`; invalid values are ignored. */
  patchPrefs: (patch: Partial<Prefs>) => void;
}

export const usePrefsStore = create<PrefsState>()((set, get) => ({
  prefs: sanitizePrefs(readLocal()),
  loaded: false,
  setPref: (key, value) => get().patchPrefs({ [key]: value } as Partial<Prefs>),
  patchPrefs: (patch) => {
    const current = get().prefs;
    const next = sanitizePrefs(patch, current);
    if (samePrefs(next, current)) return;
    if (!get().loaded) editedBeforeLoad = true;
    set({ prefs: next });
    writeLocal(next);
    scheduleSave();
  },
}));

/** Subscribes to a slice of the preferences, e.g. `usePrefs((p) => p.editorFontSize)`. */
export function usePrefs<T>(selector: (prefs: Prefs) => T): T {
  return usePrefsStore((s) => selector(s.prefs));
}

/** Current preferences outside React (event handlers, stores). */
export function getPrefs(): Readonly<Prefs> {
  return usePrefsStore.getState().prefs;
}

export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]): void {
  usePrefsStore.getState().setPref(key, value);
}

/** Test-only: back to a fresh, unloaded store with no pending save. */
export function resetPrefsForTests(prefs: Readonly<Prefs> = DEFAULT_PREFS): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = null;
  loadPromise = null;
  editedBeforeLoad = false;
  usePrefsStore.setState({ prefs: { ...prefs }, loaded: false });
}
