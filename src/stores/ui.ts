import { create } from "zustand";
import { persist } from "zustand/middleware";

export type ThemePref = "system" | "light" | "dark";
export type View = "launcher" | "workspace";

interface UiState {
  themePref: ThemePref;
  sidebarOpen: boolean;
  inspectorOpen: boolean;
  view: View;
  setThemePref: (pref: ThemePref) => void;
  toggleSidebar: () => void;
  toggleInspector: () => void;
  setView: (view: View) => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      themePref: "system",
      sidebarOpen: true,
      inspectorOpen: false,
      view: "launcher",
      setThemePref: (themePref) => set({ themePref }),
      toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
      toggleInspector: () => set((s) => ({ inspectorOpen: !s.inspectorOpen })),
      setView: (view) => set({ view }),
    }),
    {
      name: "tabler.ui",
      version: 1,
      partialize: (s) => ({
        themePref: s.themePref,
        sidebarOpen: s.sidebarOpen,
        inspectorOpen: s.inspectorOpen,
      }),
    },
  ),
);
