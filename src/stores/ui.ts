import { create } from "zustand";
import { persist } from "zustand/middleware";

export type ThemePref = "system" | "light" | "dark";

interface UiState {
  themePref: ThemePref;
  sidebarOpen: boolean;
  inspectorOpen: boolean;
  setThemePref: (pref: ThemePref) => void;
  toggleSidebar: () => void;
  toggleInspector: () => void;
}

export const useUiStore = create<UiState>()(
  persist(
    (set) => ({
      themePref: "system",
      sidebarOpen: true,
      inspectorOpen: false,
      setThemePref: (themePref) => set({ themePref }),
      toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
      toggleInspector: () => set((s) => ({ inspectorOpen: !s.inspectorOpen })),
    }),
    { name: "tabler.ui", version: 1 },
  ),
);
