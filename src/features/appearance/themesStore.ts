import { toast } from "sonner";
import { create } from "zustand";
import { ipc } from "@/lib/ipc";
import {
  cleanThemeName,
  MAX_CUSTOM_THEMES,
  newThemeId,
  sanitizeThemes,
  uniqueThemeName,
  type CustomTheme,
} from "./customTheme";

/**
 * The custom themes. One JSON list stored under the `themes` key of the app DB settings table,
 * mirrored to localStorage so the first paint already has the active theme (the same arrangement
 * as the preferences). The theme *choice* is `themePref` in the UI store (`custom:<id>`).
 */

/** App DB settings key holding the list. */
export const THEMES_KEY = "themes";
/** localStorage mirror (also how other windows hear about changes: `storage` events). */
export const THEMES_LOCAL_KEY = "tabler.themes";
export const THEMES_SAVE_DEBOUNCE_MS = 300;

function readLocal(): CustomTheme[] {
  try {
    const raw = globalThis.localStorage?.getItem(THEMES_LOCAL_KEY);
    return raw ? sanitizeThemes(JSON.parse(raw)) : [];
  } catch {
    return [];
  }
}

function writeLocal(themes: readonly CustomTheme[]): void {
  try {
    globalThis.localStorage?.setItem(THEMES_LOCAL_KEY, JSON.stringify(themes));
  } catch {
    // Storage unavailable or full: the app DB copy is the source of truth anyway.
  }
}

let saveTimer: ReturnType<typeof setTimeout> | null = null;
let loadPromise: Promise<void> | null = null;
let editedBeforeLoad = false;

async function saveNow(): Promise<void> {
  try {
    await ipc.settings.set(THEMES_KEY, useThemes.getState().themes);
  } catch (err) {
    console.warn("[themes] save failed", err);
    toast.error("Could not save themes", {
      id: "themes-save",
      description: err instanceof Error ? err.message : String(err),
    });
  }
}

function scheduleSave(): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void saveNow();
  }, THEMES_SAVE_DEBOUNCE_MS);
}

/** Writes a pending (debounced) save right away, e.g. when the window is going away. */
export function flushThemes(): Promise<void> {
  if (saveTimer === null) return Promise.resolve();
  clearTimeout(saveTimer);
  saveTimer = null;
  return saveNow();
}

export interface ThemesState {
  themes: readonly CustomTheme[];
  loaded: boolean;
  /** Adds a theme (a unique name is made of `name`) and returns it as stored; null at the limit. */
  add: (theme: Omit<CustomTheme, "id">) => CustomTheme | null;
  /** Changes a theme. A renamed theme keeps its name unique. */
  update: (id: string, patch: Partial<Omit<CustomTheme, "id">>) => void;
  remove: (id: string) => void;
  /** Applies a list another window saved, without saving it again. */
  applyExternal: (raw: unknown) => void;
}

export const useThemes = create<ThemesState>()((set, get) => {
  const commit = (themes: readonly CustomTheme[]) => {
    if (!get().loaded) editedBeforeLoad = true;
    set({ themes });
    writeLocal(themes);
    scheduleSave();
  };
  return {
    themes: readLocal(),
    loaded: false,
    add: (theme) => {
      const { themes } = get();
      if (themes.length >= MAX_CUSTOM_THEMES) return null;
      const name = uniqueThemeName(
        cleanThemeName(theme.name) ?? "Theme",
        themes.map((t) => t.name),
      );
      const added: CustomTheme = { id: newThemeId(), name, base: theme.base, tokens: theme.tokens };
      commit([...themes, added]);
      return added;
    },
    update: (id, patch) => {
      const { themes } = get();
      const current = themes.find((t) => t.id === id);
      if (!current) return;
      const next: CustomTheme = { ...current, ...patch };
      if (patch.name !== undefined) {
        const clean = cleanThemeName(patch.name);
        next.name = clean
          ? uniqueThemeName(
              clean,
              themes.filter((t) => t.id !== id).map((t) => t.name),
            )
          : current.name;
      }
      commit(themes.map((t) => (t.id === id ? next : t)));
    },
    remove: (id) => {
      const { themes } = get();
      if (themes.some((t) => t.id === id)) commit(themes.filter((t) => t.id !== id));
    },
    applyExternal: (raw) => {
      const themes = sanitizeThemes(raw);
      set({ themes });
      writeLocal(themes);
    },
  };
});

export const getCustomThemes = (): readonly CustomTheme[] => useThemes.getState().themes;

/** The custom theme with this id, if there is one. */
export const getCustomTheme = (id: string): CustomTheme | undefined =>
  useThemes.getState().themes.find((t) => t.id === id);

/**
 * Reads the app DB copy once per session (later calls return the same promise). Edits made before
 * it resolves win over the stored copy; a localStorage-only copy (first run, or the in-memory mock
 * backend) is pushed to the app DB.
 */
export function loadThemes(): Promise<void> {
  loadPromise ??= (async () => {
    let stored: unknown;
    try {
      stored = (await ipc.settings.get())[THEMES_KEY];
    } catch (err) {
      console.warn("[themes] load failed; using local copy", err);
      useThemes.setState({ loaded: true });
      return;
    }
    const { themes: local } = useThemes.getState();
    if (editedBeforeLoad || stored === undefined || stored === null) {
      useThemes.setState({ loaded: true });
      if (!editedBeforeLoad && local.length > 0) scheduleSave();
      return;
    }
    const themes = sanitizeThemes(stored);
    useThemes.setState({ themes, loaded: true });
    writeLocal(themes);
  })();
  return loadPromise;
}

/** Test-only: back to a fresh, unloaded store with no pending save. */
export function resetThemesForTests(themes: readonly CustomTheme[] = []): void {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = null;
  loadPromise = null;
  editedBeforeLoad = false;
  try {
    globalThis.localStorage?.removeItem(THEMES_LOCAL_KEY);
  } catch {
    // ignore
  }
  useThemes.setState({ themes, loaded: false });
}
