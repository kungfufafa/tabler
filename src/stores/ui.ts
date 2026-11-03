import { create } from "zustand";
import { persist } from "zustand/middleware";

/**
 * Settings ▸ Appearance ▸ Theme: Auto (follows macOS), the built-in themes, or one of the user's
 * custom themes (`custom:<id>`, see features/appearance).
 */
export const BUILTIN_THEME_PREFS = [
  "system",
  "light",
  "dark",
  "solarized-light",
  "solarized-dark",
] as const;
export type BuiltinThemePref = (typeof BUILTIN_THEME_PREFS)[number];
export type ThemePref = BuiltinThemePref | `custom:${string}`;

/** Whether `value` is a theme choice (the id of a custom theme is not checked against the list). */
export function isThemePref(value: unknown): value is ThemePref {
  if (typeof value !== "string") return false;
  if ((BUILTIN_THEME_PREFS as readonly string[]).includes(value)) return true;
  return /^custom:[A-Za-z0-9_-]{1,64}$/.test(value);
}
export type View = "launcher" | "workspace";

interface UiState {
  themePref: ThemePref;
  sidebarOpen: boolean;
  inspectorOpen: boolean;
  /** The console log (⇧⌘C) under the workspace's tab content. */
  consoleOpen: boolean;
  view: View;
  /**
   * The workspace rail's order (dragged by the user): workspace keys (`workspaceKey`: profile and
   * database), so it survives reconnects and restarts. Workspaces not listed follow, in the order
   * they were opened.
   */
  railOrder: string[];
  /** The rail's right-click toggles (TablePlus: Show/hide database name, connection name). */
  railShowDatabase: boolean;
  railShowConnection: boolean;
  setThemePref: (pref: ThemePref) => void;
  toggleSidebar: () => void;
  toggleInspector: () => void;
  toggleConsole: () => void;
  setView: (view: View) => void;
  setRailOrder: (keys: string[]) => void;
  toggleRailDatabase: () => void;
  toggleRailConnection: () => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      themePref: "system",
      sidebarOpen: true,
      inspectorOpen: false,
      consoleOpen: false,
      view: "launcher",
      railOrder: [],
      // TablePlus's rail is the icons alone; the names are in the tooltip and the right-click menu.
      railShowDatabase: false,
      railShowConnection: false,
      setThemePref: (themePref) => set({ themePref }),
      toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
      toggleInspector: () => set((s) => ({ inspectorOpen: !s.inspectorOpen })),
      toggleConsole: () => set((s) => ({ consoleOpen: !s.consoleOpen })),
      setView: (view) => set({ view }),
      setRailOrder: (railOrder) => set({ railOrder }),
      toggleRailDatabase: () => set((s) => ({ railShowDatabase: !s.railShowDatabase })),
      toggleRailConnection: () => set((s) => ({ railShowConnection: !s.railShowConnection })),
    }),
    {
      name: "tabler.ui",
      version: 2,
      // v1 showed the database and connection names under the rail icons by default; v2 hides
      // them (TablePlus), and a v1 store takes the new default rather than keep the old one.
      migrate: (persisted, version) => {
        const stored = (persisted ?? {}) as Partial<UiState>;
        if (version < 2) {
          delete stored.railShowDatabase;
          delete stored.railShowConnection;
        }
        // `merge` below fills whatever a partial store lacks from the defaults.
        return stored as UiState;
      },
      // A stored theme choice that is not one (hand-edited, from a newer version) falls back to Auto.
      merge: (persisted, current) => {
        const stored = (persisted ?? {}) as Partial<UiState>;
        return {
          ...current,
          ...stored,
          themePref: isThemePref(stored.themePref) ? stored.themePref : current.themePref,
        };
      },
      partialize: (s) => ({
        themePref: s.themePref,
        sidebarOpen: s.sidebarOpen,
        inspectorOpen: s.inspectorOpen,
        consoleOpen: s.consoleOpen,
        railOrder: s.railOrder,
        railShowDatabase: s.railShowDatabase,
        railShowConnection: s.railShowConnection,
      }),
    },
  ),
);
